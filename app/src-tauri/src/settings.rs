//! The app's own settings: a JSON file in the app-data directory. The
//! switches that concern the server are mirrored into the client config's
//! `env` block by `clients::resync_desktop_env`, because the client starts
//! the server and only the environment reaches it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub activity: bool,
    pub payloads: bool,
    pub retention_days: u32,
    pub library: Option<String>,
    pub show_in_menu_bar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 9877,
            activity: true,
            payloads: false,
            retention_days: 7,
            library: None,
            show_in_menu_bar: true,
        }
    }
}

impl Settings {
    /// The environment the client should start the server with.
    pub fn server_env(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut env = serde_json::Map::new();
        env.insert("ABLETON_HOST".into(), self.host.clone().into());
        env.insert("ABLETON_PORT".into(), self.port.to_string().into());
        env.insert(
            "ABLETON_MCP_ACTIVITY".into(),
            if self.activity { "true" } else { "false" }.into(),
        );
        env.insert(
            "ABLETON_MCP_ACTIVITY_PAYLOADS".into(),
            if self.payloads { "true" } else { "false" }.into(),
        );
        env
    }
}

fn path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".ableton-music-maker"))
        .join("settings.json")
}

pub fn load(app: &AppHandle) -> Settings {
    std::fs::read(path(app))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, s: &Settings) -> Result<(), String> {
    let p = path(app);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// The crate reads Live's address from the environment; set it for this
/// process so `app::check` and the installer see the app's settings.
pub fn apply_env(s: &Settings) {
    std::env::set_var("ABLETON_HOST", &s.host);
    std::env::set_var("ABLETON_PORT", s.port.to_string());
}
