mod common;

use berth::config::CopySource;
use berth::copy::{CopyPlan, execute_copy, plan_copy};
use berth::git::{self, Repo};
use berth::report::{Action, Item};
use common::{Fixture, git as run_git, write};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

const MAX: u64 = 1_048_576;

fn repo(fx: &Fixture) -> Repo {
    git::discover(&fx.repo).unwrap()
}

fn include(globs: &[&str]) -> CopySource {
    CopySource::Include(globs.iter().map(|s| s.to_string()).collect())
}

fn ignore(fx: &Fixture, lines: &str) {
    let path = fx.repo.join(".gitignore");
    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(lines);
    fs::write(path, body).unwrap();
}

fn plan(fx: &Fixture, globs: &[&str]) -> CopyPlan {
    plan_copy(&repo(fx), &include(globs), 1_048_576).unwrap()
}

fn worktree(fx: &Fixture) -> std::path::PathBuf {
    let wt = fx.root.path().join("wt");
    run_git(
        &fx.repo,
        &["worktree", "add", "-q", "-b", "b1", wt.to_str().unwrap()],
    );
    wt
}

fn only(items: &[Item]) -> &Item {
    assert_eq!(items.len(), 1, "{items:?}");
    &items[0]
}

#[test]
fn copies_ignored_env_byte_for_byte() {
    let fx = Fixture::new();
    let bytes = [0xff_u8, 0x00, b'\n'];
    fs::write(fx.repo.join(".env"), bytes).unwrap();
    #[cfg(unix)]
    fs::set_permissions(fx.repo.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
    let p = plan(&fx, &[".env"]);
    assert_eq!(p.files, vec![".env".to_string()]);
    assert!(p.refused.is_empty() && p.skipped.is_empty());
    let wt = worktree(&fx);
    let items = execute_copy(&fx.repo, &wt, &p.files, MAX).unwrap();
    let item = only(&items);
    assert_eq!((item.kind.as_str(), item.key.as_str()), ("copy", ".env"));
    assert_eq!(item.action, Action::Create);
    assert_eq!(fs::read(wt.join(".env")).unwrap(), bytes);
    // Unix-only: Windows has no mode bits.
    #[cfg(unix)]
    {
        let mode = fs::metadata(wt.join(".env")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(fs::read_dir(wt.join(".berth/tmp")).unwrap().count(), 0);
}

#[test]
fn untracked_not_ignored_is_refused() {
    let fx = Fixture::new();
    write(&fx.repo, ".dev.vars", "A=1\n");
    let p = plan(&fx, &[".dev.vars"]);
    assert!(p.files.is_empty());
    let item = only(&p.refused);
    assert_eq!(item.key, ".dev.vars");
    assert_eq!(item.action, Action::Refused);
    assert!(
        item.reason.contains("add it to .gitignore"),
        "{}",
        item.reason
    );
}

#[test]
fn tracked_env_is_refused() {
    let fx = Fixture::new();
    write(&fx.repo, ".env", "A=1\n");
    run_git(&fx.repo, &["add", "-f", ".env"]);
    run_git(&fx.repo, &["commit", "-q", "-m", "oops"]);
    let p = plan(&fx, &[".env"]);
    assert!(p.files.is_empty());
    let item = only(&p.refused);
    assert_eq!(item.key, ".env");
    assert!(
        item.reason.contains("git rm --cached .env"),
        "{}",
        item.reason
    );
}

#[test]
fn glob_matches_one_level() {
    let fx = Fixture::new();
    ignore(&fx, ".env.local\n");
    for d in ["apps/web", "apps/api", "apps/web/sub"] {
        write(&fx.repo, &format!("{d}/.env.local"), "A=1\n");
    }
    let p = plan(&fx, &["apps/*/.env.local"]);
    let mut files = p.files.clone();
    files.sort();
    assert_eq!(files, ["apps/api/.env.local", "apps/web/.env.local"]);
    assert!(p.refused.is_empty());
}

// Unix-only: needs symlink creation or Unix-only std APIs
#[cfg(unix)]
#[test]
fn symlink_and_oversize_refused() {
    let fx = Fixture::new();
    ignore(&fx, ".env.local\n");
    write(&fx.repo, "target.txt", "x");
    symlink("target.txt", fx.repo.join(".env.local")).unwrap();
    fs::write(fx.repo.join(".env"), vec![b'a'; 100]).unwrap();
    let p = plan_copy(&repo(&fx), &include(&[".env", ".env.local"]), 10).unwrap();
    assert!(p.files.is_empty());
    assert_eq!(p.refused.len(), 2, "{:?}", p.refused);
    let by_key = |k: &str| p.refused.iter().find(|i| i.key == k).unwrap();
    assert!(by_key(".env.local").reason.contains("symlink"));
    assert!(
        by_key(".env")
            .reason
            .contains("larger than [copy].max_bytes (10)")
    );
}

// Unix-only: needs symlink creation or Unix-only std APIs
#[cfg(unix)]
#[test]
fn target_symlink_parent_refused() {
    let fx = Fixture::new();
    ignore(&fx, ".env.local\n");
    write(&fx.repo, "apps/web/.env.local", "A=1\n");
    let outside = fx.root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    // A committed symlink `apps/web` -> outside, on a branch the worktree checks out.
    run_git(&fx.repo, &["checkout", "-q", "-b", "linked"]);
    fs::create_dir_all(fx.repo.join("apps")).unwrap();
    fs::remove_dir_all(fx.repo.join("apps/web")).unwrap();
    symlink(&outside, fx.repo.join("apps/web")).unwrap();
    run_git(&fx.repo, &["add", "apps/web"]);
    run_git(&fx.repo, &["commit", "-q", "-m", "link"]);
    run_git(&fx.repo, &["checkout", "-q", "main"]);
    let wt = fx.root.path().join("wt");
    run_git(
        &fx.repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "linked"],
    );
    assert!(
        fs::symlink_metadata(wt.join("apps/web"))
            .unwrap()
            .is_symlink()
    );
    let src = fx.root.path().join("src");
    write(&src, "apps/web/.env.local", "A=1\n");
    let items = execute_copy(&src, &wt, &["apps/web/.env.local".to_string()], MAX).unwrap();
    let item = only(&items);
    assert_eq!(item.action, Action::Refused);
    assert!(item.reason.contains("passes through a symlink"));
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn existing_target_skipped() {
    let fx = Fixture::new();
    write(&fx.repo, ".env", "NEW=1\n");
    let wt = worktree(&fx);
    write(&wt, ".env", "KEEP=1\n");
    let items = execute_copy(&fx.repo, &wt, &[".env".to_string()], MAX).unwrap();
    let item = only(&items);
    assert_eq!(item.action, Action::Skipped);
    assert_eq!(item.reason, "exists");
    assert_eq!(fs::read_to_string(wt.join(".env")).unwrap(), "KEEP=1\n");
}

#[test]
fn worktreeinclude_skips_unignored() {
    let fx = Fixture::new();
    write(&fx.repo, ".env", "A=1\n");
    write(&fx.repo, "notes.local", "B=1\n");
    write(&fx.repo, ".worktreeinclude", ".env\nnotes.local\n");
    let src = CopySource::WorktreeInclude(fx.repo.join(".worktreeinclude"));
    let p = plan_copy(&repo(&fx), &src, 1_048_576).unwrap();
    assert_eq!(p.files, vec![".env".to_string()]);
    let item = only(&p.skipped);
    assert_eq!(item.key, "notes.local");
    assert_eq!(item.action, Action::Skipped);
    assert!(item.reason.contains("not gitignored"));
    assert!(p.refused.is_empty());
}

#[test]
fn node_modules_not_scanned_for_include() {
    let fx = Fixture::new();
    ignore(&fx, "node_modules/\n");
    write(&fx.repo, ".env", "A=1\n");
    for i in 0..2000 {
        write(&fx.repo, &format!("node_modules/p{}/f{i}.js", i % 20), "x");
    }
    write(&fx.repo, "node_modules/p0/.env", "B=1\n");
    let p = plan(&fx, &[".env"]);
    assert_eq!(p.files, vec![".env".to_string()]);
    assert!(p.refused.is_empty() && p.skipped.is_empty());
}

#[test]
fn canary_never_appears_in_items() {
    let fx = Fixture::new();
    let canary = "CANARY_8f3a91c2d7e5";
    write(&fx.repo, ".env", &format!("SECRET={canary}\n"));
    write(&fx.repo, ".dev.vars", &format!("SECRET={canary}\n"));
    let p = plan(&fx, &[".env", ".dev.vars"]);
    let wt = worktree(&fx);
    let items = execute_copy(&fx.repo, &wt, &p.files, MAX).unwrap();
    let all: Vec<&Item> = p.refused.iter().chain(&p.skipped).chain(&items).collect();
    assert_eq!(all.len(), 2);
    let shown = format!("{all:?}{p:?}");
    assert!(!shown.contains(canary), "{shown}");
}

// Unix-only: needs symlink creation or Unix-only std APIs
#[cfg(unix)]
#[test]
fn symlinked_staging_dir_refused() {
    let fx = Fixture::new();
    write(&fx.repo, ".env", "SECRET=1\n");
    let outside = fx.root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    run_git(&fx.repo, &["checkout", "-q", "-b", "linked"]);
    fs::create_dir_all(fx.repo.join(".berth")).unwrap();
    symlink(&outside, fx.repo.join(".berth/tmp")).unwrap();
    run_git(&fx.repo, &["add", "-f", ".berth/tmp"]);
    run_git(&fx.repo, &["commit", "-q", "-m", "link"]);
    run_git(&fx.repo, &["checkout", "-q", "main"]);
    let wt = fx.root.path().join("wt");
    run_git(
        &fx.repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "linked"],
    );
    let items = execute_copy(&fx.repo, &wt, &[".env".to_string()], MAX).unwrap();
    assert_eq!(only(&items).action, Action::Refused);
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(!wt.join(".env").exists());
}

// Unix-only: needs symlink creation or Unix-only std APIs
#[cfg(unix)]
#[test]
fn source_changed_after_plan_refused() {
    let fx = Fixture::new();
    ignore(&fx, ".env.local\n");
    write(&fx.repo, ".env", "A=1\n");
    write(&fx.repo, ".env.local", "B=1\n");
    let p = plan_copy(&repo(&fx), &include(&[".env", ".env.local"]), 10).unwrap();
    assert_eq!(p.files.len(), 2);
    fs::remove_file(fx.repo.join(".env")).unwrap();
    symlink("/etc/hostname", fx.repo.join(".env")).unwrap();
    fs::write(fx.repo.join(".env.local"), vec![b'x'; 50]).unwrap();
    let wt = worktree(&fx);
    let items = execute_copy(&fx.repo, &wt, &p.files, 10).unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items.iter().all(|i| i.action == Action::Refused),
        "{items:?}"
    );
    assert!(!wt.join(".env").exists() && !wt.join(".env.local").exists());
}

#[test]
fn unsafe_paths_refused() {
    let fx = Fixture::new();
    let wt = worktree(&fx);
    let outside = fx.root.path().join("escape");
    let files = vec![
        "../escape".to_string(),
        "a/../../escape".to_string(),
        outside.to_str().unwrap().to_string(),
    ];
    write(&fx.repo, "escape", "x");
    let items = execute_copy(&fx.repo, &wt, &files, MAX).unwrap();
    assert_eq!(items.len(), 3);
    assert!(
        items.iter().all(|i| i.action == Action::Refused),
        "{items:?}"
    );
    assert!(!outside.exists());
}

#[test]
fn existing_target_left_alone_and_tmp_clean() {
    let fx = Fixture::new();
    write(&fx.repo, ".env", "NEW=1\n");
    let wt = worktree(&fx);
    write(&wt, ".env", "KEEP=1\n");
    execute_copy(&fx.repo, &wt, &[".env".to_string()], MAX).unwrap();
    assert_eq!(fs::read_to_string(wt.join(".env")).unwrap(), "KEEP=1\n");
    assert_eq!(fs::read_dir(wt.join(".berth/tmp")).unwrap().count(), 0);
}

// Unix-only: Windows file names are UTF-16, so a non-UTF-8 name cannot exist.
#[cfg(unix)]
#[test]
fn non_utf8_path_refused_without_failing_plan() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let fx = Fixture::new();
    ignore(&fx, "bad*\n");
    // APFS (macOS) rejects non-UTF-8 names with EILSEQ, so such a path cannot exist there and
    // there is nothing to refuse; every other platform must run the assertions below.
    match fs::write(fx.repo.join(OsStr::from_bytes(b"bad\xff")), "x") {
        Ok(()) => {}
        Err(e)
            if cfg!(target_os = "macos")
                && e.raw_os_error() == Some(92 /* EILSEQ on macOS */) =>
        {
            return;
        }
        Err(e) => panic!("create non-UTF-8 file: {e}"),
    }
    write(&fx.repo, ".env", "A=1\n");
    let p = plan(&fx, &["bad*", ".env"]);
    assert_eq!(p.files, vec![".env".to_string()]);
    let item = only(&p.refused);
    assert!(item.reason.contains("UTF-8"), "{}", item.reason);
}
