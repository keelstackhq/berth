mod common;

use berth::lock::{acquire, repo_lock, user_lock};
use berth::ports::{blocks, preferred_index};
use berth::registry::{Block, Registry, is_stale};
use common::{Fixture, git, write};
use serde_json::Value;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

/// Prints `boom` (the command text itself does not contain it) and exits 7.
const BOOM: &str = "[setup]\nrun = [\"printf 'bo%sm\\\\n' o; exit 7\"]";
const SETUP_OK: &str = "[setup]\nrun = [\"echo ok > .setup-ran\"]";

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).expect("canonicalize")
}

fn root(fx: &Fixture) -> PathBuf {
    canon(fx.root.path())
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    root(fx).join(name).join(task)
}

fn up(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().arg("up").args(args).output().expect("run berth")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {}", stdout(out)))
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn manifest_entries(fx: &Fixture) -> Vec<Value> {
    let path = fx.repo.join(".agents/worktrees.json");
    match fs::read(&path) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["worktrees"].as_array().unwrap().clone()
        }
        Err(_) => Vec::new(),
    }
}

fn reg_blocks(fx: &Fixture) -> Vec<Value> {
    match fs::read(fx.state.join("ports.json")) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["blocks"].as_array().unwrap().clone()
        }
        Err(_) => Vec::new(),
    }
}

fn branch_exists(repo: &Path, name: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .current_dir(repo)
        .status()
        .unwrap()
        .success()
}

fn assert_nothing_created(fx: &Fixture, task: &str) {
    assert!(!wt(fx, task).exists(), "worktree dir exists");
    assert!(reg_blocks(fx).is_empty(), "blocks: {:?}", reg_blocks(fx));
    assert!(
        manifest_entries(fx).iter().all(|e| e["task"] != task),
        "manifest has {task}"
    );
    assert!(!branch_exists(&fx.repo, &format!("berth/{task}")));
}

fn item<'a>(v: &'a Value, kind: &str) -> &'a Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind)
        .unwrap_or_else(|| panic!("no item of kind {kind}: {v}"))
}

fn has_action(v: &Value, action: &str) -> bool {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["action"] == action)
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_paused(fx: &Fixture, step: &str, task: &str) -> Child {
    let child = fx
        .berth_std()
        .args(["up", task])
        .env("BERTH_TEST_PAUSE", step)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&fx.state.join(format!("paused-{step}")));
    child
}

/// The allocated block start of a successful `up --json`.
fn start_of(out: &Output) -> u16 {
    let v = json(out);
    let start = v["worktrees"][0]["ports"]["start"]
        .as_u64()
        .unwrap_or_else(|| panic!("no port block: {v}"));
    u16::try_from(start).unwrap()
}

fn preferred_start(fx: &Fixture, task: &str) -> u16 {
    let bs = blocks(fx.range, 10);
    bs[preferred_index(&canon(&fx.repo), task, bs.len())]
}

fn seed_block(fx: &Fixture, repo: &Path, task: &str, start: u16) {
    let mut reg = Registry::load(&fx.state).unwrap();
    reg.add(Block {
        start,
        end: start + 9,
        repo: repo.to_path_buf(),
        task: task.into(),
        created_at: "2026-10-17T00:00:00Z".into(),
    });
    reg.save(&fx.state).unwrap();
}

fn down(fx: &Fixture, task: &str) {
    let out = fx.berth().args(["down", task]).output().expect("run berth");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[test]
fn up_creates_everything() {
    // Human output.
    let fx = Fixture::new();
    write(&fx.repo, ".env", "TOKEN=BERTH_CANARY_5d1e\n");
    fx.config(&fx.repo, "", SETUP_OK);
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let path = wt(&fx, "a");
    assert!(path.is_dir());
    assert_eq!(
        git(&path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "berth/a"
    );
    assert_eq!(
        fs::read_to_string(path.join(".env")).unwrap(),
        "TOKEN=BERTH_CANARY_5d1e\n"
    );
    assert!(path.join(".setup-ran").is_file());
    let entries = manifest_entries(&fx);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["state"], "ready");
    assert_eq!(entries[0]["copied"], serde_json::json!([".env"]));
    assert_eq!(entries[0]["setup"]["status"], "ok");
    let bs = reg_blocks(&fx);
    assert_eq!(bs.len(), 1);
    let (s, e) = (
        bs[0]["start"].as_u64().unwrap(),
        bs[0]["end"].as_u64().unwrap(),
    );
    assert_eq!(e - s + 1, 10);
    assert!(s >= u64::from(fx.range.0) && e <= u64::from(fx.range.1));
    let exclude = fs::read_to_string(canon(&fx.repo).join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == "/.berth/"), "{exclude}");
    let text = stdout(&out);
    let hint = format!("cd '{}' && eval \"$(berth env)\"\n", path.display());
    assert!(text.ends_with(&hint), "{text}");
    assert!(text.lines().count() > 2, "table before hint: {text}");
    for s in [stdout(&out), stderr(&out)] {
        assert!(!s.contains("BERTH_CANARY_5d1e"));
    }

    // JSON snapshot.
    let fx = Fixture::new();
    write(&fx.repo, ".env", "TOKEN=BERTH_CANARY_5d1e\n");
    fx.config(&fx.repo, "", SETUP_OK);
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.contains("BERTH_CANARY_5d1e"));
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(&regex_escape(&root(&fx).display().to_string()), "[ROOT]");
    settings.add_filter(r#""pid": \d+"#, r#""pid": "[PID]""#);
    settings.add_filter(r#""host": "[^"]*""#, r#""host": "[HOST]""#);
    settings.add_filter(r#""secs": [0-9.eE+-]+"#, r#""secs": "[SECS]""#);
    settings.add_filter(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", "[TIME]");
    settings.add_filter(r"\b[0-9a-f]{40}\b", "[SHA]");
    settings.add_filter(r"\b(2[1-9]|30)\d{3}\b", "[PORT]");
    settings.bind(|| insta::assert_snapshot!("up_creates_everything_json", text));
}

#[test]
fn up_twice_is_ok() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let first = up(&fx, &["a", "--json"]);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let second = up(&fx, &["a", "--json"]);
    assert_eq!(code(&second), 0, "{}", stderr(&second));
    let (a, b) = (json(&first), json(&second));
    let items = b["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i["action"] == "ok"), "{b}");
    assert_eq!(a["worktrees"][0], b["worktrees"][0]);
    assert_eq!(reg_blocks(&fx).len(), 1);
}

#[test]
fn different_base_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let before = fs::read(fx.repo.join(".agents/worktrees.json")).unwrap();
    write(&fx.repo, "new.txt", "x");
    git(&fx.repo, &["add", "new.txt"]);
    git(&fx.repo, &["commit", "-q", "-m", "new"]);
    let out = up(&fx, &["a", "--base", "HEAD", "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert!(has_action(&json(&out), "drift"));
    assert_eq!(
        fs::read(fx.repo.join(".agents/worktrees.json")).unwrap(),
        before
    );
}

#[test]
fn different_branch_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let out = up(&fx, &["a", "--branch", "other", "--json"]);
    assert_eq!(code(&out), 1);
    assert!(has_action(&json(&out), "drift"));
    assert!(!branch_exists(&fx.repo, "other"));
}

#[test]
fn count_change_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    fx.config(&fx.repo, "count = 5", "");
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 1);
    assert!(has_action(&json(&out), "drift"));
}

#[test]
fn failed_entry_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[setup]\nrun = [\"exit 1\"]");
    assert_eq!(code(&up(&fx, &["a", "--keep-on-failure"])), 3);
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 1);
    let v = json(&out);
    let drift = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["action"] == "drift")
        .expect("drift item");
    assert!(
        drift["reason"].as_str().unwrap().contains("berth down a"),
        "{v}"
    );
}

#[test]
fn path_exists_refused() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    fs::create_dir_all(wt(&fx, "a")).unwrap();
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 1);
    assert!(has_action(&json(&out), "refused"));
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
}

#[test]
fn no_setup_skips_hook() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", SETUP_OK);
    let out = up(&fx, &["a", "--no-setup"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!wt(&fx, "a").join(".setup-ran").exists());
    assert_eq!(manifest_entries(&fx)[0]["setup"]["status"], "skipped");
}

#[test]
fn up_after_down_reuses_branch() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let first = json(&up(&fx, &["a", "--json"]));
    let path = wt(&fx, "a");
    write(&path, "work.txt", "w");
    git(&path, &["add", "work.txt"]);
    git(&path, &["commit", "-q", "-m", "work"]);
    let head = git(&path, &["rev-parse", "HEAD"]).trim().to_string();
    down(&fx, "a");
    assert!(!path.exists());
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let second = json(&out);
    assert_eq!(git(&path, &["rev-parse", "HEAD"]).trim(), head);
    assert_eq!(
        first["worktrees"][0]["ports"]["start"],
        second["worktrees"][0]["ports"]["start"]
    );
    assert_eq!(
        first["worktrees"][0]["base"],
        second["worktrees"][0]["base"]
    );
}

#[test]
fn existing_branch_with_base_warns() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    git(&fx.repo, &["branch", "berth/a"]);
    let out = up(&fx, &["a", "--base", "HEAD~0", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(item(&json(&out), "branch")["action"], "skipped");
    assert!(stderr(&out).contains("warning:"), "{}", stderr(&out));
}

#[test]
fn up_from_inside_worktree() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let sub = wt(&fx, "a").join("sub");
    fs::create_dir_all(&sub).unwrap();
    let out = fx
        .berth()
        .arg("-C")
        .arg(&sub)
        .args(["up", "b"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(wt(&fx, "b").is_dir());
    assert_eq!(manifest_entries(&fx).len(), 2);
    assert!(!wt(&fx, "a").join(".agents").exists());
    assert!(!sub.join(".agents").exists());
}

#[test]
fn spaces_in_repo_path() {
    let fx = Fixture::named("my repo");
    fx.config(&fx.repo, "", SETUP_OK);
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let path = root(&fx).join("my repo.berth").join("a");
    assert!(path.join(".setup-ran").is_file());
    let hint = format!("cd '{}' && eval \"$(berth env)\"\n", path.display());
    assert!(stdout(&out).ends_with(&hint), "{}", stdout(&out));
}

#[test]
fn unknown_base_exits_3() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let out = up(&fx, &["a", "--base", "origin/main"]);
    assert_eq!(code(&out), 3);
    assert!(stderr(&out).contains("git fetch"), "{}", stderr(&out));
    assert_nothing_created(&fx, "a");
    let out = up(&fx, &["a", "--base=--output=x"]);
    assert_eq!(code(&out), 3);
    assert_nothing_created(&fx, "a");
    let found = std::process::Command::new("find")
        .arg(fx.root.path())
        .args(["-name", "x"])
        .output()
        .unwrap();
    assert!(found.stdout.is_empty(), "{}", stdout(&found));
}

#[test]
fn bad_branch_exits_2() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    for args in [vec!["a", "--branch=-x"], vec!["a", "--branch", "a..b"]] {
        let out = up(&fx, &args);
        assert_eq!(code(&out), 2, "{args:?}: {}", stderr(&out));
        assert_nothing_created(&fx, "a");
    }
}

#[test]
fn tracked_env_refused_exits_1() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    write(&fx.repo, ".env", "TOKEN=x\n");
    git(&fx.repo, &["add", "-f", ".env"]);
    git(&fx.repo, &["commit", "-q", "-m", "env"]);
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 1);
    let v = json(&out);
    assert_eq!(item(&v, "copy")["action"], "refused");
    assert_nothing_created(&fx, "a");
}

#[test]
fn max_reached_exits_1() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[worktree]\nmax = 1");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let out = up(&fx, &["b", "--json"]);
    assert_eq!(code(&out), 1);
    assert!(has_action(&json(&out), "refused"));
    assert!(!wt(&fx, "b").exists());
    assert_eq!(reg_blocks(&fx).len(), 1);
}

#[test]
fn setup_failure_rolls_back() {
    let fx = Fixture::new();
    // The command text must not contain `boom`: the JSON reason names the command.
    fx.config(&fx.repo, "", BOOM);
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 3);
    let err = stderr(&out);
    let block = &err[err.rfind("error:").expect("error block")..];
    assert!(
        block.contains("exited 7") && block.contains("boom"),
        "{err}"
    );
    assert_nothing_created(&fx, "a");

    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 3);
    let v = json(&out);
    assert_eq!(v["exit_code"], 3);
    assert!(!stdout(&out).contains("boom"));
    assert_nothing_created(&fx, "a");
}

#[test]
fn json_stdout_is_one_object_on_failure() {
    let fx = Fixture::new();
    fx.config(
        &fx.repo,
        "",
        "[setup]\nrun = [\"printf 'to-%s\\\\n' stdout; exit 1\"]",
    );
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(json(&out)["exit_code"], 3);
    assert!(!stdout(&out).contains("to-stdout"));
}

#[test]
fn keep_on_failure_keeps_failed_entry() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", BOOM);
    let out = up(&fx, &["a", "--keep-on-failure"]);
    assert_eq!(code(&out), 3);
    assert!(stderr(&out).contains("boom"));
    let entries = manifest_entries(&fx);
    assert_eq!(entries[0]["state"], "failed");
    assert_eq!(entries[0]["setup"]["status"], "failed");
    assert!(wt(&fx, "a").is_dir());
    assert_eq!(reg_blocks(&fx).len(), 1);
}

#[test]
fn live_creating_refused() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let mut child = spawn_paused(&fx, "after_worktree", "a");
    let out = up(&fx, &["a", "--json"]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["items"][0]["action"], "refused");
    assert!(
        v["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("being created by pid"),
        "{v}"
    );
}

#[test]
fn crash_after_ports_leaks_nothing() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let mut child = spawn_paused(&fx, "after_ports", "a");
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(reg_blocks(&fx).len(), 1);
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let bs = reg_blocks(&fx);
    assert_eq!(bs.len(), 1, "{bs:?}");
    assert_eq!(bs[0]["task"], "a");
    down(&fx, "a");
    assert!(reg_blocks(&fx).is_empty());
}

#[test]
fn busy_preferred_block_skipped() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let start = preferred_start(&fx, "a");
    let _l = TcpListener::bind(("127.0.0.1", start)).unwrap();
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_ne!(start_of(&out), start);
}

#[test]
fn wildcard_listener_skipped() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let start = preferred_start(&fx, "a");
    let _l = TcpListener::bind(("0.0.0.0", start)).unwrap();
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_ne!(start_of(&out), start);
}

#[test]
fn no_free_block_exits_3() {
    let fx = Fixture::new();
    let lo = fx.range.0;
    write(
        &fx.repo,
        "berth.toml",
        &format!("[ports]\nrange = [{lo}, {}]\n", lo + 9),
    );
    let _l = TcpListener::bind(("127.0.0.1", lo)).unwrap();
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("widen [ports].range"),
        "{}",
        stderr(&out)
    );
    assert!(!wt(&fx, "a").exists());
}

#[test]
fn stale_block_reclaimed() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let start = preferred_start(&fx, "a");
    let gone = fx.root.path().join("gone");
    seed_block(&fx, &gone, "x", start);
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(start_of(&out), start);
    let bs = reg_blocks(&fx);
    assert_eq!(bs.len(), 1, "{bs:?}");
    assert_eq!(bs[0]["task"], "a");
}

#[test]
fn stale_block_busy_is_kept() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let start = preferred_start(&fx, "a");
    let gone = fx.root.path().join("gone");
    seed_block(&fx, &gone, "x", start);
    let _l = TcpListener::bind(("127.0.0.1", start)).unwrap();
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_ne!(start_of(&out), start);
    let bs = reg_blocks(&fx);
    assert_eq!(bs.len(), 2, "{bs:?}");
    assert!(bs.iter().any(|b| b["task"] == "x"));
}

#[test]
fn corrupt_foreign_manifest_not_stale() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let two = canon(&fx.add_repo("two"));
    write(&two, ".agents/worktrees.json", "{");
    let start = preferred_start(&fx, "a");
    seed_block(&fx, &two, "x", start);
    let out = up(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_ne!(start_of(&out), start);
    let bs = reg_blocks(&fx);
    assert!(bs.iter().any(|b| b["task"] == "x"), "{bs:?}");
}

#[test]
fn is_stale_never_blocks() {
    let fx = Fixture::new();
    let two = canon(&fx.add_repo("two"));
    let three = canon(&fx.add_repo("three"));
    let block = |repo: &Path| Block {
        start: fx.range.0,
        end: fx.range.0 + 9,
        repo: repo.to_path_buf(),
        task: "ghost".into(),
        created_at: "2026-10-17T00:00:00Z".into(),
    };
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let lock_path = repo_lock(&two.join(".git"));
    let holder = std::thread::spawn(move || {
        let _g = acquire(&lock_path, Duration::from_secs(5)).unwrap();
        held_tx.send(()).unwrap();
        let _ = done_rx.recv();
    });
    held_rx.recv().unwrap();
    let _user = acquire(&user_lock(&fx.state), Duration::from_secs(5)).unwrap();

    let t = Instant::now();
    assert!(!is_stale(&block(&two)), "repo lock held → not stale");
    assert!(t.elapsed() < Duration::from_secs(1));

    assert!(is_stale(&block(&three)), "lock free, task absent → stale");
    assert!(!three.join(".git/berth.lock").exists());
    assert!(is_stale(&block(&fx.root.path().join("gone"))));

    done_tx.send(()).unwrap();
    holder.join().unwrap();
}

#[test]
fn rollback_keeps_preexisting_branch() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", BOOM);
    git(&fx.repo, &["branch", "berth/a"]);
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(branch_exists(&fx.repo, "berth/a"));
    assert!(!wt(&fx, "a").exists());
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
}

#[test]
fn rollback_keeps_preexisting_root_dir() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", BOOM);
    let dir = wt(&fx, "a").parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        dir.is_dir(),
        "rollback removed a directory berth did not create"
    );
    assert!(!wt(&fx, "a").exists());
}

#[test]
fn rollback_removes_root_dir_berth_created() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", BOOM);
    let dir = wt(&fx, "a").parent().unwrap().to_path_buf();
    assert_eq!(code(&up(&fx, &["a"])), 3);
    assert!(!dir.exists());
}

#[test]
fn worktree_add_failure_releases_ports_and_entry() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    // `berth/a` is checked out in the main checkout, so `git worktree add` refuses it.
    git(&fx.repo, &["checkout", "-q", "-b", "berth/a"]);
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(reg_blocks(&fx).is_empty(), "{:?}", reg_blocks(&fx));
    assert!(manifest_entries(&fx).is_empty());
    assert!(!wt(&fx, "a").exists());
    assert!(branch_exists(&fx.repo, "berth/a"));
    assert!(
        !wt(&fx, "a").parent().unwrap().exists(),
        "root dir left behind"
    );
}

#[test]
fn keep_on_failure_copy_refusal_records_skipped() {
    let fx = Fixture::new();
    fx.config(
        &fx.repo,
        "",
        "[copy]\ninclude = [\".env\"]\n[setup]\nrun = [\"true\"]",
    );
    write(&fx.repo, ".env", "T=1\n");
    // A tracked `.berth` symlink makes the staging directory pass through a symlink.
    std::os::unix::fs::symlink(fx.tmp.join("elsewhere"), fx.repo.join(".berth")).unwrap();
    git(&fx.repo, &["add", "-f", ".berth"]);
    git(&fx.repo, &["commit", "-q", "-m", "track .berth"]);
    let out = up(&fx, &["a", "--keep-on-failure", "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let entries = manifest_entries(&fx);
    assert_eq!(entries[0]["state"], "failed");
    assert_eq!(entries[0]["setup"]["status"], "skipped");
}
