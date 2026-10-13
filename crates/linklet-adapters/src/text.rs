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
    let offset = total.saturating_sub(bytes.len());
    if let Some((text, lossy)) = linklet_core::output::unicode(bytes, encoding, offset) {
        return Text::decoded(text, total, bytes.len(), encoding.tag(), lossy, None);
    }
    match legacy(bytes, source) {
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
fn legacy(bytes: &[u8], source: TextSource) -> Result<(String, Encoding, bool), String> {
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
fn legacy(bytes: &[u8], source: TextSource) -> Result<(String, Encoding, bool), String> {
    let code_page = match source {
        TextSource::File => "[Text.Encoding]::Default.CodePage",
        TextSource::Command => {
            r#"$( $c = & "$env:SystemRoot\System32\chcp.com" 2>$null; if ($LASTEXITCODE -eq 0 -and $c -match '(\d+)\s*$') { [int]$Matches[1] } else { [int][Microsoft.Win32.Registry]::GetValue('HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Nls\CodePage','OEMCP',$null) } )"#
        }
    };
    decode_code_page(bytes, code_page)
}

#[cfg(windows)]
fn decode_code_page(bytes: &[u8], code_page: &str) -> Result<(String, Encoding, bool), String> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    // Only the code-page expression is interpolated, never captured text or a path.
    let script = format!(
        r#"$ErrorActionPreference='Stop';
$p={code_page};
$m=New-Object IO.MemoryStream; [Console]::OpenStandardInput().CopyTo($m); $b=$m.ToArray();
$e=[Text.Encoding]::GetEncoding($p,[Text.EncoderFallback]::ExceptionFallback,[Text.DecoderFallback]::ExceptionFallback);
$loss=$false; try {{ $t=$e.GetString($b) }} catch {{ $loss=$true; $t=[Text.Encoding]::GetEncoding($p).GetString($b) }};
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false);
@{{text=$t;page=$p;lossy=$loss}} | ConvertTo-Json -Compress"#
    );
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
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
    let value: serde_json::Value =
        serde_json::from_slice(&output).map_err(|e| format!("invalid decoder reply: {e}"))?;
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn code_page_936_decodes_captured_bytes_and_reports_invalid_bytes() {
        let (text, encoding, lossy) =
            decode_code_page(&[0xd6, 0xd0, 0xce, 0xc4], "936").expect("decoder");
        assert_eq!(text, "\u{4e2d}\u{6587}");
        assert_eq!(encoding, Encoding::CodePage(936));
        assert!(!lossy);
        assert!(decode_code_page(&[0x81], "936").expect("decoder").2);
    }
}
