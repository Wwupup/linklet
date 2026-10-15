//! Bounded observation of the current account and operating-system identity.
use linklet_core::identity::Identity;
use serde_json::json;
use std::{
    io::Read,
    path::Path,
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Observes this process without testing sudo eligibility or changing its privileges.
/// Auxiliary failures are warnings, never a failure of authenticated identity.
pub fn observe(startup: &Path, version: &str) -> Identity {
    let mut identity = Identity::named("linklet-agent", version);
    identity.os = Some(std::env::consts::OS.into());
    identity.arch = Some(std::env::consts::ARCH.into());
    identity.startup_cwd = Some(startup.to_string_lossy().into_owned());
    identity.shell = Some(
        if cfg!(windows) {
            "cmd.exe /D /S /C"
        } else {
            "sh -c"
        }
        .into(),
    );
    identity.observed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|v| v.as_millis().min(u128::from(u64::MAX)) as u64);
    identity.limits = json!({"protocol":linklet_core::wire::PROTOCOL_VERSION,
        "exec_timeout_seconds":600,"stream_retained_bytes":1048576,
        "file_read_bytes":16777216,"tail_lines":2000,"batch_targets":256,"batch_concurrency":4,
        "transfer_bytes":4294967296u64,"spawn_observe_ms":5000});
    match account() {
        Ok((hostname, account, elevated)) => {
            identity.hostname = Some(hostname);
            identity.account = Some(account);
            identity.elevated = Some(elevated);
        }
        Err(error) => identity.warnings.push(error),
    }
    identity
}
fn account() -> Result<(String, String, bool), String> {
    #[cfg(windows)]
    let mut command = {
        let mut command = crate::background_command("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", r#"[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false);$i=[Security.Principal.WindowsIdentity]::GetCurrent();$p=New-Object Security.Principal.WindowsPrincipal($i);@{hostname=[Environment]::MachineName;account=$i.Name;elevated=$p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)}|ConvertTo-Json -Compress"#]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = crate::background_command("sh");
        command.args(["-c", "uname -n && id -un && id -u"]);
        command
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot observe process account: {e}"))?;
    let stdout = child.stdout.take().ok_or("account stdout unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(8192).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("account observation did not finish: {other:?}"));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "account reader stopped")?
        .map_err(|e| e.to_string())?;
    if !status?.success() {
        return Err("account observation failed".into());
    }
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        Ok((
            v["hostname"].as_str().ok_or("hostname missing")?.into(),
            v["account"].as_str().ok_or("account missing")?.into(),
            v["elevated"].as_bool().ok_or("elevation missing")?,
        ))
    }
    #[cfg(not(windows))]
    {
        let mut lines = text.lines();
        let hostname = lines.next().ok_or("hostname missing")?.to_string();
        let account = lines.next().ok_or("account missing")?.to_string();
        let uid = lines
            .next()
            .ok_or("effective uid missing")?
            .parse::<u32>()
            .map_err(|e| e.to_string())?;
        Ok((hostname, account, uid == 0))
    }
}
