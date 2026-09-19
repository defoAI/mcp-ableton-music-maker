//! `ableton-music-maker-install-script`: copy the embedded Remote Script into Live's
//! User Library.

use clap::Parser;
use mcp_ableton_music_maker::install::{
    discover_remote_script_dirs, install_remote_script, InstallStatus,
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

    let results = install_remote_script(cli.target.as_deref(), cli.force);
    for r in &results {
        println!(
            "{}: {} ({})",
            r.status,
            r.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".into()),
            r.detail
        );
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
    println!("\nIf Ableton was already open: restart Live, or re-select AbletonMusicMaker under Preferences → Link/Tempo/MIDI → Control Surface.");
}
