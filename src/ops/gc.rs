//! `berth gc` (spec §11): `down` applied to every finished task. The scan takes no lock; each
//! removal is `down_task` with its own locks, refusals and teardown, and never `--force`.

use crate::cli::GcArgs;
use crate::manifest::{Entry, Manifest, State};
use crate::ops::down::{down_task, is_registered_worktree};
use crate::report::{Action, Item, Report};
use crate::{BerthError, Ctx, config, git};

enum Verdict {
    Keep(String),
    Remove,
}

pub fn gc(ctx: &Ctx, args: &GcArgs) -> Result<Report, BerthError> {
    let main = &ctx.repo.main;
    let mut report = Report::new("gc");
    let manifest = Manifest::load(main)?;
    if manifest.worktrees.is_empty() {
        report
            .items
            .push(Item::new("gc", "repo", Action::Ok, "nothing to collect"));
        return Ok(report);
    }
    let cfg = config::load(main)?;
    let base_name = cfg.worktree.base.clone();
    let base = git::resolve_commit(main, &base_name)?;

    for entry in &manifest.worktrees {
        let task = entry.task.as_str();
        match judge(ctx, entry, &base, &base_name) {
            Err(err) => {
                report
                    .items
                    .push(Item::new("error", task, Action::Refused, err.reason()));
                report.exit_code = 3;
            }
            Ok(Verdict::Keep(why)) => {
                report.items.push(Item::new(
                    "worktree",
                    task,
                    Action::Skipped,
                    format!("kept: {why}"),
                ));
            }
            Ok(Verdict::Remove) if args.dry_run => {
                report
                    .items
                    .push(Item::new("worktree", task, Action::Ok, "would remove"));
                report.worktrees.push(entry.clone());
            }
            Ok(Verdict::Remove) => match down_task(ctx, task, false, args.delete_branch) {
                Ok(done) => {
                    report.items.extend(done.items);
                    if done.exit_code == 0 {
                        report.worktrees.push(entry.clone());
                    } else if report.exit_code == 0 {
                        report.exit_code = done.exit_code;
                    }
                }
                Err(err) => {
                    report
                        .items
                        .push(Item::new("error", task, Action::Refused, err.reason()));
                    report.exit_code = 3;
                }
            },
        }
    }
    Ok(report)
}

fn judge(ctx: &Ctx, entry: &Entry, base: &str, base_name: &str) -> Result<Verdict, BerthError> {
    let main = &ctx.repo.main;
    let keep = |why: String| Ok(Verdict::Keep(why));
    if entry.state != State::Ready {
        let state = serde_json::to_value(entry.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        return keep(format!("state {state}"));
    }
    if !entry.path.exists() || !is_registered_worktree(main, &entry.path)? {
        return keep("worktree missing".into());
    }
    if !git::branch_exists(main, &entry.branch)? {
        return keep("branch missing".into());
    }
    if git::commits_since(main, &entry.base, &entry.branch)? == 0 {
        return keep("no commits yet".into());
    }
    if !git::is_merged(main, &entry.branch, base)? {
        return keep(format!("not merged into {base_name}"));
    }
    if git::dirty(&entry.path)? {
        return keep("uncommitted changes".into());
    }
    Ok(Verdict::Remove)
}
