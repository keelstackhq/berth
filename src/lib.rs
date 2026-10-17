#![forbid(unsafe_code)]

pub mod cli;
pub mod error;
pub mod manifest;
pub mod ops;
pub mod report;

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
    let _ctx = Ctx {
        dir,
        verbose: cli.verbose,
        json: cli.json,
    };
    Ok(Report::new(cli.command.name()))
}
