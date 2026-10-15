//! Decoding already-captured bytes; never reopen the source file.
use linklet_core::search::Encoding;
use linklet_core::wire::Text;

/// The platform fallback when bytes do not identify their encoding.
#[derive(Clone, Copy)]
pub enum TextSource {
    /// Console output code page on Windows, OEM when there is no console.
    Command,
    /// System ANSI code page on Windows; Latin-1 on Linux.
    File,
}

/// Decodes a bounded byte window. Decoder failures preserve lossy UTF-8 text
/// and attach an explicit error. Total includes any discarded prefix.
pub fn decode_text(bytes: &[u8], total: usize, encoding: Encoding, source: TextSource) -> Text {
    decode_window(bytes, total, encoding, source, &[])
}

/// Decodes a captured command stream with its multibyte boundary state.
pub fn decode_captured(captured: linklet_core::output::Captured) -> Text {
    decode_window(
        &captured.bytes,
        captured.total,
        captured.encoding,
        TextSource::Command,
        &captured.boundary_skips,
    )
}

fn decode_window(
    bytes: &[u8],
    total: usize,
    encoding: Encoding,
    source: TextSource,
    skips: &[(u16, usize)],
) -> Text {
    let offset = total.saturating_sub(bytes.len());
    if let Some((text, lossy)) = linklet_core::output::unicode(bytes, encoding, offset) {
        return Text::decoded(text, total, bytes.len(), encoding.tag(), lossy, None);
    }
    match legacy(bytes, source, skips) {
        Ok((text, encoding, lossy)) => {
            Text::decoded(text, total, bytes.len(), encoding.tag(), lossy, None)
        }
        Err(error) => Text::decoded(
            String::from_utf8_lossy(bytes).into_owned(),
            total,
            bytes.len(),
            "utf-8".into(),
            true,
            Some(error),
        ),
    }
}

#[cfg(not(windows))]
fn legacy(
    bytes: &[u8],
    source: TextSource,
    skips: &[(u16, usize)],
) -> Result<(String, Encoding, bool), String> {
    let _ = skips;
    Ok(match source {
        TextSource::File => (
            linklet_core::search::decode_latin1(bytes),
            Encoding::Latin1,
            false,
        ),
        TextSource::Command => (
            String::from_utf8_lossy(bytes).into_owned(),
            Encoding::Utf8,
            true,
        ),
    })
}

#[cfg(windows)]
fn legacy(
    bytes: &[u8],
    source: TextSource,
    skips: &[(u16, usize)],
) -> Result<(String, Encoding, bool), String> {
    let code_page = match source {
        TextSource::File => "[Text.Encoding]::Default.CodePage",
        TextSource::Command => {
            r#"$( $c = & "$env:SystemRoot\System32\chcp.com" 2>$null; if ($LASTEXITCODE -eq 0 -and $c -match '(\d+)\s*$') { [int]$Matches[1] } else { [int][Microsoft.Win32.Registry]::GetValue('HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Nls\CodePage','OEMCP',$null) } )"#
        }
    };
    decode_code_page(bytes, code_page, skips)
}

#[cfg(windows)]
fn decode_code_page(
    bytes: &[u8],
    code_page: &str,
    skips: &[(u16, usize)],
) -> Result<(String, Encoding, bool), String> {
    // Only the code-page expression is interpolated, never captured text or a path.
    let offsets = skips
        .iter()
        .map(|(page, count)| format!("{page}={count};"))
        .collect::<String>();
    let script = format!(
        r#"$ErrorActionPreference='Stop';
$p={code_page};
$m=New-Object IO.MemoryStream; [Console]::OpenStandardInput().CopyTo($m); $b=$m.ToArray();
$skipTable=@{{{offsets}}}; $skip=0; if($skipTable.ContainsKey($p)) {{ $skip=$skipTable[$p] }};
$e=[Text.Encoding]::GetEncoding($p,[Text.EncoderFallback]::ExceptionFallback,[Text.DecoderFallback]::ExceptionFallback);
$loss=$false; try {{ $t=$e.GetString($b,$skip,$b.Length-$skip) }} catch {{ $loss=$true; $t=[Text.Encoding]::GetEncoding($p).GetString($b,$skip,$b.Length-$skip) }};
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false);
@{{text=$t;page=$p;lossy=$loss}} | ConvertTo-Json -Compress"#
    );
    let value = run_decoder(bytes, &script)?;
    let text = value["text"].as_str().ok_or("decoder omitted text")?;
    let page = value["page"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .ok_or("decoder omitted code page")?;
    let lossy = value["lossy"]
        .as_bool()
        .ok_or("decoder omitted loss flag")?;
    Ok((text.into(), Encoding::CodePage(page), lossy))
}

#[cfg(windows)]
fn run_decoder(bytes: &[u8], script: &str) -> Result<serde_json::Value, String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = crate::background_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start code-page decoder: {e}"))?;
    let mut input = child.stdin.take().ok_or("decoder stdin unavailable")?;
    let mut output = child.stdout.take().ok_or("decoder stdout unavailable")?;
    let bytes = bytes.to_vec();
    let writer = std::thread::spawn(move || input.write_all(&bytes));
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match result {
                    Err(e) => format!("decoder wait failed: {e}"),
                    _ => "code-page decoder timed out after 5 seconds".into(),
                });
            }
        }
    };
    let written = writer
        .join()
        .map_err(|_| "decoder writer stopped".to_string());
    let read = reader
        .join()
        .map_err(|_| "decoder reader stopped".to_string());
    if !status?.success() {
        return Err("code-page decoder failed".into());
    }
    written?.map_err(|e| format!("decoder input failed: {e}"))?;
    let output = read?.map_err(|e| format!("decoder output failed: {e}"))?;
    serde_json::from_slice(&output).map_err(|e| format!("invalid decoder reply: {e}"))
}

/// Uses the Windows decoder for code pages not implemented by the pure decoder.
/// Input and output are bounded, and trailing incomplete characters remain unread.
#[cfg(windows)]
pub(crate) fn decode_page(
    bytes: &[u8],
    page: u16,
    budget: usize,
) -> Result<linklet_core::tail::DecodedPage, String> {
    let bytes = &bytes[..bytes.len().min(budget.saturating_mul(4).saturating_add(4))];
    let script = format!(
        "$page={page};$budget={budget};\n{}",
        include_str!("incremental-decoder.ps1")
    );
    let value = run_decoder(bytes, &script)?;
    let text = value["text"].as_str().ok_or("decoder omitted text")?;
    let consumed = value["consumed"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("decoder omitted consumed count")?;
    let pending = value["pending"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("decoder omitted pending count")?;
    let lossy = value["lossy"]
        .as_bool()
        .ok_or("decoder omitted loss flag")?;
    if text.len() > budget || consumed > bytes.len() || pending > bytes.len() - consumed {
        return Err("invalid decoder bounds".into());
    }
    Ok(linklet_core::tail::DecodedPage {
        text: text.into(),
        consumed,
        pending,
        lossy,
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn uncommon_code_pages_preserve_page_and_character_boundaries() {
        let single = decode_page(&[0x82; 5], 437, 4).expect("OEM page");
        assert_eq!(single.text, "\u{e9}\u{e9}");
        assert_eq!(single.consumed, 2);
        assert!(!single.lossy);
        let bytes = [0x88, 0x61, 0x88, 0x61, 0x88];
        let first = decode_page(&bytes, 1361, 4).expect("Johab page");
        assert_eq!(first.text, "\u{ac00}");
        assert_eq!(first.consumed, 2);
        assert_eq!(first.pending, 1);
        let second = decode_page(&bytes[first.consumed..], 1361, 4).expect("next");
        assert_eq!(second.text, "\u{ac00}");
        assert_eq!(second.consumed, 2);
        let half = decode_page(&[0x88], 1361, 4).expect("partial");
        assert_eq!((half.consumed, half.pending), (0, 1));
        assert!(half.text.is_empty());
        let invalid = decode_page(&[0x88, 0x20, 0x88], 1361, 8).expect("invalid byte");
        assert_eq!(invalid.text, "\u{fffd}");
        assert_eq!((invalid.consumed, invalid.pending), (2, 1));
        assert!(invalid.lossy);
        let large = decode_page(&[0x82; 40000], 437, 32768).expect("bounded page");
        assert_eq!(large.text.len(), 32768);
        assert_eq!(large.consumed, 16384);
    }

    #[test]
    fn a_clipped_double_byte_stream_keeps_character_boundaries() {
        let mut capture = linklet_core::output::Capture::default();
        let mut bytes = [0xd6, 0xd0].repeat(600_000);
        bytes.push(b'X');
        capture.push(&bytes);
        let captured = capture.finish();
        let (text, _, lossy) =
            decode_code_page(&captured.bytes, "936", &captured.boundary_skips).expect("decoder");
        assert!(!lossy);
        assert!(text.trim_end_matches('X').chars().all(|c| c == '\u{4e2d}'));
    }

    #[test]
    fn code_page_936_decodes_captured_bytes_and_reports_invalid_bytes() {
        let (text, encoding, lossy) =
            decode_code_page(&[0xd6, 0xd0, 0xce, 0xc4], "936", &[]).expect("decoder");
        assert_eq!(text, "\u{4e2d}\u{6587}");
        assert_eq!(encoding, Encoding::CodePage(936));
        assert!(!lossy);
        assert!(decode_code_page(&[0x81], "936", &[]).expect("decoder").2);
    }
}
