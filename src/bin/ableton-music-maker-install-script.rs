//! `ableton-music-maker-install-script`: copy the embedded Remote Script into Live's
//! User Library.
//!
//! Two files go in: `__init__.py`, the loader Live imports once when the
//! control surface is selected, and `body.py`, every handler. With `--reload`,
//! a body that changed on its own is handed to a running Live in place — no
//! restart, no lost set. A loader that changed is the one case that still
//! needs Live restarted, and the installer says so instead of reloading.

use clap::Parser;
use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
use mcp_ableton_music_maker::install::{
    discover_remote_script_dirs, install_and_reload, install_remote_script, InstallStatus,
    ReloadOutcome,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ableton-music-maker-install-script",
    version,
    about = "Install/update the AbletonMusicMaker Remote Script into Live's User Library (Remote Scripts)"
)]
struct Cli {
    /// Explicit `Remote Scripts` directory (skips discovery)
    #[arg(long)]
    target: Option<PathBuf>,
    /// Install even if ABLETON_MCP_SKIP_SCRIPT_INSTALL is set
    #[arg(long)]
    force: bool,
    /// Print the discovered `Remote Scripts` directories and exit
    #[arg(long)]
    list_targets: bool,
    /// Tell a running Live to re-read the body when that is all that changed
    #[arg(long)]
    reload: bool,
}

fn main() {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .without_time()
        .init();

    if cli.list_targets {
        for dir in discover_remote_script_dirs() {
            println!("{}", dir.display());
        }
        return;
    }

    let (results, outcome) = if cli.reload {
        let (host, port) = live_address();
        let bridge = AbletonConnection::new(host, port);
        install_and_reload(cli.target.as_deref(), cli.force, Some(&bridge))
    } else {
        (
            install_remote_script(cli.target.as_deref(), cli.force),
            ReloadOutcome::NotAsked,
        )
    };

    for r in &results {
        println!(
            "{}: {} ({})",
            r.status,
            r.path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".into()),
            r.detail
        );
        for f in &r.files {
            println!(
                "  {:<12} {:<10} v{}",
                f.name,
                f.status.to_string(),
                f.version
            );
        }
    }
    let any_ok = results.iter().any(|r| {
        matches!(
            r.status,
            InstallStatus::Installed | InstallStatus::Updated | InstallStatus::Unchanged
        )
    });
    if results.iter().any(|r| r.status == InstallStatus::Error) && !any_ok {
        std::process::exit(1);
    }

    println!();
    match outcome {
        ReloadOutcome::NotAsked => {
            if results.iter().any(|r| r.loader_changed()) {
                println!("The loader changed. If Ableton is open, restart Live, or re-select AbletonMusicMaker under Settings → Link, Tempo & MIDI.");
            } else if results.iter().any(|r| r.body_only_changed()) {
                println!("Only the handlers changed. Run this again with --reload and a running Live picks them up in place, with nothing restarted.");
            } else {
                println!("Nothing changed.");
            }
        }
        ReloadOutcome::NothingToDo => println!("Nothing changed, so there is nothing to reload."),
        ReloadOutcome::LoaderChanged => println!(
            "The Remote Script's loader changed, which is the one part Live only reads when it starts. Restart Live, or re-select AbletonMusicMaker under Settings → Link, Tempo & MIDI. No reload was sent: the handlers update on their own; the loader does not."
        ),
        ReloadOutcome::Reloaded(result) => {
            println!(
                "Live re-read its body: v{} → v{} in {} ms on Live's main thread.",
                result["was"].as_str().unwrap_or("?"),
                result["now"].as_str().unwrap_or("?"),
                result["main_ms"]
            );
            let kept = &result["kept"];
            println!(
                "Kept: {} client(s), {} subscription(s), {} pending cue(s), performance mode {}, {} section phrase(s), {} mix snapshot(s).",
                kept["clients"],
                kept["subscriptions"],
                kept["cues"],
                if kept["performance_mode"].as_bool().unwrap_or(false) {
                    "on"
                } else {
                    "off"
                },
                kept["scene_phrases"],
                kept["mix_snapshots"]
            );
            println!("Nothing to restart. Keep playing.");
        }
        ReloadOutcome::Failed(e) => {
            println!("The files are installed, but Live did not reload: {e}");
            println!("Restart Live, or re-select AbletonMusicMaker under Settings → Link, Tempo & MIDI.");
            std::process::exit(1);
        }
    }
}
