#![forbid(unsafe_code)]

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand, ValueEnum};
use imessage_exporter::auracle::{DEFAULT_PROGRESS_EVERY, ExportOptions, export_jsonl};

#[derive(Debug, Parser)]
#[command(
    name = "auracle-imessage-exporter",
    version,
    about = "Stream a macOS Messages archive as Auracle JSONL v1"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Stream an Auracle JSONL v1 archive to stdout.
    ExportJsonl {
        /// Messages SQLite database.
        #[arg(long)]
        db_path: PathBuf,
        /// Export attachment metadata only. Attachment bodies are never read.
        #[arg(long, value_enum)]
        attachments: AttachmentMode,
        /// Opaque cursor returned by a prior successful export.
        #[arg(long)]
        cursor: Option<String>,
        /// Opaque cursor from a `progress` record of an interrupted pass.
        /// Messages that pass already streamed are skipped; handles and chats
        /// are streamed again so the resumed output stands on its own.
        #[arg(long)]
        resume: Option<String>,
    },
}

#[derive(Clone, Debug, ValueEnum)]
enum AttachmentMode {
    Metadata,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::ExportJsonl {
            db_path,
            attachments: AttachmentMode::Metadata,
            cursor,
            resume,
        } => export_jsonl(
            &ExportOptions {
                db_path,
                cursor,
                resume,
                progress_every: DEFAULT_PROGRESS_EVERY,
            },
            std::io::stdout().lock(),
        ),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Error categories are deliberately data-free: no paths, handles,
            // attachment names, or message text may reach diagnostics.
            eprintln!("auracle-imessage-exporter: {error}");
            ExitCode::FAILURE
        }
    }
}
