use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "codex-cache")]
#[command(about = "Inspect the Codex CLI standalone release cache")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show a compact release table.
    List,
    /// Show detected Codex standalone releases and paths.
    Scan,
    /// Show size totals and retention estimates.
    Report,
    /// Verify the Codex standalone cache layout without modifying it.
    Verify,
    /// Plan Codex standalone release cleanup without deleting anything.
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
            if !dry_run {
                bail!("clean only supports planning; pass --dry-run");
            }
            let scan = codex_cache::scan_default()?;
            let plan = codex_cache::plan_deletions(&scan.releases, keep)?;
            print!("{}", codex_cache::render_clean_dry_run(&plan));
        }
    }

    Ok(())
}
