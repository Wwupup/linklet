//! Stateless log pages and character-safe byte accounting.
use crate::{result::OperationError, search::Encoding};
use serde::{Deserialize, Serialize};
/// Maximum decoded text in one CLI page. MCP requests a smaller fixed budget.
pub const PAGE_BYTES: usize = 1024 * 1024;
/// Maximum cursor JSON size before parsing.
pub const CURSOR_BYTES: usize = 128 * 1024;
/// Source byte interval, with an exclusive end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    /// First delivered source byte.
    pub start: u64,
    /// First source byte not delivered by this page.
    pub end: u64,
}
/// A bounded log page preserving the file's original newline characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// Resolved absolute path.
    pub path: String,
    /// Decoded text, including original newline characters.
    pub text: String,
    /// Encoding fixed by the cursor until a detected file reset.
    pub encoding: String,
    /// Invalid source bytes were replaced while decoding.
    pub lossy: bool,
    /// Actual source byte range returned, excluding unconsumed fragments.
    pub range: ByteRange,
    /// Opaque continuation; no server-side cursor session is retained.
    pub next_cursor: String,
    /// Additional complete source content remains beyond this page.
    pub has_more: bool,
    /// Incomplete trailing character bytes left for a subsequent append.
    pub pending_bytes: usize,
    /// File size observed during this call.
    pub file_bytes: u64,
    /// Raw bytes read, including encoding and boundary samples.
    pub bytes_read: u64,
    /// Returned line count, including a final line fragment.
    pub count: usize,
    /// The page begins in a line whose prefix was delivered or omitted earlier.
    pub starts_mid_line: bool,
    /// The page ends before a newline.
    pub ends_mid_line: bool,
    /// A detected reset: replaced, shortened, or boundary_changed.
    pub file_changed: Option<String>,
    /// Boundary checks are observations, not a proof against all in-place edits.
    pub continuity: String,
    /// Whether the initial bounded tail omitted an earlier byte prefix.
    pub skipped_prefix: bool,
    /// Additional observations that limit interpretation.
    pub warnings: Vec<String>,
}
/// Decoder output with a cursor-safe source byte count.
#[derive(Debug)]
pub struct DecodedPage {
    /// Text that fits the requested UTF-8 content budget.
    pub text: String,
    /// Source bytes consumed, never including a trailing partial character.
    pub consumed: usize,
    /// Trailing incomplete character bytes at the source window's end.
    pub pending: usize,
    /// Whether invalid complete byte sequences needed replacement.
    pub lossy: bool,
}
fn codec(encoding: Encoding) -> Option<&'static encoding_rs::Encoding> {
    match encoding {
        Encoding::Utf8 | Encoding::Utf8Bom | Encoding::CodePage(65001) => Some(encoding_rs::UTF_8),
        Encoding::Utf16Le => Some(encoding_rs::UTF_16LE),
        Encoding::Utf16Be => Some(encoding_rs::UTF_16BE),
        Encoding::CodePage(932) => Some(encoding_rs::SHIFT_JIS),
        Encoding::CodePage(936) => Some(encoding_rs::GBK),
        Encoding::CodePage(949) => Some(encoding_rs::EUC_KR),
        Encoding::CodePage(950) => Some(encoding_rs::BIG5),
        Encoding::CodePage(54936) => Some(encoding_rs::GB18030),
        Encoding::CodePage(874) => Some(encoding_rs::WINDOWS_874),
        Encoding::CodePage(866) => Some(encoding_rs::IBM866),
        Encoding::CodePage(page) => {
            encoding_rs::Encoding::for_label(format!("windows-{page}").as_bytes())
        }
        _ => None,
    }
}
fn complete(bytes: &[u8], codec: &'static encoding_rs::Encoding) -> usize {
    let mut decoder = codec.new_decoder_without_bom_handling();
    let mut offset = 0;
    let mut output = [0u16; 4096];
    loop {
        let (status, read, _) =
            decoder.decode_to_utf16_without_replacement(&bytes[offset..], &mut output, false);
        offset += read;
        if matches!(status, encoding_rs::DecoderResult::InputEmpty) {
            break;
        }
    }
    match decoder
        .decode_to_utf16_without_replacement(&[], &mut output, true)
        .0
    {
        encoding_rs::DecoderResult::Malformed(pending, _) => {
            bytes.len().saturating_sub(usize::from(pending))
        }
        _ => bytes.len(),
    }
}
/// Decodes one page without consuming bytes whose text was not returned.
/// Incomplete final code units stay before the next cursor. Invalid complete
/// sequences use replacement characters and set lossy.
pub fn decode_page(
    bytes: &[u8],
    encoding: Encoding,
    offset: u64,
    budget: usize,
) -> Result<DecodedPage, OperationError> {
    let codec = codec(encoding);
    if codec.is_none() && encoding != Encoding::Latin1 {
        return Err(OperationError::new(
            crate::result::ErrorCode::IoError,
            "decode",
            format!("incremental decoding is unavailable for {}", encoding.tag()),
        ));
    }
    let bytes = &bytes[..bytes.len().min(budget.saturating_mul(4).saturating_add(4))];
    let full = codec.map_or(bytes.len(), |c| complete(bytes, c));
    let decode = |length: usize| {
        let end = codec.map_or(length, |c| complete(&bytes[..length], c));
        let (mut text, lossy) = match codec {
            Some(c) => {
                let (t, l) = c.decode_without_bom_handling(&bytes[..end]);
                (t.into_owned(), l)
            }
            None => (crate::search::decode_latin1(&bytes[..end]), false),
        };
        if offset == 0 && text.starts_with('\u{feff}') {
            text.remove(0);
        }
        (end, text, lossy)
    };
    let mut low = 0;
    let mut high = full;
    let mut best = (0, String::new(), false);
    while low <= high {
        let middle = low + (high - low) / 2;
        let value = decode(middle);
        if value.1.len() <= budget {
            best = value;
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    Ok(DecodedPage {
        text: best.1,
        consumed: best.0,
        pending: bytes.len() - full,
        lossy: best.2,
    })
}

/// Stateless cursor facts. Callers should pass the token through unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    /// Cursor format version, independent of the wire protocol.
    pub version: u8,
    /// Controller target address, checked before every authenticated read.
    pub target: String,
    /// Resolved remote path.
    pub path: String,
    /// Open-handle filesystem identity.
    pub identity: String,
    /// Decoder label carried across calls.
    pub encoding: String,
    /// Next unconsumed source byte.
    pub position: u64,
    /// File size when the cursor was issued.
    pub size: u64,
    /// Digest of a bounded prefix used to detect in-place rewrites.
    pub prefix: String,
    /// Number of prefix bytes hashed (a growing file must not change the sample).
    pub prefix_bytes: usize,
    /// Digest of up to 64 bytes immediately before position.
    pub anchor: String,
    /// Whether the next page continues an unfinished line.
    pub mid_line: bool,
}
impl Cursor {
    /// Encodes bounded cursor facts; this contains no credential or server session.
    pub fn token(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    /// Parses a bounded token and checks its basic invariants.
    pub fn parse(token: &str) -> Result<Self, OperationError> {
        let bad = |message: &str| {
            OperationError::new(crate::result::ErrorCode::InvalidArgument, "cursor", message)
        };
        if token.len() > CURSOR_BYTES {
            return Err(bad("cursor is too large"));
        }
        let cursor: Self = serde_json::from_str(token).map_err(|_| bad("invalid cursor"))?;
        if cursor.version != 1
            || cursor.position > cursor.size
            || cursor.prefix_bytes > 256
            || crate::search::Encoding::named(&cursor.encoding).is_none()
        {
            return Err(bad("invalid cursor version, encoding or position"));
        }
        Ok(cursor)
    }
}
