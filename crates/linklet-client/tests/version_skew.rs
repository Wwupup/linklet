//! What happens when the two ends do not speak the same protocol.
//!
//! `docs/decisions.md` D2 recorded the cost of the release that had no version in it: *"an old
//! host and a new agent fail with 'the first byte is 0x47' or an unknown `op` rather than with
//! a 406"*. The handshake now carries a number, and these are the tests that say what the
//! number is for.
//!
//! # The case that matters, and how it is produced without a second binary
//!
//! The deployment that goes wrong is **a new tool against an agent that was pushed months
//! ago**. Reproducing it needs a binary built before this field existed, which a test cannot
//! have -- so the test is the *other* side of the same wire: a listener that answers a
//! handshake with exactly what an old agent sends, which is `ephemeral_public` and nothing
//! else.
//!
//! That is the honest fixture. A fake that sent a *wrong* protocol number would test the
//! comparison; this one tests the case that actually happens, which is **absence**.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use linklet_client::{AgentAddress, CallError, probe};
use linklet_core::auth::Token;
use linklet_core::frame::{self, Kind};
use linklet_core::json;
use linklet_core::wire;

/// A token long enough for the agent's own rule, so the call gets as far as the handshake.
const TEST_TOKEN: &str = "protocol-test-token-0123456789";

/// A listener that answers one handshake with `hello`, then holds the connection open.
///
/// Holding it open matters: the client reads the hello and then decides, and a peer that hung
/// up immediately would be testing a different failure.
fn agent_that_says(hello: linklet_core::json::Json) -> (u16, std::thread::JoinHandle<()>) {
    // A **reply**, not a bare handshake body: the hello frame carries the same result-or-refusal
    // shape every other message does, which is why a body sent on its own is refused with "a
    // reply needs an ok field". Getting that wrong made this fixture fail for a reason that had
    // nothing to do with versions, which is what a fixture is for finding out.
    let hello = wire::reply_result(hello);
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener
        .local_addr()
        .expect("a bound listener knows its address")
        .port();

    let handle = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };

        // The client's hello is read and discarded: what it says is not what this test is
        // about, and an old agent would not have looked at it either.
        let mut head = [0u8; frame::HEADER_BYTES];
        if std::io::Read::read_exact(&mut stream, &mut head).is_err() {
            return;
        }
        let length = u32::from_be_bytes([head[2], head[3], head[4], head[5]]) as usize;
        let mut body = vec![0u8; length];
        if std::io::Read::read_exact(&mut stream, &mut body).is_err() {
            return;
        }

        let payload = json::write(&hello);
        let Ok(bytes) = frame::encode(Kind::Hello, payload.as_bytes()) else {
            return;
        };
        if stream.write_all(&bytes).is_err() {
            return;
        }
        let _ = stream.flush();

        // Long enough for the client to finish deciding, short enough not to hold the suite.
        std::thread::sleep(Duration::from_millis(500));
    });

    (port, handle)
}

/// An address with a token, pointed at a port on this machine.
fn address(port: u16) -> AgentAddress {
    AgentAddress::new(format!("127.0.0.1:{port}"))
        .expect("a valid address")
        .with_token(Token::new(TEST_TOKEN).expect("a usable token"))
}

#[test]
fn an_agent_that_predates_the_version_field_is_read_as_the_oldest_protocol() {
    // **The case the field was added for**: this tool was upgraded, the agent on the machine
    // was not. There is no second binary here, so the test is the *other side of the same
    // wire* -- a listener that answers a handshake with exactly what an old agent sends, which
    // is `ephemeral_public` and nothing else.
    //
    // **Absence reads as 1, and 1 is what this build speaks, so nothing is refused** -- which is
    // the compatibility guarantee, not a gap in the check. It is also why this test asserts the
    // *reading* and the test below asserts the *refusal*: with both numbers at 1 there is no
    // skew to report, and when `PROTOCOL_VERSION` is raised the comparison in the client is what
    // starts refusing this peer. That is the whole point of pinning absence to a named constant
    // rather than to the current version.
    let old = wire::handshake_to_json(&[0u8; 32]);
    let linklet_core::json::Json::Object(mut fields) = old else {
        panic!("a handshake is an object");
    };
    fields.remove("protocol");
    let old = linklet_core::json::Json::Object(fields);

    assert_eq!(
        wire::handshake_protocol_from_json(&old).expect("a number"),
        wire::OLDEST_PROTOCOL,
        "a handshake with no protocol field is the oldest protocol, not the current one"
    );

    // And the call still reaches the agent, which is the deployment guarantee: an old agent is
    // not locked out by a tool that learned to negotiate.
    let (port, agent) = agent_that_says(old);
    let outcome = probe(&address(port), Duration::from_secs(2));

    if let Err(error) = &outcome {
        assert!(
            !error.to_string().contains("docs/VERSIONING.md"),
            "an agent at the oldest protocol must not be refused as a skew: {error}"
        );
    }

    agent.join().expect("the fake agent finishes");
}

#[test]
fn a_peer_speaking_a_different_protocol_is_refused_with_both_numbers() {
    // The other end of the same check, and the one that makes it worth having. A peer that
    // states a number this build is not is refused at the handshake rather than at whichever
    // later operation happens to need a feature it does not have -- which is what a caller got
    // before this: `op: "ls" is not one of identity, run, push, ...`, true and useless.
    use linklet_core::json::Json;
    use std::collections::BTreeMap;

    let mut object: BTreeMap<String, Json> = BTreeMap::new();
    object.insert("ephemeral_public".to_string(), Json::str("00".repeat(32)));
    object.insert(
        "protocol".to_string(),
        Json::Int(wire::PROTOCOL_VERSION + 1),
    );

    let (port, agent) = agent_that_says(Json::Object(object));
    let outcome = probe(&address(port), Duration::from_secs(2));

    let Err(error) = outcome else {
        panic!("a peer speaking a different protocol must not be probed as healthy: {outcome:?}");
    };
    let text = error.to_string();

    assert!(
        matches!(error, CallError::Protocol(_)),
        "a version skew is a protocol problem and not a transport one: {error:?}"
    );
    assert!(
        text.contains(&format!("protocol {}", wire::PROTOCOL_VERSION))
            && text.contains(&format!("protocol {}", wire::PROTOCOL_VERSION + 1)),
        "the sentence must name both numbers, and said: {text}"
    );
    assert!(
        text.contains("docs/VERSIONING.md"),
        "and it must point at the policy, and said: {text}"
    );
    // **Which end is older, because that is the actionable half.** "Protocol mismatch" leaves a
    // reader to work out what to upgrade; a peer numbered above this build means the *agent* is
    // ahead and this tool is the one to replace.
    assert!(
        text.contains("this tool is the older one"),
        "the sentence must say which end to upgrade, and said: {text}"
    );

    agent.join().expect("the fake agent finishes");
}

#[test]
fn a_handshake_that_states_the_same_protocol_is_not_refused() {
    // The other half, and the one that keeps the check from being a gate that breaks working
    // deployments: a peer that agrees gets through the handshake. It fails later, on the
    // sealed call, because this fake agent does not speak the rest of the protocol -- and that
    // is the point: the *version check* let it past.
    let (port, agent) = agent_that_says(wire::handshake_to_json(&[0u8; 32]));
    let outcome = probe(&address(port), Duration::from_secs(2));

    if let Err(error) = &outcome {
        let text = error.to_string();
        assert!(
            !text.contains("docs/VERSIONING.md"),
            "a matching protocol must not be reported as a skew: {text}"
        );
    }

    agent.join().expect("the fake agent finishes");
}

#[test]
fn a_protocol_that_is_not_a_number_is_a_bad_message_and_not_an_old_agent() {
    // **Absence and nonsense are different.** Absence is a peer older than the field, which is
    // a real deployment and is read as the oldest protocol. A `protocol` key that is a string
    // is a peer disagreeing about what the field is, and reading that as "old" would hide a
    // malformed message behind a version story.
    use linklet_core::json::Json;
    use std::collections::BTreeMap;

    let mut object: BTreeMap<String, Json> = BTreeMap::new();
    object.insert("ephemeral_public".to_string(), Json::str("00".repeat(32)));
    object.insert("protocol".to_string(), Json::str("two"));

    let (port, agent) = agent_that_says(Json::Object(object));
    let outcome = probe(&address(port), Duration::from_secs(2));

    let Err(error) = outcome else {
        panic!("a malformed handshake must be refused: {outcome:?}");
    };
    let text = error.to_string();
    assert!(
        text.contains("must be a number"),
        "the message should say what was wrong with it, and said: {text}"
    );
    assert!(
        !text.contains("docs/VERSIONING.md"),
        "and it must not be dressed up as a version skew: {text}"
    );

    agent.join().expect("the fake agent finishes");
}

#[test]
fn the_handshake_this_build_sends_carries_the_version() {
    // The number has to be on the wire, or none of the above can happen. Asserted separately
    // from the client's use of it, because a writer that dropped the field and a reader that
    // defaulted to the current version would agree with each other perfectly and never notice.
    let hello = wire::handshake_to_json(&[7u8; 32]);
    let text = json::write(&hello);

    assert!(
        text.contains("protocol"),
        "the handshake must say which protocol it is: {text}"
    );
    assert_eq!(
        wire::handshake_protocol_from_json(&hello).expect("a number"),
        wire::PROTOCOL_VERSION
    );

    // And the field an old agent needs is still there and still first-class, because a peer
    // that predates `protocol` reads this message too.
    assert!(text.contains("ephemeral_public"), "{text}");
}

/// Keeps the unused-import warning away if the fixture above stops needing it.
#[allow(dead_code)]
fn _stream_is_used(_: &TcpStream) {}
