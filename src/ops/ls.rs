//! `berth ls` (spec §12): one item per manifest entry plus unmanaged worktrees under the root.
//! Read-only: takes no lock and creates no file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config;
use crate::git;
use crate::manifest::{Entry, Manifest, State};
use crate::ops::up::{state_name, worktree_path};
use crate::ports::{BindProber, Prober};
use crate::report::{Action, Item, Report};
use crate::{BerthError, Ctx, sys};

pub fn ls(ctx: &Ctx) -> Result<Report, BerthError> {
    let main = &ctx.repo.main;
    let cfg = config::load(main)?;
    let manifest = Manifest::load(main)?;
    let listed = git::worktree_list(main)?;
    let registered: BTreeSet<PathBuf> = listed.iter().map(|w| canon(&w.path)).collect();

    let mut report = Report::new("ls");
    for entry in &manifest.worktrees {
        report.items.push(entry_item(main, entry, &registered));
    }

    let managed: BTreeSet<PathBuf> = manifest.worktrees.iter().map(|e| canon(&e.path)).collect();
    let root = worktree_path(&ctx.repo, &cfg, "x")
        .parent()
        .map(canon)
        .unwrap_or_default();
    let mut unmanaged: Vec<(String, PathBuf)> = Vec::new();
    for w in &listed {
        let path = canon(&w.path);
        let under_root = path != *main && path.parent() == Some(root.as_path());
        if under_root
            && !managed.contains(&path)
            && let Some(name) = path.file_name()
        {
            unmanaged.push((name.to_string_lossy().into_owned(), path));
        }
    }
    unmanaged.sort();
    for (name, path) in unmanaged {
        report.items.push(Item::new(
            "worktree",
            name,
            Action::Drift,
            format!("unmanaged · {}", path.display()),
        ));
    }
    report.worktrees = manifest.worktrees;
    Ok(report)
}

fn canon(path: &Path) -> PathBuf {
    crate::sys::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn entry_item(main: &Path, entry: &Entry, registered: &BTreeSet<PathBuf>) -> Item {
    let task = entry.task.as_str();
    let exists = entry.path.exists();
    if !exists {
        return Item::new(
            "worktree",
            task,
            Action::Drift,
            format!("missing: worktree directory deleted; run berth down {task}"),
        );
    }
    if entry.state == State::Creating && !sys::pid_alive(entry.owner.pid, &entry.owner.host) {
        return Item::new(
            "worktree",
            task,
            Action::Drift,
            format!(
                "interrupted: owner pid {} is gone; run berth down {task}",
                entry.owner.pid
            ),
        );
    }
    let ahead_behind = match git::ahead_behind(main, &entry.base, &entry.branch) {
        Ok((ahead, behind)) => format!("ahead {ahead} behind {behind}"),
        Err(_) => "ahead ? behind ?".to_string(),
    };
    if !registered.contains(&canon(&entry.path)) {
        return Item::new(
            "worktree",
            task,
            Action::Drift,
            format!("unregistered: not a worktree of this repo; run berth down {task}"),
        );
    }
    let tree = match git::dirty(&entry.path) {
        Ok(true) => "dirty",
        Ok(false) => "clean",
        Err(_) => "dirty ?",
    };
    let mut parts = vec![
        state_name(entry.state).to_string(),
        entry.branch.clone(),
        ahead_behind,
        tree.to_string(),
    ];
    let listening = listening_ports(entry);
    if !listening.is_empty() {
        parts.push(format!("listening {}", listening.join(",")));
    }
    parts.push(entry.path.display().to_string());
    Item::new("worktree", task, Action::Ok, parts.join(" · "))
}

/// Names of the entry's ports something listens on (port numbers when the block has no names).
fn listening_ports(entry: &Entry) -> Vec<String> {
    let prober = BindProber;
    if entry.ports.named.is_empty() {
        (entry.ports.start..=entry.ports.end)
            .filter(|&p| !prober.is_free(p))
            .map(|p| p.to_string())
            .collect()
    } else {
        entry
            .ports
            .named
            .iter()
            .filter(|&(_, &p)| !prober.is_free(p))
            .map(|(name, _)| name.clone())
            .collect()
    }
}
