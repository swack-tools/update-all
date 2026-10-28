//! Remembers answers between runs: the first-run shell choices and whether a
//! restart is pending. It lives outside config.toml on purpose.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::exec;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Shell frameworks the user was already asked about.
    pub shells_seen: Vec<String>,
    /// Shell frameworks the user chose to update.
    pub shells_enabled: Vec<String>,
    /// Set right before a restart-required install.
    pub pending_restart: Option<PendingRestart>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PendingRestart {
    pub at: String,
    pub updates: Vec<String>,
}

pub fn path() -> PathBuf {
    exec::home().join("Library/Application Support/update-all/state.json")
}

impl State {
    pub fn load() -> State {
        fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, text);
        }
    }
}
