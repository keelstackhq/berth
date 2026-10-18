mod common;

use berth::BerthError;
use berth::git;
use common::{Fixture, git as run_git, write};
use std::fs;
use std::path::Path;

fn canon(p: &Path) -> std::path::PathBuf {
    fs::canonicalize(p).unwrap()
}

#[test]
fn discover_from_subdir_and_worktree() {
    let fx = Fixture::new();
    write(&fx.repo, "src/a.txt", "x");
    let wt = fx.root.path().join("wt");
    run_git(
        &fx.repo,
        &["worktree", "add", "-q", "-b", "b1", wt.to_str().unwrap()],
    );
    write(&wt, "sub/b.txt", "y");
    let a = git::discover(&fx.repo.join("src")).unwrap();
    let b = git::discover(&wt.join("sub")).unwrap();
    assert_eq!(a.main, canon(&fx.repo));
    assert_eq!(a.main, b.main);
    assert_eq!(a.common_dir, b.common_dir);
    assert_eq!(a.name, "repo");
}

#[test]
fn discover_separate_git_dir() {
    let fx = Fixture::new();
    let root = fx.root.path();
    let sep = root.join("sep");
    let gd = root.join("gd");
    fs::create_dir_all(&sep).unwrap();
    run_git(
        root,
        &[
            "init",
            "-q",
            "-b",
            "main",
            "--separate-git-dir",
            gd.to_str().unwrap(),
            sep.to_str().unwrap(),
        ],
    );
    write(&sep, "f", "1");
    run_git(&sep, &["add", "f"]);
    run_git(&sep, &["commit", "-q", "-m", "c"]);
    let repo = git::discover(&sep).unwrap();
    assert_eq!(repo.main, canon(&sep));
    assert_eq!(repo.common_dir, canon(&gd));
}

#[test]
fn bare_repo_is_env_error() {
    let fx = Fixture::new();
    let bare = fx.root.path().join("bare.git");
    run_git(
        fx.root.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    match git::discover(&bare) {
        Err(BerthError::Env(m)) => assert!(m.contains("non-bare"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn not_a_repo_is_env_error() {
    let fx = Fixture::new();
    let plain = fx.root.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    match git::discover(&plain) {
        Err(BerthError::Env(m)) => assert!(m.contains("not inside a git repository"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn version_is_at_least_2_36() {
    let (maj, min) = git::check_version().unwrap();
    assert!((maj, min) >= (2, 36), "{maj}.{min}");
}

#[test]
fn git_missing_exits_3() {
    let fx = Fixture::new();
    let empty = fx.root.path().join("empty-path");
    fs::create_dir_all(&empty).unwrap();
    let out = fx.berth().env("PATH", &empty).arg("ls").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("git 2.36 or newer is required"), "{err}");
}

#[test]
fn old_git_exits_3() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new();
    let bin = fx.root.path().join("old-bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("git");
    fs::write(&script, "#!/bin/sh\necho 'git version 2.30.0'\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let out = fx.berth().env("PATH", &bin).arg("ls").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("found 2.30.0"), "{err}");
}

#[test]
fn unknown_base_names_ref() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    match git::resolve_commit(&main, "origin/main") {
        Err(BerthError::Env(m)) => assert!(m.contains("origin/main") && m.contains("git fetch")),
        other => panic!("{other:?}"),
    }
    match git::resolve_commit(&main, "--output=x") {
        Err(BerthError::Env(m)) => assert!(m.contains("--output=x") && m.contains("git fetch")),
        other => panic!("{other:?}"),
    }
    assert!(!main.join("x").exists());
    assert_eq!(git::resolve_commit(&main, "main").unwrap().len(), 40);
}

#[test]
fn branch_name_checked() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    for bad in ["-x", "a..b"] {
        assert!(matches!(
            git::check_branch_name(&main, bad),
            Err(BerthError::Usage(_))
        ));
    }
    git::check_branch_name(&main, "berth/a").unwrap();
}

#[test]
fn worktree_add_list_remove() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    let sha = git::resolve_commit(&main, "main").unwrap();
    let wt = canon(fx.root.path()).join("wt-a");
    assert!(!git::branch_exists(&main, "berth/a").unwrap());
    git::worktree_add(&main, &wt, "berth/a", Some(&sha)).unwrap();
    assert!(git::branch_exists(&main, "berth/a").unwrap());
    let list = git::worktree_list(&main).unwrap();
    let hit = list.iter().find(|w| w.path == wt).expect("listed");
    assert_eq!(hit.branch.as_deref(), Some("refs/heads/berth/a"));
    assert_eq!(hit.head, sha);
    assert_eq!(list[0].path, main);
    git::worktree_remove(&main, &wt, false).unwrap();
    assert!(
        git::worktree_list(&main)
            .unwrap()
            .iter()
            .all(|w| w.path != wt)
    );
    git::worktree_prune(&main).unwrap();
    // existing branch, no -b
    git::worktree_add(&main, &wt, "berth/a", None).unwrap();
    git::worktree_remove(&main, &wt, true).unwrap();
    git::delete_branch(&main, "berth/a").unwrap();
    assert!(!git::branch_exists(&main, "berth/a").unwrap());
}

#[test]
fn ahead_behind_order() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    run_git(&main, &["branch", "feat"]);
    for n in ["f1", "f2"] {
        run_git(&main, &["checkout", "-q", "feat"]);
        write(&main, n, n);
        run_git(&main, &["add", n]);
        run_git(&main, &["commit", "-q", "-m", n]);
    }
    run_git(&main, &["checkout", "-q", "main"]);
    write(&main, "m1", "m");
    run_git(&main, &["add", "m1"]);
    run_git(&main, &["commit", "-q", "-m", "m1"]);
    assert_eq!(git::ahead_behind(&main, "main", "feat").unwrap(), (2, 1));
    assert_eq!(git::commits_since(&main, "main", "feat").unwrap(), 2);
    assert!(!git::is_ancestor(&main, "feat", "main").unwrap());
    let fork = git::merge_base(&main, "main", "feat").unwrap();
    assert!(git::is_ancestor(&main, &fork, "feat").unwrap());
    assert_eq!(fork, git::resolve_commit(&main, "main~1").unwrap());
}

#[test]
fn ignored_returns_empty_when_none() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    let got = git::ignored(&main, &["a.txt".to_string()]).unwrap();
    assert!(got.is_empty());
}

#[test]
fn ignored_finds_env() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    let got = git::ignored(&main, &[".env".to_string(), "a.txt".to_string()]).unwrap();
    assert_eq!(
        got.into_iter().collect::<Vec<_>>(),
        vec![".env".to_string()]
    );
}

#[test]
fn ensure_excluded_is_idempotent() {
    let fx = Fixture::new();
    let repo = git::discover(&fx.repo).unwrap();
    assert!(git::ensure_excluded(&repo.common_dir, "/.agents/").unwrap());
    assert!(!git::ensure_excluded(&repo.common_dir, "/.agents/").unwrap());
    let text = fs::read_to_string(repo.common_dir.join("info/exclude")).unwrap();
    assert_eq!(text.lines().filter(|l| *l == "/.agents/").count(), 1);
}

#[test]
fn dirty_ignores_ignored_files() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    write(&main, ".env", "SECRET=1");
    assert!(!git::dirty(&main).unwrap());
    write(&main, "x.txt", "x");
    assert!(git::dirty(&main).unwrap());
}

#[test]
fn verbose_logs_git_to_stderr() {
    let fx = Fixture::new();
    let out = fx.berth().args(["-v", "ls"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("berth: git rev-parse"), "{err}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.lines().any(|l| l.starts_with("berth:")), "{stdout}");
}

#[test]
fn separate_git_dir_linked_worktree_fails_closed() {
    let fx = Fixture::new();
    let root = fx.root.path();
    let sep = root.join("sep2");
    let gd = root.join("gd2");
    fs::create_dir_all(&sep).unwrap();
    run_git(
        root,
        &[
            "init",
            "-q",
            "-b",
            "main",
            "--separate-git-dir",
            gd.to_str().unwrap(),
            sep.to_str().unwrap(),
        ],
    );
    write(&sep, "f", "1");
    run_git(&sep, &["add", "f"]);
    run_git(&sep, &["commit", "-q", "-m", "c"]);
    let wt = root.join("wt2");
    run_git(
        &sep,
        &["worktree", "add", "-q", "-b", "b2", wt.to_str().unwrap()],
    );
    match git::discover(&wt) {
        Err(e @ BerthError::Env(_)) => {
            assert_eq!(e.exit_code(), 3);
            assert!(
                e.to_string().contains("run berth from the main checkout"),
                "{e}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn not_a_repo_error_carries_git_stderr() {
    let fx = Fixture::new();
    let plain = fx.root.path().join("plain2");
    fs::create_dir_all(&plain).unwrap();
    let missing = fx.root.path().join("nope");
    for dir in [&plain, &missing] {
        match git::discover(dir) {
            Err(BerthError::Env(m)) => {
                assert!(
                    m.starts_with(
                        "not inside a git repository; run berth from your repo or pass -C <dir>"
                    ),
                    "{m}"
                );
                assert!(m.contains(": "), "{m}");
                assert!(m.lines().count() == 1, "{m}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn branch_name_rejects_reflog_syntax() {
    let fx = Fixture::new();
    let main = canon(&fx.repo);
    for bad in ["@{-1}", "a@{u}", "x@{0}"] {
        assert!(
            matches!(
                git::check_branch_name(&main, bad),
                Err(BerthError::Usage(_))
            ),
            "{bad}"
        );
    }
}
