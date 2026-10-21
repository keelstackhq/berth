mod common;

use berth::lock;
use common::{Fixture, git, write};
use serde_json::Value;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

const CANARY: &str = "BERTH_CANARY_7c2e";

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).expect("canonicalize")
}

fn base_dir(fx: &Fixture) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    canon(fx.root.path()).join(name)
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    base_dir(fx).join(task)
}

fn up(fx: &Fixture, task: &str) {
    let out = fx.berth().args(["up", task]).output().expect("run berth");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn berth(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {}", stdout(out)))
}

fn item<'a>(v: &'a Value, key: &str) -> &'a Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == key)
        .unwrap_or_else(|| panic!("no item {key}: {v}"))
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || c == '/' || c == '_' || c == '-' {
                vec![c]
            } else {
                vec!['\\', c]
            }
        })
        .collect()
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}

#[test]
fn ls_shows_two_tasks() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    up(&fx, "b");
    write(&wt(&fx, "b"), "x.txt", "work");

    let out = berth(&fx, &["ls", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["command"], "ls");
    assert_eq!(v["worktrees"].as_array().unwrap().len(), 2);
    let b = item(&v, "b");
    assert_eq!(b["kind"], "worktree");
    assert_eq!(b["action"], "ok");
    let reason = b["reason"].as_str().unwrap();
    assert!(reason.contains("dirty"), "{reason}");
    assert!(reason.contains("berth/b"), "{reason}");
    assert!(reason.contains(wt(&fx, "b").to_str().unwrap()), "{reason}");
    assert!(item(&v, "a")["reason"].as_str().unwrap().contains("clean"));

    let out = berth(&fx, &["ls"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(
        &regex_escape(&canon(fx.root.path()).display().to_string()),
        "[ROOT]",
    );
    settings.bind(|| insta::assert_snapshot!("ls_shows_two_tasks_table", stdout(&out)));
}

#[test]
fn ls_creates_no_file() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let before = (files_under(&fx.state), files_under(&fx.repo.join(".git")));
    let out = berth(&fx, &["ls"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!fx.repo.join(".agents").exists());
    assert_eq!(
        before,
        (files_under(&fx.state), files_under(&fx.repo.join(".git")))
    );
}

#[test]
fn ls_reports_listening_port() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let out = berth(&fx, &["env", "a", "--json"]);
    let port: u16 = json(&out)["worktrees"][0]["env"]["WEB_PORT"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let _listener = TcpListener::bind(("127.0.0.1", port)).expect("hold the port");
    let out = berth(&fx, &["ls", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let a = item(&json(&out), "a").clone();
    assert_eq!(a["action"], "ok");
    assert!(
        a["reason"].as_str().unwrap().contains("listening web"),
        "{a}"
    );
}

#[test]
fn ls_reports_unmanaged() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let x = wt(&fx, "x");
    git(
        &fx.repo,
        &["worktree", "add", "-q", "-b", "hand", x.to_str().unwrap()],
    );
    let out = berth(&fx, &["ls", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    let x = item(&v, "x");
    assert_eq!(x["action"], "drift");
    assert!(x["reason"].as_str().unwrap().starts_with("unmanaged · "));
    assert!(v["worktrees"].as_array().unwrap().is_empty());
}

#[test]
fn ls_reports_missing_directory() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    fs::remove_dir_all(wt(&fx, "a")).unwrap();
    let out = berth(&fx, &["ls", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let a = item(&json(&out), "a").clone();
    assert_eq!(a["action"], "drift");
    assert_eq!(
        a["reason"],
        "missing: worktree directory deleted; run berth down a"
    );
}

#[test]
fn ls_reports_interrupted_up() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let path = fx.repo.join(".agents/worktrees.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["worktrees"][0]["state"] = "creating".into();
    m["worktrees"][0]["owner"]["pid"] = 4_000_000.into();
    fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    let out = berth(&fx, &["ls", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let a = item(&json(&out), "a").clone();
    assert_eq!(a["action"], "drift");
    assert_eq!(
        a["reason"],
        "interrupted: owner pid 4000000 is gone; run berth down a"
    );
}

#[test]
fn ls_never_blocks_on_repo_lock() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let common = canon(&fx.repo.join(".git"));
    let _held = lock::acquire(&common.join("berth.lock"), Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    let out = fx
        .berth()
        .env("BERTH_LOCK_TIMEOUT_MS", "300")
        .args(["ls"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn env_from_cwd() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let sub = wt(&fx, "a").join("sub");
    fs::create_dir_all(&sub).unwrap();
    let out = fx
        .berth()
        .args(["-C", sub.to_str().unwrap(), "env"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("export BERTH_TASK='a'"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn env_format_json_vs_json_flag() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let out = berth(&fx, &["env", "a", "--format", "json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert!(v["PORT"].is_string());
    assert!(v.get("version").is_none());

    let out = berth(&fx, &["env", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["version"], 1);
    assert!(v["worktrees"][0]["env"]["PORT"].is_string());

    // --json wins over --format.
    let out = berth(&fx, &["env", "a", "--json", "--format", "fish"]);
    assert_eq!(json(&out)["version"], 1);
}

#[test]
fn env_outside_worktree_exits_2() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let out = berth(&fx, &["env"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("not inside a berth worktree; pass <task>"));
    let out = berth(&fx, &["env", "--json"]);
    assert_eq!(code(&out), 2);
    assert_eq!(json(&out)["items"][0]["key"], "usage");
}

#[test]
fn env_unknown_task_exits_3() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let out = berth(&fx, &["env", "nope"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(stderr(&out).contains("no berth task `nope`; run berth up nope"));
}

#[test]
fn env_does_not_include_copied_values() {
    let fx = Fixture::new();
    write(&fx.repo, ".env", &format!("TOKEN={CANARY}\n"));
    fx.config(&fx.repo, "", "[copy]\ninclude = [\".env\"]");
    up(&fx, "a");
    for fmt in ["sh", "fish", "dotenv", "json"] {
        let out = berth(&fx, &["env", "a", "--format", fmt]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert!(!stdout(&out).contains(CANARY), "{fmt}");
        assert!(!stderr(&out).contains(CANARY), "{fmt}");
    }
    let out = berth(&fx, &["env", "a", "--json"]);
    assert!(!stdout(&out).contains(CANARY));
    let out = berth(&fx, &["ls", "--json"]);
    assert!(!stdout(&out).contains(CANARY));
}

#[test]
fn ls_leaves_the_worktree_index_alone() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let path = wt(&fx, "a");
    write(&path, "t.txt", "tracked");
    git(&path, &["add", "t.txt"]);
    git(&path, &["commit", "-q", "-m", "t"]);
    // Newer mtime, same content: a plain `git status` would refresh and rewrite the index.
    std::thread::sleep(Duration::from_millis(1100));
    write(&path, "t.txt", "tracked");
    write(&path, "u.txt", "untracked");
    let index = PathBuf::from(
        git(
            &path,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )
        .trim(),
    );
    let lock = PathBuf::from(format!("{}.lock", index.display()));
    let before = (
        fs::read(&index).unwrap(),
        fs::metadata(&index).unwrap().modified().unwrap(),
    );
    let out = berth(&fx, &["ls"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).contains("dirty"), "{}", stdout(&out));
    let after = (
        fs::read(&index).unwrap(),
        fs::metadata(&index).unwrap().modified().unwrap(),
    );
    assert!(before == after, "ls rewrote the index");
    assert!(!lock.exists());
}

#[test]
fn ls_unregistered_directory_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let path = wt(&fx, "a");
    git(
        &fx.repo,
        &["worktree", "remove", "--force", path.to_str().unwrap()],
    );
    fs::create_dir_all(&path).unwrap();
    let out = berth(&fx, &["ls", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let a = item(&json(&out), "a").clone();
    assert_eq!(a["action"], "drift");
    assert_eq!(
        a["reason"],
        "unregistered: not a worktree of this repo; run berth down a"
    );
}

#[test]
fn env_survives_a_broken_berth_toml() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let before = stdout(&berth(&fx, &["env", "a"]));
    let toml = fx.repo.join("berth.toml");
    let old = fs::read_to_string(&toml).unwrap();
    fs::write(&toml, format!("{old}\nbogus_key = 1\n")).unwrap();
    let out = berth(&fx, &["env", "a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), before);
    assert!(
        stderr(&out).contains("warning: ") && stderr(&out).contains("; hoard values not exported"),
        "{}",
        stderr(&out)
    );
}
