use std::io::Read;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use tracing_subscriber::prelude::*;

/// Agent EDR — endpoint detection & response for AI coding agents.
///
/// Captures and logs events from AI coding tool hooks for auditing and analysis.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the log file
    #[arg(long, default_value = "/tmp/agent-edr.log")]
    log_file: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Process events from Kiro CLI hooks.
    ///
    /// Reads JSON hook event data from STDIN and logs it to the configured log file.
    /// Kiro CLI pipes hook event data as JSON via STDIN.
    Kiro,
}

/// Represents a Kiro CLI hook event.
///
/// All events share `hook_event_name` and `cwd`. Additional fields are present
/// depending on the event type:
///   - agentSpawn: no extra fields
///   - userPromptSubmit: `prompt`
///   - preToolUse: `tool_name`, `tool_input`
///   - postToolUse: `tool_name`, `tool_input`, `tool_response`
#[derive(Debug, Deserialize, Serialize)]
struct KiroHookEvent {
    hook_event_name: String,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_response: Option<serde_json::Value>,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Kiro => {
            if let Err(e) = handle_kiro(&cli.log_file) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn handle_kiro(log_file: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // Set up file-based tracing.
    let log_dir = log_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let log_filename = log_file
        .file_name()
        .ok_or("invalid log file path")?
        .to_str()
        .ok_or("log filename is not valid UTF-8")?;

    let file_appender = tracing_appender::rolling::never(log_dir, log_filename);
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(non_blocking)
                .with_target(false),
        )
        .init();

    // Read JSON from STDIN.
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let event: KiroHookEvent = serde_json::from_str(&input)?;

    // Log the event as clean JSON (validated through deserialization).
    let event_json = serde_json::to_string(&event)?;
    tracing::info!(event = %event_json, "kiro hook event received");

    // Also log the raw payload for full fidelity.
    tracing::info!(raw_json = %input.trim(), "kiro hook raw payload");

    Ok(())
}
