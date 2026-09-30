//! Install / update the AbletonMusicMaker Remote Script into Live's User Library.
//!
//! Since Live 10.1.13, third-party control surface scripts are loaded from
//! `<User Library>/Remote Scripts/`. The installer reads the configured User
//! Library location from each installed Live version's `Library.cfg`, falls
//! back to the platform defaults, and only ever writes into libraries that
//! already exist.
//!
//! Two files go into that folder: `__init__.py`, the loader Live holds, and
//! `body.py`, every handler. Each keeps its own `.bak`, and each is reported
//! separately, because which one changed decides whether Live has to be
//! restarted or merely told to re-read the body.

use crate::handshake::{expected_loader_version, expected_remote_script_version};
use crate::{REMOTE_SCRIPT_BODY, REMOTE_SCRIPT_SOURCE};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const REMOTE_SCRIPT_FOLDER_NAME: &str = "AbletonMusicMaker";
/// The loader: what Live imports once, when the control surface is selected.
pub const LOADER_FILE_NAME: &str = "__init__.py";
/// The body: every handler, and what `reload_body` re-reads.
pub const BODY_FILE_NAME: &str = "body.py";

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
    /// Per file, in the order they were written: the loader then the body.
    /// Empty when nothing was written (skipped, or no library found).
    pub files: Vec<FileResult>,
}

/// One of the two files the installer writes.
#[derive(Debug, Clone)]
pub struct FileResult {
    pub name: &'static str,
    pub path: PathBuf,
    pub status: InstallStatus,
    pub backup: Option<PathBuf>,
    pub version: String,
}

impl InstallResult {
    /// True when the loader on disk changed. That is the one change Live only
    /// sees when it starts, so the caller must not offer a reload.
    pub fn loader_changed(&self) -> bool {
        self.files.iter().any(|f| {
            f.name == LOADER_FILE_NAME
                && matches!(f.status, InstallStatus::Installed | InstallStatus::Updated)
        })
    }

    /// True when the body changed and the loader did not: a reload, in place.
    pub fn body_only_changed(&self) -> bool {
        !self.loader_changed()
            && self.files.iter().any(|f| {
                f.name == BODY_FILE_NAME
                    && matches!(f.status, InstallStatus::Installed | InstallStatus::Updated)
            })
    }
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
            files: Vec::new(),
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
            files: Vec::new(),
        }];
    }
    targets.iter().map(|root| install_into(root)).collect()
}

/// Write one file, keeping a `.bak` of anything that differed.
fn write_file(
    dest_dir: &Path,
    name: &'static str,
    source: &str,
    version: String,
) -> std::io::Result<FileResult> {
    let dest = dest_dir.join(name);
    let bytes = source.as_bytes();
    let (status, backup) = match std::fs::read(&dest).ok() {
        None => {
            std::fs::write(&dest, bytes)?;
            (InstallStatus::Installed, None)
        }
        Some(existing) if existing == bytes => (InstallStatus::Unchanged, None),
        Some(existing) => {
            // The existing file differs, perhaps a user's own edit: keep it.
            let backup = dest_dir.join(format!("{name}.bak"));
            std::fs::write(&backup, existing)?;
            std::fs::write(&dest, bytes)?;
            (InstallStatus::Updated, Some(backup))
        }
    };
    if std::fs::read(&dest)? != bytes {
        return Err(std::io::Error::other(format!(
            "verification failed: {name} does not match the embedded script"
        )));
    }
    Ok(FileResult {
        name,
        path: dest,
        status,
        backup,
        version,
    })
}

fn install_into(root: &Path) -> InstallResult {
    let dest_dir = root.join(REMOTE_SCRIPT_FOLDER_NAME);
    let dest = dest_dir.join(LOADER_FILE_NAME);
    let attempt = (|| -> std::io::Result<Vec<FileResult>> {
        std::fs::create_dir_all(&dest_dir)?;
        // The loader first: a body that needs a newer loader must never be on
        // disk beside an older one, and the loader refuses that pairing.
        Ok(vec![
            write_file(
                &dest_dir,
                LOADER_FILE_NAME,
                REMOTE_SCRIPT_SOURCE,
                expected_loader_version().to_string(),
            )?,
            write_file(
                &dest_dir,
                BODY_FILE_NAME,
                REMOTE_SCRIPT_BODY,
                expected_remote_script_version().to_string(),
            )?,
        ])
    })();
    match attempt {
        Ok(files) => {
            // One status for the pair: what actually happened to the folder.
            let status = if files.iter().any(|f| f.status == InstallStatus::Installed) {
                InstallStatus::Installed
            } else if files.iter().any(|f| f.status == InstallStatus::Updated) {
                InstallStatus::Updated
            } else {
                InstallStatus::Unchanged
            };
            let mut detail = format!(
                "script_version={} loader_version={}",
                expected_remote_script_version(),
                expected_loader_version()
            );
            for f in &files {
                if f.status != InstallStatus::Unchanged {
                    detail.push_str(&format!("; {} {}", f.name, f.status));
                }
                if let Some(b) = &f.backup {
                    detail.push_str(&format!(
                        ", previous kept as {}",
                        b.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
            let backup = files.iter().find_map(|f| f.backup.clone());
            tracing::info!("Remote Script {} → {}", status, dest_dir.display());
            InstallResult {
                path: Some(dest),
                status,
                detail,
                backup,
                files,
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
                files: Vec::new(),
            }
        }
    }
}

/// Install, then tell a running Live to re-read the body when that is all
/// that changed. The loader is the one file Live only reads when it starts,
/// so a loader change skips the reload and says why.
///
/// Returns the results and, when a reload was attempted, what it answered.
pub fn install_and_reload(
    target_root: Option<&Path>,
    force: bool,
    bridge: Option<&dyn crate::connection::LiveBridge>,
) -> (Vec<InstallResult>, ReloadOutcome) {
    let results = install_remote_script(target_root, force);
    let outcome = match bridge {
        None => ReloadOutcome::NotAsked,
        Some(bridge) => {
            if results.iter().any(|r| r.loader_changed()) {
                ReloadOutcome::LoaderChanged
            } else if results.iter().any(|r| r.body_only_changed()) {
                reload_body(bridge)
            } else {
                ReloadOutcome::NothingToDo
            }
        }
    };
    (results, outcome)
}

/// What happened when the body was handed to a running Live.
#[derive(Debug, Clone, PartialEq)]
pub enum ReloadOutcome {
    /// No reload was asked for (no `--reload`, or no Live to ask).
    NotAsked,
    /// Nothing on disk changed, so there is nothing to reload.
    NothingToDo,
    /// The loader changed: Live has to be restarted, and no reload was sent.
    LoaderChanged,
    /// Live re-read its body. Carries the script's own `result` block.
    Reloaded(Value),
    /// Live was asked and refused or could not be reached.
    Failed(String),
}

/// Send `reload_body` and classify the answer.
pub fn reload_body(bridge: &dyn crate::connection::LiveBridge) -> ReloadOutcome {
    match bridge.send_command("reload_body", None) {
        Ok(result) => ReloadOutcome::Reloaded(result),
        Err(e) => ReloadOutcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_update_unchanged_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Remote Scripts");
        let folder = root.join("AbletonMusicMaker");
        let loader = folder.join(LOADER_FILE_NAME);
        let body = folder.join(BODY_FILE_NAME);

        let first = install_remote_script(Some(&root), false);
        assert_eq!(first[0].status, InstallStatus::Installed);
        assert_eq!(
            std::fs::read_to_string(&loader).unwrap(),
            REMOTE_SCRIPT_SOURCE
        );
        assert_eq!(std::fs::read_to_string(&body).unwrap(), REMOTE_SCRIPT_BODY);
        assert_eq!(first[0].files.len(), 2);
        assert!(first[0].loader_changed());

        let second = install_remote_script(Some(&root), false);
        assert_eq!(second[0].status, InstallStatus::Unchanged);
        assert!(!second[0].loader_changed());
        assert!(!second[0].body_only_changed());

        // A body that changed on its own: the one case that reloads.
        std::fs::write(&body, "# user edit\n").unwrap();
        let third = install_remote_script(Some(&root), false);
        assert_eq!(third[0].status, InstallStatus::Updated);
        assert!(third[0].body_only_changed(), "{:?}", third[0].files);
        assert!(!third[0].loader_changed());
        assert_eq!(
            std::fs::read_to_string(folder.join("body.py.bak")).unwrap(),
            "# user edit\n"
        );
        assert_eq!(std::fs::read_to_string(&body).unwrap(), REMOTE_SCRIPT_BODY);
        assert!(third[0].detail.contains("previous kept as body.py.bak"));

        // A loader that changed: no reload, whatever else moved.
        std::fs::write(&loader, "# user edit\n").unwrap();
        let fourth = install_remote_script(Some(&root), false);
        assert!(fourth[0].loader_changed());
        assert!(!fourth[0].body_only_changed());
        assert_eq!(
            std::fs::read_to_string(folder.join("__init__.py.bak")).unwrap(),
            "# user edit\n"
        );
    }

    /// Each file keeps its own version, and they are the two the binary
    /// embeds — so a `--check` against a half-installed folder says which
    /// half is behind rather than one number for both.
    #[test]
    fn each_file_reports_its_own_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Remote Scripts");
        let r = install_remote_script(Some(&root), false);
        let by_name: std::collections::HashMap<_, _> =
            r[0].files.iter().map(|f| (f.name, f)).collect();
        assert_eq!(by_name[LOADER_FILE_NAME].version, expected_loader_version());
        assert_eq!(
            by_name[BODY_FILE_NAME].version,
            expected_remote_script_version()
        );
        assert!(r[0].detail.contains("loader_version="));
    }

    /// Nothing is sent to Live unless a bridge is handed in, and a loader
    /// change never reloads: Live only reads that file when it starts, so a
    /// reload would leave two halves that disagree.
    #[test]
    fn a_loader_change_is_never_reloaded() {
        use crate::connection::{LiveBridge, LiveResult};
        use std::sync::Mutex;

        #[derive(Default)]
        struct Recorder(Mutex<Vec<String>>);
        impl LiveBridge for Recorder {
            fn send_command(&self, name: &str, _: Option<Value>) -> LiveResult<Value> {
                self.0.lock().unwrap().push(name.to_string());
                Ok(serde_json::json!({"was": "1.0.0", "now": "1.1.0", "main_ms": 0.2}))
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Remote Scripts");
        let bridge = Recorder::default();
        // First install: the loader is written, so no reload.
        let (_, outcome) = install_and_reload(Some(&root), false, Some(&bridge));
        assert_eq!(outcome, ReloadOutcome::LoaderChanged);
        assert!(
            bridge.0.lock().unwrap().is_empty(),
            "a command reached Live"
        );

        // Nothing changed: nothing to reload.
        let (_, outcome) = install_and_reload(Some(&root), false, Some(&bridge));
        assert_eq!(outcome, ReloadOutcome::NothingToDo);
        assert!(bridge.0.lock().unwrap().is_empty());

        // Only the body changed: one `reload_body`, and nothing else.
        std::fs::write(
            root.join("AbletonMusicMaker").join(BODY_FILE_NAME),
            "# stale\n",
        )
        .unwrap();
        let (_, outcome) = install_and_reload(Some(&root), false, Some(&bridge));
        assert!(matches!(outcome, ReloadOutcome::Reloaded(_)), "{outcome:?}");
        assert_eq!(*bridge.0.lock().unwrap(), vec!["reload_body".to_string()]);
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
