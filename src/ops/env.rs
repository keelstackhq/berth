//! `berth env` (spec §8): print the environment recorded for a task. Read-only, lock-free.

use std::path::PathBuf;

use crate::cache;
use crate::cli::{EnvArgs, Format};
use crate::config;
use crate::env::{format_env, format_env_hoard};
use crate::manifest::Manifest;
use crate::report::Report;
use crate::{BerthError, Ctx};

pub fn env(ctx: &Ctx, args: &EnvArgs) -> Result<Report, BerthError> {
    let manifest = Manifest::load(&ctx.repo.main)?;
    let entry = match &args.task {
        Some(task) => {
            let task = task.as_str();
            manifest.find(task).ok_or_else(|| {
                BerthError::Env(format!("no berth task `{task}`; run berth up {task}"))
            })?
        }
        None => {
            let here = canon(&ctx.dir);
            manifest
                .worktrees
                .iter()
                .find(|e| here.starts_with(canon(&e.path)))
                .ok_or_else(|| {
                    BerthError::Usage("not inside a berth worktree; pass <task>".into())
                })?
        }
    };
    let mut report = Report::new("env");
    // A broken berth.toml must not hide the recorded environment: only hoard needs the config.
    let hoard = match config::load(&ctx.repo.main) {
        Ok(cfg) => cache::hoard(&ctx.repo.main, &cfg.cache),
        Err(err) => {
            eprintln!("warning: {err}; hoard values not exported");
            None
        }
    };
    report.text = Some(match hoard {
        Some(h) => {
            if matches!(args.format, Format::Dotenv | Format::Json) && !ctx.json {
                eprintln!(
                    "note: TURBO_TOKEN is not printed; read it from {}",
                    h.token_file.display()
                );
            }
            format_env_hoard(&entry.env, args.format, &h.api, &h.token_file)
        }
        None => format_env(&entry.env, args.format),
    });
    report.worktrees = vec![entry.clone()];
    Ok(report)
}

fn canon(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
