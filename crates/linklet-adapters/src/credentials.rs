//! Account-local credentials, with exclusive ownership and atomic replacement.
use linklet_core::{auth::Token, wire};
use rand::{Rng, RngCore, rngs::OsRng};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The current account's configuration directory, independent of the working directory.
pub fn config_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")));
    let base = base
        .filter(|p| p.is_absolute())
        .ok_or("cannot locate the account configuration directory")?;
    Ok(base.join("linklet"))
}

fn private_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|e| format!("cannot create credential directory: {e}"))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot protect credential directory: {e}"))?;
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("cannot create credential directory: {e}"))?;
        // Restrict this account application directory. Child files inherit this DACL.
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("powershell.exe")
            .creation_flags(0x08000000)
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; $acl=New-Object System.Security.AccessControl.DirectorySecurity; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); $acl.SetAccessRuleProtection($true,$false); $acl.AddAccessRule($rule); [System.IO.Directory]::SetAccessControl($env:LINKLET_CREDENTIAL_DIRECTORY,$acl)"])
            .env("LINKLET_CREDENTIAL_DIRECTORY", path)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .status().map_err(|e| format!("cannot protect credential directory: {e}"))?;
        if !status.success() {
            return Err("cannot protect credential directory".into());
        }
    }
    Ok(())
}

fn open_private(path: &Path, new: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|e| format!("cannot open credential file {}: {e}", path.display()))
}

fn load(path: &Path) -> Result<Option<Value>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read credentials: {e}")),
    };
    let mut bytes = Vec::new();
    file.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 8192 {
        return Err("credential file exceeds 8192 bytes".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "credential file is invalid".into())
}

fn save(path: &Path, value: &Value) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", random_token()));
    let result = (|| {
        let mut file = open_private(&temporary, true)?;
        file.write_all(value.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|e| format!("cannot save credentials: {e}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    wire::to_hex(&bytes)
}

/// Persistent target credentials. Holding this value prevents another managed agent or reset.
pub struct AgentCredentials {
    code: String,
    token: Token,
    _lock: File,
}
impl AgentCredentials {
    /// Loads or creates credentials. With reset, rotates both secrets under an exclusive lock.
    /// The caller must retain the value until the listener and its connections have stopped.
    pub fn acquire(directory: &Path, reset: bool) -> Result<Self, String> {
        private_directory(directory)?;
        let lock = open_private(&directory.join("agent.lock"), false)?;
        lock.try_lock().map_err(
            |_| "this account's agent is running; stop it before starting or initializing again",
        )?;
        let path = directory.join("agent.json");
        let previous_code = if reset {
            load(&path)
                .ok()
                .flatten()
                .and_then(|v| v["code"].as_str().map(str::to_string))
        } else {
            None
        };
        let value = if reset { None } else { load(&path)? };
        let value = match value {
            Some(value) => value,
            None => {
                let code = loop {
                    let code = format!("{:06}", OsRng.gen_range(0..1_000_000));
                    if previous_code.as_ref() != Some(&code) {
                        break code;
                    }
                };
                let value = json!({"version":1, "code":code, "token":random_token()});
                save(&path, &value)?;
                value
            }
        };
        if value["version"] != 1 {
            return Err("unsupported credential file version".into());
        }
        let code = value["code"]
            .as_str()
            .ok_or("credential file has no code")?
            .to_string();
        linklet_core::pairing::validate_code(&code)?;
        let token = Token::new(
            value["token"]
                .as_str()
                .ok_or("credential file has no token")?,
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            code,
            token,
            _lock: lock,
        })
    }
    /// The six digits for display only on the target's local terminal.
    pub fn code(&self) -> &str {
        &self.code
    }
    /// The strong secret used by normal command connections.
    pub fn token(&self) -> &Token {
        &self.token
    }
}

fn controller_path(directory: &Path, agent: &str) -> PathBuf {
    directory.join(format!(
        "peer-{}.json",
        wire::to_hex(&Sha256::digest(agent.to_ascii_lowercase().as_bytes()))
    ))
}

/// Reads a credential saved for this host:port. It does not read environment token overrides.
pub fn saved_token(directory: &Path, agent: &str) -> Result<Option<Token>, String> {
    let Some(value) = load(&controller_path(directory, agent))? else {
        return Ok(None);
    };
    if value["version"] != 1 || value["agent"].as_str() != Some(agent.to_ascii_lowercase().as_str())
    {
        return Err("saved credential does not match this agent".into());
    }
    Token::new(
        value["token"]
            .as_str()
            .ok_or("saved credential has no token")?,
    )
    .map(Some)
    .map_err(|e| e.to_string())
}

/// Atomically remembers a successfully paired target for this controller account.
pub fn save_token(directory: &Path, agent: &str, token: &Token) -> Result<(), String> {
    private_directory(directory)?;
    save(
        &controller_path(directory, agent),
        &json!({"version":1,"agent":agent.to_ascii_lowercase(),"token":token.expose()}),
    )
}
