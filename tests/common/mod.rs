#![allow(dead_code)]

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Std;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

const GIT_ENV: [(&str, &str); 6] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "berth-test"),
    ("GIT_AUTHOR_EMAIL", "berth-test@example.invalid"),
    ("GIT_COMMITTER_NAME", "berth-test"),
    ("GIT_COMMITTER_EMAIL", "berth-test@example.invalid"),
];

/// A throwaway world: repo, HOME, state dir and TMPDIR all live under `root`.
pub struct Fixture {
    pub root: TempDir,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub state: PathBuf,
    pub tmp: PathBuf,
    pub range: (u16, u16),
}

impl Fixture {
    pub fn new() -> Self {
        Self::named("repo")
    }

    /// Like `new`, with a chosen repo directory name (e.g. `"my repo"`).
    pub fn named(dir: &str) -> Self {
        let root = TempDir::new().expect("tempdir");
        let base = root.path().to_path_buf();
        let fx = Fixture {
            repo: base.join(dir),
            home: base.join("home"),
            state: base.join("state"),
            tmp: base.join("tmp"),
            range: port_range(),
            root,
        };
        for d in [&fx.home, &fx.state, &fx.tmp] {
            fs::create_dir_all(d).expect("mkdir");
        }
        init_repo(&fx.repo);
        fx
    }

    /// Another repo under the same root (same state dir and port range).
    pub fn add_repo(&self, name: &str) -> PathBuf {
        let path = self.root.path().join(name);
        init_repo(&path);
        path
    }

    /// Write an untracked `berth.toml`: `[ports] range = [lo, hi]`, then the extras.
    pub fn config(&self, repo: &Path, ports_extra: &str, rest: &str) {
        let (lo, hi) = self.range;
        let body = format!("[ports]\nrange = [{lo}, {hi}]\n{ports_extra}\n{rest}\n");
        write(repo, "berth.toml", &body);
    }

    pub fn berth(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_berth"));
        cmd.current_dir(&self.repo)
            .env("BERTH_STATE_DIR", &self.state)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env_remove("XDG_STATE_HOME");
        for (k, v) in GIT_ENV {
            cmd.env(k, v);
        }
        cmd
    }

    pub fn berth_std(&self) -> Std {
        let mut cmd = Std::new(env!("CARGO_BIN_EXE_berth"));
        cmd.current_dir(&self.repo)
            .env("BERTH_STATE_DIR", &self.state)
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmp)
            .env_remove("XDG_STATE_HOME");
        for (k, v) in GIT_ENV {
            cmd.env(k, v);
        }
        cmd
    }
}

fn init_repo(path: &Path) {
    fs::create_dir_all(path).expect("mkdir repo");
    git(path, &["init", "-q", "-b", "main"]);
    write(path, ".gitignore", ".env\n");
    git(path, &["add", ".gitignore"]);
    git(path, &["commit", "-q", "-m", "init"]);
}

/// Run git in `dir` with the hermetic env; panics with stderr on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Std::new("git");
    cmd.args(args).current_dir(dir);
    for (k, v) in GIT_ENV {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A 100-port window aligned to 100 inside 21000-30999 (ten blocks of ten), distinct per
/// fixture within a process and spread across processes.
pub fn port_range() -> (u16, u16) {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let slot = (std::process::id() as usize * 7 + n) % 100;
    let lo = 21000 + 100 * slot as u16;
    (lo, lo + 99)
}

/// Write `content` to `dir/rel`, creating parent directories.
pub fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, content).expect("write");
}
