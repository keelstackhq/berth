mod common;

use common::{Fixture, git, write};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Output;

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).expect("canonicalize")
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    canon(fx.root.path()).join(name).join(task)
}

fn run(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("stdout is not one JSON object ({e}): {}", stdout(out));
    })
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn items(out: &Output, kind: &str) -> Vec<Value> {
    json(out)["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == kind)
        .cloned()
        .collect()
}

/// A repo whose committed `.gitignore` ignores `.turbo/`, with a seedable cache in main.
fn seeded(mode: &str) -> Fixture {
    let fx = Fixture::new();
    write(&fx.repo, ".gitignore", ".env\n.turbo/\n");
    git(&fx.repo, &["add", ".gitignore"]);
    git(&fx.repo, &["commit", "-q", "-m", "ignore turbo"]);
    let cache = fx.repo.join(".turbo/cache");
    write(&cache, "h1.tar.zst", "archive one");
    write(&cache, "h1-meta.json", "{\"hash\":\"h1\"}");
    write(&cache, "h2.tar.zst", "archive two");
    write(&cache, "x.tmp", "partial");
    write(&cache, "sub/deep.tar.zst", "nested");
    if !mode.is_empty() {
        fx.config(&fx.repo, "", &format!("[cache]\nturbo_seed = \"{mode}\""));
    }
    fx
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn link_mode_shares_inodes_and_skips_tmp_and_subdirs() {
    let fx = seeded("link");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache.len(), 1, "{cache:?}");
    assert_eq!(cache[0]["action"], "create");
    assert_eq!(cache[0]["reason"], "seeded 3 entries (3 linked, 0 copied)");
    let seeded_dir = wt(&fx, "a").join(".turbo/cache");
    assert_eq!(
        names(&seeded_dir),
        ["h1-meta.json", "h1.tar.zst", "h2.tar.zst"]
    );
    for name in ["h1.tar.zst", "h1-meta.json", "h2.tar.zst"] {
        let a = fs::metadata(fx.repo.join(".turbo/cache").join(name)).unwrap();
        let b = fs::metadata(seeded_dir.join(name)).unwrap();
        assert_eq!(a.ino(), b.ino(), "{name} should be hard linked");
    }
}

#[test]
fn turbo_style_rewrite_in_worktree_leaves_main_untouched() {
    let fx = seeded("link");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let dir = wt(&fx, "a").join(".turbo/cache");
    // turbo writes a temp file and renames it over the entry.
    fs::write(dir.join("h1.tar.zst.tmp"), "rewritten").unwrap();
    fs::rename(dir.join("h1.tar.zst.tmp"), dir.join("h1.tar.zst")).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("h1.tar.zst")).unwrap(),
        "rewritten"
    );
    assert_eq!(
        fs::read_to_string(fx.repo.join(".turbo/cache/h1.tar.zst")).unwrap(),
        "archive one"
    );
}

#[test]
fn copy_mode_copies_bytes_with_new_inodes() {
    let fx = seeded("copy");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache[0]["action"], "create");
    assert_eq!(cache[0]["reason"], "seeded 3 entries (0 linked, 3 copied)");
    let seeded_dir = wt(&fx, "a").join(".turbo/cache");
    assert_eq!(
        names(&seeded_dir),
        ["h1-meta.json", "h1.tar.zst", "h2.tar.zst"]
    );
    for name in ["h1.tar.zst", "h1-meta.json", "h2.tar.zst"] {
        let src = fx.repo.join(".turbo/cache").join(name);
        let dst = seeded_dir.join(name);
        assert_ne!(
            fs::metadata(&src).unwrap().ino(),
            fs::metadata(&dst).unwrap().ino()
        );
        assert_eq!(fs::read(&src).unwrap(), fs::read(&dst).unwrap());
    }
}

#[test]
fn turbo_not_ignored_is_skipped_and_leaves_no_directory() {
    let fx = seeded("link");
    // Undo the ignore rule in the committed .gitignore and in main.
    write(&fx.repo, ".gitignore", ".env\n");
    git(&fx.repo, &["add", ".gitignore"]);
    git(&fx.repo, &["commit", "-q", "-m", "unignore"]);
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache[0]["action"], "skipped");
    assert_eq!(
        cache[0]["reason"],
        ".turbo/ is not gitignored; add it to .gitignore"
    );
    assert!(!wt(&fx, "a").join(".turbo").exists());
}

#[test]
fn missing_source_is_skipped() {
    let fx = seeded("link");
    fs::remove_dir_all(fx.repo.join(".turbo")).unwrap();
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache[0]["action"], "skipped");
    assert_eq!(cache[0]["reason"], "no .turbo/cache in the main checkout");
    assert!(!wt(&fx, "a").join(".turbo").exists());
}

#[test]
fn default_config_has_no_cache_item_and_no_turbo_dir() {
    let fx = seeded("");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(items(&out, "cache").is_empty());
    assert!(!wt(&fx, "a").join(".turbo").exists());
}

#[test]
fn existing_targets_are_kept() {
    let fx = seeded("copy");
    // A file the worktree already has (tracked) must not be replaced.
    write(&fx.repo, ".turbo/cache/h1.tar.zst", "archive one");
    git(&fx.repo, &["add", "-f", ".turbo/cache/h1.tar.zst"]);
    git(&fx.repo, &["commit", "-q", "-m", "track one entry"]);
    write(&fx.repo, ".turbo/cache/h1.tar.zst", "changed in main");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let dir = wt(&fx, "a").join(".turbo/cache");
    assert_eq!(
        fs::read_to_string(dir.join("h1.tar.zst")).unwrap(),
        "archive one"
    );
    assert_eq!(
        items(&out, "cache")[0]["reason"],
        "seeded 2 entries (0 linked, 2 copied)"
    );
}

#[test]
fn down_after_seeding_is_not_dirty() {
    let fx = seeded("link");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let out = run(&fx, &["down", "a"]);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    assert!(!wt(&fx, "a").exists());
}
