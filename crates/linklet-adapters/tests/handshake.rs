//! The specification for the handshake.
//!
//! Two properties here cannot be checked by looking at the code and cannot be
//! checked by a round trip:
//!
//! - **Forward secrecy** -- a session recorded today must not be readable by
//!   someone who learns the shared secret tomorrow. A round trip passes whether or
//!   not this holds, because both ends are doing the same thing.
//! - **Authentication** -- someone who does not know the secret must not be able
//!   to sit in the middle and read the traffic. A round trip passes for that too,
//!   which is exactly why a channel with no authentication looks finished.
//!
//! A third property is checked here because it is the one that fails silently:
//! **the two ends must agree on the salt**. The salt is the two public keys, and if
//! each side put its own first the keys would differ -- loud, since nothing would
//! open. But a version that agreed in one direction and not the other would be the
//! silent kind, so both are checked.

use linklet_adapters::{HkdfChannel, new_session_id};
use linklet_core::channel::{
    Channel, ChannelError, EphemeralPublic, Handshake, Role, Sealed, SessionId,
};

/// The secret both ends share.
const SECRET: &[u8] = b"a shared secret for the handshake test";

/// A completed handshake, as the two processes would each be left holding it.
fn handshake() -> (Box<dyn Sealed>, Box<dyn Sealed>) {
    let (initiator_public, pending) = HkdfChannel.propose(SECRET).expect("proposing");
    let (responder_public, responder) = HkdfChannel
        .accept(SECRET, &initiator_public)
        .expect("accepting");
    let initiator = pending.finish(&responder_public).expect("finishing");
    (initiator, responder)
}

// --- it works at all ---------------------------------------------------------

#[test]
fn a_message_round_trips_through_a_handshake() {
    let (mut host, mut agent) = handshake();
    let sealed = host.seal(b"the build finished").expect("sealing");
    assert_eq!(agent.open(&sealed).expect("opening"), b"the build finished");
}

#[test]
fn both_directions_work() {
    // The handshake builds two keys from one exchange, so a bug that mixed up the
    // roles shows up as one direction working and the other not.
    let (mut host, mut agent) = handshake();

    let request = host.seal(b"run the build").expect("sealing");
    assert_eq!(agent.open(&request).expect("opening"), b"run the build");

    let reply = agent.seal(b"exit 0").expect("sealing");
    assert_eq!(host.open(&reply).expect("opening"), b"exit 0");
}

#[test]
fn many_messages_in_one_session_all_round_trip() {
    // The counter advances on both sides. A handshake that reset it, or that
    // shared one between the directions, shows up here.
    let (mut host, mut agent) = handshake();
    for i in 0..100u32 {
        let message = format!("message {i}");
        let sealed = host.seal(message.as_bytes()).expect("sealing");
        assert_eq!(agent.open(&sealed).expect("opening"), message.as_bytes());
    }
}

// --- forward secrecy ---------------------------------------------------------

#[test]
fn two_handshakes_with_the_same_secret_produce_different_sessions() {
    // This is the observable half of forward secrecy. Both handshakes use the same
    // shared secret; if the session key came from the secret alone, the two
    // sessions would be identical and a message from one would open in the other.
    //
    // They are not identical, which means the key depends on the ephemeral
    // exchange -- and the private halves of that exchange were dropped when the
    // `Proposer` and the responder's session were built. An attacker who records
    // the traffic and later learns the secret has neither.
    let (mut first_host, mut first_agent) = handshake();
    let (mut second_host, mut second_agent) = handshake();

    let sealed = first_host.seal(b"from the first session").expect("sealing");
    assert_eq!(
        first_agent
            .open(&sealed)
            .expect("opening in its own session"),
        b"from the first session"
    );

    assert_eq!(
        second_agent.open(&sealed),
        Err(ChannelError::NotAuthentic),
        "a message crossed between two handshakes with the same secret"
    );

    // And the second session works on its own, so the refusal above was about the
    // key and not about the session being broken.
    let other = second_host.seal(b"from the second").expect("sealing");
    assert_eq!(
        second_agent.open(&other).expect("opening"),
        b"from the second"
    );
}

#[test]
fn the_same_bytes_seal_differently_across_handshakes() {
    // A second view of the same property: identical plaintext, two handshakes,
    // different ciphertext, because the keys differ.
    let (mut first, _) = handshake();
    let (mut second, _) = handshake();

    let a = first.seal(b"identical").expect("sealing");
    let b = second.seal(b"identical").expect("sealing");
    assert_ne!(a, b, "two handshakes produced the same ciphertext");
}

// --- authentication ----------------------------------------------------------

#[test]
fn a_wrong_secret_produces_a_session_that_cannot_open() {
    // The responder uses a different secret, so it derives a different key. This is
    // the case a channel without authentication also passes -- both sides simply
    // fail to talk -- which is why it is not the test that matters. The next one is.
    let (initiator_public, pending) = HkdfChannel.propose(SECRET).expect("proposing");
    let (responder_public, mut wrong_responder) = HkdfChannel
        .accept(b"a different secret entirely", &initiator_public)
        .expect("accepting");
    let mut initiator = pending.finish(&responder_public).expect("finishing");

    let sealed = initiator.seal(b"secret").expect("sealing");
    assert_eq!(
        wrong_responder.open(&sealed),
        Err(ChannelError::NotAuthentic)
    );
}

#[test]
fn someone_in_the_middle_cannot_read_the_traffic() {
    // The property that makes the exchange worth having, and the one a round trip
    // cannot see.
    //
    // An attacker who can rewrite traffic gives the initiator *their* public key
    // instead of the responder's. Without the shared secret in the derivation, the
    // attacker would then hold a working key with each side and read everything.
    // With it, the attacker can compute a session -- they have their own private
    // key and the initiator's public key -- but that session is keyed by a secret
    // they do not have, so it is not the one the initiator built.
    let (initiator_public, pending) = HkdfChannel.propose(SECRET).expect("proposing");

    // The attacker answers instead of the agent, knowing no secret.
    let (attacker_public, _attacker_session) = HkdfChannel
        .accept(b"the attacker's guess", &initiator_public)
        .expect("the attacker can complete a handshake");

    // The initiator finishes against the substituted key. It gets a session, and it
    // is not the attacker's.
    let mut initiator = pending.finish(&attacker_public).expect("finishing");

    let sealed = initiator.seal(b"the real command").expect("sealing");

    // The attacker's session cannot open it, which is the whole point: they were
    // able to complete the exchange and still cannot read a byte.
    let mut attacker = _attacker_session;
    assert_eq!(
        attacker.open(&sealed),
        Err(ChannelError::NotAuthentic),
        "a man in the middle opened the message"
    );
}

#[test]
fn the_real_responder_also_cannot_open_a_substituted_handshake() {
    // The other half of the same scenario: the attacker's key reached the
    // initiator, so the real responder built a different session. Neither end of
    // the substitution can read the other, which is what turns an attack into a
    // failure rather than a silent compromise.
    let (initiator_public, pending) = HkdfChannel.propose(SECRET).expect("proposing");

    // What the real responder would have done with the same initiator key.
    let (_real_public, mut real) = HkdfChannel
        .accept(SECRET, &initiator_public)
        .expect("accepting");

    // The attacker substituted their own key, so the initiator is talking to a
    // session the real responder knows nothing about.
    let (attacker_public, _) = HkdfChannel
        .accept(b"the attacker's guess", &initiator_public)
        .expect("the attacker");
    let mut initiator = pending.finish(&attacker_public).expect("finishing");

    let sealed = initiator.seal(b"the real command").expect("sealing");
    assert_eq!(
        real.open(&sealed),
        Err(ChannelError::NotAuthentic),
        "the substitution went unnoticed"
    );
}

// --- the salt both ends must agree on ----------------------------------------

#[test]
fn both_ends_derive_the_same_keys_from_the_two_public_keys() {
    // If each side put its own public key first in the salt, the keys would differ
    // and nothing would open. This is the test that pins the ordering, and it is
    // here rather than in a comment because the failure mode of getting it wrong in
    // only one direction is silence.
    //
    // Four handshakes, so the check is not one lucky pairing.
    for round in 0..4 {
        let (mut host, mut agent) = handshake();
        let sealed = host
            .seal(format!("round {round}").as_bytes())
            .expect("sealing");
        assert_eq!(
            agent.open(&sealed).expect("opening"),
            format!("round {round}").as_bytes()
        );
    }
}

// --- what the core checks before the curve sees it ---------------------------

#[test]
fn a_public_key_of_the_wrong_length_is_refused() {
    for length in [0usize, 1, 31, 33, 64] {
        let error = EphemeralPublic::from_bytes(vec![0u8; length]).expect_err("should be refused");
        assert!(
            matches!(error, ChannelError::BadPublicKey { bytes } if bytes == length),
            "{length} gave {error:?}"
        );
    }
    assert!(EphemeralPublic::from_bytes(vec![0u8; EphemeralPublic::BYTES]).is_ok());
}

#[test]
fn a_public_key_of_zeros_is_refused_rather_than_producing_a_zero_key() {
    // The low-order-point case. X25519 against a point of small order can produce
    // an all-zero result, and a session key derived from a value an attacker can
    // force to zero is a session key the attacker knows. It is refused instead.
    let (_, pending) = HkdfChannel.propose(SECRET).expect("proposing");
    let zeros = EphemeralPublic::from_bytes(vec![0u8; EphemeralPublic::BYTES]).expect("32 bytes");

    assert!(
        matches!(pending.finish(&zeros), Err(ChannelError::NotAuthentic)),
        "a zero public key produced a session"
    );
}

#[test]
fn accepting_a_zero_public_key_is_refused_too() {
    let zeros = EphemeralPublic::from_bytes(vec![0u8; EphemeralPublic::BYTES]).expect("32 bytes");
    assert!(
        matches!(
            HkdfChannel.accept(SECRET, &zeros),
            Err(ChannelError::NotAuthentic)
        ),
        "a zero public key produced a session"
    );
}

// --- the shared-secret path still works --------------------------------------

#[test]
fn the_shared_secret_path_is_unchanged() {
    // `open_session` is kept for the case where an operator does not want the round
    // trip. It has no forward secrecy, which is why the handshake exists, but it
    // must not have been broken by adding one.
    let session = new_session_id().expect("randomness");
    let mut host = HkdfChannel
        .open_session(SECRET, &session, Role::Host)
        .expect("host");
    let mut agent = HkdfChannel
        .open_session(SECRET, &session, Role::Agent)
        .expect("agent");

    let sealed = host.seal(b"no handshake here").expect("sealing");
    assert_eq!(agent.open(&sealed).expect("opening"), b"no handshake here");
}

#[test]
fn the_two_paths_do_not_interoperate() {
    // Which is the point: a handshake session and a shared-secret session use
    // different derivations, so a message from one cannot be opened by the other.
    // If they did interoperate, the handshake would be adding nothing.
    let session: SessionId = new_session_id().expect("randomness");
    let mut direct = HkdfChannel
        .open_session(SECRET, &session, Role::Host)
        .expect("host");

    let (_host_end, mut through_handshake) = handshake();

    let sealed = direct.seal(b"direct").expect("sealing");
    assert_eq!(
        through_handshake.open(&sealed),
        Err(ChannelError::NotAuthentic),
        "a shared-secret message opened in a handshake session"
    );
}
