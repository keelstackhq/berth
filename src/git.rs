//! git through its CLI only. Every child gets piped stdout and stderr (never inherited), no
//! stdin prompt, and is logged as `berth: git <args>` under `-v`.

use crate::{BerthError, vlog};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const MIN_VERSION: (u32, u32) = (2, 36);

/// The main checkout of a repository and its common git dir (both canonical).
#[derive(Debug, Clone)]
pub struct Repo {
    pub main: PathBuf,
    pub common_dir: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
}

fn missing_git() -> BerthError {
    BerthError::Env("git 2.36 or newer is required (found none on PATH)".into())
}

fn io_err(path: &Path, source: std::io::Error) -> BerthError {
    BerthError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// Run `git -C <dir> <args>` and return the raw output whatever the exit status.
fn exec(dir: &Path, args: &[&OsStr], stdin: Option<Vec<u8>>) -> Result<Output, BerthError> {
    let shown: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    vlog(&format!("git {}", shown.join(" ")));
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            missing_git()
        } else {
            io_err(Path::new("git"), e)
        }
    })?;
    // Feed stdin from a thread so a full stdout pipe cannot deadlock us.
    let writer = stdin.and_then(|data| {
        child.stdin.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let _ = pipe.write_all(&data);
            })
        })
    });
    let out = child
        .wait_with_output()
        .map_err(|e| io_err(Path::new("git"), e))?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(out)
}

fn failure(args: &[&OsStr], out: &Output) -> BerthError {
    let shown: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    BerthError::Git {
        args: shown.join(" "),
        stderr: text(&out.stderr),
    }
}

/// Run git; any non-zero exit is a `Git` error. Returns raw stdout.
fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, BerthError> {
    let os: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    run_os(dir, &os)
}

fn run_os(dir: &Path, args: &[&OsStr]) -> Result<Vec<u8>, BerthError> {
    let out = exec(dir, args, None)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(failure(args, &out))
    }
}

/// Run git where exit 0 → true, exit 1 → false, anything else is an error.
fn run_bool(dir: &Path, args: &[&str]) -> Result<bool, BerthError> {
    let os: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let out = exec(dir, &os, None)?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(failure(&os, &out)),
    }
}

pub fn check_version() -> Result<(u32, u32), BerthError> {
    vlog("git --version");
    let out = Command::new("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                missing_git()
            } else {
                io_err(Path::new("git"), e)
            }
        })?;
    let line = text(&out.stdout);
    let version = line.split_whitespace().nth(2).unwrap_or("");
    let mut nums = version.split('.').map(|p| p.parse::<u32>());
    let (Some(Ok(major)), Some(Ok(minor))) = (nums.next(), nums.next()) else {
        return Err(BerthError::Env(format!(
            "git 2.36 or newer is required (could not read version from `{line}`)"
        )));
    };
    if (major, minor) < MIN_VERSION {
        return Err(BerthError::Env(format!(
            "git 2.36 or newer is required (found {version})"
        )));
    }
    Ok((major, minor))
}

pub fn discover(dir: &Path) -> Result<Repo, BerthError> {
    check_version()?;
    let common = exec(
        dir,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--path-format=absolute"),
            OsStr::new("--git-common-dir"),
        ],
        None,
    )?;
    if !common.status.success() {
        let mut message =
            String::from("not inside a git repository; run berth from your repo or pass -C <dir>");
        let stderr = text(&common.stderr);
        if let Some(first) = stderr.lines().next() {
            message.push_str(&format!(" (git: {first})"));
        }
        return Err(BerthError::Env(message));
    }
    let common_dir = PathBuf::from(text(&common.stdout));
    let common_dir = fs::canonicalize(&common_dir).map_err(|e| io_err(&common_dir, e))?;
    if text(&run(dir, &["rev-parse", "--is-bare-repository"])?) == "true" {
        return Err(BerthError::Env("berth needs a non-bare repository".into()));
    }
    let list = worktree_list(dir)?;
    let first = list
        .into_iter()
        .next()
        .ok_or_else(|| BerthError::Env("git lists no worktrees for this repository".into()))?;
    let mut main = fs::canonicalize(&first.path).map_err(|e| io_err(&first.path, e))?;
    if main == common_dir {
        // `--separate-git-dir`: git lists the git dir itself as the main entry.
        main = separate_git_dir_checkout(dir, &common_dir)?;
    }
    let name = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Repo {
        main,
        common_dir,
        name,
    })
}

/// The checkout of a `--separate-git-dir` repo. Only the main checkout can say where it is.
fn separate_git_dir_checkout(dir: &Path, common_dir: &Path) -> Result<PathBuf, BerthError> {
    let cannot = || {
        BerthError::Env(
            "cannot locate the main checkout of this --separate-git-dir repository; \
             run berth from the main checkout"
                .into(),
        )
    };
    let git_dir = run(dir, &["rev-parse", "--path-format=absolute", "--git-dir"])
        .ok()
        .map(|b| PathBuf::from(text(&b)))
        .and_then(|p| fs::canonicalize(p).ok());
    if git_dir.as_deref() != Some(common_dir) {
        return Err(cannot());
    }
    let top = run(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| cannot())?;
    let top = PathBuf::from(text(&top));
    fs::canonicalize(&top).map_err(|e| io_err(&top, e))
}

pub fn resolve_commit(main: &Path, rev: &str) -> Result<String, BerthError> {
    let unknown = || {
        BerthError::Env(format!(
            "unknown --base ref `{rev}`; berth never fetches, run `git fetch` first"
        ))
    };
    if rev.starts_with('-') {
        return Err(unknown());
    }
    let spec = format!("{rev}^{{commit}}");
    let out = exec(
        main,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("--end-of-options"),
            OsStr::new(&spec),
        ],
        None,
    )?;
    if !out.status.success() {
        return Err(unknown());
    }
    Ok(text(&out.stdout))
}

pub fn check_branch_name(main: &Path, name: &str) -> Result<(), BerthError> {
    let bad = || BerthError::Usage(format!("invalid branch name `{name}`"));
    if name.starts_with('-') || name.contains("@{") {
        return Err(bad());
    }
    // check-ref-format exits 128 (not 1) for an invalid name.
    let args = [
        OsStr::new("check-ref-format"),
        OsStr::new("--branch"),
        OsStr::new(name),
    ];
    if exec(main, &args, None)?.status.success() {
        Ok(())
    } else {
        Err(bad())
    }
}

pub fn branch_exists(main: &Path, name: &str) -> Result<bool, BerthError> {
    let full = format!("refs/heads/{name}");
    run_bool(main, &["show-ref", "--verify", "--quiet", &full])
}

/// `new_from = Some(sha)` creates `branch` at `sha`; `None` checks out the existing branch.
pub fn worktree_add(
    main: &Path,
    path: &Path,
    branch: &str,
    new_from: Option<&str>,
) -> Result<(), BerthError> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("add")];
    match new_from {
        Some(sha) => {
            args.extend([
                OsStr::new("-b"),
                OsStr::new(branch),
                OsStr::new("--end-of-options"),
                path.as_os_str(),
                OsStr::new(sha),
            ]);
        }
        None => {
            args.extend([
                OsStr::new("--end-of-options"),
                path.as_os_str(),
                OsStr::new(branch),
            ]);
        }
    }
    run_os(main, &args).map(drop)
}

pub fn worktree_remove(main: &Path, path: &Path, force: bool) -> Result<(), BerthError> {
    let mut args: Vec<&OsStr> = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.extend([OsStr::new("--end-of-options"), path.as_os_str()]);
    run_os(main, &args).map(drop)
}

pub fn worktree_prune(main: &Path) -> Result<(), BerthError> {
    run(main, &["worktree", "prune"]).map(drop)
}

pub fn worktree_list(main: &Path) -> Result<Vec<WorktreeInfo>, BerthError> {
    let raw = run(main, &["worktree", "list", "--porcelain", "-z"])?;
    let mut list: Vec<WorktreeInfo> = Vec::new();
    for field in raw.split(|b| *b == 0) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            list.push(WorktreeInfo {
                path: PathBuf::from(std::ffi::OsString::from_vec(path.to_vec())),
                head: String::new(),
                branch: None,
            });
        } else if let Some(cur) = list.last_mut() {
            if let Some(head) = field.strip_prefix(b"HEAD ") {
                cur.head = text(head);
            } else if let Some(branch) = field.strip_prefix(b"branch ") {
                cur.branch = Some(text(branch));
            }
        }
    }
    Ok(list)
}

/// True when `wt` has staged, unstaged or untracked changes; ignored files do not count.
pub fn dirty(wt: &Path) -> Result<bool, BerthError> {
    Ok(!run(wt, &["status", "--porcelain", "-z"])?.is_empty())
}

/// The subset of `paths` (relative to `main`) that git ignores.
pub fn ignored(main: &Path, paths: &[String]) -> Result<BTreeSet<String>, BerthError> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    let args = [
        OsStr::new("check-ignore"),
        OsStr::new("-z"),
        OsStr::new("--stdin"),
    ];
    let out = exec(main, &args, Some(input))?;
    match out.status.code() {
        Some(0) => Ok(out
            .stdout
            .split(|b| *b == 0)
            .filter(|f| !f.is_empty())
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect()),
        Some(1) => Ok(BTreeSet::new()),
        _ => Err(failure(&args, &out)),
    }
}

/// Append `line` to `<common_dir>/info/exclude` unless an identical line exists.
pub fn ensure_excluded(common_dir: &Path, line: &str) -> Result<bool, BerthError> {
    let file = common_dir.join("info").join("exclude");
    let existing = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io_err(&file, e)),
    };
    if existing
        .split(|b| *b == b'\n')
        .any(|l| l == line.as_bytes())
    {
        return Ok(false);
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    }
    let mut add = Vec::new();
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        add.push(b'\n');
    }
    add.extend_from_slice(line.as_bytes());
    add.push(b'\n');
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| io_err(&file, e))?;
    f.write_all(&add).map_err(|e| io_err(&file, e))?;
    Ok(true)
}

/// Commits `branch` has over `base` (ahead) and `base` has over `branch` (behind).
pub fn ahead_behind(main: &Path, base: &str, branch: &str) -> Result<(u32, u32), BerthError> {
    let range = format!("{base}...{branch}");
    let out = run(
        main,
        &[
            "rev-list",
            "--left-right",
            "--count",
            "--end-of-options",
            &range,
        ],
    )?;
    let shown = text(&out);
    let mut parts = shown.split_whitespace().map(|n| n.parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(behind)), Some(Ok(ahead))) => Ok((ahead, behind)),
        _ => Err(BerthError::Git {
            args: format!("rev-list --left-right --count {range}"),
            stderr: format!("unexpected output `{shown}`"),
        }),
    }
}

pub fn is_ancestor(main: &Path, a: &str, b: &str) -> Result<bool, BerthError> {
    run_bool(
        main,
        &["merge-base", "--is-ancestor", "--end-of-options", a, b],
    )
}

pub fn merge_base(main: &Path, a: &str, b: &str) -> Result<String, BerthError> {
    Ok(text(&run(main, &["merge-base", "--end-of-options", a, b])?))
}

/// `git branch -D`; callers check the merge state first.
pub fn delete_branch(main: &Path, name: &str) -> Result<(), BerthError> {
    run(main, &["branch", "-D", "--end-of-options", name]).map(drop)
}

pub fn commits_since(main: &Path, base: &str, branch: &str) -> Result<u32, BerthError> {
    let range = format!("{base}..{branch}");
    let out = text(&run(
        main,
        &["rev-list", "--count", "--end-of-options", &range],
    )?);
    out.parse().map_err(|_| BerthError::Git {
        args: format!("rev-list --count {range}"),
        stderr: format!("unexpected output `{out}`"),
    })
}

/// `git ls-files -z <args>` in `main`, as raw path bytes (paths need not be UTF-8). Only the
/// patterns passed in `args` are used: callers never add `--exclude-standard`, so ignored
/// directories are not filtered out.
pub fn ls_files(main: &Path, args: &[String]) -> Result<Vec<Vec<u8>>, BerthError> {
    let mut full: Vec<&OsStr> = vec![OsStr::new("ls-files"), OsStr::new("-z")];
    full.extend(args.iter().map(OsStr::new));
    let out = run_os(main, &full)?;
    Ok(out
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}
