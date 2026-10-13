//! Starts one process and reports only facts observed during a bounded window.
use linklet_core::{
    result::{ErrorCode, OperationError},
    wire::{SpawnReport, SpawnRequest, Text},
};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Child, Stdio},
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

/// Starts a child with closed stdin and a named output file. Zero observation
/// returns immediately; otherwise an early exit ends observation. Running children
/// are reaped without storing queryable task history. No state asserts readiness.
pub fn start(request: &SpawnRequest, output: &Path) -> Result<SpawnReport, OperationError> {
    let (mut command, directory) =
        crate::shell::invocation(&request.command, request.cwd.as_deref())?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(output)
        .map_err(|e| linklet_adapters::failure::io(e, "open_output", output))?;
    let errors = file
        .try_clone()
        .map_err(|e| linklet_adapters::failure::io(e, "open_output", output))?;
    let mut child = command
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        .spawn()
        .map_err(|e| linklet_adapters::failure::io(e, "spawn", &directory))?;
    let pid = child.id();
    let started = Instant::now();
    let mut exit_code = None;
    let mut error = None;
    let mut state = "started";
    let mut exited = false;
    if request.observe_ms > 0 {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited = true;
                    state = "exited";
                    exit_code = status.code();
                    if !status.success() {
                        error = Some(OperationError::new(
                            ErrorCode::CommandFailed,
                            "observe",
                            format!("process exited with {:?}", status.code()),
                        ));
                    }
                    break;
                }
                Ok(None) if started.elapsed() >= Duration::from_millis(request.observe_ms) => {
                    state = "running_at_observation";
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => {
                    error = Some(OperationError::new(
                        ErrorCode::ResultUnknown,
                        "observe",
                        e.to_string(),
                    ));
                    break;
                }
            }
        }
    }
    let observed_ms = if request.observe_ms == 0 {
        0
    } else {
        started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    };
    if !exited {
        reap(child);
    }
    let tail = log_tail(output).unwrap_or_else(|e| {
        Text::decoded(
            String::new(),
            0,
            0,
            "unknown".into(),
            false,
            Some(e.to_string()),
        )
    });
    Ok(SpawnReport {
        pid,
        command: request.command.to_string(),
        cwd: directory.to_string_lossy().into_owned(),
        output: output.to_string_lossy().into_owned(),
        pid_kind: request.command.pid_kind().into(),
        state: state.into(),
        observed_ms,
        exit_code,
        error,
        tail,
    })
}

fn reap(child: Child) {
    static REAPER: OnceLock<mpsc::Sender<Child>> = OnceLock::new();
    let sender = REAPER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Child>();
        std::thread::spawn(move || {
            let mut children: Vec<Child> = Vec::new();
            loop {
                let incoming = if children.is_empty() {
                    receiver
                        .recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                } else {
                    receiver.recv_timeout(Duration::from_millis(50))
                };
                match incoming {
                    Ok(child) => children.push(child),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                children.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
            }
        });
        sender
    });
    let _ = sender.send(child);
}

fn log_tail(path: &Path) -> std::io::Result<Text> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut prefix = vec![0; size.min(256) as usize];
    file.read_exact(&mut prefix)?;
    let start = size.saturating_sub(linklet_core::output::OUTPUT_LIMIT as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(linklet_core::output::OUTPUT_LIMIT as u64)
        .read_to_end(&mut bytes)?;
    let sample = linklet_core::search::sniff(&prefix);
    let encoding = match sample {
        linklet_core::search::Encoding::Utf8Bom
        | linklet_core::search::Encoding::Utf16Le
        | linklet_core::search::Encoding::Utf16Be => sample,
        _ => {
            let skip = if start > 0 {
                bytes
                    .iter()
                    .take(3)
                    .take_while(|b| **b & 0xc0 == 0x80)
                    .count()
            } else {
                0
            };
            linklet_core::search::sniff(&bytes[skip..])
        }
    };
    Ok(linklet_adapters::decode_text(
        &bytes,
        size as usize,
        encoding,
        linklet_adapters::TextSource::Command,
    ))
}
