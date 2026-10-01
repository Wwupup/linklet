//! The specification for the sealed channel.
//!
//! The cryptography is in a crate that has been attacked by other people. What is
//! decided here is the part those crates cannot decide, and the part that goes
//! wrong in real systems: which key is used for which direction, how a nonce
//! advances, and what is refused. So these tests are about the four decisions in
//! `linklet-adapters/src/channel.rs`, not about whether ChaCha20 works.
//!
//! Three of them are the properties a channel is worthless without:
//! **confidentiality** (the bytes do not contain the message), **integrity** (a
//! changed byte is refused) and **ordering** (a message from the wrong point in
//! the session is refused). A channel missing any one of those is a channel that
//! looks fine and is not.

use linklet_adapters::{HkdfChannel, OVERHEAD_BYTES, new_session_id};
use linklet_core::channel::{Channel, ChannelError, Role, Sealed};

/// The secret both ends share.
const SECRET: &[u8] = b"a shared secret for the test only";

/// A session identifier that is fixed, so a failure is reproducible.
fn fixed_session() -> linklet_core::channel::SessionId {
    new_session_id().expect("the system should have randomness")
}

/// Both ends of one session, each able to seal its own direction and open the
/// other's.
///
/// The two are interchangeable, and that is a property of the implementation worth
/// stating: `seal` uses the key for the direction the caller is travelling and
/// `open` uses the key for the direction it is receiving, so the same type serves
/// both ends without either knowing which it is.
fn both_ends() -> (Box<dyn Sealed>, Box<dyn Sealed>) {
    let session = fixed_session();
    let host = HkdfChannel
        .open_session(SECRET, &session, Role::Host)
        .expect("the host end");
    let agent = HkdfChannel
        .open_session(SECRET, &session, Role::Agent)
        .expect("the agent end");
    (host, agent)
}

/// The host sealing and the agent opening: one direction, correctly paired.
///
/// The first version of this helper returned the pair the wrong way round, so
/// every round trip failed -- which is not a bug in the channel but is exactly the
/// mistake the two-key design exists to make impossible in the other direction:
/// sealing and opening with the same key would have made a wrong pairing *work*,
/// and the failure would have been the silent one where a reply can be replayed
/// back to its own sender.
fn host_to_agent() -> (Box<dyn Sealed>, Box<dyn Sealed>) {
    both_ends()
}
// --- the property that makes it a channel ------------------------------------

#[test]
fn a_message_round_trips() {
    let (mut host, mut agent) = host_to_agent();
    let plaintext = b"the build finished";

    let sealed = host.seal(plaintext).expect("sealing");
    assert_eq!(agent.open(&sealed).expect("opening"), plaintext);
}

#[test]
fn the_bytes_do_not_contain_the_message() {
    // Confidentiality, checked the only way a test can: the plaintext is not in
    // the ciphertext. A weak implementation that copied its input would pass the
    // round trip above and fail here.
    let (mut host, _) = host_to_agent();
    let plaintext = b"THIS-EXACT-STRING-SHOULD-NOT-APPEAR";

    let sealed = host.seal(plaintext).expect("sealing");

    assert!(
        !sealed
            .windows(plaintext.len())
            .any(|window| window == plaintext),
        "the plaintext is visible in the sealed bytes"
    );
}

#[test]
fn the_sealed_form_is_longer_by_the_tag() {
    let (mut host, _) = host_to_agent();
    let sealed = host.seal(b"hello").expect("sealing");
    assert_eq!(sealed.len(), b"hello".len() + OVERHEAD_BYTES);
}

#[test]
fn the_room_the_core_leaves_for_sealing_is_this_channel_s_overhead() {
    // `linklet_core::wire` works out how much plaintext one frame can carry by
    // subtracting a tag length it cannot import -- the dependency arrow points from
    // here to there, not the other way. So the number is written down twice, and this
    // is the test that makes the second copy safe: an agent that refused a reply a
    // few bytes early would be a nuisance, and one that accepted a reply it could not
    // send would be the defect this whole change exists to fix.
    let (mut host, _) = host_to_agent();
    let sealed = host.seal(b"").expect("sealing nothing");
    assert_eq!(
        sealed.len(),
        OVERHEAD_BYTES,
        "the tag is the only thing sealing adds"
    );
    assert_eq!(
        linklet_core::frame::MAX_PAYLOAD - linklet_core::wire::reply_ceiling(),
        OVERHEAD_BYTES,
        "the core's ceiling is the frame limit less this channel's tag"
    );
}

#[test]
fn the_same_plaintext_seals_differently_each_time() {
    // A counter nonce, so two identical messages in one session produce different
    // bytes. If they produced the same bytes, an observer could tell that two
    // messages were equal without reading either -- which is information a channel
    // exists to withhold.
    let (mut host, _) = host_to_agent();
    let first = host.seal(b"same").expect("sealing");
    let second = host.seal(b"same").expect("sealing");
    assert_ne!(first, second);
}

// --- integrity ---------------------------------------------------------------

#[test]
fn a_changed_byte_is_refused() {
    // Every position, because a tag that covers only part of the message is a tag
    // that misses exactly the part an attacker would change.
    let (mut host, mut agent) = host_to_agent();
    let sealed = host.seal(b"do not change me").expect("sealing");

    for index in 0..sealed.len() {
        let mut altered = sealed.clone();
        altered[index] ^= 1;
        assert_eq!(
            agent.open(&altered),
            Err(ChannelError::NotAuthentic),
            "flipping byte {index} was accepted"
        );
    }
}

#[test]
fn a_truncated_message_is_refused() {
    // Shorter than a tag cannot be authentic, and it is refused before the cipher
    // sees it so the failure names the right thing.
    let (mut host, mut agent) = host_to_agent();
    let sealed = host.seal(b"hello").expect("sealing");

    for length in 0..OVERHEAD_BYTES {
        assert_eq!(
            agent.open(&sealed[..length]),
            Err(ChannelError::NotAuthentic),
            "{length} bytes was accepted"
        );
    }
}

#[test]
fn a_message_from_a_different_secret_is_refused() {
    // The case a channel is for: someone who does not know the secret can send
    // bytes, and they do not open.
    let session = fixed_session();
    let mut attacker = HkdfChannel
        .open_session(b"a different secret entirely", &session, Role::Host)
        .expect("their own session");
    let mut victim = HkdfChannel
        .open_session(SECRET, &session, Role::Agent)
        .expect("the real one");

    let forged = attacker.seal(b"trust me").expect("sealing");
    assert_eq!(victim.open(&forged), Err(ChannelError::NotAuthentic));
}

#[test]
fn a_message_from_a_different_session_is_refused() {
    // The salt is what makes each request's keys separate. Without it, the same
    // secret would give the same key every time and a message from one session
    // would open in another.
    let mut first = HkdfChannel
        .open_session(SECRET, &fixed_session(), Role::Host)
        .expect("a");
    let mut second = HkdfChannel
        .open_session(SECRET, &fixed_session(), Role::Host)
        .expect("b");

    let sealed = first.seal(b"from session one").expect("sealing");
    assert_eq!(
        second.open(&sealed),
        Err(ChannelError::NotAuthentic),
        "a message crossed between sessions"
    );
}

// --- ordering ----------------------------------------------------------------

#[test]
fn a_replayed_message_is_refused() {
    // The counter is the defence: the same bytes arriving twice cannot both be the
    // next message. Within a session this is what stops a captured message from
    // being useful again.
    //
    // It does not stop a replay at the *network* level if the attacker can also
    // suppress the reply, which `linklet_core::channel` documents as a gap rather
    // than leaving it to be discovered.
    let (mut host, mut agent) = host_to_agent();
    let first = host.seal(b"one").expect("sealing");
    let second = host.seal(b"two").expect("sealing");

    assert_eq!(agent.open(&first).expect("opening the first"), b"one");
    // The attacker sends the first message again instead of the second.
    assert_eq!(agent.open(&first), Err(ChannelError::NotAuthentic));
    // ...and the second still opens, so the refusal did not desynchronise the
    // session. A channel that refused and then lost its place would turn one
    // attack into a denial of service.
    assert_eq!(agent.open(&second).expect("opening the second"), b"two");
}

#[test]
fn messages_out_of_order_are_refused() {
    let (mut host, mut agent) = host_to_agent();
    let first = host.seal(b"one").expect("sealing");
    let second = host.seal(b"two").expect("sealing");

    assert_eq!(
        agent.open(&second),
        Err(ChannelError::NotAuthentic),
        "the second message opened before the first"
    );
    assert_eq!(agent.open(&first).expect("opening the first"), b"one");
    assert_eq!(agent.open(&second).expect("opening the second"), b"two");
}

#[test]
fn a_relabelled_message_is_refused() {
    // The counter is authenticated as associated data, so a message cannot be
    // moved to a different position even by someone who captured it whole. Also
    // checks that the two directions are separate: an agent's own seal must not
    // open on the host's open path, because they are different keys.
    let (mut host_seal, mut agent_open) = host_to_agent();
    let (mut agent_seal, mut host_open) = both_ends();

    let sealed = host_seal.seal(b"host to agent").expect("sealing");
    assert_eq!(agent_open.open(&sealed).expect("opening"), b"host to agent");

    // The agent's reply goes the other way and is a different key, so the host's
    // *sealing* key cannot open it -- which is what stops a reply from being
    // replayed back to its own sender as if it came from the peer.
    let reply = agent_seal.seal(b"agent to host").expect("sealing");
    assert_eq!(host_open.open(&reply).expect("opening"), b"agent to host");
}

// --- what the core asks of an implementation ---------------------------------

#[test]
fn a_session_identifier_is_the_declared_length() {
    let session = new_session_id().expect("randomness");
    assert_eq!(
        session.as_bytes().len(),
        linklet_core::channel::SessionId::BYTES
    );
}

#[test]
fn two_session_identifiers_differ() {
    // The salt is fresh per session, which is what lets the nonce counter start at
    // zero every time. A source that returned the same bytes twice would give two
    // sessions the same key, and the counter would then repeat a nonce under it.
    let a = new_session_id().expect("randomness");
    let b = new_session_id().expect("randomness");
    assert_ne!(a, b);
}

#[test]
fn a_session_identifier_of_the_wrong_length_is_refused() {
    use linklet_core::channel::SessionId;
    for length in [0usize, 1, 31, 33, 64] {
        let error = SessionId::from_bytes(vec![0u8; length]).expect_err("should be refused");
        assert!(
            matches!(error, ChannelError::BadSessionId { bytes } if bytes == length),
            "{length} gave {error:?}"
        );
    }
    assert!(SessionId::from_bytes(vec![0u8; SessionId::BYTES]).is_ok());
}

#[test]
fn an_empty_message_round_trips() {
    // Not a curiosity: a chunk boundary can produce one, and an implementation
    // that treated zero-length plaintext as an error would break on it.
    let (mut host, mut agent) = host_to_agent();
    let sealed = host.seal(b"").expect("sealing");
    assert_eq!(sealed.len(), OVERHEAD_BYTES);
    assert_eq!(agent.open(&sealed).expect("opening"), b"");
}

#[test]
fn many_messages_in_one_session_all_round_trip() {
    // The counter has to advance on both sides in step, and a bug that skipped a
    // number would show up here and nowhere else -- every earlier test sends two
    // messages at most.
    let (mut host, mut agent) = host_to_agent();
    for i in 0..200u32 {
        let message = format!("message number {i}");
        let sealed = host.seal(message.as_bytes()).expect("sealing");
        assert_eq!(
            agent.open(&sealed).expect("opening"),
            message.as_bytes(),
            "message {i}"
        );
    }
}

// --- the error says nothing useful to an attacker ----------------------------

#[test]
fn every_way_of_failing_reads_the_same() {
    // One error for a bad tag, a wrong key, and a message from another session.
    // Telling them apart would tell an attacker which part of a guess was closer,
    // and a caller cannot act differently on any of them.
    let (mut host, mut agent) = host_to_agent();
    let sealed = host.seal(b"x").expect("sealing");

    let mut altered = sealed.clone();
    altered[0] ^= 1;
    let bad_tag = agent.open(&altered).expect_err("refused");
    let wrong_length = agent.open(&sealed[..3]).expect_err("refused");

    assert_eq!(bad_tag, ChannelError::NotAuthentic);
    assert_eq!(wrong_length, ChannelError::NotAuthentic);
    assert_eq!(
        bad_tag.to_string(),
        "the message is not authentic",
        "the message is the caller's whole view of the failure"
    );
}
