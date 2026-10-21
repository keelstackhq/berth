//! `berth doctor` (spec §11): read-only checks, one or more items each. Takes no lock and
//! creates no file; registry staleness goes through `registry::is_stale` (a `lock::is_held`
//! probe, never `acquire`). Exit 1 on any `drift`, 3 on errors.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::cache;
use crate::config::{Config, Provider, Remote, Seed};
use crate::copy::plan_copy;
use crate::db;
use crate::manifest::{Manifest, State, manifest_path};
use crate::ops::up::worktree_path;
use crate::ports::{BindProber, Prober};
use crate::registry::{self, Registry};
use crate::report::{Action, Item, Report};
use crate::{BerthError, Ctx, config, git, sys};

/// What `up` keeps out of git, checked as paths below the entry (a directory pattern matches).
const EXCLUDED: [&str; 2] = [".agents/worktrees.json", ".berth/x"];

/// A `creating` entry whose owner is alive on this host is an `up` in progress, not drift.
/// An owner on another host cannot be verified, so it is never exempt.
fn in_progress(e: &crate::manifest::Entry) -> bool {
    e.state == State::Creating
        && e.owner.host == sys::hostname()
        && sys::pid_alive(e.owner.pid, &e.owner.host)
}

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

    items.extend(service_items(main, &cfg)?);

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
        items.extend(db_entry_items(&manifest));
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
        let creating = in_progress(e);
        if creating && !e.path.exists() {
            out.push(Item::new("manifest", task, Action::Ok, "creating"));
        } else if !e.path.exists() {
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
            if !listening.is_empty()
                && !in_progress(e)
                && (e.state != State::Ready || !e.path.exists())
            {
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
    let here = sys::hostname();
    let bad: Vec<Item> = manifest
        .worktrees
        .iter()
        .filter(|e| e.state == State::Creating)
        .filter_map(|e| {
            let task = e.task.as_str();
            if e.owner.host != here {
                Some(Item::new(
                    "owner",
                    task,
                    Action::Drift,
                    format!(
                        "owner on another host ({}); cannot verify; run berth down {task} there, \
                         or berth down --force {task} if that up is gone",
                        e.owner.host
                    ),
                ))
            } else if !sys::pid_alive(e.owner.pid, &e.owner.host) {
                Some(Item::new(
                    "owner",
                    task,
                    Action::Drift,
                    format!(
                        "interrupted: owner pid {} is gone; run berth down {task}",
                        e.owner.pid
                    ),
                ))
            } else {
                None
            }
        })
        .collect();
    if bad.is_empty() {
        return vec![Item::new(
            "owner",
            "creating",
            Action::Ok,
            "no interrupted creations",
        )];
    }
    bad
}

/// `[db]` and `[cache]` checks. Reads config and PATH only: no database connection, no
/// `createdb`/`docker`, no file written. Problems are `drift`, never an error exit.
fn service_items(main: &Path, cfg: &Config) -> Result<Vec<Item>, BerthError> {
    let mut out = Vec::new();
    if let Some(db_cfg) = &cfg.db {
        if db_cfg.provider != Provider::Sqlite {
            out.push(match db::check_tools(db_cfg) {
                Ok(()) => Item::new("db", "tools", Action::Ok, "found on PATH"),
                Err(BerthError::Env(why)) => Item::new("db", "tools", Action::Drift, why),
                Err(other) => return Err(other),
            });
        } else {
            out.push(match db::check_sqlite(main, db_cfg) {
                Ok(None) => Item::new("db", "sqlite", Action::Ok, "template and target are fine"),
                Ok(Some(refused)) => Item::new("db", "sqlite", Action::Drift, refused.reason),
                Err(BerthError::Env(why)) => Item::new("db", "sqlite", Action::Drift, why),
                Err(other) => return Err(other),
            });
        }
    }
    if cfg.cache.turbo_seed != Seed::Off {
        let ignored = !git::ignored(main, &[".turbo/cache/x".to_string()])?.is_empty();
        out.push(if ignored {
            Item::new("cache", "turbo", Action::Ok, ".turbo/ is ignored by git")
        } else {
            Item::new(
                "cache",
                "turbo",
                Action::Drift,
                ".turbo/ is not gitignored, so a seeded cache would make worktrees dirty and \
                 `berth up` skips seeding; add .turbo/ to .gitignore",
            )
        });
    }
    if cfg.cache.remote == Remote::Hoard {
        out.push(match cache::hoard_quiet(main, &cfg.cache) {
            Some(h) => Item::new("cache", "hoard", Action::Ok, h.api),
            None => Item::new("cache", "hoard", Action::Skipped, "no running hoard server"),
        });
    }
    Ok(out)
}

/// A ready postgres entry whose database berth did not create: `down` will leave it.
fn db_entry_items(manifest: &Manifest) -> Vec<Item> {
    manifest
        .worktrees
        .iter()
        .filter(|e| e.state == State::Ready)
        .filter_map(|e| e.db.as_ref().map(|d| (e, d)))
        .filter(|(_, d)| d.provider == "postgres" && !d.created)
        .map(|(_, d)| {
            Item::new(
                "db",
                &d.name,
                Action::Drift,
                format!(
                    "database {} was not recorded as created; berth down will leave it",
                    d.name
                ),
            )
        })
        .collect()
}
