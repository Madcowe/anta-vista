//! x0x daemon integration helpers for Tier 3 testing
//!
//! Helpers for discovering x0xd daemon, managing named instances, and injecting test payloads

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use base64::Engine;
use directories::ProjectDirs;

/// Discover x0xd daemon from default or named data directory
pub struct X0xDaemonConfig {
    pub api_port: String,
    pub api_token: String,
    pub data_dir: PathBuf,
}

impl X0xDaemonConfig {
    /// Discover from default data directory
    pub fn discover_default() -> Result<Self, String> {
        let data_dir = Self::default_data_dir()?;
        Self::from_data_dir(&data_dir)
    }

    /// Discover from named instance
    pub fn discover_named(name: &str) -> Result<Self, String> {
        let mut data_dir = Self::default_data_dir()?;
        data_dir.push(format!("-{}", name));
        Self::from_data_dir(&data_dir)
    }

    /// Read daemon config from data directory
    fn from_data_dir(data_dir: &Path) -> Result<Self, String> {
        if !data_dir.exists() {
            return Err(format!("data_dir does not exist: {:?}", data_dir));
        }

        let api_port_file = data_dir.join("api.port");
        let api_token_file = data_dir.join("api-token");

        let api_port = fs::read_to_string(&api_port_file)
            .map_err(|e| format!("failed to read api.port: {}", e))?
            .trim()
            .to_string();

        let api_token = fs::read_to_string(&api_token_file)
            .map_err(|e| format!("failed to read api-token: {}", e))?
            .trim()
            .to_string();

        Ok(Self {
            api_port,
            api_token,
            data_dir: data_dir.to_path_buf(),
        })
    }

    fn default_data_dir() -> Result<PathBuf, String> {
        #[cfg(target_os = "macos")]
        {
            if let Some(proj) = ProjectDirs::from("", "", "x0x") {
                return Ok(proj.data_local_dir().to_path_buf());
            }
            Err("no data dir".to_string())
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(proj) = ProjectDirs::from("", "", "x0x") {
                return Ok(proj.data_local_dir().to_path_buf());
            }
            Err("no data dir".to_string())
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(proj) = ProjectDirs::from("", "", "x0x") {
                return Ok(proj.config_dir().to_path_buf());
            }
            Err("no config dir".to_string())
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Err("unsupported OS".to_string())
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.api_port)
    }
}

/// Guard: skip test if x0xd is not running
pub fn skip_if_no_daemon() -> bool {
    X0xDaemonConfig::discover_default().is_err()
}

/// Inject adversarial gossip payload via HTTP POST (bypasses MockNetClient)
pub fn inject_gossip_payload(
    config: &X0xDaemonConfig,
    topic: &str,
    payload: &[u8],
) -> Result<(), String> {
    let base64_payload = base64::engine::general_purpose::STANDARD.encode(payload);
    let curl_cmd = format!(
        "curl -X POST {}/publish \
         -H 'Authorization: Bearer {}' \
         -H 'Content-Type: application/json' \
         -d '{{\"topic\": \"{}\", \"payload\": \"{}\"}}' \
         2>/dev/null",
        config.base_url(),
        config.api_token,
        topic,
        base64_payload
    );

    let output = Command::new("sh")
        .arg("-c")
        .arg(&curl_cmd)
        .output()
        .map_err(|e| format!("curl failed: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "curl returned non-zero: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

/// Helper to spawn named x0x instances for two-node tests
pub fn spawn_named_instance(name: &str) -> Result<std::process::Child, String> {
    Command::new("x0x")
        .arg("start")
        .arg("--name")
        .arg(name)
        .spawn()
        .map_err(|e| format!("failed to spawn x0x: {}", e))
}
