mod common;

use berth::registry::{Block, Registry};
use common::{Fixture, git, write};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

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

fn up(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().arg("up").args(args).output().expect("run berth")
}

fn down(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth()
        .arg("down")
        .args(args)
        .output()
        .expect("run berth")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn manifest_entries(fx: &Fixture) -> Vec<Value> {
    match fs::read(fx.repo.join(".agents/worktrees.json")) {
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

fn worktree_count(fx: &Fixture) -> usize {
    git(&fx.repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|l| l.starts_with("worktree "))
        .count()
}

fn find<'a>(v: &'a Value, kind: &str) -> Option<&'a Value> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == kind)
}

fn leftover_tmp_files(dir: &Path) -> Vec<String> {
    match fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn assert_all_intact(fx: &Fixture, task: &str) {
    assert!(wt(fx, task).is_dir(), "worktree removed");
    assert_eq!(reg_blocks(fx).len(), 1);
    assert_eq!(manifest_entries(fx).len(), 1);
    assert_eq!(manifest_entries(fx)[0]["state"], "ready");
    assert!(branch_exists(&fx.repo, &format!("berth/{task}")));
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

#[test]
fn up_down_restores_state() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    assert!(wt(&fx, "a").is_dir());
    let out = down(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["command"], "down");
    for kind in ["worktree", "ports"] {
        assert_eq!(find(&v, kind).unwrap()["action"], "remove", "{v}");
    }
    assert!(find(&v, "branch").is_none(), "{v}");
    assert!(!wt(&fx, "a").exists());
    assert!(!base_dir(&fx).exists(), "worktree root dir left behind");
    assert_eq!(worktree_count(&fx), 1);
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
    assert!(branch_exists(&fx.repo, "berth/a"));
    assert!(leftover_tmp_files(&fx.repo.join(".agents")).is_empty());
    assert!(leftover_tmp_files(&fx.state).is_empty());
}

#[test]
fn dirty_down_refused_then_force() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    write(&wt(&fx, "a"), "x.txt", "work");
    let out = down(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["items"][0]["action"], "refused");
    assert!(
        v["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("uncommitted changes; commit them or pass --force"),
        "{v}"
    );
    assert_all_intact(&fx, "a");
    assert!(wt(&fx, "a").join("x.txt").is_file());

    let out = down(&fx, &["a", "--force"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!wt(&fx, "a").exists());
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
}

#[test]
fn copied_env_does_not_block() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[copy]\ninclude = [\".env\"]");
    write(&fx.repo, ".env", "TOKEN=1\n");
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(wt(&fx, "a").join(".env").is_file());
    let out = down(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!wt(&fx, "a").exists());
}

#[test]
fn delete_branch_only_when_merged() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let path = wt(&fx, "a");
    write(&path, "work.txt", "w");
    git(&path, &["add", "work.txt"]);
    git(&path, &["commit", "-q", "-m", "work"]);

    let out = down(&fx, &["a", "--delete-branch", "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["items"][0]["action"], "refused");
    assert!(
        v["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("branch has unmerged commits; merge it or delete it with git branch -D"),
        "{v}"
    );
    assert_all_intact(&fx, "a");

    git(&fx.repo, &["merge", "--ff-only", "berth/a"]);
    let out = down(&fx, &["a", "--delete-branch", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(find(&json(&out), "branch").unwrap()["action"], "remove");
    assert!(!branch_exists(&fx.repo, "berth/a"));
    assert!(!path.exists());
}

#[test]
fn deleted_dir_is_cleaned() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[teardown]\nrun = [\"true\"]");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    fs::remove_dir_all(wt(&fx, "a")).unwrap();
    let out = down(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(find(&json(&out), "teardown").unwrap()["action"], "skipped");
    assert_eq!(worktree_count(&fx), 1);
    assert!(
        !git(&fx.repo, &["worktree", "list", "--porcelain"]).contains("a\n"),
        "stale worktree record"
    );
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
    let out = up(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn unknown_task_is_ok() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let out = down(&fx, &["nope", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["items"][0]["action"], "ok");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

#[test]
fn unknown_task_frees_orphan_block() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let mut reg = Registry::load(&fx.state).unwrap();
    reg.add(Block {
        start: fx.range.0,
        end: fx.range.0 + 9,
        repo: canon(&fx.repo),
        task: "a".into(),
        created_at: "2026-10-17T00:00:00Z".into(),
    });
    reg.save(&fx.state).unwrap();
    let out = down(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(find(&json(&out), "ports").unwrap()["action"], "remove");
    assert!(reg_blocks(&fx).is_empty());
}

#[test]
fn down_during_up_refused() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let mut child = spawn_paused(&fx, "after_copy", "a");
    let out = down(&fx, &["a", "--json"]);
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
    assert!(wt(&fx, "a").is_dir());
    assert_eq!(reg_blocks(&fx).len(), 1);
    assert_eq!(manifest_entries(&fx)[0]["state"], "creating");

    child.kill().unwrap();
    child.wait().unwrap();
    let out = down(&fx, &["a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!wt(&fx, "a").exists());
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
}

#[test]
fn teardown_failure_exits_3_and_keeps_entry() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[teardown]\nrun = [\"exit 5\"]");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let out = down(&fx, &["a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    let entries = manifest_entries(&fx);
    assert_eq!(entries[0]["state"], "removing");
    assert!(wt(&fx, "a").is_dir());
    assert_eq!(reg_blocks(&fx).len(), 1);

    let out = down(&fx, &["a", "--force", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    let t = find(&v, "teardown").unwrap();
    assert_eq!(t["action"], "drift");
    assert_eq!(t["reason"], "teardown failed (ignored by --force)");
    assert!(!wt(&fx, "a").exists());
    assert!(reg_blocks(&fx).is_empty());
    assert!(manifest_entries(&fx).is_empty());
}

#[test]
fn unregistered_directory_is_left_alone() {
    // A `removing` entry whose directory is not a git worktree: berth did not create it.
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&up(&fx, &["a"])), 0);
    let path = wt(&fx, "a");
    git(
        &fx.repo,
        &["worktree", "remove", "--force", path.to_str().unwrap()],
    );
    fs::create_dir_all(&path).unwrap();
    write(&path, "precious.txt", "mine");
    let out = down(&fx, &["a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(find(&json(&out), "worktree").unwrap()["action"], "skipped");
    assert!(path.join("precious.txt").is_file());
    assert!(manifest_entries(&fx).is_empty());
    assert!(reg_blocks(&fx).is_empty());
}
