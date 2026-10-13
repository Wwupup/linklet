//! Reading stateless incremental log pages from an open file handle.
use linklet_core::{
    result::{ErrorCode, OperationError},
    search::{Encoding, sniff},
    tail::{ByteRange, Cursor, Page, decode_page},
    transfer::Destination,
    wire::TailRequest,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
const READ_LIMIT: u64 = 16 * 1024 * 1024;
fn bad(message: impl Into<String>) -> OperationError {
    OperationError::new(ErrorCode::InvalidArgument, "cursor", message)
}
fn hash(bytes: &[u8]) -> String {
    linklet_core::wire::to_hex(&Sha256::digest(bytes))
}
fn window(
    file: &mut File,
    path: &Path,
    start: u64,
    count: u64,
    read: &mut u64,
) -> Result<Vec<u8>, OperationError> {
    file.seek(SeekFrom::Start(start))
        .map_err(|e| crate::failure::io(e, "seek", path))?;
    let mut bytes = Vec::new();
    file.take(count.min(READ_LIMIT.saturating_sub(*read)))
        .read_to_end(&mut bytes)
        .map_err(|e| crate::failure::io(e, "read", path))?;
    *read += bytes.len() as u64;
    Ok(bytes)
}
fn anchor(
    file: &mut File,
    path: &Path,
    position: u64,
    read: &mut u64,
) -> Result<String, OperationError> {
    let count = position.min(64);
    Ok(hash(&window(file, path, position - count, count, read)?))
}
#[cfg(windows)]
fn identity(file: &File) -> std::io::Result<String> {
    let info = winapi_util::file::information(file)?;
    Ok(format!(
        "{}:{}",
        info.volume_serial_number(),
        info.file_index()
    ))
}
#[cfg(unix)]
fn identity(file: &File) -> std::io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}
fn detect_encoding(prefix: &[u8], bytes: &[u8], offset: u64) -> Result<Encoding, OperationError> {
    if prefix == [0xff] {
        return Ok(Encoding::Utf16Le);
    }
    if prefix == [0xfe] {
        return Ok(Encoding::Utf16Be);
    }
    let first = sniff(prefix);
    if matches!(
        first,
        Encoding::Utf8Bom | Encoding::Utf16Le | Encoding::Utf16Be
    ) {
        return Ok(first);
    }
    let skip = if offset > 0 {
        bytes
            .iter()
            .take(3)
            .take_while(|b| **b & 0xc0 == 0x80)
            .count()
    } else {
        0
    };
    let bytes = &bytes[skip..];
    if std::str::from_utf8(bytes).is_ok()
        || std::str::from_utf8(bytes).is_err_and(|e| e.error_len().is_none())
    {
        return Ok(Encoding::Utf8);
    }
    let sample = &bytes[..bytes.len().min(4096)];
    let decoded = crate::decode_text(sample, sample.len(), Encoding::Oem, crate::TextSource::File);
    if let Some(error) = decoded.decode_error() {
        return Err(OperationError::new(ErrorCode::IoError, "decode", error));
    }
    Encoding::named(decoded.encoding()).ok_or_else(|| bad("unknown file encoding"))
}
fn last_lines(bytes: &[u8], encoding: Encoding, count: usize) -> usize {
    let width = if matches!(encoding, Encoding::Utf16Le | Encoding::Utf16Be) {
        2
    } else {
        1
    };
    let mut seen = 0;
    for index in (0..bytes.len() / width).rev().map(|n| n * width) {
        let newline = match encoding {
            Encoding::Utf16Le => bytes[index..].starts_with(&[10, 0]),
            Encoding::Utf16Be => bytes[index..].starts_with(&[0, 10]),
            _ => bytes[index] == 10,
        };
        if newline && index + width != bytes.len() {
            seen += 1;
            if seen == count {
                return index + width;
            }
        }
    }
    0
}
/// Reads a bounded page or returns a typed file/cursor failure. The cursor advances
/// only over delivered text. Identity comes from the same handle as the bytes.
/// Boundary samples detect many rewrites, but cannot prove arbitrary edit-free history.
pub fn read(paths: &Destination, request: &TailRequest) -> Result<Page, OperationError> {
    if !(4..=linklet_core::tail::PAGE_BYTES).contains(&request.content_bytes) {
        return Err(bad("invalid content budget"));
    }
    let target = paths
        .resolve(&request.path)
        .map_err(|e| bad(e.to_string()))?;
    let path =
        std::fs::canonicalize(&target).map_err(|e| crate::failure::io(e, "resolve", &target))?;
    let name = path.to_string_lossy().into_owned();
    let mut file = File::open(&path).map_err(|e| crate::failure::io(e, "open", &path))?;
    let metadata = file
        .metadata()
        .map_err(|e| crate::failure::io(e, "stat", &path))?;
    if !metadata.is_file() {
        return Err(bad("tail requires a regular file"));
    }
    let size = metadata.len();
    let identity = identity(&file).map_err(|e| crate::failure::io(e, "file_identity", &path))?;
    let old = request.cursor.as_deref().map(Cursor::parse).transpose()?;
    if old.as_ref().is_some_and(|c| c.path != name) {
        return Err(bad("cursor belongs to a different resolved path"));
    }
    let mut bytes_read = 0;
    let prefix = window(&mut file, &path, 0, size.min(256), &mut bytes_read)?;
    let mut changed = None;
    if let Some(cursor) = &old {
        changed = if cursor.identity != identity {
            Some("replaced")
        } else if size < cursor.size || size < cursor.position {
            Some("shortened")
        } else if prefix.len() < cursor.prefix_bytes
            || hash(&prefix[..cursor.prefix_bytes]) != cursor.prefix
            || anchor(&mut file, &path, cursor.position, &mut bytes_read)? != cursor.anchor
        {
            Some("boundary_changed")
        } else {
            None
        };
    }
    let mut warnings = Vec::new();
    let (mut start, mut bytes, mut encoding, mut mid_line) = if let Some(cursor) = &old
        && changed.is_none()
    {
        let bytes = window(
            &mut file,
            &path,
            cursor.position,
            (size - cursor.position).min((request.content_bytes * 4 + 4) as u64),
            &mut bytes_read,
        )?;
        (
            cursor.position,
            bytes,
            Encoding::named(&cursor.encoding).ok_or_else(|| bad("unknown cursor encoding"))?,
            cursor.mid_line,
        )
    } else {
        let mut start = if changed.is_some() {
            0
        } else {
            size.saturating_sub(READ_LIMIT - 512)
        };
        let sample = sniff(&prefix);
        if matches!(sample, Encoding::Utf16Le | Encoding::Utf16Be) && start % 2 != 0 {
            start += 1;
        }
        let bytes = window(
            &mut file,
            &path,
            start,
            (size - start).min(READ_LIMIT - 512),
            &mut bytes_read,
        )?;
        let encoding = detect_encoding(&prefix, &bytes, start)?;
        let skip = if changed.is_some() {
            0
        } else {
            last_lines(&bytes, encoding, request.count)
        };
        if start > 0 && skip == 0 {
            warnings.push("Initial tail starts inside the bounded file window; earlier bytes of this line were omitted.".into());
        }
        (
            start + skip as u64,
            bytes[skip..].to_vec(),
            encoding,
            start > 0 && skip == 0,
        )
    };
    if old.is_some()
        && changed.is_none()
        && encoding == Encoding::Utf8
        && !prefix.starts_with(&[0xef, 0xbb, 0xbf])
        && std::str::from_utf8(&bytes).is_err_and(|e| e.error_len().is_some())
    {
        let detected = detect_encoding(&prefix, &bytes, start)?;
        if detected != encoding {
            changed = Some("encoding_changed");
            start = 0;
            mid_line = false;
            bytes = window(
                &mut file,
                &path,
                0,
                READ_LIMIT.saturating_sub(bytes_read + 128),
                &mut bytes_read,
            )?;
            encoding = detect_encoding(&prefix, &bytes, 0)?;
        }
    }
    let mut skip = 0;
    if old.is_none() && mid_line {
        match encoding {
            Encoding::Utf8 | Encoding::Utf8Bom => {
                skip = bytes
                    .iter()
                    .take(3)
                    .take_while(|b| **b & 0xc0 == 0x80)
                    .count()
            }
            Encoding::Utf16Le | Encoding::Utf16Be if bytes.len() >= 2 => {
                let unit = if encoding == Encoding::Utf16Le {
                    u16::from_le_bytes([bytes[0], bytes[1]])
                } else {
                    u16::from_be_bytes([bytes[0], bytes[1]])
                };
                if (0xdc00..=0xdfff).contains(&unit) {
                    skip = 2;
                }
            }
            _ => {}
        }
    }
    start += skip as u64;
    let decoded = decode_page(&bytes[skip..], encoding, start, request.content_bytes)?;
    let end = start + decoded.consumed as u64;
    let pending = if end + decoded.pending as u64 == size {
        decoded.pending
    } else {
        0
    };
    let has_more = end + (pending as u64) < size;
    let ends_mid_line = if decoded.text.is_empty() {
        mid_line
    } else {
        !decoded.text.ends_with('\n')
    };
    if changed.is_some() {
        warnings.push("File changed; restarted from its beginning. Continuity with the previous file cannot be established.".into());
        mid_line = false;
    }
    let cursor = Cursor {
        version: 1,
        target: old.as_ref().map(|c| c.target.clone()).unwrap_or_default(),
        path: name.clone(),
        identity,
        encoding: encoding.tag(),
        position: end,
        size,
        prefix: hash(&prefix),
        prefix_bytes: prefix.len(),
        anchor: anchor(&mut file, &path, end, &mut bytes_read)?,
        mid_line: ends_mid_line,
    };
    Ok(Page {
        path: name,
        text: decoded.text.clone(),
        encoding: encoding.tag(),
        lossy: decoded.lossy,
        range: ByteRange { start, end },
        next_cursor: cursor.token(),
        has_more,
        pending_bytes: pending,
        file_bytes: size,
        bytes_read,
        count: decoded.text.lines().count(),
        starts_mid_line: mid_line,
        ends_mid_line,
        file_changed: changed.map(str::to_string),
        continuity: if old.is_some() && changed.is_none() {
            "boundary_checked"
        } else {
            "unverified"
        }
        .into(),
        skipped_prefix: old.is_none() && start > 0,
        warnings,
    })
}
