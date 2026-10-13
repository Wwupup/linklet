//! Isolated saved credentials for process tests; pairing itself has dedicated tests.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Removes a test-only account's credential store when its processes have stopped.
pub struct Credentials {
    home: PathBuf,
}
impl Credentials {
    /// Seeds managed state and per-target credentials without using production accounts.
    pub fn new(agent_token: Option<&str>, peers: &[(&str, &str)]) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let home = std::env::temp_dir().join(format!(
            "linklet-test-credentials-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let directory = home.join("linklet");
        std::fs::create_dir_all(&directory).expect("test credential directory");
        if let Some(token) = agent_token {
            std::fs::write(
                directory.join("agent.json"),
                serde_json::json!({"version":1,"code":"000017","token":token}).to_string(),
            )
            .expect("test agent credentials");
        }
        for (agent, secret) in peers {
            let token = linklet_core::auth::Token::new(*secret).expect("test token");
            linklet_adapters::credentials::save_token(&directory, agent, &token)
                .expect("test controller credentials");
        }
        Self { home }
    }

    /// Selects the isolated account directory on either supported operating system.
    pub fn env(&self) -> [(&'static str, &Path); 2] {
        [
            ("LOCALAPPDATA", &self.home),
            ("XDG_CONFIG_HOME", &self.home),
        ]
    }
}
impl Drop for Credentials {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
