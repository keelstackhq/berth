mod common;

use berth::lock;
use common::{Fixture, git};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

const SETUP_SLOW: &str = "[worktree]\nmax = 10\n[setup]\nrun = [\"sleep 0.2\"]";

fn spawn_up(fx: &Fixture, repo: &Path, task: &str) -> Child {
    fx.berth_std()
        .current_dir(repo)
        .args(["up", task, "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn berth")
}

fn finish(children: Vec<Child>) {
    for child in children {
        let out = child.wait_with_output().expect("wait");
        assert_eq!(
            out.status.code(),
            Some(0),
            "up failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn manifest(repo: &Path) -> Value {
    let bytes = fs::read(repo.join(".agents/worktrees.json")).expect("manifest");
    serde_json::from_slice(&bytes).expect("manifest is valid JSON")
}

fn blocks(m: &Value) -> Vec<(u64, u64)> {
    m["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["ports"]["start"].as_u64().unwrap(),
                e["ports"]["end"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn assert_disjoint(all: &[(u64, u64)]) {
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert!(a.1 < b.0 || b.1 < a.0, "overlapping blocks {a:?} {b:?}");
        }
    }
}

fn tmp_leftovers(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_name().to_string_lossy().contains(".tmp.") {
            found.push(p.clone());
        }
        if p.is_dir() && !fs::symlink_metadata(&p).unwrap().is_symlink() {
            tmp_leftovers(&p, found);
        }
    }
}

#[test]
fn eight_ups_one_repo() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", SETUP_SLOW);
    let tasks: Vec<String> = (1..=8).map(|i| format!("t{i}")).collect();
    let children: Vec<Child> = tasks.iter().map(|t| spawn_up(&fx, &fx.repo, t)).collect();
    finish(children);

    let m = manifest(&fx.repo);
    let entries = m["worktrees"].as_array().unwrap();
    assert_eq!(entries.len(), 8);
    assert!(entries.iter().all(|e| e["state"] == "ready"), "{m}");
    assert_disjoint(&blocks(&m));
    let listed = git(&fx.repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        listed
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        9
    );

    let mut found = Vec::new();
    tmp_leftovers(&fx.repo.join(".agents"), &mut found);
    tmp_leftovers(&fx.state, &mut found);
    for e in entries {
        let staging = PathBuf::from(e["path"].as_str().unwrap()).join(".berth/tmp");
        let files: Vec<_> = fs::read_dir(&staging)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(files.is_empty(), "files left in {staging:?}: {files:?}");
    }
    assert!(found.is_empty(), "temp files left: {found:?}");
}

#[test]
fn four_ups_two_repos() {
    let fx = Fixture::new();
    let two = fx.add_repo("two");
    fx.config(&fx.repo, "", SETUP_SLOW);
    fx.config(&two, "", SETUP_SLOW);
    let mut children = Vec::new();
    for i in 1..=4 {
        children.push(spawn_up(&fx, &fx.repo, &format!("a{i}")));
        children.push(spawn_up(&fx, &two, &format!("b{i}")));
    }
    finish(children);

    let (ma, mb) = (manifest(&fx.repo), manifest(&two));
    let tasks = |m: &Value| -> BTreeSet<String> {
        m["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["task"].as_str().unwrap().to_string())
            .collect()
    };
    let want = |p: &str| -> BTreeSet<String> { (1..=4).map(|i| format!("{p}{i}")).collect() };
    assert_eq!(tasks(&ma), want("a"));
    assert_eq!(tasks(&mb), want("b"));
    let mut all = blocks(&ma);
    all.extend(blocks(&mb));
    assert_eq!(all.len(), 8);
    assert_disjoint(&all);
    let (lo, hi) = (u64::from(fx.range.0), u64::from(fx.range.1));
    assert!(all.iter().all(|b| b.0 >= lo && b.1 <= hi), "{all:?}");
}

#[test]
fn killed_up_recovers() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let mut child = fx
        .berth_std()
        .args(["up", "a"])
        .env("BERTH_TEST_PAUSE", "after_worktree")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let marker = fx.state.join("paused-after_worktree");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("up never reached after_worktree");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().expect("kill");
    child.wait().expect("reap");

    let ls = fx.berth().args(["ls", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&ls.stdout).unwrap();
    let drift = v["items"].as_array().unwrap().iter().any(|i| {
        i["action"] == "drift" && i["reason"].as_str().unwrap_or("").contains("interrupted")
    });
    assert!(drift, "ls shows no interrupted drift: {v}");
    let doctor = fx.berth().args(["doctor"]).output().unwrap();
    assert_eq!(doctor.status.code(), Some(1));
    let down = fx.berth().args(["down", "a"]).output().unwrap();
    assert_eq!(
        down.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&down.stderr)
    );
    let up = fx.berth().args(["up", "a"]).output().unwrap();
    assert_eq!(
        up.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&up.stderr)
    );
}

#[test]
fn lock_timeout_names_holder() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let common = fs::canonicalize(fx.repo.join(".git")).unwrap();
    let _guard = lock::acquire(&lock::repo_lock(&common), Duration::from_secs(5)).unwrap();
    let out = fx
        .berth()
        .args(["up", "a"])
        .env("BERTH_LOCK_TIMEOUT_MS", "300")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("pid={}", std::process::id())),
        "{err}"
    );
}

#[test]
fn lock_timeout_after_setup_says_run_down() {
    let fx = Fixture::new();
    let marker = fx.root.path().join("setup-started");
    let setup = format!(
        "[setup]\nrun = [\"touch '{}'; sleep 1\"]",
        common::sh_path(&marker)
    );
    fx.config(&fx.repo, "", &setup);
    let child = fx
        .berth_std()
        .current_dir(&fx.repo)
        .args(["up", "a"])
        .env("BERTH_LOCK_TIMEOUT_MS", "300")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "setup never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let common = fs::canonicalize(fx.repo.join(".git")).unwrap();
    let _guard = lock::acquire(&lock::repo_lock(&common), Duration::from_secs(5)).unwrap();
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.contains("berth down a"), "{err}");
}
