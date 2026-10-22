//! `berth hook <event>` (spec §12): Claude Code hook payloads on stdin, the harness's own protocol
//! on stdout. No Report is built for the caller: `main` hands the whole command to [`run`], which
//! prints what the harness expects and returns the exit code. Everything else goes to stderr.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cache;
use crate::cli::{Cli, Format, HookArgs, HookEvent, Task, UpArgs};
use crate::config;
use crate::env::{format_env, format_env_hoard};
use crate::git;
use crate::manifest::{Entry, Manifest};
use crate::ops::{down, up};
use crate::report::{Report, render_table};
use crate::{BerthError, Ctx};

/// Run one hook event and return the process exit code. Never prints a Report on stdout.
pub fn run(cli: &Cli, args: &HookArgs) -> i32 {
    let quiet_on_error = args.event == HookEvent::Statusline;
    let result = if cli.json {
        Err(BerthError::Usage(
            "berth hook speaks the harness's protocol on stdout; --json is not supported".into(),
        ))
    } else {
        payload().and_then(|payload| dispatch(cli, args.event, &payload))
    };
    match result {
        Ok(code) => code,
        // A status line must never show or fail on an error.
        Err(_) if quiet_on_error && !cli.json => 0,
        Err(err) => {
            eprintln!("error: {err}");
            err.exit_code()
        }
    }
}

fn dispatch(cli: &Cli, event: HookEvent, payload: &Value) -> Result<i32, BerthError> {
    match event {
        HookEvent::WorktreeCreate => worktree_create(cli, payload),
        HookEvent::WorktreeRemove => worktree_remove(cli, payload),
        HookEvent::SessionStart => session_start(payload),
        HookEvent::Statusline => Ok(statusline(payload)),
    }
}

/// The single JSON object on stdin.
fn payload() -> Result<Value, BerthError> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| BerthError::Usage(format!("cannot read the hook payload from stdin: {e}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| BerthError::Usage(format!("hook payload is not valid JSON: {e}")))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(BerthError::Usage(
            "hook payload must be one JSON object".into(),
        ))
    }
}

fn required<'a>(payload: &'a Value, key: &str) -> Result<&'a str, BerthError> {
    match payload.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(BerthError::Usage(format!(
            "hook payload: missing string field `{key}`"
        ))),
    }
}

fn context(dir: &Path, cli: &Cli) -> Result<Ctx, BerthError> {
    Ok(Ctx {
        dir: dir.to_path_buf(),
        repo: git::discover(dir)?,
        verbose: cli.verbose,
        json: false,
    })
}

/// A report for the harness's log: the table, on stderr.
fn log_report(report: &Report) {
    eprint!("{}", render_table(report, false));
}

/// `berth up <name>` from `cwd`; stdout is the absolute worktree path and nothing else.
fn worktree_create(cli: &Cli, payload: &Value) -> Result<i32, BerthError> {
    let cwd = required(payload, "cwd")?;
    let task: Task = required(payload, "name")?
        .parse()
        .map_err(BerthError::Usage)?;
    let ctx = context(Path::new(cwd), cli)?;
    let args = UpArgs {
        task,
        base: None,
        branch: None,
        no_setup: false,
        keep_on_failure: false,
    };
    let report = up::up(&ctx, &args)?;
    log_report(&report);
    if report.exit_code != 0 {
        return Ok(report.exit_code);
    }
    let entry = report
        .worktrees
        .first()
        .ok_or_else(|| BerthError::Env("berth up returned no worktree".into()))?;
    // Claude Code refuses `.` and `..` segments.
    let path = up::normalize(&entry.path);
    let mut out = std::io::stdout();
    writeln!(out, "{}", path.display())
        .and_then(|()| out.flush())
        .map_err(|source| BerthError::Io {
            path: PathBuf::from("<stdout>"),
            source,
        })?;
    Ok(0)
}

/// `berth down <task>` for the entry whose path is `worktree_path`: never `--force`, branch kept.
fn worktree_remove(cli: &Cli, payload: &Value) -> Result<i32, BerthError> {
    let target = PathBuf::from(required(payload, "worktree_path")?);
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    // The directory may be gone already; then the repo is found from `cwd`.
    let mut starts = Vec::new();
    if target.is_dir() {
        starts.push(target.clone());
    }
    starts.extend(cwd);
    let mut last = None;
    let mut found = None;
    for dir in &starts {
        match git::discover(dir) {
            Ok(repo) => {
                found = Some(repo);
                break;
            }
            Err(err) => last = Some(err),
        }
    }
    let Some(repo) = found else {
        return match last {
            Some(err) => Err(err),
            None => {
                eprintln!(
                    "note: {} is gone and the payload has no `cwd`; no berth task to remove",
                    target.display()
                );
                Ok(0)
            }
        };
    };
    let manifest = Manifest::load(&repo.main)?;
    let wanted = canon_lossy(&target);
    let Some(entry) = manifest
        .worktrees
        .iter()
        .find(|e| canon_lossy(&e.path) == wanted)
    else {
        eprintln!(
            "note: no berth task owns {}; nothing to remove",
            target.display()
        );
        return Ok(0);
    };
    let task = entry.task.clone();
    // Run from the main checkout: the worktree being removed must not be git's cwd.
    let ctx = Ctx {
        dir: repo.main.clone(),
        repo,
        verbose: cli.verbose,
        json: false,
    };
    let report = down::down_task(&ctx, &task, false, false)?;
    log_report(&report);
    Ok(report.exit_code)
}

/// Append `# berth <task>` and `berth env --format sh` to `$CLAUDE_ENV_FILE`.
fn session_start(payload: &Value) -> Result<i32, BerthError> {
    let cwd = required(payload, "cwd")?;
    let Some(file) = std::env::var_os("CLAUDE_ENV_FILE").filter(|v| !v.is_empty()) else {
        return Ok(0);
    };
    let Some((main, entry)) = locate(Path::new(cwd)) else {
        return Ok(0);
    };
    let hoard = match config::load(&main) {
        Ok(cfg) => cache::hoard(&main, &cfg.cache),
        Err(err) => {
            eprintln!("warning: {err}; hoard values not exported");
            None
        }
    };
    // The hoard token stays a `$(cat <file>)` substitution; its value is never read here.
    let body = match hoard {
        Some(h) => format_env_hoard(&entry.env, Format::Sh, &h.api, &h.token_file),
        None => format_env(&entry.env, Format::Sh),
    };
    let file = PathBuf::from(file);
    let io = |source| BerthError::Io {
        path: file.clone(),
        source,
    };
    // Other hooks write this file too: append only, and start on a fresh line.
    let mut block = String::new();
    if !ends_with_newline_or_empty(&file) {
        block.push('\n');
    }
    block.push_str(&format!("# berth {}\n{body}", entry.task));
    let mut out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(io)?;
    out.write_all(block.as_bytes()).map_err(io)?;
    Ok(0)
}

fn ends_with_newline_or_empty(file: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(file) else {
        return true;
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return true;
    };
    if len == 0 || f.seek(SeekFrom::Start(len - 1)).is_err() {
        return true;
    }
    let mut last = [0u8; 1];
    f.read_exact(&mut last).is_err() || last[0] == b'\n'
}

/// `<task> :<PORT>` when the directory is inside a berth worktree; otherwise nothing. Lock-free,
/// reads only the manifest, never fails.
fn statusline(payload: &Value) -> i32 {
    let dir = payload
        .pointer("/workspace/current_dir")
        .and_then(Value::as_str)
        .or_else(|| payload.get("cwd").and_then(Value::as_str));
    if let Some((_, entry)) = dir.and_then(|d| locate(Path::new(d))) {
        let line = match entry.env.get("PORT") {
            Some(port) => format!("{} :{port}", entry.task),
            None => entry.task.clone(),
        };
        println!("{line}");
    }
    0
}

/// The main checkout and entry of the berth worktree containing `dir`, if any.
fn locate(dir: &Path) -> Option<(PathBuf, Entry)> {
    // A directory removed since the payload was built still belongs to its nearest ancestor.
    let existing = dir.ancestors().find(|a| a.is_dir())?;
    let repo = git::discover(existing).ok()?;
    let manifest = Manifest::load(&repo.main).ok()?;
    let here = canon_lossy(dir);
    let entry = manifest
        .worktrees
        .iter()
        .find(|e| here.starts_with(canon_lossy(&e.path)))?
        .clone();
    Some((repo.main, entry))
}

/// Canonical form that also works for a path that no longer exists: the deepest existing
/// ancestor is canonicalized, the rest is appended lexically.
fn canon_lossy(path: &Path) -> PathBuf {
    let path = up::normalize(path);
    let mut rest = Vec::new();
    let mut cur = path.as_path();
    loop {
        if let Ok(real) = crate::sys::canonicalize(cur) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return path,
        }
    }
}
