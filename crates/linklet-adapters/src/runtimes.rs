//! Startup observations of interpreters in the agent process's PATH.
use linklet_core::identity::{Runtime, RuntimeName, RuntimeResult, RuntimeSnapshot};
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const OUTPUT_LIMIT: u64 = 8192;
const PYTHON_PROBE: &str = "import json,sys; sys.exit(1) if sys.version_info.major != 3 else None; print(json.dumps({'linklet_runtime':1,'program':sys.executable,'version':sys.version.split()[0]}))";
const POWERSHELL_PROBE: &str = "[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false); @{linklet_runtime=1;program=[Diagnostics.Process]::GetCurrentProcess().MainModule.FileName;version=$PSVersionTable.PSVersion.ToString()}|ConvertTo-Json -Compress";

/// Checks Python 3 and PowerShell on PATH, once per agent startup.
/// Each family shares a two-second execution budget across its candidates.
/// Failures are observations, not startup errors; no packages are installed.
pub fn observe() -> RuntimeSnapshot {
    let observed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut names = vec![RuntimeName::Python, RuntimeName::Pwsh];
    if cfg!(windows) {
        names.push(RuntimeName::Powershell);
    }
    RuntimeSnapshot {
        observed_at_ms,
        entries: names
            .into_iter()
            .map(|name| Runtime {
                name,
                result: probe(name, &path, Duration::from_secs(2)),
            })
            .collect(),
    }
}

fn probe(name: RuntimeName, path: &OsStr, budget: Duration) -> RuntimeResult {
    let deadline = Instant::now() + budget;
    let candidates: &[&str] = match name {
        RuntimeName::Python => &["python3", "python"],
        RuntimeName::Powershell => &["powershell"],
        RuntimeName::Pwsh => &["pwsh"],
    };
    let mut result = RuntimeResult::NotFound {
        reason: format!(
            "{} not found on the agent's startup PATH",
            candidates.join(" or ")
        ),
    };
    for candidate in candidates {
        if Instant::now() >= deadline {
            return timed_out();
        }
        let program = match find(candidate, path) {
            Ok(Some(program)) => program,
            Ok(None) => continue,
            Err(reason) => {
                result = RuntimeResult::Failed { reason };
                continue;
            }
        };
        result = execute_probe(name, &program, deadline);
        if matches!(
            result,
            RuntimeResult::Available { .. } | RuntimeResult::TimedOut { .. }
        ) {
            break;
        }
    }
    result
}

fn find(name: &str, path: &OsStr) -> Result<Option<PathBuf>, String> {
    let filename = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    for directory in std::env::split_paths(path) {
        let candidate = directory.join(&filename);
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => {
                return std::path::absolute(&candidate)
                    .map(Some)
                    .map_err(|e| e.to_string());
            }
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(format!("cannot inspect {}: {e}", candidate.display())),
        }
    }
    Ok(None)
}

fn timed_out() -> RuntimeResult {
    RuntimeResult::TimedOut {
        reason: "runtime probe deadline reached".into(),
    }
}

fn capture(stream: impl Read + Send + 'static) -> Receiver<std::io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    rx
}

fn execute_probe(name: RuntimeName, program: &Path, deadline: Instant) -> RuntimeResult {
    let mut command = Command::new(program);
    match name {
        RuntimeName::Python => {
            command.args(["-I", "-S", "-c", PYTHON_PROBE]);
        }
        _ => {
            command.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                POWERSHELL_PROBE,
            ]);
        }
    }
    // Probe code cannot prompt, load Python startup hooks, or open a console window.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return RuntimeResult::Failed {
                reason: format!("cannot start {}: {e}", program.display()),
            };
        }
    };
    let result = (|| {
        let stdout = capture(child.stdout.take().ok_or_else(|| RuntimeResult::Failed {
            reason: "probe stdout unavailable".into(),
        })?);
        let stderr = capture(child.stderr.take().ok_or_else(|| RuntimeResult::Failed {
            reason: "probe stderr unavailable".into(),
        })?);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => return Err(timed_out()),
                Err(e) => {
                    return Err(RuntimeResult::Failed {
                        reason: format!("cannot observe probe: {e}"),
                    });
                }
            }
        };
        let read = |rx: Receiver<std::io::Result<Vec<u8>>>| {
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| timed_out())?
                .map_err(|e| RuntimeResult::Failed {
                    reason: format!("cannot read probe: {e}"),
                })
        };
        let stdout = read(stdout)?;
        let stderr = read(stderr)?;
        if stdout.len() > OUTPUT_LIMIT as usize || stderr.len() > OUTPUT_LIMIT as usize {
            return Err(RuntimeResult::Failed {
                reason: "runtime probe exceeded 8192 bytes per stream".into(),
            });
        }
        if !status.success() {
            return Err(RuntimeResult::Failed {
                reason: format!(
                    "{} exited with {status}: {}",
                    program.display(),
                    String::from_utf8_lossy(&stderr)
                        .chars()
                        .take(512)
                        .collect::<String>()
                        .trim()
                ),
            });
        }
        let value: serde_json::Value =
            serde_json::from_slice(&stdout).map_err(|e| RuntimeResult::Failed {
                reason: format!("invalid runtime probe reply: {e}"),
            })?;
        let executable = value["program"].as_str().unwrap_or_default();
        let version = value["version"].as_str().unwrap_or_default();
        if value["linklet_runtime"] != 1
            || !Path::new(executable).is_absolute()
            || executable.contains('\0')
            || version.trim().is_empty()
            || version.len() > 128
        {
            return Err(RuntimeResult::Failed {
                reason: "probe did not report an absolute interpreter path and version".into(),
            });
        }
        Ok(RuntimeResult::Available {
            program: executable.into(),
            version: version.into(),
        })
    })();
    // Never join a pipe reader without a deadline: a broken shim can leave a
    // descendant holding that pipe even after its own process exits.
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result.unwrap_or_else(|error| error)
}

#[cfg(test)]
#[path = "../../test-support/runtime_fixture_support.rs"]
mod runtime_fixture_support;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Discovery success is not a scheduling test. Expiration has its own short budget.
    const TEST_BUDGET: Duration = Duration::from_secs(10);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "linklet runtimes {} {}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            runtime_fixture_support::build(&root);
            Self(root)
        }
        fn mode(&self, name: &str, mode: &str) {
            std::fs::write(self.0.join(format!("{name}.mode")), mode).expect("mode");
        }
        fn probe(&self, name: RuntimeName, budget: Duration) -> RuntimeResult {
            probe(
                name,
                &std::env::join_paths([&self.0]).expect("PATH"),
                budget,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn verifies_code_execution_and_returns_a_direct_absolute_program() {
        let f = Fixture::new();
        for name in [
            RuntimeName::Python,
            RuntimeName::Powershell,
            RuntimeName::Pwsh,
        ] {
            let observation = f.probe(name, TEST_BUDGET);
            let RuntimeResult::Available { program, version } = observation else {
                panic!("fixture runtime should execute: {name:?}: {observation:?}");
            };
            assert!(PathBuf::from(program).starts_with(&f.0));
            assert_eq!(version, "3.13.1");
        }
    }

    #[test]
    fn missing_failed_invalid_and_timed_out_are_distinct_observations() {
        let f = Fixture::new();
        assert!(matches!(
            probe(
                RuntimeName::Python,
                f.0.join("missing").as_os_str(),
                Duration::from_secs(1)
            ),
            RuntimeResult::NotFound { .. }
        ));
        for mode in ["fail", "invalid", "flood"] {
            f.mode("pwsh", mode);
            assert!(
                matches!(
                    f.probe(RuntimeName::Pwsh, TEST_BUDGET),
                    RuntimeResult::Failed { .. }
                ),
                "mode: {mode}"
            );
        }
        f.mode("pwsh", "hang");
        assert!(matches!(
            f.probe(RuntimeName::Pwsh, Duration::from_millis(100)),
            RuntimeResult::TimedOut { .. }
        ));
        // A timed-out child must be gone, including on Windows where its image is locked.
        std::fs::remove_file(f.0.join(format!("pwsh{}", std::env::consts::EXE_SUFFIX)))
            .expect("probe child was reaped");
    }

    #[test]
    fn python_falls_back_to_the_other_path_name_after_a_failed_candidate() {
        let f = Fixture::new();
        f.mode("python3", "fail");
        let observation = f.probe(RuntimeName::Python, TEST_BUDGET);
        let RuntimeResult::Available { program, .. } = observation else {
            panic!("python remains usable after python3 fails: {observation:?}");
        };
        assert_eq!(PathBuf::from(program).file_stem().expect("name"), "python");
        f.mode("python", "fail");
        assert!(matches!(
            f.probe(RuntimeName::Python, TEST_BUDGET),
            RuntimeResult::Failed { .. }
        ));
    }
}
