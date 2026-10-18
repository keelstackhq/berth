//! `berth env` (spec §8): print the environment recorded for a task. Read-only, lock-free.

use std::path::PathBuf;

use crate::cli::EnvArgs;
use crate::env::format_env;
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
    report.text = Some(format_env(&entry.env, args.format));
    report.worktrees = vec![entry.clone()];
    Ok(report)
}

fn canon(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
