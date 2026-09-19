//! Install / update the AbletonMusicMaker Remote Script into Live's User Library.
//!
//! Since Live 10.1.13, third-party control surface scripts are loaded from
//! `<User Library>/Remote Scripts/`. The installer reads the configured User
//! Library location from each installed Live version's `Library.cfg`, falls
//! back to the platform defaults, and only ever writes into libraries that
//! already exist.

use crate::handshake::expected_remote_script_version;
use crate::REMOTE_SCRIPT_SOURCE;
use std::path::{Path, PathBuf};

pub const REMOTE_SCRIPT_FOLDER_NAME: &str = "AbletonMusicMaker";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    Installed,
    Updated,
    Unchanged,
    Skipped,
    Error,
}

impl std::fmt::Display for InstallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            InstallStatus::Installed => "installed",
            InstallStatus::Updated => "updated",
            InstallStatus::Unchanged => "unchanged",
            InstallStatus::Skipped => "skipped",
            InstallStatus::Error => "error",
        })
    }
}

#[derive(Debug, Clone)]
pub struct InstallResult {
    pub path: Option<PathBuf>,
    pub status: InstallStatus,
    pub detail: String,
    pub backup: Option<PathBuf>,
}

fn live_version_key(dir: &Path) -> Vec<u64> {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut nums = Vec::new();
    let mut current = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            current.push(c);
        } else if !current.is_empty() {
            nums.push(current.parse().unwrap_or(0));
            current.clear();
        }
    }
    if !current.is_empty() {
        nums.push(current.parse().unwrap_or(0));
    }
    nums
}

/// Ableton preference folders (`…/Ableton/Live x.x.x`), newest first.
fn live_preference_dirs(home: &Path) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    match std::env::consts::OS {
        "macos" => bases.push(home.join("Library").join("Preferences").join("Ableton")),
        "windows" => {
            let roaming = std::env::var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("AppData").join("Roaming"));
            bases.push(roaming.join("Ableton"));
        }
        _ => {
            bases.push(home.join(".config").join("ableton"));
            bases.push(home.join(".local").join("share").join("Ableton"));
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() && name.starts_with("Live ") {
                dirs.push(path);
            }
        }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(live_version_key(d)));
    dirs
}

/// Best-effort read of the (relocatable) User Library path from Library.cfg.
/// Live 10–12 store it under `<UserLibrary><LibraryProject>` as
/// `<ProjectPath Value="…parent dir"/>` + `<ProjectName Value="User Library"/>`.
pub fn user_library_from_library_cfg(cfg: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(cfg).ok()?;
    let doc = roxmltree::Document::parse(&text).ok()?;
    for node in doc.descendants().filter(|n| n.is_element()) {
        if !node
            .tag_name()
            .name()
            .to_lowercase()
            .contains("userlibrary")
        {
            continue;
        }
        let mut project_path: Option<String> = None;
        let mut project_name: Option<String> = None;
        let mut fallback: Option<PathBuf> = None;
        for sub in node.descendants().filter(|n| n.is_element()) {
            let Some(value) = sub.attribute("Value") else {
                continue;
            };
            let tag = sub.tag_name().name().to_lowercase();
            if tag.contains("projectpath") {
                project_path = Some(value.to_string());
            } else if tag.contains("projectname") {
                project_name = Some(value.to_string());
            } else if fallback.is_none() {
                let p = expand_home(value);
                if p.is_absolute() && p.is_dir() {
                    fallback = Some(p);
                }
            }
        }
        if let (Some(path), Some(name)) = (&project_path, &project_name) {
            let candidate = expand_home(path).join(name);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        if fallback.is_some() {
            return fallback;
        }
    }
    None
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

/// `<User Library>/Remote Scripts` directories for every installed Live
/// version, deduplicated, existing libraries only.
pub fn discover_remote_script_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut libraries: Vec<PathBuf> = Vec::new();
    for pref_dir in live_preference_dirs(&home) {
        for cfg in [
            pref_dir.join("Library.cfg"),
            pref_dir.join("Preferences").join("Library.cfg"),
        ] {
            if cfg.is_file() {
                if let Some(lib) = user_library_from_library_cfg(&cfg) {
                    libraries.push(lib);
                }
            }
        }
    }
    for default in [
        home.join("Music").join("Ableton").join("User Library"),
        home.join("Documents").join("Ableton").join("User Library"),
    ] {
        if default.is_dir() {
            libraries.push(default);
        }
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for lib in libraries {
        let target = lib.join("Remote Scripts");
        if !unique.contains(&target) {
            unique.push(target);
        }
    }
    unique
}

/// Copy the embedded Remote Script into each target's `AbletonMusicMaker/__init__.py`.
pub fn install_remote_script(target_root: Option<&Path>, force: bool) -> Vec<InstallResult> {
    if crate::env_flag("ABLETON_MCP_SKIP_SCRIPT_INSTALL") && !force {
        return vec![InstallResult {
            path: None,
            status: InstallStatus::Skipped,
            detail: "ABLETON_MCP_SKIP_SCRIPT_INSTALL set".into(),
            backup: None,
        }];
    }
    let targets: Vec<PathBuf> = match target_root {
        Some(t) => vec![t.to_path_buf()],
        None => discover_remote_script_dirs(),
    };
    if targets.is_empty() {
        return vec![InstallResult {
            path: None,
            status: InstallStatus::Error,
            detail: "No Ableton User Library found. Create the 'Remote Scripts' folder in your User Library (default: ~/Music/Ableton/User Library on macOS, %USERPROFILE%\\Documents\\Ableton\\User Library on Windows) or pass --target <dir>.".into(),
            backup: None,
        }];
    }
    targets.iter().map(|root| install_into(root)).collect()
}

fn install_into(root: &Path) -> InstallResult {
    let dest_dir = root.join(REMOTE_SCRIPT_FOLDER_NAME);
    let dest = dest_dir.join("__init__.py");
    let src_bytes = REMOTE_SCRIPT_SOURCE.as_bytes();
    let attempt = (|| -> std::io::Result<(InstallStatus, Option<PathBuf>)> {
        std::fs::create_dir_all(&dest_dir)?;
        let existing = std::fs::read(&dest).ok();
        let (status, backup) = match existing {
            None => {
                std::fs::write(&dest, src_bytes)?;
                (InstallStatus::Installed, None)
            }
            Some(bytes) if bytes == src_bytes => (InstallStatus::Unchanged, None),
            Some(bytes) => {
                // The existing file differs, perhaps a user's own edit: keep it.
                let backup = dest_dir.join("__init__.py.bak");
                std::fs::write(&backup, bytes)?;
                std::fs::write(&dest, src_bytes)?;
                (InstallStatus::Updated, Some(backup))
            }
        };
        if std::fs::read(&dest)? != src_bytes {
            return Err(std::io::Error::other(
                "verification failed: destination does not match the embedded script",
            ));
        }
        Ok((status, backup))
    })();
    match attempt {
        Ok((status, backup)) => {
            let mut detail = format!("script_version={}", expected_remote_script_version());
            if let Some(b) = &backup {
                detail.push_str(&format!(
                    "; previous version backed up to {}",
                    b.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            tracing::info!("Remote Script {} → {}", status, dest.display());
            InstallResult {
                path: Some(dest),
                status,
                detail,
                backup,
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to install Remote Script into {}: {}",
                root.display(),
                e
            );
            InstallResult {
                path: Some(dest),
                status: InstallStatus::Error,
                detail: e.to_string(),
                backup: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_update_unchanged_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Remote Scripts");

        let first = install_remote_script(Some(&root), false);
        assert_eq!(first[0].status, InstallStatus::Installed);
        let dest = root.join("AbletonMusicMaker").join("__init__.py");
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            REMOTE_SCRIPT_SOURCE
        );

        let second = install_remote_script(Some(&root), false);
        assert_eq!(second[0].status, InstallStatus::Unchanged);

        std::fs::write(&dest, "# user edit\n").unwrap();
        let third = install_remote_script(Some(&root), false);
        assert_eq!(third[0].status, InstallStatus::Updated);
        assert_eq!(
            std::fs::read_to_string(root.join("AbletonMusicMaker").join("__init__.py.bak"))
                .unwrap(),
            "# user edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            REMOTE_SCRIPT_SOURCE
        );
        assert!(third[0].detail.contains("backed up"));
    }

    #[test]
    fn library_cfg_project_path_and_name() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("User Library");
        std::fs::create_dir_all(&lib).unwrap();
        let cfg = dir.path().join("Library.cfg");
        std::fs::write(
            &cfg,
            format!(
                r#"<?xml version="1.0"?><Ableton><Library><UserLibrary><LibraryProject><ProjectPath Value="{}"/><ProjectName Value="User Library"/></LibraryProject></UserLibrary></Library></Ableton>"#,
                dir.path().display()
            ),
        )
        .unwrap();
        assert_eq!(user_library_from_library_cfg(&cfg).unwrap(), lib);
    }

    #[test]
    fn version_keys_sort_newest_first() {
        let mut dirs = [
            PathBuf::from("Live 11.3.4"),
            PathBuf::from("Live 12.1"),
            PathBuf::from("Live 9.7"),
        ];
        dirs.sort_by_key(|d| std::cmp::Reverse(live_version_key(d)));
        assert_eq!(dirs[0], PathBuf::from("Live 12.1"));
        assert_eq!(dirs[2], PathBuf::from("Live 9.7"));
    }
}
