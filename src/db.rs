//! The task database (spec §4, §7): the pure plan (`resolve`, `entry`) and the provider I/O
//! (`check_tools`, `check_sqlite`, `create`, `drop`). External tools run through
//! `hooks::run_hooks` with every argument `sh_quote`d; berth never handles a password.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{DbCfg, Provider};
use crate::copy::{copy_one, safe_relative, symlink_in_parents};
use crate::env::{expand_with, sh_quote};
use crate::error::BerthError;
use crate::git;
use crate::hooks::run_hooks;
use crate::manifest::{DbEntry, PortBlock};
use crate::report::{Action, Item};
use crate::vlog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbPlan {
    pub provider: Provider,
    /// Postgres database, compose project, or sqlite path relative to the worktree.
    pub name: String,
    pub url: Option<String>,
    /// The variable the URL is exported as; `None` when `[db].env = ""`.
    pub env_var: Option<String>,
    /// sqlite: absolute path of the database file in the worktree.
    pub abs_path: Option<PathBuf>,
}

fn cfg_err(message: String) -> BerthError {
    BerthError::Config {
        file: "berth.toml".into(),
        message,
    }
}

fn sanitize(repo: &str, keep_dash: bool) -> String {
    repo.to_lowercase()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '_' => c,
            '-' if keep_dash => c,
            _ => {
                if keep_dash {
                    '-'
                } else {
                    '_'
                }
            }
        })
        .collect()
}

/// First 6 hex chars of the FNV-1a hash of the main checkout path: tells apart two clones whose
/// directories share a name.
fn path_hash(main: &Path) -> String {
    let h = crate::ports::fnv1a64(main.as_os_str().as_encoded_bytes());
    format!("{h:016x}")[..6].to_string()
}

/// `<repo>_<h>_<task_snake>`: repo lowercased with every char outside `[a-z0-9_]` turned into `_`
/// and cut so the whole name fits 63 bytes (`db` when nothing is left); `<h>` hashes `main`.
pub fn default_pg_name(repo: &str, main: &Path, task: &str) -> String {
    let task = task.replace('-', "_");
    let room = 63usize.saturating_sub(task.len() + 8);
    let mut repo = sanitize(repo, false);
    repo.truncate(room);
    if repo.is_empty() {
        repo = "db".to_string();
    }
    format!("{repo}_{}_{task}", path_hash(main))
}

/// `<repo>-<h>-<task>`: repo lowercased, chars outside `[a-z0-9_-]` turned into `-`, leading
/// `-`/`_` trimmed, cut to 30 (`db` when nothing is left); `<h>` hashes `main`.
pub fn default_project(repo: &str, main: &Path, task: &str) -> String {
    let repo = sanitize(repo, true);
    let repo: String = repo
        .trim_start_matches(['-', '_'])
        .chars()
        .take(30)
        .collect();
    let repo = if repo.is_empty() { "db" } else { &repo };
    format!("{repo}-{}-{task}", path_hash(main))
}

fn valid_pg_name(n: &str) -> bool {
    (1..=63).contains(&n.len())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_project(n: &str) -> bool {
    let mut bytes = n.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Turn `[db]` plus task, repo and ports into the plan. Errors are `Config` (exit 3) and name the
/// key to set.
pub fn resolve(
    cfg: &DbCfg,
    repo_name: &str,
    main: &Path,
    task: &str,
    worktree: &Path,
    ports: &PortBlock,
) -> Result<DbPlan, BerthError> {
    let expand = |key: &str, template: &str, extra: &BTreeMap<String, String>| {
        expand_with(template, task, repo_name, worktree, ports, extra).map_err(|tok| {
            let what = if tok.starts_with('{') && tok.ends_with('}') && tok.len() > 1 {
                format!("unknown placeholder {tok}")
            } else {
                format!("unbalanced brace {tok}")
            };
            cfg_err(format!("{what} in [db].{key}"))
        })
    };
    let (name, abs_path, default_url) = match cfg.provider {
        Provider::Postgres => {
            let name = match &cfg.name {
                Some(t) => expand("name", t, &BTreeMap::new())?,
                None => default_pg_name(repo_name, main, task),
            };
            if !valid_pg_name(&name) {
                return Err(cfg_err(format!(
                    "database name {name:?} must match [a-z0-9_]{{1,63}}; set [db].name"
                )));
            }
            let user = cfg
                .user
                .as_deref()
                .map(|u| format!("{u}@"))
                .unwrap_or_default();
            let url = format!(
                "postgres://{user}{}:{}/{name}",
                cfg.pg_host(),
                cfg.pg_port()
            );
            (name, None, Some(url))
        }
        Provider::Sqlite => {
            let rel = cfg.sqlite_path().ok_or_else(|| {
                cfg_err("[db].template is required for provider = \"sqlite\"".into())
            })?;
            // Native separators in the path; `/` in the URL, which SQLite takes on every OS.
            let abs = worktree.join(crate::sys::native(rel));
            let shown = abs.display().to_string();
            let url = if cfg!(windows) {
                format!("file:{}", shown.replace('\\', "/"))
            } else {
                format!("file:{shown}")
            };
            (rel.to_string(), Some(abs), Some(url))
        }
        Provider::Compose => {
            let name = match &cfg.project {
                Some(t) => expand("project", t, &BTreeMap::new())?,
                None => default_project(repo_name, main, task),
            };
            if !valid_project(&name) {
                return Err(cfg_err(format!(
                    "compose project {name:?} must match [a-z0-9][a-z0-9_-]*; set [db].project"
                )));
            }
            (name, None, None)
        }
    };
    let url = match &cfg.url {
        Some(template) => {
            let mut extra = BTreeMap::new();
            match &abs_path {
                Some(p) => extra.insert("db.path".to_string(), p.display().to_string()),
                None => extra.insert("db.name".to_string(), name.clone()),
            };
            Some(expand("url", template, &extra)?)
        }
        None => default_url,
    };
    Ok(DbPlan {
        provider: cfg.provider,
        name,
        url,
        env_var: (!cfg.env.is_empty()).then(|| cfg.env.clone()),
        abs_path,
    })
}

/// The manifest record for a plan (`created: false`; postgres keeps its connection settings).
pub fn entry(plan: &DbPlan, cfg: &DbCfg) -> DbEntry {
    let pg = plan.provider == Provider::Postgres;
    DbEntry {
        provider: plan.provider.as_str().to_string(),
        name: plan.name.clone(),
        created: false,
        host: pg.then(|| cfg.pg_host().to_string()),
        port: pg.then(|| cfg.pg_port()),
        user: if pg { cfg.user.clone() } else { None },
    }
}

fn io_err(path: &Path, source: std::io::Error) -> BerthError {
    BerthError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Unix: a regular file with an exec bit. Windows has no exec bits: any regular file counts
/// (`PATH` entries there are `.exe`/`.cmd` by construction).
fn is_executable_file(p: &Path) -> bool {
    #[cfg(unix)]
    {
        fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        fs::metadata(p).is_ok_and(|m| m.is_file())
    }
}

/// Whether `dir` holds `tool`; on Windows also as `tool.exe`, `.cmd` or `.bat`.
fn tool_in(dir: &Path, tool: &str) -> bool {
    if is_executable_file(&dir.join(tool)) {
        return true;
    }
    cfg!(windows)
        && ["exe", "cmd", "bat"]
            .iter()
            .any(|ext| is_executable_file(&dir.join(format!("{tool}.{ext}"))))
}

/// The external tools the provider needs, searched in the process `PATH`.
pub fn check_tools(cfg: &DbCfg) -> Result<(), BerthError> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    check_tools_in(cfg, &path)
}

/// `check_tools` against an explicit `PATH` value (tests cannot change the process `PATH`).
pub fn check_tools_in(cfg: &DbCfg, path: &OsStr) -> Result<(), BerthError> {
    let (tools, hint): (&[&str], &str) = match cfg.provider {
        Provider::Postgres => (
            &["createdb", "dropdb"],
            "install the PostgreSQL client tools or remove [db]",
        ),
        Provider::Compose => (&["docker"], "install Docker or remove [db]"),
        Provider::Sqlite => return Ok(()),
    };
    for tool in tools {
        let found = std::env::split_paths(path).any(|dir| tool_in(&dir, tool));
        if !found {
            return Err(BerthError::Env(format!("{tool} not found on PATH; {hint}")));
        }
    }
    Ok(())
}

fn template_of(cfg: &DbCfg) -> Result<&str, BerthError> {
    cfg.template.as_deref().ok_or_else(|| {
        BerthError::Env("[db].template is required for provider = \"sqlite\"".into())
    })
}

/// sqlite preflight in the main checkout: a regular template without pending WAL, and a target
/// path git ignores (else `Some(refused)`).
pub fn check_sqlite(main: &Path, cfg: &DbCfg) -> Result<Option<Item>, BerthError> {
    let template = template_of(cfg)?;
    let source = main.join(template);
    let bad = |why: &str| BerthError::Env(format!("[db].template {template} {why}"));
    match fs::symlink_metadata(&source) {
        Ok(m) if m.file_type().is_symlink() => return Err(bad("is a symlink; name the real file")),
        Ok(m) if !m.is_file() => return Err(bad("is not a regular file")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(bad(
                "does not exist in the main checkout; create it or fix [db].template",
            ));
        }
        Err(e) => return Err(io_err(&source, e)),
    }
    let wal = main.join(format!("{template}-wal"));
    match fs::symlink_metadata(&wal) {
        Ok(m) if m.len() > 0 => {
            return Err(BerthError::Env(format!(
                "{template} has a non-empty -wal file; checkpoint it \
                 (sqlite3 {template} 'PRAGMA wal_checkpoint(TRUNCATE)') or stop the app"
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&wal, e)),
    }
    let target = cfg.sqlite_path().unwrap_or(template).to_string();
    if !git::ignored(main, std::slice::from_ref(&target))?.contains(&target) {
        return Ok(Some(Item::new(
            "db",
            &target,
            Action::Refused,
            format!("add {target} to .gitignore: the agent would commit the database"),
        )));
    }
    Ok(None)
}

fn run_db(
    cmd: String,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<(), BerthError> {
    run_hooks("db", &[cmd], cwd, env, timeout).map(|_| ())
}

fn quoted(args: &[&str]) -> String {
    args.iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn pg_conn_args(host: &str, port: u16, user: Option<&str>) -> Vec<String> {
    let mut a = vec![
        "--no-password".to_string(),
        "--host".into(),
        host.into(),
        "--port".into(),
        port.to_string(),
    ];
    if let Some(u) = user {
        a.push("--username".into());
        a.push(u.into());
    }
    a
}

fn join_quoted(args: &[String]) -> String {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    quoted(&refs)
}

/// Create the task's database. The caller records `created` in the manifest afterwards.
pub fn create(
    main: &Path,
    wt: &Path,
    cfg: &DbCfg,
    plan: &DbPlan,
    env: &BTreeMap<String, String>,
) -> Result<Item, BerthError> {
    match plan.provider {
        Provider::Sqlite => create_sqlite(main, wt, cfg, plan),
        Provider::Postgres => {
            let mut args = vec!["createdb".to_string()];
            args.extend(pg_conn_args(
                cfg.pg_host(),
                cfg.pg_port(),
                cfg.user.as_deref(),
            ));
            if let Some(t) = &cfg.template {
                args.push("--template".into());
                args.push(t.clone());
            }
            args.push("--".into());
            args.push(plan.name.clone());
            run_db(join_quoted(&args), wt, env, cfg.timeout())?;
            let reason = match &cfg.template {
                Some(t) => format!("created from template {t}"),
                None => "created (empty)".to_string(),
            };
            Ok(Item::new("db", &plan.name, Action::Create, reason))
        }
        Provider::Compose => {
            let mut args = vec![
                "docker".to_string(),
                "compose".into(),
                "--project-name".into(),
                plan.name.clone(),
                "up".into(),
                "--detach".into(),
                "--wait".into(),
            ];
            args.extend(cfg.services.iter().flatten().cloned());
            run_db(join_quoted(&args), wt, env, cfg.timeout())?;
            Ok(Item::new("db", &plan.name, Action::Create, "compose up"))
        }
    }
}

fn create_sqlite(main: &Path, wt: &Path, cfg: &DbCfg, plan: &DbPlan) -> Result<Item, BerthError> {
    let template = template_of(cfg)?;
    let rel = plan.name.as_str();
    let refused = |why: &str| Ok(Item::new("db", rel, Action::Refused, why));
    if !safe_relative(rel) || !safe_relative(template) {
        return refused("unsafe path");
    }
    let source = main.join(template);
    let target = wt.join(rel);
    if symlink_in_parents(wt, ".berth/tmp/x")? {
        return refused("staging directory .berth/tmp passes through a symlink");
    }
    if symlink_in_parents(wt, rel)? {
        return refused("target path passes through a symlink");
    }
    if fs::symlink_metadata(&target).is_ok() {
        return Ok(Item::new("db", rel, Action::Skipped, "exists"));
    }
    let tmp_dir = wt.join(".berth").join("tmp");
    fs::create_dir_all(&tmp_dir).map_err(|e| io_err(&tmp_dir, e))?;
    vlog(&format!("db copy {template} -> {rel}"));
    if copy_one(&source, &target, &tmp_dir)? {
        Ok(Item::new(
            "db",
            rel,
            Action::Create,
            format!("copied from {template}"),
        ))
    } else {
        Ok(Item::new("db", rel, Action::Skipped, "exists"))
    }
}

/// Remove the task's database. `None` for sqlite (the file goes with the worktree).
pub fn drop(
    _main: &Path,
    cwd: &Path,
    db: &DbEntry,
    force: bool,
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<Option<Item>, BerthError> {
    match db.provider.as_str() {
        "postgres" => {
            if !db.created {
                return Ok(Some(Item::new(
                    "db",
                    &db.name,
                    Action::Skipped,
                    format!(
                        "not created by berth; left in place; drop it with dropdb {} if it is a leftover",
                        db.name
                    ),
                )));
            }
            let mut args = vec!["dropdb".to_string()];
            args.extend(pg_conn_args(
                db.host.as_deref().unwrap_or("localhost"),
                db.port.unwrap_or(5432),
                db.user.as_deref(),
            ));
            args.push("--if-exists".into());
            if force {
                args.push("--force".into());
            }
            args.push("--".into());
            args.push(db.name.clone());
            run_db(join_quoted(&args), cwd, env, timeout)?;
            Ok(Some(Item::new("db", &db.name, Action::Remove, "dropped")))
        }
        "compose" => {
            let args = [
                "docker",
                "compose",
                "--project-name",
                &db.name,
                "down",
                "--volumes",
                "--remove-orphans",
            ];
            run_db(quoted(&args), cwd, env, timeout)?;
            Ok(Some(Item::new(
                "db",
                &db.name,
                Action::Remove,
                "compose down",
            )))
        }
        _ => Ok(None),
    }
}
