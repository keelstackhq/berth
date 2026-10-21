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
    up(&fx, "a");
    // Add a stale block of another repo after `up` (which prunes stale blocks), in a slot that
    // `a` does not hold.
    let (lo, _) = fx.range;
    let ports = fx.state.join("ports.json");
    let mut reg: Value = serde_json::from_slice(&fs::read(&ports).unwrap()).unwrap();
    let taken = reg["blocks"][0]["start"].as_u64().unwrap();
    let start = if taken == u64::from(lo) { lo + 10 } else { lo };
    reg["blocks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "start": start, "end": start + 9, "repo": other.display().to_string(),
            "task": "t", "created_at": "2026-10-17T00:00:00Z"
        }));
    fs::write(&ports, serde_json::to_vec_pretty(&reg).unwrap()).unwrap();
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

#[test]
fn creating_entry_on_another_host_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "a");
    mark_creating_live(&fx);
    let path = fx.repo.join(".agents/worktrees.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["worktrees"][0]["owner"]["host"] = "elsewhere.invalid".into();
    fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    let owner = items(&v, "owner");
    assert!(
        owner.iter().any(|i| i["action"] == "drift"
            && i["reason"]
                .as_str()
                .unwrap()
                .contains("owner on another host (elsewhere.invalid); cannot verify")
            && i["reason"]
                .as_str()
                .unwrap()
                .contains("berth down --force a")),
        "{v}"
    );
}

fn ignore(fx: &Fixture, line: &str) {
    let path = fx.repo.join(".git/info/exclude");
    let old = fs::read_to_string(&path).unwrap_or_default();
    fs::write(path, format!("{old}{line}\n")).unwrap();
}

fn find<'a>(v: &'a Value, kind: &str, key: &str) -> &'a Value {
    items(v, kind)
        .into_iter()
        .find(|i| i["key"] == key)
        .unwrap_or_else(|| panic!("no {kind} {key} item: {v}"))
}

fn fake_db_tools(fx: &Fixture) -> PathBuf {
    let log = fx.root.path().join("tools.log");
    for tool in ["createdb", "dropdb", "docker"] {
        fx.fake_bin(tool, &format!("echo {tool} >> '{}'", log.display()));
    }
    log
}

fn pg(fx: &Fixture) {
    fx.config(&fx.repo, "", "[db]\nprovider = \"postgres\"");
}

#[test]
fn no_db_or_cache_config_adds_no_items() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert!(items(&v, "db").is_empty());
    assert!(items(&v, "cache").is_empty());
}

#[test]
fn db_tools_present_are_ok_and_never_run() {
    let fx = Fixture::new();
    let log = fake_db_tools(&fx);
    pg(&fx);
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert_eq!(find(&v, "db", "tools")["action"], "ok");
    assert!(!log.exists(), "doctor ran a database tool");
}

#[test]
fn missing_db_tools_are_drift_with_the_hint() {
    let fx = Fixture::new();
    pg(&fx);
    let out = fx
        .berth()
        .env("PATH", fx.isolated_path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tools = find(&v, "db", "tools");
    assert_eq!(tools["action"], "drift");
    let reason = tools["reason"].as_str().unwrap();
    assert!(reason.contains("createdb not found on PATH"), "{reason}");
    assert!(
        reason.contains("install the PostgreSQL client tools"),
        "{reason}"
    );
}

#[test]
fn compose_without_docker_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[db]\nprovider = \"compose\"");
    let out = fx
        .berth()
        .env("PATH", fx.isolated_path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(find(&v, "db", "tools")["action"], "drift");
    assert!(
        find(&v, "db", "tools")["reason"]
            .as_str()
            .unwrap()
            .contains("docker not found")
    );
}

fn sqlite(fx: &Fixture) {
    write(&fx.repo, "db/dev.sqlite", "SQLITE-TEMPLATE");
    ignore(fx, "/db/dev.sqlite");
    fx.config(
        &fx.repo,
        "",
        "[db]\nprovider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"",
    );
}

#[test]
fn sqlite_healthy_is_ok() {
    let fx = Fixture::new();
    sqlite(&fx);
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert_eq!(find(&v, "db", "sqlite")["action"], "ok");
    assert!(items(&v, "db").iter().all(|i| i["key"] != "tools"));
}

#[test]
fn sqlite_problems_are_drift_not_errors() {
    // Missing template.
    let fx = Fixture::new();
    sqlite(&fx);
    fs::remove_file(fx.repo.join("db/dev.sqlite")).unwrap();
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert_eq!(find(&v, "db", "sqlite")["action"], "drift");
    assert!(
        find(&v, "db", "sqlite")["reason"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );
    // Non-empty -wal.
    let fx = Fixture::new();
    sqlite(&fx);
    write(&fx.repo, "db/dev.sqlite-wal", "pending");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    assert!(
        find(&v, "db", "sqlite")["reason"]
            .as_str()
            .unwrap()
            .contains("-wal")
    );
    // Target not ignored.
    let fx = Fixture::new();
    write(&fx.repo, "db/dev.sqlite", "SQLITE-TEMPLATE");
    fx.config(
        &fx.repo,
        "",
        "[db]\nprovider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"",
    );
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    let item = find(&v, "db", "sqlite");
    assert_eq!(item["action"], "drift");
    assert!(item["reason"].as_str().unwrap().contains(".gitignore"));
}

fn set_created(fx: &Fixture, created: bool) {
    let path = fx.repo.join(".agents/worktrees.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["worktrees"][0]["db"]["created"] = created.into();
    fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

#[test]
fn postgres_entry_not_recorded_as_created_is_drift() {
    let fx = Fixture::new();
    let log = fake_db_tools(&fx);
    pg(&fx);
    up(&fx, "a");
    let before = fs::read_to_string(&log).unwrap();
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert!(items(&v, "db").iter().all(|i| i["action"] == "ok"), "{v}");
    set_created(&fx, false);
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    let item = find(&v, "db", &fx.pg_name("a"));
    assert_eq!(item["action"], "drift");
    assert_eq!(
        item["reason"],
        format!(
            "database {} was not recorded as created; berth down will leave it",
            fx.pg_name("a")
        )
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        before,
        "doctor ran a tool"
    );
}

#[test]
fn turbo_seed_without_ignore_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[cache]\nturbo_seed = \"link\"");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 1, "{v}");
    let item = find(&v, "cache", "turbo");
    assert_eq!(item["action"], "drift");
    assert!(item["reason"].as_str().unwrap().contains(".turbo/"));
    ignore(&fx, ".turbo/");
    let (code, v) = doctor(&fx);
    assert_eq!(code, 0, "{v}");
    assert_eq!(find(&v, "cache", "turbo")["action"], "ok");
}

#[test]
fn hoard_server_found_is_ok_and_absent_is_skipped_without_warning() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[cache]\nremote = \"hoard\"");
    ignore(&fx, ".hoard/");
    let out = berth(&fx, &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("warning"), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let item = find(&v, "cache", "hoard");
    assert_eq!(item["action"], "skipped");
    assert_eq!(item["reason"], "no running hoard server");

    write(&fx.repo, ".hoard/port", "41234");
    write(&fx.repo, ".hoard/token", "BERTH_CANARY_hoard_7c1f");
    let out = berth(&fx, &["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &stderr(&out);
    assert!(!text.contains("BERTH_CANARY"), "token leaked");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let item = find(&v, "cache", "hoard");
    assert_eq!(item["action"], "ok");
    assert_eq!(item["reason"], "http://127.0.0.1:41234");
}
