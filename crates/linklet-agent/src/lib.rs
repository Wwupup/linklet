//! Shared target service for the console and Windows tray entry points.
#![forbid(unsafe_code)]

mod execute;
#[cfg(test)]
mod lifecycle;
mod log;
mod server;
mod shell;
mod spawn;
mod usage;

pub use usage::USAGE;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use linklet_adapters::credentials::AgentCredentials;
use linklet_core::{identity::RuntimeSnapshot, log::Rotation, transfer::Destination};

/// Default listening port for either entry point.
pub const DEFAULT_PORT: u16 = 8787;

/// Startup failure, with the same exit code used by the console entry point.
#[derive(Debug)]
pub struct StartupError {
    /// Invocation/configuration failures use 2; binding failures use 1.
    pub exit_code: i32,
    /// Human-readable explanation suitable for stderr or a dialog.
    pub message: String,
}

fn configuration(message: impl Into<String>) -> StartupError {
    StartupError {
        exit_code: 2,
        message: message.into(),
    }
}

/// Parsed startup action. Help and version never acquire credentials or bind a port.
pub enum Invocation {
    /// Display usage.
    Help,
    /// Display the build version.
    Version,
    /// Start with these options.
    Start(Options),
}

/// Startup options shared by the console and tray. No service or autostart is installed.
pub struct Options {
    /// Port to bind; zero asks the OS to choose one.
    pub port: u16,
    /// Explicit log path, including a value supplied by LINKLET_LOG.
    pub log_path: Option<String>,
    /// Disable logging; mutually exclusive with log_path.
    pub no_log: bool,
    /// Rotate credentials under the existing exclusive account lock.
    pub reset: bool,
}

impl Options {
    /// Parse the existing CLI flags. Reads LINKLET_LOG as the default log override.
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Invocation, StartupError> {
        let mut options = Self {
            port: DEFAULT_PORT,
            log_path: std::env::var("LINKLET_LOG").ok(),
            no_log: false,
            reset: false,
        };
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "init" if !options.reset => options.reset = true,
                "--version" => return Ok(Invocation::Version),
                "-h" | "--help" => return Ok(Invocation::Help),
                "--port" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| configuration("--port needs a value"))?;
                    options.port = value
                        .parse()
                        .map_err(|_| configuration(format!("{value:?} is not a port number")))?;
                }
                "--log" => {
                    options.log_path = Some(
                        arguments
                            .next()
                            .ok_or_else(|| configuration("--log needs a file"))?,
                    )
                }
                "--no-log" => options.no_log = true,
                other => {
                    return Err(configuration(format!(
                        "unexpected argument {other:?}\n{USAGE}"
                    )));
                }
            }
        }
        Ok(Invocation::Start(options))
    }
}

/// Startup facts for local presentation. The pairing code is sensitive; do not log it.
#[derive(Clone, Debug)]
pub struct AgentInfo {
    /// Actual bound port, including when port zero was requested.
    pub port: u16,
    /// Permanent account-local pairing code.
    pub pairing_code: String,
    /// Base for relative command/file paths, not an access boundary.
    pub base: PathBuf,
    /// Actual log path; None when logging is disabled or unavailable.
    pub log_path: Option<PathBuf>,
    /// Nonfatal startup failures to show to the local operator.
    pub warnings: Vec<String>,
}

impl AgentInfo {
    /// The existing console startup banner, also suitable for a local details dialog.
    pub fn banner(&self) -> String {
        let log = match &self.log_path {
            Some(path) => format!("log {}", path.display()),
            None => "no log".into(),
        };
        format!(
            "linklet-agent listening on 0.0.0.0:{} file access as process account; relative paths from {} {log}",
            self.port,
            self.base.display()
        )
    }
}

/// A bound target service. Retains exclusive credential ownership until all calls finish.
pub struct Agent {
    listener: TcpListener,
    credentials: AgentCredentials,
    pairing: Arc<server::PairingState>,
    paths: Destination,
    request_log: log::RequestLog,
    runtimes: Arc<RuntimeSnapshot>,
    info: AgentInfo,
}

impl Agent {
    /// Acquire account credentials, probe runtimes and bind. Does not spawn a service thread.
    /// A second console or tray instance for this account is refused by the same lock.
    pub fn start(options: Options) -> Result<Self, StartupError> {
        let directory = linklet_adapters::credentials::config_dir().map_err(configuration)?;
        Self::start_in(options, &directory)
    }

    fn start_in(options: Options, credentials_directory: &Path) -> Result<Self, StartupError> {
        let credentials = AgentCredentials::acquire(credentials_directory, options.reset)
            .map_err(configuration)?;
        let pairing =
            Arc::new(server::PairingState::new(credentials.code()).map_err(configuration)?);
        let base = std::env::current_dir().map_err(|error| {
            configuration(format!("cannot read the working directory: {error}"))
        })?;
        let paths = Destination::new(&base.to_string_lossy())
            .map_err(|error| configuration(format!("working directory: {error}")))?;
        let mut warnings = Vec::new();
        let (request_log, log_path) = if options.no_log {
            if options.log_path.is_some() {
                return Err(configuration(
                    "--log and --no-log were both given; name one of them",
                ));
            }
            (log::RequestLog::none(), None)
        } else {
            let explicit = options.log_path.is_some();
            let path = options.log_path.map(PathBuf::from).unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| base.clone())
                    .join("logs")
                    .join("agent.log")
            });
            match log::RequestLog::open(&path, Rotation::DEFAULT) {
                Ok(log) => (log, Some(path)),
                Err(reason) if explicit => return Err(configuration(format!("--log: {reason}"))),
                Err(reason) => {
                    warnings.push(format!("cannot keep a log at {}: {reason}; serving without one. Pass --log <file> to choose where it goes, or --no-log to say this is intended.", path.display()));
                    (log::RequestLog::none(), None)
                }
            }
        };
        let runtimes = Arc::new(linklet_adapters::runtimes::observe());
        let listener =
            TcpListener::bind(("0.0.0.0", options.port)).map_err(|error| StartupError {
                exit_code: 1,
                message: format!("cannot listen on port {}: {error}", options.port),
            })?;
        let port = listener
            .local_addr()
            .map_err(|error| configuration(format!("cannot read bound address: {error}")))?
            .port();
        let info = AgentInfo {
            port,
            pairing_code: credentials.code().into(),
            base,
            log_path,
            warnings,
        };
        Ok(Self {
            listener,
            credentials,
            pairing,
            paths,
            request_log,
            runtimes,
            info,
        })
    }

    /// Facts are available only after successful binding and startup.
    pub fn info(&self) -> &AgentInfo {
        &self.info
    }

    /// Accept until a stop message or sender disconnect, then close the listener and drain
    /// accepted calls using their existing timeouts. Detached spawn children are not stopped.
    /// Keeps the credential lock while draining. Returns an error if a worker panics or accept fails.
    pub fn serve(self, shutdown: mpsc::Receiver<()>) -> Result<(), String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let mut workers = Vec::<std::thread::JoinHandle<()>>::new();
        let mut failure = None;
        loop {
            if !matches!(shutdown.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                break;
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    // Winsock inherits the listener's nonblocking mode. Framed calls use
                    // blocking reads with their own deadlines on both platforms.
                    if let Err(error) = stream.set_nonblocking(false) {
                        failure = Some(format!("cannot configure accepted connection: {error}"));
                        break;
                    }
                    let token = self.credentials.token().clone();
                    let paths = self.paths.clone();
                    let log = self.request_log.clone();
                    let pairing = self.pairing.clone();
                    let runtimes = self.runtimes.clone();
                    workers.push(std::thread::spawn(move || {
                        server::serve_connection(stream, &token, &paths, &log, &pairing, &runtimes)
                    }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if !matches!(
                        shutdown.recv_timeout(Duration::from_millis(25)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    failure = Some(format!("cannot accept a connection: {error}"));
                    break;
                }
            }
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    if workers.swap_remove(index).join().is_err() {
                        failure = Some("a request worker panicked".into());
                    }
                } else {
                    index += 1;
                }
            }
            if failure.is_some() {
                break;
            }
        }
        drop(self.listener);
        for worker in workers {
            if worker.join().is_err() {
                failure = Some("a request worker panicked".into());
            }
        }
        // Keep the exclusive account lock until the last accepted request has finished.
        drop(self.credentials);
        failure.map_or(Ok(()), Err)
    }
}
