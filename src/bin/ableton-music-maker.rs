//! `ableton-music-maker`: the MCP server, spoken over stdio.
//!
//! stdout is the protocol channel; all diagnostics go to stderr.

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "mcp-ableton-music-maker",
    version,
    about = "Ableton Live integration through the Model Context Protocol"
)]
struct Cli {
    /// Print version, paths and activity-log settings as JSON and exit.
    #[arg(long)]
    status: bool,
    /// Ask Live for the loaded Remote Script and the current session, print
    /// the result as JSON, and exit 0 if the script is loaded and up to date.
    /// Changes nothing in the set.
    #[arg(long)]
    check: bool,
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_logging();
    if cli.status {
        println!(
            "{}",
            serde_json::to_string_pretty(&mcp_ableton_music_maker::app::status())
                .unwrap_or_default()
        );
        return;
    }
    if cli.check {
        let (report, ok) = tokio::task::spawn_blocking(mcp_ableton_music_maker::app::check)
            .await
            .unwrap_or_else(|e| (serde_json::json!({"error": e.to_string()}), false));
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        std::process::exit(if ok { 0 } else { 1 });
    }
    if let Err(e) = mcp_ableton_music_maker::app::serve_stdio().await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
