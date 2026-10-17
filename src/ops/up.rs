//! `berth up` (spec §8): create the worktree of a task, or confirm the one that exists.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cli::UpArgs;
use crate::config::{self, Config};
use crate::copy;
use crate::env::{EnvInput, build_env, sh_quote};
use crate::git::{self, Repo};
use crate::hooks::run_hooks;
use crate::lock::{self, LockGuard};
use crate::manifest::{Entry, Manifest, Owner, PortBlock, SetupStatus, State};
use crate::ports::{self, BindProber};
use crate::registry::{self, Block, Registry};
use crate::report::{Action, Item, Report};
use crate::{BerthError, Ctx, sys};

/// Lines `up` keeps in `<git-common-dir>/info/exclude` (runtime state, never committed).
const EXCLUDE_ALWAYS: [&str; 2] = ["/.berth/", "/.agents/.*.tmp.*"];
const MANIFEST_REL: &str = ".agents/worktrees.json";

/// `[worktree].root` with `{repo}` replaced, relative to the main checkout, joined with `task`,
/// normalized lexically (no `.` or `..` left).
pub fn worktree_path(repo: &Repo, cfg: &Config, task: &str) -> PathBuf {
    let root = cfg.worktree.root.replace("{repo}", &repo.name);
    normalize(&repo.main.join(root).join(task))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub(super) fn repo_lock(repo: &Repo) -> Result<LockGuard, BerthError> {
    lock::acquire(&lock::repo_lock(&repo.common_dir), lock::lock_timeout())
}

pub(super) fn user_lock(state: &Path) -> Result<LockGuard, BerthError> {
    lock::acquire(&lock::user_lock(state), lock::lock_timeout())
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Creating => "creating",
        State::Ready => "ready",
        State::Failed => "failed",
        State::Removing => "removing",
    }
}

fn hint(path: &Path) -> String {
    format!(
        "cd {} && eval \"$(berth env)\"",
        sh_quote(&path.display().to_string())
    )
}

pub(super) fn ports_item(block: &PortBlock, action: Action) -> Item {
    let named: Vec<String> = block
        .named
        .iter()
        .map(|(name, port)| format!("{name}={port}"))
        .collect();
    let reason = if named.is_empty() {
        format!("block of {}", block.count())
    } else {
        named.join(" ")
    };
    Item::new(
        "ports",
        format!("{}-{}", block.start, block.end),
        action,
        reason,
    )
}

pub fn up(ctx: &Ctx, args: &UpArgs) -> Result<Report, BerthError> {
    let repo = &ctx.repo;
    let main = &repo.main;
    let task = args.task.as_str();

    // 1. Everything that can fail on input, before any lock, port or file.
    let cfg = config::load(main)?;
    let branch = match &args.branch {
        Some(name) => name.clone(),
        None => format!("{}{task}", cfg.worktree.branch_prefix),
    };
    git::check_branch_name(main, &branch)?;
    let base_sha = git::resolve_commit(main, args.base.as_deref().unwrap_or(&cfg.worktree.base))?;
    let path = worktree_path(repo, &cfg, task);
    let state = registry::state_dir()?;

    // 2. Repo lock.
    let repo_guard = repo_lock(repo)?;

    // 3. An existing entry is confirmed or reported; otherwise check before creating anything.
    let mut manifest = Manifest::load(main)?;
    if let Some(entry) = manifest.find(task) {
        return Ok(existing(entry, args, &cfg, &base_sha));
    }
    let mut refused = Vec::new();
    if std::fs::symlink_metadata(&path).is_ok() {
        refused.push(Item::new(
            "worktree",
            task,
            Action::Refused,
            format!(
                "path exists: {}; remove it or choose another task",
                path.display()
            ),
        ));
    }
    if manifest.worktrees.len() >= cfg.worktree.max as usize {
        refused.push(Item::new(
            "worktree",
            task,
            Action::Refused,
            format!(
                "[worktree].max = {} reached; `berth down` an unused task or raise max",
                cfg.worktree.max
            ),
        ));
    }
    let plan = copy::plan_copy(repo, &cfg.copy_source(main), cfg.max_bytes())?;
    refused.extend(plan.refused.iter().cloned());
    if !refused.is_empty() {
        let mut report = Report::new("up");
        report.items = refused;
        report.exit_code = 1;
        return Ok(report);
    }

    // 4. Ports, under the user lock (released inside).
    let start = reserve_ports(repo, &cfg, task, &manifest, &state)?;
    sys::test_pause("after_ports");

    // 5. Exclude lines, the `creating` entry, then the worktree; still under the repo lock.
    let block = PortBlock::new(start, cfg.ports.count, &cfg.ports.names());
    let created = create_worktree(
        repo,
        &cfg,
        args,
        &mut manifest,
        &path,
        &branch,
        &base_sha,
        block,
    );
    let Created {
        entry,
        branch_existed,
        root_created,
        mut items,
    } = match created {
        Ok(created) => created,
        Err(err) => {
            cleanup("release ports", unreserve(&state, main, task));
            cleanup("remove entry", remove_entry(main, task));
            return Err(err);
        }
    };
    drop(repo_guard);
    sys::test_pause("after_worktree");

    let job = Job {
        repo,
        cfg: &cfg,
        state: &state,
        task,
        path: &path,
        branch: &branch,
        base_sha: &base_sha,
        branch_existed,
        root_created,
        env: &entry.env,
    };

    // 6. Copy (unlocked).
    let copy_items = match copy::execute_copy(main, &path, &plan.files, cfg.max_bytes()) {
        Ok(items) => items,
        Err(err) => {
            job.fail(args.keep_on_failure, &[], setup_status("skipped", 0.0));
            return Err(err);
        }
    };
    let copied: Vec<String> = copy_items
        .iter()
        .filter(|i| i.action == Action::Create)
        .map(|i| i.key.clone())
        .collect();
    if copy_items.iter().any(|i| i.action == Action::Refused) {
        // Setup never ran.
        job.fail(args.keep_on_failure, &copied, setup_status("skipped", 0.0));
        let mut report = Report::new("up");
        report.items = copy_items;
        report.exit_code = 1;
        return Ok(report);
    }
    sys::test_pause("after_copy");

    // 7. Setup (unlocked).
    let setup = if args.no_setup {
        SetupStatus {
            status: "skipped".into(),
            secs: 0.0,
        }
    } else {
        let started = Instant::now();
        let timeout = Duration::from_secs(cfg.setup.timeout_secs);
        match run_hooks("setup", &cfg.setup.run, &path, &entry.env, timeout) {
            Ok(secs) => SetupStatus {
                status: "ok".into(),
                secs,
            },
            Err(err) => {
                job.fail(
                    args.keep_on_failure,
                    &copied,
                    setup_status("failed", started.elapsed().as_secs_f64()),
                );
                return Err(err);
            }
        }
    };

    // 8. Ready.
    let entry = {
        let _guard = repo_lock(repo)?;
        let mut manifest = Manifest::load(main)?;
        let mut done = manifest.find(task).cloned().unwrap_or(entry);
        done.state = State::Ready;
        done.copied = copied;
        done.setup = setup;
        manifest.upsert(done.clone());
        manifest.save(main)?;
        done
    };

    let mut report = Report::new("up");
    let mut all = vec![Item::new(
        "worktree",
        task,
        Action::Create,
        path.display().to_string(),
    )];
    all.push(items.remove(0));
    all.push(ports_item(&entry.ports, Action::Create));
    all.extend(copy_items);
    all.extend(plan.skipped);
    all.append(&mut items);
    let setup_reason = if args.no_setup {
        "skipped (--no-setup)".to_string()
    } else {
        entry.setup.status.clone()
    };
    let setup_action = if args.no_setup {
        Action::Skipped
    } else {
        Action::Create
    };
    all.push(Item::new("setup", task, setup_action, setup_reason));
    report.items = all;
    report.hint = Some(hint(&path));
    report.worktrees = vec![entry];
    Ok(report)
}

/// Step 3 for a task already in the manifest: refused, drift, or every item `ok`.
fn existing(entry: &Entry, args: &UpArgs, cfg: &Config, base_sha: &str) -> Report {
    let task = entry.task.as_str();
    let mut report = Report::new("up");
    report.worktrees = vec![entry.clone()];
    let owner_alive = sys::pid_alive(entry.owner.pid, &entry.owner.host);
    if entry.state == State::Creating && owner_alive {
        report.items.push(Item::new(
            "worktree",
            task,
            Action::Refused,
            format!("task is being created by pid {}", entry.owner.pid),
        ));
        report.exit_code = 1;
        return report;
    }
    let mut drift = Vec::new();
    if args.base.is_some() && base_sha != entry.base {
        drift.push(Item::new(
            "base",
            task,
            Action::Drift,
            format!(
                "task exists with base {}, not {base_sha}; run `berth down {task}` to change it",
                entry.base
            ),
        ));
    }
    if let Some(name) = args.branch.as_ref().filter(|b| **b != entry.branch) {
        drift.push(Item::new(
            "branch",
            task,
            Action::Drift,
            format!(
                "task exists on branch {}, not {name}; run `berth down {task}` to change it",
                entry.branch
            ),
        ));
    }
    if cfg.ports.count != entry.ports.count() {
        drift.push(Item::new(
            "ports",
            task,
            Action::Drift,
            format!(
                "task has {} ports, [ports].count is {}; run `berth down {task}` to change it",
                entry.ports.count(),
                cfg.ports.count
            ),
        ));
    }
    let state_drift = match entry.state {
        State::Creating => Some(format!(
            "interrupted: owner pid {} is gone; run `berth down {task}` first",
            entry.owner.pid
        )),
        State::Failed | State::Removing => Some(format!(
            "state {}; run `berth down {task}` first",
            state_name(entry.state)
        )),
        State::Ready if !entry.path.exists() => Some(format!(
            "missing: worktree directory deleted; run `berth down {task}`"
        )),
        State::Ready => None,
    };
    if let Some(reason) = state_drift {
        drift.push(Item::new("worktree", task, Action::Drift, reason));
    }
    if !drift.is_empty() {
        report.items = drift;
        report.exit_code = 1;
        return report;
    }
    report.items.push(Item::new(
        "worktree",
        task,
        Action::Ok,
        entry.path.display().to_string(),
    ));
    report
        .items
        .push(Item::new("branch", &entry.branch, Action::Ok, "exists"));
    report.items.push(ports_item(&entry.ports, Action::Ok));
    for path in &entry.copied {
        report
            .items
            .push(Item::new("copy", path, Action::Ok, "copied"));
    }
    report.items.push(Item::new(
        "setup",
        task,
        Action::Ok,
        entry.setup.status.clone(),
    ));
    report.hint = Some(hint(&entry.path));
    report
}

/// Step 4: allocate and record a block. Caller holds the repo lock; this takes the user lock and
/// never blocks on another repo's lock (staleness uses the read-only `is_held` probe).
fn reserve_ports(
    repo: &Repo,
    cfg: &Config,
    task: &str,
    manifest: &Manifest,
    state: &Path,
) -> Result<u16, BerthError> {
    let main = &repo.main;
    let _guard = user_lock(state)?;
    let mut reg = Registry::load(state)?;
    // (a) Our blocks without a manifest entry are provably stale: we hold our repo lock.
    reg.remove_where(|b| b.repo == *main && manifest.find(&b.task).is_none());
    // (b) Everything else is taken, except foreign blocks that are stale.
    let mut taken = Vec::new();
    let mut stale = Vec::new();
    for b in &reg.blocks {
        if b.repo != *main && registry::is_stale(b) {
            stale.push(b.clone());
        } else {
            taken.push((b.start, b.end));
        }
    }
    // (c) Allocate; the bind probe has the last word.
    let count = cfg.ports.count;
    let range = (cfg.ports.range[0], cfg.ports.range[1]);
    let start = ports::allocate(main, task, range, count, &taken, &BindProber)
        .ok_or(BerthError::NoPorts { range, count })?;
    let end = start + (count - 1);
    // (d) Reclaim only the stale blocks our block overlaps; the others stay.
    let reclaim: Vec<Block> = stale
        .into_iter()
        .filter(|b| b.start <= end && start <= b.end)
        .collect();
    reg.remove_where(|b| reclaim.contains(b));
    // (e) Record ours.
    reg.add(Block {
        start,
        end,
        repo: main.clone(),
        task: task.to_string(),
        created_at: sys::now_rfc3339(),
    });
    reg.save(state)?;
    Ok(start)
}

fn setup_status(status: &str, secs: f64) -> SetupStatus {
    SetupStatus {
        status: status.into(),
        secs,
    }
}

/// What step 5 created: the `creating` entry, whether the branch existed, whether berth made
/// the worktree root directory, and the items (the branch item first, then any `exclude` items).
struct Created {
    entry: Entry,
    branch_existed: bool,
    root_created: bool,
    items: Vec<Item>,
}

/// Step 5 under the repo lock.
#[allow(clippy::too_many_arguments)]
fn create_worktree(
    repo: &Repo,
    cfg: &Config,
    args: &UpArgs,
    manifest: &mut Manifest,
    path: &Path,
    branch: &str,
    base_sha: &str,
    ports: PortBlock,
) -> Result<Created, BerthError> {
    let main = &repo.main;
    let task = args.task.as_str();
    let mut excludes = Vec::new();
    let mut lines: Vec<&str> = EXCLUDE_ALWAYS.to_vec();
    if git::ignored(main, &[MANIFEST_REL.to_string()])?.is_empty() {
        lines.push("/.agents/worktrees.json");
    }
    for line in lines {
        if git::ensure_excluded(&repo.common_dir, line)? {
            excludes.push(Item::new(
                "exclude",
                line,
                Action::Create,
                "added to info/exclude",
            ));
        }
    }

    let existed = git::branch_exists(main, branch)?;
    let (base, branch_item) = if existed {
        let base = git::merge_base(main, branch, base_sha)?;
        let item = if args.base.is_some() {
            eprintln!("warning: branch {branch} exists; --base ignored");
            Item::new(
                "branch",
                branch,
                Action::Skipped,
                "branch exists; --base ignored",
            )
        } else {
            Item::new("branch", branch, Action::Ok, "exists; checked out")
        };
        (base, item)
    } else {
        let item = Item::new("branch", branch, Action::Create, format!("from {base_sha}"));
        (base_sha.to_string(), item)
    };

    let env = build_env(&EnvInput {
        repo_name: &repo.name,
        main,
        worktree: path,
        task,
        ports: &ports,
        extra: &cfg.env,
    })?;
    let entry = Entry {
        task: task.to_string(),
        branch: branch.to_string(),
        base,
        path: path.to_path_buf(),
        state: State::Creating,
        ports,
        env,
        copied: Vec::new(),
        setup: SetupStatus {
            status: "running".into(),
            secs: 0.0,
        },
        created_at: sys::now_rfc3339(),
        owner: Owner {
            pid: std::process::id(),
            host: sys::hostname(),
        },
    };
    manifest.upsert(entry.clone());
    manifest.save(main)?;
    let new_from = if existed { None } else { Some(base_sha) };
    // Recorded before `worktree add` creates it: only a root berth made is berth's to remove.
    let root_created = path.parent().is_some_and(|root| !root.exists());
    if let Err(err) = git::worktree_add(main, path, branch, new_from) {
        if root_created && let Some(root) = path.parent() {
            let _ = std::fs::remove_dir(root);
        }
        return Err(err);
    }
    let mut items = vec![branch_item];
    items.append(&mut excludes);
    Ok(Created {
        entry,
        branch_existed: existed,
        root_created,
        items,
    })
}

/// Report a failed cleanup step on stderr; the original error is what the caller returns.
fn cleanup(what: &str, result: Result<(), BerthError>) {
    if let Err(err) = result {
        eprintln!("warning: rollback could not {what}: {err}");
    }
}

/// Free every block of `(main, task)` under the user lock.
fn unreserve(state: &Path, main: &Path, task: &str) -> Result<(), BerthError> {
    let _guard = user_lock(state)?;
    let mut reg = Registry::load(state)?;
    if reg.release(main, task) > 0 {
        reg.save(state)?;
    }
    Ok(())
}

/// Drop the task's manifest entry. The caller holds the repo lock.
fn remove_entry(main: &Path, task: &str) -> Result<(), BerthError> {
    let mut manifest = Manifest::load(main)?;
    if manifest.remove(task).is_some() {
        manifest.save(main)?;
    }
    Ok(())
}

/// What a failure after step 5 needs to undo or keep.
struct Job<'a> {
    repo: &'a Repo,
    cfg: &'a Config,
    state: &'a Path,
    task: &'a str,
    path: &'a Path,
    branch: &'a str,
    base_sha: &'a str,
    branch_existed: bool,
    root_created: bool,
    env: &'a BTreeMap<String, String>,
}

impl Job<'_> {
    /// Roll back, or with `--keep-on-failure` mark the entry `failed` and keep everything.
    /// Cleanup errors are reported on stderr; the caller returns the original failure.
    fn fail(&self, keep: bool, copied: &[String], setup: SetupStatus) {
        if keep {
            cleanup("mark the entry failed", self.mark_failed(copied, setup));
        } else {
            self.rollback();
        }
    }

    fn mark_failed(&self, copied: &[String], setup: SetupStatus) -> Result<(), BerthError> {
        let main = &self.repo.main;
        let _guard = repo_lock(self.repo)?;
        let mut manifest = Manifest::load(main)?;
        if let Some(entry) = manifest.worktrees.iter_mut().find(|e| e.task == self.task) {
            entry.state = State::Failed;
            entry.copied = copied.to_vec();
            entry.setup = setup;
        }
        manifest.save(main)
    }

    /// Teardown (errors ignored), remove the worktree, delete the branch if berth made it and it
    /// has no commits, free the ports, drop the entry. One lock at a time.
    fn rollback(&self) {
        let main = &self.repo.main;
        if !self.cfg.teardown.run.is_empty() && self.path.is_dir() {
            let timeout = Duration::from_secs(self.cfg.teardown.timeout_secs);
            let _ = run_hooks(
                "teardown",
                &self.cfg.teardown.run,
                self.path,
                self.env,
                timeout,
            );
        }
        cleanup(
            "remove the worktree",
            git::worktree_remove(main, self.path, true),
        );
        cleanup("prune worktrees", git::worktree_prune(main));
        if !self.branch_existed
            && matches!(git::commits_since(main, self.base_sha, self.branch), Ok(0))
        {
            cleanup("delete the branch", git::delete_branch(main, self.branch));
        }
        if self.root_created
            && let Some(root) = self.path.parent()
        {
            // Only succeeds when empty.
            let _ = std::fs::remove_dir(root);
        }
        cleanup("release ports", unreserve(self.state, main, self.task));
        cleanup(
            "remove the entry",
            repo_lock(self.repo).and_then(|_guard| remove_entry(main, self.task)),
        );
    }
}
