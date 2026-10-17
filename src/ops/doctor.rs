//! `berth doctor` (spec §11): read-only checks, one or more items each. Takes no lock and
//! creates no file; registry staleness goes through `registry::is_stale` (a `lock::is_held`
//! probe, never `acquire`). Exit 1 on any `drift`, 3 on errors.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::copy::plan_copy;
use crate::manifest::{Manifest, State, manifest_path};
use crate::ops::up::worktree_path;
use crate::ports::{BindProber, Prober};
use crate::registry::{self, Registry};
use crate::report::{Action, Item, Report};
use crate::{BerthError, Ctx, config, git, sys};

/// What `up` keeps out of git, checked as paths below the entry (a directory pattern matches).
const EXCLUDED: [&str; 2] = [".agents/worktrees.json", ".berth/x"];

fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn doctor(ctx: &Ctx) -> Result<Report, BerthError> {
    let main = &ctx.repo.main;
    let mut report = Report::new("doctor");
    let items = &mut report.items;

    let (major, minor) = git::check_version()?;
    items.push(Item::new(
        "git",
        "version",
        Action::Ok,
        format!("git {major}.{minor}"),
    ));

    let cfg = config::load(main)?;
    items.push(Item::new("config", "berth.toml", Action::Ok, "valid"));

    let plan = plan_copy(&ctx.repo, &cfg.copy_source(main), cfg.max_bytes())?;
    for refused in plan.refused {
        items.push(Item::new(
            "copy",
            refused.key,
            Action::Drift,
            refused.reason,
        ));
    }
    if !items.iter().any(|i| i.kind == "copy") {
        items.push(Item::new(
            "copy",
            "patterns",
            Action::Ok,
            "all matches are ignored files",
        ));
    }

    let root = worktree_path(&ctx.repo, &cfg, "x")
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    items.push(root_item(main, &root)?);

    let state = registry::state_dir()?;
    let stale: Vec<Item> = Registry::load(&state)?
        .blocks
        .iter()
        .filter(|b| registry::is_stale(b))
        .map(|b| {
            Item::new(
                "registry",
                format!("{}-{}", b.start, b.end),
                Action::Drift,
                format!("stale: {} {}", b.repo.display(), b.task),
            )
        })
        .collect();
    if stale.is_empty() {
        items.push(Item::new(
            "registry",
            "ports",
            Action::Ok,
            "no stale blocks",
        ));
    }
    items.extend(stale);

    if manifest_path(main).exists() {
        let manifest = Manifest::load(main)?;
        items.extend(manifest_items(&manifest, main, &root)?);
        items.extend(exclude_items(main)?);
        items.extend(port_items(&manifest));
        items.extend(owner_items(&manifest));
        report.worktrees = manifest.worktrees;
    }
    if report.items.iter().any(|i| i.action == Action::Drift) {
        report.exit_code = 1;
    }
    Ok(report)
}

fn root_item(main: &Path, root: &Path) -> Result<Item, BerthError> {
    let key = root.display().to_string();
    let Ok(rel) = root.strip_prefix(main) else {
        return Ok(Item::new(
            "root",
            key,
            Action::Ok,
            "outside the main checkout",
        ));
    };
    let probe = rel.join("x").to_string_lossy().into_owned();
    if git::ignored(main, std::slice::from_ref(&probe))?.is_empty() {
        return Ok(Item::new(
            "root",
            key,
            Action::Drift,
            format!(
                "inside the repo and not ignored by git; Node resolves dependencies upward \
                 into the main checkout (spec §5): add {}/ to .gitignore or move [worktree].root",
                rel.display()
            ),
        ));
    }
    Ok(Item::new(
        "root",
        key,
        Action::Ok,
        "inside the repo and ignored by git",
    ))
}

fn manifest_items(manifest: &Manifest, main: &Path, root: &Path) -> Result<Vec<Item>, BerthError> {
    let listed = git::worktree_list(main)?;
    let registered: BTreeSet<PathBuf> = listed.iter().map(|w| canon(&w.path)).collect();
    let mut out = Vec::new();
    for e in &manifest.worktrees {
        let task = e.task.as_str();
        let creating = e.state == State::Creating && sys::pid_alive(e.owner.pid, &e.owner.host);
        if !e.path.exists() {
            out.push(Item::new(
                "manifest",
                task,
                Action::Drift,
                format!(
                    "missing: {} does not exist; run berth down {task}",
                    e.path.display()
                ),
            ));
        } else if !creating && !registered.contains(&canon(&e.path)) {
            out.push(Item::new(
                "manifest",
                task,
                Action::Drift,
                format!("unregistered: not in git worktree list; run berth down {task}"),
            ));
        }
    }
    let managed: BTreeSet<PathBuf> = manifest.worktrees.iter().map(|e| canon(&e.path)).collect();
    let root = canon(root);
    for w in &listed {
        let path = canon(&w.path);
        if path != *main && path.parent() == Some(root.as_path()) && !managed.contains(&path) {
            out.push(Item::new(
                "manifest",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                Action::Drift,
                format!("unmanaged: {} has no manifest entry", path.display()),
            ));
        }
    }
    if out.is_empty() {
        out.push(Item::new(
            "manifest",
            "worktrees",
            Action::Ok,
            format!(
                "{} entries match git worktree list",
                manifest.worktrees.len()
            ),
        ));
    }
    Ok(out)
}

fn exclude_items(main: &Path) -> Result<Vec<Item>, BerthError> {
    let paths: Vec<String> = EXCLUDED.iter().map(|s| s.to_string()).collect();
    let ignored = git::ignored(main, &paths)?;
    let missing: Vec<Item> = EXCLUDED
        .iter()
        .filter(|p| !ignored.contains(**p))
        .map(|p| {
            let shown = p.trim_end_matches("/x");
            Item::new(
                "exclude",
                shown,
                Action::Drift,
                format!("not ignored by git; add /{shown} to .git/info/exclude or .gitignore"),
            )
        })
        .collect();
    if missing.is_empty() {
        return Ok(vec![Item::new(
            "exclude",
            "berth files",
            Action::Ok,
            "manifest and .berth/ are ignored",
        )]);
    }
    Ok(missing)
}

fn port_items(manifest: &Manifest) -> Vec<Item> {
    let prober = BindProber;
    manifest
        .worktrees
        .iter()
        .map(|e| {
            let listening: Vec<u16> = (e.ports.start..=e.ports.end)
                .filter(|&p| !prober.is_free(p))
                .collect();
            let task = e.task.as_str();
            let shown: Vec<String> = listening.iter().map(u16::to_string).collect();
            if !listening.is_empty() && (e.state != State::Ready || !e.path.exists()) {
                Item::new(
                    "port",
                    task,
                    Action::Drift,
                    format!(
                        "port {} is in use but the worktree is not ready; stop the process, \
                         then berth down {task}",
                        shown.join(",")
                    ),
                )
            } else if listening.is_empty() {
                Item::new("port", task, Action::Ok, "no listener")
            } else {
                Item::new(
                    "port",
                    task,
                    Action::Ok,
                    format!("listening {}", shown.join(",")),
                )
            }
        })
        .collect()
}

fn owner_items(manifest: &Manifest) -> Vec<Item> {
    let dead: Vec<Item> = manifest
        .worktrees
        .iter()
        .filter(|e| e.state == State::Creating && !sys::pid_alive(e.owner.pid, &e.owner.host))
        .map(|e| {
            Item::new(
                "owner",
                e.task.as_str(),
                Action::Drift,
                format!(
                    "interrupted: owner pid {} is gone; run berth down {}",
                    e.owner.pid, e.task
                ),
            )
        })
        .collect();
    if dead.is_empty() {
        return vec![Item::new(
            "owner",
            "creating",
            Action::Ok,
            "no interrupted creations",
        )];
    }
    dead
}
