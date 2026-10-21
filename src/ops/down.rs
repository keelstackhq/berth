//! `berth down` (spec §11): remove the worktree of a task, free its ports, drop its entry.
//! The branch stays unless `--delete-branch` and it is merged into `[worktree].base`.

use std::path::Path;
use std::time::Duration;

use crate::cache;
use crate::cli::DownArgs;
use crate::config;
use crate::db;
use crate::git;
use crate::hooks::run_hooks;
use crate::manifest::{Entry, Manifest, State};
use crate::ops::up::{ports_item, repo_lock, unreserve};
use crate::registry;
use crate::report::{Action, Item, Report};
use crate::{BerthError, Ctx, sys};

pub fn down(ctx: &Ctx, args: &DownArgs) -> Result<Report, BerthError> {
    let repo = &ctx.repo;
    let main = &repo.main;
    let task = args.task.as_str();
    let state = registry::state_dir()?;

    // 1. Repo lock. A task without an entry may still own a block a crashed `up` left behind.
    let guard = repo_lock(repo)?;
    let mut manifest = Manifest::load(main)?;
    let Some(entry) = manifest.find(task).cloned() else {
        let freed = unreserve(&state, main, task)?;
        let mut report = Report::new("down");
        report.items.push(if freed > 0 {
            Item::new("ports", task, Action::Remove, "released an orphaned block")
        } else {
            Item::new("worktree", task, Action::Ok, "no such task")
        });
        return Ok(report);
    };
    let cfg = config::load(main)?;
    let path = entry.path.clone();

    // 2. Someone is still creating it. An owner on another host cannot be verified, so
    // `--force` may override it; a live pid on this host may be a real `up` and always refuses.
    if entry.state == State::Creating && sys::pid_alive(entry.owner.pid, &entry.owner.host) {
        let foreign = entry.owner.host != sys::hostname();
        if !(foreign && args.force) {
            let reason = if foreign {
                format!(
                    "task is being created by pid {} on another host ({}); cannot verify; \
                     if that up is gone, run berth down --force {task}",
                    entry.owner.pid, entry.owner.host
                )
            } else {
                format!("task is being created by pid {}", entry.owner.pid)
            };
            return Ok(refused(
                vec![Item::new("worktree", task, Action::Refused, reason)],
                &entry,
            ));
        }
    }

    // Only a registered linked worktree is berth's to inspect, run hooks in or remove.
    let registered = is_registered_worktree(main, &path)?;

    // 3 and 4. Refusals change nothing.
    let mut refusals = Vec::new();
    if registered && path.exists() && !args.force && git::dirty(&path)? {
        refusals.push(Item::new(
            "worktree",
            task,
            Action::Refused,
            "uncommitted changes; commit them or pass --force",
        ));
    }
    let branch_present = git::branch_exists(main, &entry.branch)?;
    if args.delete_branch && branch_present {
        let base = git::resolve_commit(main, &cfg.worktree.base)?;
        if !git::is_merged(main, &entry.branch, &base)? {
            refusals.push(Item::new(
                "branch",
                &entry.branch,
                Action::Refused,
                "branch has unmerged commits; merge it or delete it with git branch -D",
            ));
        }
    }
    if !refusals.is_empty() {
        return Ok(refused(refusals, &entry));
    }

    // 5. Mark `removing`, then work without the repo lock (the teardown hook may be slow).
    let mut removing = entry.clone();
    removing.state = State::Removing;
    manifest.upsert(removing);
    manifest.save(main)?;
    drop(guard);

    let mut items = Vec::new();

    // 6. Teardown.
    if !path.exists() {
        items.push(Item::new(
            "teardown",
            task,
            Action::Skipped,
            "worktree directory missing",
        ));
    } else if registered {
        let timeout = Duration::from_secs(cfg.teardown.timeout_secs);
        let hoard = cache::hoard(main, &cfg.cache).filter(|_| !cfg.teardown.run.is_empty());
        let hook_env = cache::hook_env(&entry.env, hoard.as_ref());
        if let Err(err) = run_hooks("teardown", &cfg.teardown.run, &path, &hook_env, timeout) {
            if !args.force {
                return Err(err);
            }
            eprintln!("warning: {err}");
            items.push(Item::new(
                "teardown",
                task,
                Action::Drift,
                "teardown failed (ignored by --force)",
            ));
        }
    }

    // 6b. The database, before its worktree goes (sqlite lives in the worktree and needs nothing).
    if let Some(record) = &entry.db {
        let cwd = if registered && path.exists() {
            &path
        } else {
            main
        };
        let timeout = cfg
            .db
            .as_ref()
            .map_or(Duration::from_secs(300), |d| d.timeout());
        match db::drop(main, cwd, record, args.force, &entry.env, timeout) {
            Ok(item) => items.extend(item),
            Err(err) if args.force => {
                eprintln!("warning: {err}");
                items.push(Item::new(
                    "db",
                    &record.name,
                    Action::Drift,
                    "not dropped (ignored by --force)",
                ));
            }
            Err(err) => {
                return Err(BerthError::Env(format!(
                    "{err}; the entry stays `removing`: fix it and run berth down {task} again, \
                     or pass --force"
                )));
            }
        }
    }

    // 7. Remove the worktree, free the ports, drop the entry, then the branch.
    let _guard = repo_lock(repo)?;
    if !registered && path.exists() {
        items.push(Item::new(
            "worktree",
            task,
            Action::Skipped,
            format!(
                "{} is not a worktree of this repo; left in place",
                path.display()
            ),
        ));
    } else {
        if path.exists() {
            git::worktree_remove(main, &path, args.force).map_err(|err| match err {
                BerthError::Git { .. } if !args.force => BerthError::Env(format!(
                    "{err}; the entry stays `removing`: fix that or pass --force to remove it anyway"
                )),
                other => other,
            })?;
        }
        items.push(Item::new(
            "worktree",
            task,
            Action::Remove,
            path.display().to_string(),
        ));
    }
    git::worktree_prune(main)?;
    unreserve(&state, main, task)?;
    items.push(ports_item(&entry.ports, Action::Remove));
    let mut manifest = Manifest::load(main)?;
    manifest.remove(task);
    manifest.save(main)?;
    if args.delete_branch && branch_present {
        // The teardown hook may have committed since the check in step 4: never lose commits.
        let merged = git::resolve_commit(main, &cfg.worktree.base)
            .and_then(|base| git::is_merged(main, &entry.branch, &base))
            .map_err(|err| {
                BerthError::Env(format!(
                    "{err}; the worktree and ports were already removed and branch {} was kept; \
                     delete it with git branch -d once it is merged",
                    entry.branch
                ))
            })?;
        if merged {
            git::delete_branch(main, &entry.branch)?;
            items.push(Item::new("branch", &entry.branch, Action::Remove, "merged"));
        } else {
            items.push(Item::new(
                "branch",
                &entry.branch,
                Action::Skipped,
                "branch has unmerged commits (made during teardown); kept",
            ));
        }
    }
    if let Some(root) = path.parent() {
        // Only succeeds when empty.
        let _ = std::fs::remove_dir(root);
    }

    let mut report = Report::new("down");
    report.items = items;
    Ok(report)
}

fn refused(items: Vec<Item>, entry: &Entry) -> Report {
    let mut report = Report::new("down");
    report.items = items;
    report.exit_code = 1;
    report.worktrees = vec![entry.clone()];
    report
}

/// True when `path` is a linked worktree git knows about (never the main checkout).
fn is_registered_worktree(main: &Path, path: &Path) -> Result<bool, BerthError> {
    let want = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if want == main {
        return Ok(false);
    }
    Ok(git::worktree_list(main)?.iter().any(|w| {
        let have = std::fs::canonicalize(&w.path).unwrap_or_else(|_| w.path.clone());
        have == want
    }))
}
