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
    /// Print the state of every telemetry and dataset gate as JSON and exit.
    #[arg(long)]
    privacy_status: bool,
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
    if cli.privacy_status {
        println!(
            "{}",
            serde_json::to_string_pretty(&mcp_ableton_music_maker::app::privacy_status())
                .unwrap_or_default()
        );
        return;
    }
    if let Err(e) = mcp_ableton_music_maker::app::serve_stdio().await {
        tracing::error!("server error: {}", e);
        std::process::exit(1);
    }
}
