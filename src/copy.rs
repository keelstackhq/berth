//! Copy allow-listed, gitignored files from the main checkout into a worktree (spec §10).
//! Contents and hashes are never read into a string, logged or reported: items carry paths
//! and fixed reasons only.

use crate::config::CopySource;
use crate::git::{self, Repo};
use crate::report::{Action, Item};
use crate::{BerthError, vlog};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyPlan {
    /// Paths (relative to the main checkout) that passed every check.
    pub files: Vec<String>,
    pub refused: Vec<Item>,
    pub skipped: Vec<Item>,
}

fn io_err(path: &Path, source: std::io::Error) -> BerthError {
    BerthError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn item(path: &str, action: Action, reason: impl Into<String>) -> Item {
    Item::new("copy", path, action, reason)
}

fn anchored(glob: &str) -> String {
    format!("/{}", glob.trim_start_matches('/'))
}

pub fn plan_copy(repo: &Repo, src: &CopySource, max_bytes: u64) -> Result<CopyPlan, BerthError> {
    let main = &repo.main;
    let mut plan = CopyPlan::default();
    let candidates: Vec<String> = match src {
        CopySource::Include(globs) => {
            if globs.is_empty() {
                return Ok(plan);
            }
            let excludes: Vec<String> = globs
                .iter()
                .map(|g| format!("--exclude={}", anchored(g)))
                .collect();
            let mut tracked_args = vec!["--cached".to_string(), "--ignored".to_string()];
            tracked_args.extend(excludes.iter().cloned());
            for path in git::ls_files(main, &tracked_args)? {
                let reason = format!(
                    "tracked by git; run `git rm --cached {path}` and add it to .gitignore"
                );
                plan.refused.push(item(&path, Action::Refused, reason));
            }
            let mut args = vec!["--others".to_string(), "--ignored".to_string()];
            args.extend(excludes);
            let found = git::ls_files(main, &args)?;
            let ignored = git::ignored(main, &found)?;
            let mut keep = Vec::new();
            for path in found {
                if ignored.contains(&path) {
                    keep.push(path);
                } else {
                    plan.refused.push(item(
                        &path,
                        Action::Refused,
                        "not ignored by git; add it to .gitignore",
                    ));
                }
            }
            keep
        }
        CopySource::WorktreeInclude(file) => {
            let args = vec![
                "--others".to_string(),
                "--ignored".to_string(),
                format!("--exclude-from={}", file.display()),
            ];
            let found = git::ls_files(main, &args)?;
            let ignored = git::ignored(main, &found)?;
            let mut keep = Vec::new();
            for path in found {
                if ignored.contains(&path) {
                    keep.push(path);
                } else {
                    plan.skipped.push(item(
                        &path,
                        Action::Skipped,
                        "not gitignored (.worktreeinclude copies only ignored files)",
                    ));
                }
            }
            keep
        }
    };
    let mut seen = BTreeSet::new();
    for path in candidates {
        if !seen.insert(path.clone()) {
            continue;
        }
        let full = main.join(&path);
        let meta = match fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) => return Err(io_err(&full, e)),
        };
        if meta.file_type().is_symlink() {
            plan.refused.push(item(&path, Action::Refused, "symlink"));
        } else if !meta.is_file() {
            plan.refused
                .push(item(&path, Action::Refused, "not a regular file"));
        } else if meta.len() > max_bytes {
            let reason = format!("larger than [copy].max_bytes ({max_bytes})");
            plan.refused.push(item(&path, Action::Refused, reason));
        } else {
            plan.files.push(path);
        }
    }
    Ok(plan)
}

/// First symlink among the existing components of `rel`'s parent chain inside `wt`.
fn symlink_in_parents(wt: &Path, rel: &str) -> Result<bool, BerthError> {
    let mut cur = wt.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        cur.push(part);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(io_err(&cur, e)),
        }
    }
    Ok(false)
}

fn copy_one(src: &Path, target: &Path, tmp_dir: &Path) -> Result<(), BerthError> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp: PathBuf = tmp_dir.join(format!("copy-{}-{n}", std::process::id()));
    let result = (|| {
        fs::copy(src, &tmp).map_err(|e| io_err(src, e))?;
        fs::File::open(&tmp)
            .and_then(|f| f.sync_all())
            .map_err(|e| io_err(&tmp, e))?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        }
        fs::rename(&tmp, target).map_err(|e| io_err(target, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Copy `files` (relative paths) from `main` into `wt`. Refusals (a symlink in the target's
/// parent chain) are returned as items; the caller treats them as failures.
pub fn execute_copy(main: &Path, wt: &Path, files: &[String]) -> Result<Vec<Item>, BerthError> {
    let berth_dir = wt.join(".berth");
    if fs::symlink_metadata(&berth_dir).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(BerthError::Env(format!(
            "{} is a symlink; remove it so berth can stage copies safely",
            berth_dir.display()
        )));
    }
    let tmp_dir = berth_dir.join("tmp");
    fs::create_dir_all(&tmp_dir).map_err(|e| io_err(&tmp_dir, e))?;
    let mut items = Vec::new();
    for rel in files {
        let target = wt.join(rel);
        if symlink_in_parents(wt, rel)? {
            items.push(item(
                rel,
                Action::Refused,
                "target path passes through a symlink",
            ));
            continue;
        }
        if fs::symlink_metadata(&target).is_ok() {
            items.push(item(rel, Action::Skipped, "exists"));
            continue;
        }
        vlog(&format!("copy {rel}"));
        copy_one(&main.join(rel), &target, &tmp_dir)?;
        items.push(item(rel, Action::Create, "copied"));
    }
    Ok(items)
}
