//! Bounded command output and Unicode detection, independent of I/O.
use crate::search::{Encoding, sniff};
use std::collections::VecDeque;

/// Maximum raw bytes retained per command stream (one mebibyte).
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
const PREFIX_LIMIT: usize = 256;

/// A stream's byte count, small encoding sample, and bounded tail.
pub struct Capture {
    tail: VecDeque<u8>,
    prefix: Vec<u8>,
    total: usize,
    utf8_pending: Vec<u8>,
    invalid_utf8: bool,
    legacy: Vec<(u16, encoding_rs::Decoder)>,
    discarded_end: VecDeque<u8>,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            tail: VecDeque::new(),
            prefix: Vec::new(),
            total: 0,
            utf8_pending: Vec::new(),
            invalid_utf8: false,
            discarded_end: VecDeque::new(),
            legacy: [
                (932, encoding_rs::SHIFT_JIS),
                (936, encoding_rs::GBK),
                (949, encoding_rs::EUC_KR),
                (950, encoding_rs::BIG5),
                (54936, encoding_rs::GB18030),
            ]
            .into_iter()
            .map(|(page, encoding)| (page, encoding.new_decoder_without_bom_handling()))
            .collect(),
        }
    }
}

/// A bounded stream window and the facts needed to decode its first character.
pub struct Captured {
    /// Final raw bytes, at most OUTPUT_LIMIT.
    pub bytes: Vec<u8>,
    /// Total raw bytes emitted.
    pub total: usize,
    /// Encoding detected before clipping.
    pub encoding: Encoding,
    /// Leading fragment lengths for Windows multibyte code pages.
    pub boundary_skips: Vec<(u16, usize)>,
}

impl Capture {
    /// Adds bytes while discarding the oldest bytes beyond the fixed ceiling.
    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        let prefix_room = PREFIX_LIMIT - self.prefix.len();
        self.prefix
            .extend_from_slice(&bytes[..bytes.len().min(prefix_room)]);
        for chunk in bytes.chunks(4096) {
            if self.invalid_utf8 {
                break;
            }
            self.utf8_pending.extend_from_slice(chunk);
            match std::str::from_utf8(&self.utf8_pending) {
                Ok(_) => self.utf8_pending.clear(),
                Err(error) if error.error_len().is_none() => {
                    self.utf8_pending.drain(..error.valid_up_to());
                }
                Err(_) => {
                    self.invalid_utf8 = true;
                    self.utf8_pending.clear();
                }
            }
        }
        if bytes.len() >= OUTPUT_LIMIT {
            self.discard_tail(self.tail.len());
            observe_discard(
                &mut self.legacy,
                &mut self.discarded_end,
                &bytes[..bytes.len() - OUTPUT_LIMIT],
            );
            self.tail.extend(&bytes[bytes.len() - OUTPUT_LIMIT..]);
        } else {
            let discard = (self.tail.len() + bytes.len()).saturating_sub(OUTPUT_LIMIT);
            self.discard_tail(discard);
            self.tail.extend(bytes);
        }
    }
    fn discard_tail(&mut self, count: usize) {
        let (first, second) = self.tail.as_slices();
        let first_count = count.min(first.len());
        observe_discard(
            &mut self.legacy,
            &mut self.discarded_end,
            &first[..first_count],
        );
        observe_discard(
            &mut self.legacy,
            &mut self.discarded_end,
            &second[..count - first_count],
        );
        self.tail.drain(..count);
    }

    /// Returns the raw tail, total byte count, and encoding detected over the stream.
    /// UTF-8 validation includes discarded bytes, so clipping cannot cause a legacy guess.
    pub fn finish(self) -> Captured {
        let sample = sniff(&self.prefix);
        let encoding = match sample {
            Encoding::Utf8Bom | Encoding::Utf16Le | Encoding::Utf16Be => sample,
            _ if !self.invalid_utf8 && self.utf8_pending.is_empty() => Encoding::Utf8,
            _ => Encoding::Oem,
        };
        let bytes: Vec<u8> = self.tail.into_iter().collect();
        let previous: Vec<u8> = self.discarded_end.into_iter().collect();
        let mut boundary_skips = Vec::new();
        for (page, mut decoder) in self.legacy {
            let mut output = [0u16; 8];
            let (status, _, _) =
                decoder.decode_to_utf16_without_replacement(&[], &mut output, true);
            if let encoding_rs::DecoderResult::Malformed(pending, _) = status {
                let start = previous.len().saturating_sub(usize::from(pending));
                for count in 1..=bytes.len().min(4) {
                    let input = [&previous[start..], &bytes[..count]].concat();
                    let mut fresh = decoder.encoding().new_decoder_without_bom_handling();
                    let (status, _, written) =
                        fresh.decode_to_utf16_without_replacement(&input, &mut output, false);
                    if matches!(status, encoding_rs::DecoderResult::Malformed(..)) {
                        break;
                    }
                    if written > 0 {
                        boundary_skips.push((page, count));
                        break;
                    }
                }
            }
        }
        Captured {
            bytes,
            total: self.total,
            encoding,
            boundary_skips,
        }
    }
}

fn observe_discard(
    decoders: &mut [(u16, encoding_rs::Decoder)],
    previous: &mut VecDeque<u8>,
    bytes: &[u8],
) {
    for chunk in bytes.chunks(4096) {
        let mut output = [0u16; 8192];
        for (_, decoder) in decoders.iter_mut() {
            let (_, read, _, _) = decoder.decode_to_utf16(chunk, &mut output, false);
            debug_assert_eq!(read, chunk.len());
        }
    }
    for byte in bytes.iter().rev().take(3).rev() {
        if previous.len() == 3 {
            previous.pop_front();
        }
        previous.push_back(*byte);
    }
}

/// Decodes Unicode without platform code pages. The offset is the number of discarded
/// leading raw bytes; incomplete characters at that boundary are omitted, not replaced.
/// Returns text and whether bytes inside the retained portion were invalid.
pub fn unicode(bytes: &[u8], encoding: Encoding, offset: usize) -> Option<(String, bool)> {
    match encoding {
        Encoding::Utf8 | Encoding::Utf8Bom => {
            let mut bytes = bytes;
            if offset > 0 {
                let skip = bytes
                    .iter()
                    .take(3)
                    .take_while(|b| **b & 0xc0 == 0x80)
                    .count();
                bytes = &bytes[skip..];
            } else if encoding == Encoding::Utf8Bom && bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                bytes = &bytes[3..];
            }
            Some((
                String::from_utf8_lossy(bytes).into_owned(),
                std::str::from_utf8(bytes).is_err(),
            ))
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let bytes = &bytes[(offset % 2).min(bytes.len())..];
            let mut units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|b| {
                    if encoding == Encoding::Utf16Le {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                })
                .collect();
            if (offset == 0 && units.first() == Some(&0xfeff))
                || (offset > 0 && units.first().is_some_and(|u| (0xdc00..=0xdfff).contains(u)))
            {
                units.remove(0);
            }
            let mut lossy = !bytes.len().is_multiple_of(2);
            let mut text: String = char::decode_utf16(units)
                .map(|c| {
                    c.unwrap_or_else(|_| {
                        lossy = true;
                        char::REPLACEMENT_CHARACTER
                    })
                })
                .collect();
            if !bytes.len().is_multiple_of(2) {
                text.push(char::REPLACEMENT_CHARACTER);
            }
            Some((text, lossy))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipping_and_chunk_boundaries_do_not_change_utf8_detection() {
        let mut capture = Capture::default();
        let bytes = "\u{4e2d}".repeat(OUTPUT_LIMIT / 3 + 10).into_bytes();
        for chunk in bytes.chunks(8192) {
            capture.push(chunk);
        }
        let Captured {
            bytes: tail,
            total,
            encoding,
            ..
        } = capture.finish();
        assert_eq!(tail.len(), OUTPUT_LIMIT);
        assert_eq!(total, bytes.len());
        assert_eq!(encoding, Encoding::Utf8);
        let (text, lossy) = unicode(&tail, encoding, total - tail.len()).expect("unicode");
        assert!(!lossy);
        assert!(text.chars().all(|c| c == '\u{4e2d}'));
    }
    #[test]
    fn utf16_clipping_drops_half_a_surrogate_pair() {
        assert_eq!(
            unicode(&[0x00, 0xde, b'x', 0], Encoding::Utf16Le, 2),
            Some(("x".into(), false))
        );
    }
}
