mod common;

use common::Fixture;
use predicates::prelude::*;

#[test]
fn version_prints_name() {
    let fx = Fixture::new();
    fx.berth()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("berth"));
}

#[test]
fn unknown_command_exits_2() {
    let fx = Fixture::new();
    fx.berth()
        .arg("nope")
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty());
}

#[test]
fn bad_task_slug_exits_2() {
    let fx = Fixture::new();
    let long = "a".repeat(49);
    for args in [vec!["up", "Bad_Task"], vec!["up", "-x"], vec!["up", &long]] {
        fx.berth().args(&args).assert().code(2);
    }
}

#[test]
fn json_is_one_object() {
    let fx = Fixture::new();
    let out = fx
        .berth()
        .args(["ls", "--json", "-C"])
        .arg(&fx.repo)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&out).expect("stdout is one JSON value");
    let obj = value.as_object().expect("an object");
    assert_eq!(obj["version"], 1);
    assert_eq!(obj["command"], "ls");
    for key in ["ok", "exit_code", "items", "worktrees"] {
        assert!(obj.contains_key(key), "missing {key}");
    }
}

#[test]
fn fixture_is_hermetic() {
    let fx = Fixture::new();
    let cmd = fx.berth_std();
    let envs: std::collections::HashMap<_, _> = cmd
        .get_envs()
        .filter_map(|(k, v)| Some((k.to_owned(), v?.to_owned())))
        .collect();
    let root = fx.root.path();
    assert!(std::path::Path::new(&envs[std::ffi::OsStr::new("HOME")]).starts_with(root));
    assert!(std::path::Path::new(&envs[std::ffi::OsStr::new("BERTH_STATE_DIR")]).starts_with(root));
    let listing = common::git(&fx.repo, &["config", "--global", "--list"]);
    assert_eq!(listing.trim(), "");
}

fn usage_json(args: &[&str]) -> (serde_json::Value, String) {
    let fx = Fixture::new();
    let out = fx.berth().args(args).assert().code(2).get_output().clone();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object ({e}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (value, stderr)
}

#[test]
fn usage_error_with_json_prints_one_report() {
    let (v, stderr) = usage_json(&["up", "Bad Slug", "--json"]);
    assert_eq!(v["version"], 1);
    assert_eq!(v["command"], "up");
    assert_eq!(v["ok"], false);
    assert_eq!(v["exit_code"], 2);
    assert_eq!(v["items"][0]["kind"], "error");
    assert_eq!(v["items"][0]["key"], "usage");
    assert_eq!(v["items"][0]["action"], "refused");
    assert!(v["worktrees"].as_array().unwrap().is_empty(), "{v}");
    assert!(stderr.contains("Bad Slug"), "{stderr}");

    let (v, _) = usage_json(&["--json", "down"]);
    assert_eq!(v["command"], "down", "{v}");
}

#[test]
fn unknown_subcommand_with_json_prints_one_report() {
    let (v, stderr) = usage_json(&["nope", "--json"]);
    assert_eq!(v["command"], "unknown");
    assert_eq!(v["exit_code"], 2);
    assert_eq!(v["items"][0]["key"], "usage");
    assert!(stderr.contains("nope"), "{stderr}");
}
