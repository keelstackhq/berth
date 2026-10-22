mod common;

use common::{Fixture, git, write};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

fn canon(p: &Path) -> PathBuf {
    berth::sys::canonicalize(p).expect("canonicalize")
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    canon(fx.root.path()).join(name).join(task)
}

fn berth(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn up(fx: &Fixture, task: &str) {
    let out = berth(fx, &["up", task]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

fn commit(fx: &Fixture, task: &str, file: &str) {
    let dir = wt(fx, task);
    write(&dir, file, &format!("{task} {file}\n"));
    git(&dir, &["add", file]);
    git(&dir, &["commit", "-q", "-m", &format!("{task} {file}")]);
}

fn branch_exists(fx: &Fixture, name: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .current_dir(&fx.repo)
        .status()
        .unwrap()
        .success()
}

fn manifest_tasks(fx: &Fixture) -> Vec<String> {
    match fs::read(fx.repo.join(".agents/worktrees.json")) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["worktrees"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["task"].as_str().unwrap().to_string())
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn reg_blocks(fx: &Fixture) -> usize {
    match fs::read(fx.state.join("ports.json")) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["blocks"].as_array().unwrap().len()
        }
        Err(_) => 0,
    }
}

fn item<'a>(v: &'a Value, task: &str) -> &'a Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "worktree" && i["key"] == task)
        .unwrap_or_else(|| panic!("no worktree item for {task}: {v}"))
}

fn squash_supported() -> bool {
    berth::git::version().expect("git version") >= (2, 38)
}

/// merged (ff), squashed (2 commits, squash-merged), open (unmerged), fresh (no commits),
/// dirty (merged but an untracked file), broken (merged but state `failed`).
fn world() -> Fixture {
    let fx = Fixture::new();
    let log = fx.root.path().join("teardown.log");
    fx.config(
        &fx.repo,
        "",
        &format!(
            "[teardown]\nrun = ['echo \"$PWD\" >> \"{}\"']",
            common::sh_path(&log)
        ),
    );
    for t in ["merged", "squashed", "open", "fresh", "dirty", "broken"] {
        up(&fx, t);
    }
    commit(&fx, "merged", "m.txt");
    git(&fx.repo, &["merge", "-q", "--ff-only", "berth/merged"]);
    commit(&fx, "squashed", "s1.txt");
    commit(&fx, "squashed", "s2.txt");
    git(&fx.repo, &["merge", "-q", "--squash", "berth/squashed"]);
    git(&fx.repo, &["commit", "-q", "-m", "squash squashed"]);
    commit(&fx, "open", "o.txt");
    commit(&fx, "dirty", "d.txt");
    git(&fx.repo, &["merge", "-q", "--no-edit", "berth/dirty"]);
    write(&wt(&fx, "dirty"), "junk.txt", "untracked\n");
    commit(&fx, "broken", "b.txt");
    git(&fx.repo, &["merge", "-q", "--no-edit", "berth/broken"]);
    let path = fx.repo.join(".agents/worktrees.json");
    let mut v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for e in v["worktrees"].as_array_mut().unwrap() {
        if e["task"] == "broken" {
            e["state"] = "failed".into();
        }
    }
    fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    fx
}

#[test]
fn dry_run_reports_and_changes_nothing() {
    let fx = world();
    let before = manifest_tasks(&fx);
    let blocks = reg_blocks(&fx);
    let out = berth(&fx, &["gc", "--dry-run", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["command"], "gc");
    assert_eq!(item(&v, "merged")["action"], "ok");
    assert_eq!(item(&v, "merged")["reason"], "would remove");
    if squash_supported() {
        assert_eq!(item(&v, "squashed")["reason"], "would remove");
    } else {
        assert_eq!(item(&v, "squashed")["action"], "skipped");
    }
    let kept = |t: &str, why: &str| {
        let i = item(&v, t);
        assert_eq!(i["action"], "skipped", "{t}");
        assert_eq!(i["reason"], why, "{t}");
    };
    kept("open", "kept: not merged into HEAD");
    kept("fresh", "kept: no commits yet");
    kept("dirty", "kept: uncommitted changes");
    kept("broken", "kept: state failed");
    let removable = if squash_supported() { 2 } else { 1 };
    assert_eq!(v["worktrees"].as_array().unwrap().len(), removable);
    assert_eq!(manifest_tasks(&fx), before);
    assert_eq!(reg_blocks(&fx), blocks);
    for t in ["merged", "squashed", "open", "fresh", "dirty", "broken"] {
        assert!(wt(&fx, t).is_dir(), "{t}");
    }
    assert!(!fx.root.path().join("teardown.log").exists());
}

#[test]
fn gc_removes_merged_and_keeps_branches() {
    let fx = world();
    let out = berth(&fx, &["gc", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    let mut gone = vec!["merged"];
    if squash_supported() {
        gone.push("squashed");
    }
    for t in &gone {
        assert!(!wt(&fx, t).exists(), "{t}");
        assert!(!manifest_tasks(&fx).contains(&t.to_string()), "{t}");
        assert!(branch_exists(&fx, &format!("berth/{t}")), "{t}");
    }
    assert_eq!(v["worktrees"].as_array().unwrap().len(), gone.len());
    let kept: Vec<&str> = ["open", "fresh", "dirty", "broken"].into();
    for t in &kept {
        assert!(wt(&fx, t).is_dir(), "{t}");
        assert!(manifest_tasks(&fx).contains(&t.to_string()), "{t}");
    }
    assert_eq!(reg_blocks(&fx), kept.len() + (2 - gone.len()));
    let log = fs::read_to_string(fx.root.path().join("teardown.log")).unwrap();
    assert_eq!(log.lines().count(), gone.len(), "{log}");
    assert!(log.contains("merged"));
}

#[test]
fn gc_delete_branch_deletes_removed_branches() {
    let fx = world();
    let out = berth(&fx, &["gc", "--delete-branch"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!branch_exists(&fx, "berth/merged"));
    if squash_supported() {
        assert!(!branch_exists(&fx, "berth/squashed"));
    }
    for t in ["open", "fresh", "dirty", "broken"] {
        assert!(branch_exists(&fx, &format!("berth/{t}")), "{t}");
        assert!(wt(&fx, t).is_dir(), "{t}");
    }
}

#[test]
fn gc_without_entries_says_nothing_to_collect() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let out = berth(&fx, &["gc", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "gc");
    assert_eq!(items[0]["reason"], "nothing to collect");
}

#[test]
fn gc_keeps_a_missing_worktree() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    up(&fx, "gone");
    commit(&fx, "gone", "g.txt");
    git(&fx.repo, &["merge", "-q", "--ff-only", "berth/gone"]);
    fs::remove_dir_all(wt(&fx, "gone")).unwrap();
    let out = berth(&fx, &["gc", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(item(&v, "gone")["reason"], "kept: worktree missing");
    assert_eq!(manifest_tasks(&fx), vec!["gone"]);
}

#[test]
fn gc_dry_run_json_snapshot() {
    let fx = world();
    let out = berth(&fx, &["gc", "--dry-run", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let mut v = json(&out);
    if !squash_supported() {
        return;
    }
    let items = v["items"].take();
    insta::assert_json_snapshot!("gc_dry_run_items", items);
}

#[test]
fn gc_usage_error_json_names_the_command() {
    let fx = Fixture::new();
    let out = berth(&fx, &["gc", "--bogus", "--json"]);
    assert_eq!(code(&out), 2);
    let v = json(&out);
    assert_eq!(v["command"], "gc");
    assert_eq!(v["exit_code"], 2);
}

#[test]
fn a_failing_removal_is_an_error_item_and_the_rest_still_run() {
    let fx = Fixture::new();
    fx.config(
        &fx.repo,
        "",
        "[teardown]\nrun = ['case \"$PWD\" in *bad) exit 1;; esac']",
    );
    for t in ["bad", "good"] {
        up(&fx, t);
        commit(&fx, t, "f.txt");
        git(
            &fx.repo,
            &["merge", "-q", "--no-edit", &format!("berth/{t}")],
        );
    }
    let out = berth(&fx, &["gc", "--json"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    let v = json(&out);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "error" && i["key"] == "bad")
    );
    assert!(!wt(&fx, "good").exists());
    assert!(wt(&fx, "bad").is_dir());
    assert_eq!(manifest_tasks(&fx), vec!["bad"]);
    assert_eq!(v["worktrees"].as_array().unwrap().len(), 1);
}
