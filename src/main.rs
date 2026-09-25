use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "codex-cache")]
#[command(about = "Inspect, verify, and clean Codex release caches")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show a compact release table.
    List,
    /// Show detected Codex releases and paths.
    Scan,
    /// Show size totals and retention estimates.
    Report,
    /// Verify Codex package cache layouts without modifying them.
    Verify,
    /// Delete inactive Codex releases selected by the keep policy.
    Clean {
        /// Show what would be removed without deleting anything.
        #[arg(long)]
        dry_run: bool,
        /// Releases to keep: current or current,previous.
        #[arg(long)]
        keep: codex_cache::KeepPolicy,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::List => {
            let scan = codex_cache::scan_default()?;
            print!("{}", codex_cache::render_list(&scan));
        }
        Command::Scan => {
            let scan = codex_cache::scan_default()?;
            print!("{}", codex_cache::render_scan(&scan));
        }
        Command::Report => {
            let scan = codex_cache::scan_default()?;
            print!("{}", codex_cache::render_report(&scan));
        }
        Command::Verify => {
            let report = codex_cache::verify_default()?;
            print!("{}", codex_cache::render_verify(&report));
        }
        Command::Clean { dry_run, keep } => {
            if dry_run {
                let scan = codex_cache::scan_default()?;
                let plans = codex_cache::plan_scan_deletions(&scan, keep)?;
                print!("{}", codex_cache::render_clean_dry_runs(&plans));
            } else {
                let result = codex_cache::clean_default(keep)?;
                print!("{}", codex_cache::render_clean_result(&result));
                if result.has_failures() {
                    bail!("one or more deletions failed");
                }
            }
        }
    }

    Ok(())
}
