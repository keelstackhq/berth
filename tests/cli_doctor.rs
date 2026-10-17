mod common;

use common::{Fixture, git, write};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).expect("canonicalize")
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    canon(fx.root.path()).join(name).join(task)
}

fn berth(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
}

fn up(fx: &Fixture, task: &str) {
    let out = berth(fx, &["up", task]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn doctor(fx: &Fixture) -> (i32, Value) {
    let out = berth(fx, &["doctor", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("not one JSON object ({e}): {}", stderr(&out)));
    (out.status.code().expect("exit code"), v)
}

fn items<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == kind)
        .collect()
}

fn assert_drift(v: &Value, kind: &str) {
    assert!(
        items(v, kind).iter().any(|i| i["action"] == "drift"),
        "no drift item of kind {kind}: {v}"
    );
}

fn tree(dir: &Path, out: &mut BTreeMap<PathBuf, std::time::SystemTime>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let meta = fs::symlink_metadata(&p).unwrap();
            out.insert(p.clone(), meta.modified().unwrap());
            if meta.is_dir() {
                tree(&p, out);
            }
        }
    }
}

fn snapshot(paths: &[&Path]) -> BTreeMap<PathBuf, std::time::SystemTime> {
    let mut out = BTreeMap::new();
    for p in paths {
        tree(p, &mut out);
    }
    out
}

#[test]
fn healthy_repo_exits_0() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["command"], "doctor");
    assert_eq!(items(&v, "git")[0]["action"], "ok");
    up(&fx, "a");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert_eq!(items(&v, "exclude")[0]["action"], "ok");
}

#[test]
fn deleted_worktree_dir_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    fs::remove_dir_all(wt(&fx, "a")).unwrap();
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "manifest");
}

#[test]
fn unmanaged_worktree_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let x = wt(&fx, "x");
    git(
        &fx.repo,
        &["worktree", "add", "-q", "-b", "hand", x.to_str().unwrap()],
    );
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "manifest");
}

#[test]
fn stale_registry_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let gone = fx.root.path().join("gone");
    let (lo, _) = fx.range;
    let reg = format!(
        r#"{{"version":1,"blocks":[{{"start":{lo},"end":{},"repo":"{}","task":"t","created_at":"2026-10-17T00:00:00Z"}}]}}"#,
        lo + 9,
        gone.display()
    );
    write(&fx.state, "ports.json", &reg);
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "registry");
    assert!(
        items(&v, "registry")[0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("stale:")
    );
}

#[test]
fn doctor_writes_nothing() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let other = fx.add_repo("other");
    let (lo, _) = fx.range;
    let reg = format!(
        r#"{{"version":1,"blocks":[{{"start":{lo},"end":{},"repo":"{}","task":"t","created_at":"2026-10-17T00:00:00Z"}}]}}"#,
        lo + 9,
        other.display()
    );
    write(&fx.state, "ports.json", &reg);
    up(&fx, "a");
    let git_a = fx.repo.join(".git");
    let git_b = other.join(".git");
    let before = snapshot(&[&git_a, &git_b, &fx.state, &fx.repo]);
    let (_, v) = doctor(&fx);
    assert_eq!(snapshot(&[&git_a, &git_b, &fx.state, &fx.repo]), before);
    assert!(!git_b.join("berth.lock").exists());
    // `up` made repo A's lock; the snapshot above proves doctor did not touch it.
    assert_drift(&v, "registry");
}

#[test]
fn dead_creating_owner_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    let path = fx.repo.join(".agents/worktrees.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["worktrees"][0]["state"] = "creating".into();
    m["worktrees"][0]["owner"]["pid"] = pid.into();
    fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "owner");
}

#[test]
fn root_inside_repo_not_ignored_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[worktree]\nroot = \".worktrees\"");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "root");
    assert!(
        items(&v, "root")[0]["reason"]
            .as_str()
            .unwrap()
            .contains("upward")
    );
}

#[test]
fn root_inside_repo_ignored_is_ok() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[worktree]\nroot = \".worktrees\"");
    write(&fx.repo, ".git/info/exclude", "/.worktrees/\n");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
}

#[test]
fn unignored_copy_pattern_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[copy]\ninclude = [\"notes.txt\"]");
    write(&fx.repo, "notes.txt", "x");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "copy");
}

#[test]
fn exclude_missing_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    write(&fx.repo, ".git/info/exclude", "# nothing\n");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "exclude");
}

#[test]
fn port_of_missing_worktree_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let out = berth(&fx, &["env", "a", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let port: u16 = v["worktrees"][0]["env"]["PORT"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    fs::remove_dir_all(wt(&fx, "a")).unwrap();
    let _listener = TcpListener::bind(("127.0.0.1", port)).expect("hold the port");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_drift(&v, "port");
}

/// Mark task `a` as `creating`, owned by this (live) test process.
fn mark_creating_live(fx: &Fixture) {
    let path = fx.repo.join(".agents/worktrees.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["worktrees"][0]["state"] = "creating".into();
    m["worktrees"][0]["owner"]["pid"] = std::process::id().into();
    fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
}

#[test]
fn live_creating_entry_is_in_progress_not_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    let out = berth(&fx, &["env", "a", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let port: u16 = v["worktrees"][0]["env"]["PORT"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    mark_creating_live(&fx);
    fs::remove_dir_all(wt(&fx, "a")).unwrap();
    let _listener = TcpListener::bind(("127.0.0.1", port)).expect("hold the port");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    let manifest = items(&v, "manifest");
    assert!(manifest.iter().any(|i| i["reason"] == "creating"), "{v}");
}
