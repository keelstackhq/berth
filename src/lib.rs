#![forbid(unsafe_code)]

pub mod cache;
pub mod cli;
pub mod config;
pub mod copy;
pub mod db;
pub mod env;
pub mod error;
pub mod git;
pub mod hooks;
pub mod lock;
pub mod manifest;
pub mod ops;
pub mod ports;
pub mod registry;
pub mod report;
pub mod sys;

pub use cli::Cli;
pub use error::BerthError;
pub use report::{Report, render_json, render_table};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Log a line to stderr as `berth: <msg>` when `-v` is set. Never stdout.
pub fn vlog(msg: &str) {
    if verbose() {
        eprintln!("berth: {msg}");
    }
}

/// What every command needs: where it runs and how it reports.
pub struct Ctx {
    pub dir: PathBuf,
    /// The repository discovered from `dir` (main checkout, common dir, name).
    pub repo: git::Repo,
    pub verbose: bool,
    pub json: bool,
}

pub fn run(cli: Cli) -> Result<Report, BerthError> {
    let dir = match cli.dir {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(|source| BerthError::Io {
            path: PathBuf::from("."),
            source,
        })?,
    };
    let repo = git::discover(&dir)?;
    let ctx = Ctx {
        dir,
        repo,
        verbose: cli.verbose,
        json: cli.json,
    };
    match &cli.command {
        cli::Command::Up(args) => ops::up::up(&ctx, args),
        cli::Command::Down(args) => ops::down::down(&ctx, args),
        cli::Command::Ls => ops::ls::ls(&ctx),
        cli::Command::Env(args) => ops::env::env(&ctx, args),
        cli::Command::Doctor => ops::doctor::doctor(&ctx),
    }
}
