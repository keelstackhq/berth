//! `berth up` (spec §8): create the worktree of a task, or confirm the one that exists.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cache;
use crate::cli::UpArgs;
use crate::config::{self, Config, Provider, Seed};
use crate::copy;
use crate::db::{self, DbPlan};
use crate::env::{EnvInput, build_env, sh_quote};
use crate::git::{self, Repo};
use crate::hooks::run_hooks;
use crate::lock::{self, LockGuard};
use crate::manifest::{DbEntry, Entry, Manifest, Owner, PortBlock, SetupStatus, State};
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

pub(crate) fn normalize(path: &Path) -> PathBuf {
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

pub(super) fn state_name(state: State) -> &'static str {
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
    if git::is_tracked(main, MANIFEST_REL)? {
        return Err(BerthError::Env(format!(
            "{MANIFEST_REL} is tracked by git; berth never edits tracked files: run \
             git rm --cached {MANIFEST_REL} and commit"
        )));
    }
    let path = worktree_path(repo, &cfg, task);
    let mut sqlite_refusal = None;
    if let Some(dbcfg) = &cfg.db {
        // Validation only: ports are digits, so the real block cannot change a name's validity.
        let stand_in = PortBlock::new(cfg.ports.range[0], cfg.ports.count, &cfg.ports.names());
        db::resolve(dbcfg, &repo.name, main, task, &path, &stand_in)?;
        // Tools and the sqlite template matter only when `up` will create the database; an
        // existing task must stay confirmable (lock-free peek, re-checked under the lock).
        let known = Manifest::load(main)?.find(task).is_some();
        if !known {
            db::check_tools(dbcfg)?;
        }
        if !known && dbcfg.provider == Provider::Sqlite {
            sqlite_refusal = db::check_sqlite(main, dbcfg)?;
        }
    }
    let state = registry::state_dir()?;

    // 2. Repo lock.
    let repo_guard = repo_lock(repo)?;

    // 3. An existing entry is confirmed or reported; otherwise check before creating anything.
    let mut manifest = Manifest::load(main)?;
    if let Some(entry) = manifest.find(task) {
        return existing(entry, args, &cfg, repo, &base_sha);
    }
    let mut refused = Vec::new();
    refused.extend(sqlite_refusal);
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
        db: db_plan,
        mut items,
    } = match created {
        Ok(created) => created,
        Err(err) => {
            cleanup("release ports", unreserve(&state, main, task).map(drop));
            cleanup("remove entry", remove_entry(main, task));
            return Err(err);
        }
    };
    drop(repo_guard);
    sys::test_pause("after_worktree");

    // Hooks (setup and rollback teardown) get the hoard variables; nothing else does.
    let hook_env = cache::hook_env(&entry.env, cache::hoard(main, &cfg.cache).as_ref());
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
        hook_env: &hook_env,
        db: entry.db.clone(),
        db_created: Cell::new(false),
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

    // 6a. Turbo cache seed (unlocked); never fails `up`.
    let mut cache_items = Vec::new();
    if cfg.cache.turbo_seed != Seed::Off {
        cache_items.push(cache::seed_turbo(main, &path, cfg.cache.turbo_seed));
    }

    // 6b. Database (unlocked), after the seed.
    let mut db_items = Vec::new();
    if let (Some(dbcfg), Some(plan)) = (&cfg.db, &db_plan) {
        // compose: `up` may start containers before it fails, so an attempt is enough to undo.
        if plan.provider == Provider::Compose {
            job.db_created.set(true);
        }
        let item = match db::create(main, &path, dbcfg, plan, &entry.env) {
            Ok(item) => item,
            Err(err) => {
                if plan.provider == Provider::Postgres {
                    eprintln!(
                        "hint: if database {0} is a leftover, drop it with dropdb {0}, or set [db].name",
                        plan.name
                    );
                }
                job.fail(args.keep_on_failure, &copied, setup_status("skipped", 0.0));
                return Err(err);
            }
        };
        if item.action == Action::Refused {
            job.fail(args.keep_on_failure, &copied, setup_status("skipped", 0.0));
            let mut report = Report::new("up");
            report.items = copy_items;
            report.items.push(item);
            report.exit_code = 1;
            return Ok(report);
        }
        if plan.provider != Provider::Sqlite {
            job.db_created.set(true);
            sys::test_pause("after_db_create");
            if let Err(err) = mark_db_created(repo, task) {
                // The entry cannot record the database, so `down` would never drop it: drop it
                // now even with --keep-on-failure.
                if args.keep_on_failure {
                    job.drop_db();
                }
                job.fail(args.keep_on_failure, &copied, setup_status("skipped", 0.0));
                return Err(err);
            }
        }
        db_items.push(item);
    }

    // 7. Setup (unlocked).
    let setup = if args.no_setup {
        SetupStatus {
            status: "skipped".into(),
            secs: 0.0,
        }
    } else {
        let started = Instant::now();
        let timeout = Duration::from_secs(cfg.setup.timeout_secs);
        match run_hooks("setup", &cfg.setup.run, &path, &hook_env, timeout) {
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
        let _guard = repo_lock(repo).map_err(|err| {
            BerthError::Env(format!(
                "{err}; setup finished but {task} was not marked ready: run berth down {task}, \
                 then berth up {task}"
            ))
        })?;
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
    all.append(&mut cache_items);
    all.append(&mut db_items);
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
fn existing(
    entry: &Entry,
    args: &UpArgs,
    cfg: &Config,
    repo: &Repo,
    base_sha: &str,
) -> Result<Report, BerthError> {
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
        return Ok(report);
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
    let have = entry
        .db
        .as_ref()
        .map(|d| format!("{} {}", d.provider, d.name));
    let want = match &cfg.db {
        Some(d) => {
            let plan = db::resolve(d, &repo.name, &repo.main, task, &entry.path, &entry.ports)?;
            Some(format!("{} {}", plan.provider.as_str(), plan.name))
        }
        None => None,
    };
    if have != want {
        let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".into());
        drift.push(Item::new(
            "db",
            task,
            Action::Drift,
            format!(
                "task has database {}; [db] now gives {}; run `berth down {task}` to change it",
                show(&have),
                show(&want)
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
        return Ok(report);
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
    if let Some(d) = &entry.db {
        report
            .items
            .push(Item::new("db", &d.name, Action::Ok, d.provider.clone()));
    }
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
    Ok(report)
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
    db: Option<DbPlan>,
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

    let db_plan = cfg
        .db
        .as_ref()
        .map(|d| db::resolve(d, &repo.name, main, task, path, &ports))
        .transpose()?;
    let env = build_env(&EnvInput {
        repo_name: &repo.name,
        main,
        worktree: path,
        task,
        ports: &ports,
        extra: &cfg.env,
        db: db_plan.as_ref(),
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
        db: cfg
            .db
            .as_ref()
            .zip(db_plan.as_ref())
            .map(|(c, p)| db::entry(p, c)),
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
        db: db_plan,
        items,
    })
}

/// Report a failed cleanup step on stderr; the original error is what the caller returns.
fn cleanup(what: &str, result: Result<(), BerthError>) {
    if let Err(err) = result {
        eprintln!("warning: rollback could not {what}: {err}");
    }
}

/// Free every block of `(main, task)` under the user lock; returns how many went.
pub(super) fn unreserve(state: &Path, main: &Path, task: &str) -> Result<usize, BerthError> {
    let _guard = user_lock(state)?;
    let mut reg = Registry::load(state)?;
    let freed = reg.release(main, task);
    if freed > 0 {
        reg.save(state)?;
    }
    Ok(freed)
}

/// Drop the task's manifest entry. The caller holds the repo lock.
fn remove_entry(main: &Path, task: &str) -> Result<(), BerthError> {
    let mut manifest = Manifest::load(main)?;
    if manifest.remove(task).is_some() {
        manifest.save(main)?;
    }
    Ok(())
}

/// Record `db.created = true` once the database exists (short repo lock).
fn mark_db_created(repo: &Repo, task: &str) -> Result<(), BerthError> {
    let _guard = repo_lock(repo)?;
    let mut manifest = Manifest::load(&repo.main)?;
    if let Some(db) = manifest
        .worktrees
        .iter_mut()
        .find(|e| e.task == task)
        .and_then(|e| e.db.as_mut())
    {
        db.created = true;
    }
    manifest.save(&repo.main)
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
    /// `env` plus the hoard variables, for hooks only.
    hook_env: &'a BTreeMap<String, String>,
    db: Option<DbEntry>,
    /// This run created the database (postgres) or started the compose project.
    db_created: Cell<bool>,
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

    /// Drop the database (or stop the compose project) this run created; errors are warnings.
    fn drop_db(&self) {
        let main = &self.repo.main;
        if let Some(db) = self.db.as_ref().filter(|_| self.db_created.get()) {
            let mut record = db.clone();
            record.created = true;
            let cwd = if self.path.is_dir() { self.path } else { main };
            let timeout = self
                .cfg
                .db
                .as_ref()
                .map_or(Duration::from_secs(300), |d| d.timeout());
            // `--force`: berth made this database moments ago; stray sessions must not leak it.
            cleanup(
                "drop the database",
                db::drop(main, cwd, &record, true, self.env, timeout).map(drop),
            );
        }
    }

    /// Teardown (errors ignored), drop the database this run created (errors are warnings),
    /// remove the worktree, delete the branch if berth made it and it
    /// has no commits, free the ports, drop the entry. One lock at a time.
    fn rollback(&self) {
        let main = &self.repo.main;
        if !self.cfg.teardown.run.is_empty() && self.path.is_dir() {
            let timeout = Duration::from_secs(self.cfg.teardown.timeout_secs);
            let _ = run_hooks(
                "teardown",
                &self.cfg.teardown.run,
                self.path,
                self.hook_env,
                timeout,
            );
        }
        self.drop_db();
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
        cleanup(
            "release ports",
            unreserve(self.state, main, self.task).map(drop),
        );
        cleanup(
            "remove the entry",
            repo_lock(self.repo).and_then(|_guard| remove_entry(main, self.task)),
        );
    }
}
