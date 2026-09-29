//! The specification for the framing.
//!
//! `src/frame.rs` lists ten ways a length-prefixed protocol goes wrong. This file
//! has a test for each one, and the numbering in the test names matches the
//! numbering in that list -- so a reader can tell which defence is being checked
//! and, more usefully, which one has no test.
//!
//! The claim being defended is narrow and worth restating, because it is the reason
//! replacing HTTP with something smaller is acceptable at all: **a bug in this
//! module causes a refusal, not a forged message**, because everything it frames
//! goes straight into an AEAD. The tests below are therefore about *refusing
//! correctly*, and the one test that is about the bytes being right is about the
//! two ends agreeing rather than about integrity.

use linklet_core::frame::{
    FrameError, HEADER_BYTES, Kind, MAGIC, MAX_PAYLOAD, decode_header, encode, frame_length, header,
};

// --- the format, pinned ------------------------------------------------------

#[test]
fn the_header_is_exactly_these_bytes() {
    // Item 5. The wire format is a fact rather than a convention shared by two
    // files: big-endian, magic first, kind second. Pinned by value, so a change to
    // the layout cannot be made in one place and forgotten in the other.
    // A length that shows all four bytes of the field in order, and is under the
    // limit: the first version of this test used 0x01020304, which is 16,909,060 and
    // therefore *above* the 16 MiB ceiling. The test failed and the code was right,
    // which is the limit check doing its job on the person writing the test.
    let head = header(Kind::Hello, 0x0001_0203).expect("a valid length");
    assert_eq!(head, [0x4C, 0x01, 0x00, 0x01, 0x02, 0x03]);

    let sealed = header(Kind::Sealed, 3).expect("a valid length");
    assert_eq!(sealed, [0x4C, 0x02, 0x00, 0x00, 0x00, 0x03]);
}

#[test]
fn the_kind_bytes_are_pinned() {
    // Item 8. If these two numbers ever swap, an agent and a host built at
    // different times disagree about what a message is -- and the failure would
    // look like an authentication problem, which is the wrong place to look.
    assert_eq!(Kind::Hello.as_byte(), 1);
    assert_eq!(Kind::Sealed.as_byte(), 2);
    assert_eq!(Kind::from_byte(1), Some(Kind::Hello));
    assert_eq!(Kind::from_byte(2), Some(Kind::Sealed));
    assert_eq!(Kind::from_byte(0), None);
    assert_eq!(Kind::from_byte(3), None);
}

#[test]
fn the_magic_byte_is_pinned() {
    assert_eq!(MAGIC, 0x4C);
    assert_eq!(HEADER_BYTES, 6);
}

#[test]
fn a_frame_round_trips() {
    let payload = b"a sealed body would go here";
    let bytes = encode(Kind::Sealed, payload).expect("a valid length");

    assert_eq!(
        bytes.len(),
        frame_length(Kind::Sealed, payload.len()).expect("valid")
    );
    let (kind, length) = decode_header(&bytes).expect("a header this module wrote");
    assert_eq!(kind, Kind::Sealed);
    assert_eq!(length, payload.len());
    assert_eq!(&bytes[HEADER_BYTES..], payload);
}

// --- item 1: the length lies, and is longer than the data --------------------
//
// The reader waits forever. This module has no clock, so the defence is a read
// timeout in the adapter. What can be checked here is that the header is usable
// without the payload: a reader must be able to learn the length and then decide
// whether to keep waiting, rather than blocking inside this module.

#[test]
fn a_header_can_be_read_without_the_payload_having_arrived() {
    let head = header(Kind::Sealed, 1024).expect("a valid length");
    let (kind, length) = decode_header(&head).expect("a complete header");
    assert_eq!((kind, length), (Kind::Sealed, 1024));
}

// --- item 3: the length is enormous ------------------------------------------

#[test]
fn a_hostile_length_is_refused_before_anything_is_allocated() {
    // The attacker's number: the largest a u32 can carry. `decode_header` returns
    // the length rather than allocating from it, so this is refused with the
    // number in the error and nothing was reserved to hold it.
    let hostile = [MAGIC, Kind::Sealed.as_byte(), 0xFF, 0xFF, 0xFF, 0xFF];
    assert_eq!(
        decode_header(&hostile),
        Err(FrameError::TooLarge {
            declared: 0xFFFF_FFFF
        })
    );
}

#[test]
fn the_limit_itself_is_allowed_and_one_past_it_is_not() {
    // An off-by-one at the boundary is the version of this that ships.
    let at_limit = [MAGIC, Kind::Sealed.as_byte()]
        .into_iter()
        .chain((MAX_PAYLOAD as u32).to_be_bytes())
        .collect::<Vec<u8>>();
    assert_eq!(
        decode_header(&at_limit),
        Ok((Kind::Sealed, MAX_PAYLOAD)),
        "the limit itself must be usable"
    );

    let past = [MAGIC, Kind::Sealed.as_byte()]
        .into_iter()
        .chain(((MAX_PAYLOAD + 1) as u32).to_be_bytes())
        .collect::<Vec<u8>>();
    assert_eq!(
        decode_header(&past),
        Err(FrameError::TooLarge {
            declared: MAX_PAYLOAD + 1
        })
    );
}

#[test]
fn a_sender_cannot_build_a_frame_the_receiver_would_refuse() {
    // The two checks are the same checks, so a length problem is found before a
    // partial transfer rather than after one.
    assert_eq!(
        encode(Kind::Sealed, &vec![0u8; MAX_PAYLOAD + 1]),
        Err(FrameError::TooLarge {
            declared: MAX_PAYLOAD + 1
        })
    );
    assert_eq!(
        header(Kind::Sealed, MAX_PAYLOAD + 1),
        Err(FrameError::TooLarge {
            declared: MAX_PAYLOAD + 1
        })
    );
}

// --- item 4: the length is zero ----------------------------------------------

#[test]
fn an_empty_payload_is_refused_in_both_directions() {
    // Nothing this protocol sends is empty, and accepting zero would make a
    // zero-filled buffer a valid message -- which is a bad property to have for
    // something whose reader is fed by a network.
    assert_eq!(encode(Kind::Sealed, b""), Err(FrameError::Empty));
    assert_eq!(header(Kind::Sealed, 0), Err(FrameError::Empty));

    let zero = [MAGIC, Kind::Sealed.as_byte(), 0, 0, 0, 0];
    assert_eq!(decode_header(&zero), Err(FrameError::Empty));
}

// --- items 6 and 7: truncation, and a service that is not this one -----------

#[test]
fn every_truncated_header_is_refused_and_says_how_much_arrived() {
    // Item 6. `read` may return fewer bytes than asked for, and the classic bug is
    // treating a partial read as a whole one. This module takes a complete header,
    // so there is no way to hand it half of one and have it guess -- and the error
    // carries the count so a reader can tell a slow peer from a wrong one.
    let full = header(Kind::Hello, 32).expect("a valid length");
    for got in 0..HEADER_BYTES {
        assert_eq!(
            decode_header(&full[..got]),
            Err(FrameError::Truncated { got }),
            "{got} bytes should be too few"
        );
    }
}

#[test]
fn a_first_byte_that_is_not_the_magic_is_refused_by_name() {
    // Item 7. A service on the wrong port is refused at the first byte rather than
    // being read as a length three bytes later -- which is the difference between
    // a clear error and a puzzling one.
    for wrong in [0x00u8, 0x47, 0x4B, 0xFF] {
        let head = [wrong, Kind::Sealed.as_byte(), 0, 0, 0, 8];
        assert_eq!(
            decode_header(&head),
            Err(FrameError::NotThisProtocol { found: wrong }),
            "{wrong:#04x} should not be accepted as the magic byte"
        );
    }
}

// --- item 8: the right protocol, the wrong kind ------------------------------

#[test]
fn an_unknown_kind_is_refused_by_number() {
    for found in [0u8, 3, 4, 255] {
        let head = [MAGIC, found, 0, 0, 0, 8];
        assert_eq!(
            decode_header(&head),
            Err(FrameError::UnknownKind { found }),
            "kind {found} should be refused"
        );
    }
}

// --- the refusals say what happened ------------------------------------------

#[test]
fn every_refusal_names_the_number_that_caused_it() {
    // A protocol reader's error is the whole of what a person sees. "too large"
    // does not say whether the sender is broken or hostile; "declared 4294967295"
    // does.
    let cases: [(FrameError, &str); 5] = [
        (
            FrameError::TooLarge {
                declared: 0xFFFF_FFFF,
            },
            "4294967295",
        ),
        (FrameError::Truncated { got: 3 }, "3"),
        (FrameError::UnknownKind { found: 9 }, "9"),
        (FrameError::NotThisProtocol { found: 0x41 }, "0x41"),
        (FrameError::Empty, "no payload"),
    ];

    for (error, expected) in cases {
        let text = error.to_string();
        assert!(
            text.contains(expected),
            "{error:?} rendered as {text:?}, which does not mention {expected:?}"
        );
    }
}

// --- what the layer above relies on ------------------------------------------

#[test]
fn a_frame_carries_arbitrary_bytes_unchanged() {
    // The payload is ciphertext, so it contains every byte value and every length
    // of run. A framing layer that mangled one of them would corrupt a message that
    // the AEAD would then reject -- a refusal, but one that sends a reader looking
    // at the wrong layer.
    let payload: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    let bytes = encode(Kind::Sealed, &payload).expect("a valid length");
    let (_, length) = decode_header(&bytes).expect("a header this module wrote");

    assert_eq!(length, payload.len());
    assert_eq!(&bytes[HEADER_BYTES..], &payload[..]);
}

#[test]
fn the_payload_is_not_interpreted_at_all() {
    // Including a payload that begins with the magic byte and a plausible header.
    // The framing must not resynchronise on it: the length said where the message
    // ends, and that is where it ends. This is item 2 seen from the inside -- the
    // defence against desynchronisation is that nothing is ever scanned for.
    let mut payload = header(Kind::Sealed, 8).expect("a valid length").to_vec();
    payload.extend_from_slice(&[0xAA; 64]);

    let bytes = encode(Kind::Sealed, &payload).expect("a valid length");
    let (kind, length) = decode_header(&bytes).expect("a header this module wrote");
    assert_eq!((kind, length), (Kind::Sealed, payload.len()));
}
