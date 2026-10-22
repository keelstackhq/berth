//! `berth hook <event>` (spec §12): Claude Code hook payloads on stdin, the harness's own
//! protocol on stdout. Hermetic: everything under the Fixture.

mod common;

use common::{Fixture, git, write};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const HOARD_CANARY: &str = "BERTH_CANARY_hook_3b9d";
const ENV_CANARY: &str = "BERTH_CANARY_hook_8e41";

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).expect("canonicalize")
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    canon(fx.root.path()).join(name).join(task)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn hook(fx: &Fixture, event: &str, payload: &str) -> Output {
    hook_env(fx, event, payload, &[])
}

fn hook_env(fx: &Fixture, event: &str, payload: &str, env: &[(&str, &Path)]) -> Output {
    let mut cmd = fx.berth();
    cmd.env_remove("CLAUDE_ENV_FILE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.args(["hook", event])
        .write_stdin(payload.to_string())
        .output()
        .expect("run berth")
}

fn create(fx: &Fixture, name: &str) -> Output {
    let payload = json!({
        "session_id": "s1", "transcript_path": "/x", "hook_event_name": "WorktreeCreate",
        "cwd": fx.repo, "name": name, "unknown_field": [1, 2]
    });
    hook(fx, "worktree-create", &payload.to_string())
}

fn remove_payload(fx: &Fixture, path: &Path) -> String {
    json!({ "hook_event_name": "WorktreeRemove", "worktree_path": path, "cwd": fx.repo })
        .to_string()
}

fn manifest(fx: &Fixture) -> Value {
    let text = fs::read_to_string(fx.repo.join(".agents/worktrees.json")).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(json!({ "worktrees": [] }))
}

fn entries(fx: &Fixture) -> Vec<Value> {
    manifest(fx)["worktrees"].as_array().unwrap().clone()
}

fn ports_of(fx: &Fixture, task: &str) -> u16 {
    let m = manifest(fx);
    let e = m["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["task"] == task)
        .expect("entry");
    e["ports"]["start"].as_u64().unwrap() as u16
}

// ---- worktree-create ----

#[test]
fn create_prints_only_the_absolute_path() {
    let fx = Fixture::new();
    fx.config(
        &fx.repo,
        "",
        "[setup]\nrun = [\"echo SETUP-STDOUT; echo SETUP-STDERR >&2\"]",
    );
    let out = create(&fx, "bold-oak-a3f2");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let expected = wt(&fx, "bold-oak-a3f2");
    assert_eq!(stdout(&out), format!("{}\n", expected.display()));
    assert!(expected.is_dir());
    assert!(expected.is_absolute());
    assert!(
        !stdout(&out).contains("SETUP"),
        "setup output must not reach stdout"
    );
    // The human table goes to stderr.
    assert!(stderr(&out).contains("bold-oak-a3f2"), "{}", stderr(&out));
    assert_eq!(entries(&fx).len(), 1);
}

#[test]
fn create_path_has_no_dot_segments() {
    let fx = Fixture::new();
    fx.config(
        &fx.repo,
        "",
        "[worktree]\nroot = \"./../x/../{repo}.wt/./\"",
    );
    let out = create(&fx, "a");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let printed = stdout(&out);
    let path = Path::new(printed.trim_end());
    assert!(path.is_absolute());
    assert!(
        path.components().all(|c| !matches!(
            c,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )),
        "{printed}"
    );
    assert!(path.is_dir());
}

#[test]
fn create_is_idempotent() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let first = create(&fx, "a");
    let second = create(&fx, "a");
    assert_eq!(code(&second), 0, "{}", stderr(&second));
    assert_eq!(stdout(&first), stdout(&second));
}

#[test]
fn create_refusal_exits_1_with_empty_stdout() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[worktree]\nmax = 1");
    assert_eq!(code(&create(&fx, "a")), 0);
    let out = create(&fx, "b");
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("refused"), "{}", stderr(&out));
    assert!(!wt(&fx, "b").exists());
}

#[test]
fn create_config_error_exits_3() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "[nonsense]\nx = 1");
    let out = create(&fx, "a");
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

#[test]
fn create_bad_name_exits_2() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    for name in ["Bold Oak", "a/b", "", "-x"] {
        let out = create(&fx, name);
        assert_eq!(code(&out), 2, "{name}: {}", stderr(&out));
        assert_eq!(stdout(&out), "");
    }
    assert!(entries(&fx).is_empty());
}

#[test]
fn missing_field_exits_2() {
    let fx = Fixture::new();
    for payload in [
        json!({ "cwd": fx.repo }),
        json!({ "name": "a" }),
        json!({ "cwd": 5, "name": "a" }),
        json!([1]),
    ] {
        let out = hook(&fx, "worktree-create", &payload.to_string());
        assert_eq!(code(&out), 2, "{payload}: {}", stderr(&out));
        assert_eq!(stdout(&out), "");
        assert!(!stderr(&out).is_empty());
    }
    let out = hook(
        &fx,
        "worktree-remove",
        &json!({ "cwd": fx.repo }).to_string(),
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let out = hook(
        &fx,
        "session-start",
        &json!({ "source": "startup" }).to_string(),
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

#[test]
fn malformed_stdin_exits_2() {
    let fx = Fixture::new();
    for event in ["worktree-create", "worktree-remove", "session-start"] {
        for input in ["", "not json", "{\"cwd\": "] {
            let out = hook(&fx, event, input);
            assert_eq!(code(&out), 2, "{event} {input:?}: {}", stderr(&out));
            assert_eq!(stdout(&out), "");
            assert!(!stderr(&out).is_empty());
        }
    }
}

#[test]
fn json_flag_is_a_usage_error() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    let payload = json!({ "cwd": fx.repo, "name": "a" }).to_string();
    let out = fx
        .berth()
        .args(["hook", "worktree-create", "--json"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(entries(&fx).is_empty());
}

#[test]
fn unknown_event_exits_2() {
    let fx = Fixture::new();
    let out = hook(&fx, "bogus", "{}");
    assert_eq!(code(&out), 2);
}

// ---- worktree-remove ----

#[test]
fn remove_clean_worktree() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let path = wt(&fx, "a");
    let out = hook(&fx, "worktree-remove", &remove_payload(&fx, &path));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(!path.exists());
    assert!(entries(&fx).is_empty());
    // Work is never lost: the branch stays.
    let branches = git(&fx.repo, &["branch", "--list"]);
    assert!(branches.lines().count() >= 2, "{branches}");
}

#[test]
fn remove_works_when_cwd_is_the_worktree_and_path_is_unnormalized() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let path = wt(&fx, "a");
    let odd = format!("{}/../a/.", path.display());
    let payload = json!({ "worktree_path": odd, "cwd": path }).to_string();
    let out = hook(&fx, "worktree-remove", &payload);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!path.exists());
    assert!(entries(&fx).is_empty());
}

#[test]
fn remove_dirty_worktree_exits_1_and_keeps_it() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let path = wt(&fx, "a");
    write(&path, "work.txt", "unsaved\n");
    let out = hook(&fx, "worktree-remove", &remove_payload(&fx, &path));
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("refused"), "{}", stderr(&out));
    assert!(path.join("work.txt").exists());
    assert_eq!(entries(&fx).len(), 1);
}

#[test]
fn remove_unknown_path_exits_0() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let other = fx.root.path().join("elsewhere");
    fs::create_dir_all(&other).unwrap();
    for path in [other, fx.root.path().join("never-existed")] {
        let out = hook(&fx, "worktree-remove", &remove_payload(&fx, &path));
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert_eq!(stdout(&out), "");
        assert!(stderr(&out).contains("no berth"), "{}", stderr(&out));
    }
    assert_eq!(entries(&fx).len(), 1);
    assert!(wt(&fx, "a").exists());
}

#[test]
fn remove_when_the_directory_is_already_gone() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let path = wt(&fx, "a");
    fs::remove_dir_all(&path).unwrap();
    let out = hook(&fx, "worktree-remove", &remove_payload(&fx, &path));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(entries(&fx).is_empty());
}

// ---- session-start ----

fn env_file(fx: &Fixture) -> PathBuf {
    fx.root.path().join("claude-env")
}

fn session_payload(cwd: &Path) -> String {
    json!({ "hook_event_name": "SessionStart", "source": "startup", "cwd": cwd }).to_string()
}

#[test]
fn session_start_appends_env_inside_a_worktree() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let file = env_file(&fx);
    fs::write(&file, "export OTHER_HOOK=1\n").unwrap();
    let sub = wt(&fx, "a").join("deep/er");
    fs::create_dir_all(&sub).unwrap();
    let out = hook_env(
        &fx,
        "session-start",
        &session_payload(&sub),
        &[("CLAUDE_ENV_FILE", &file)],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        text.starts_with("export OTHER_HOOK=1\n# berth a\n"),
        "{text}"
    );
    let sourced = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            ". '{}'; printf '%s %s %s' \"$PORT\" \"$BERTH_TASK\" \"$OTHER_HOOK\"",
            file.display()
        ))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&sourced.stdout),
        format!("{} a 1", ports_of(&fx, "a"))
    );
    // A second session appends again; earlier content is never truncated.
    hook_env(
        &fx,
        "session-start",
        &session_payload(&sub),
        &[("CLAUDE_ENV_FILE", &file)],
    );
    let again = fs::read_to_string(&file).unwrap();
    assert!(again.starts_with(&text));
    assert_eq!(again.matches("# berth a\n").count(), 2);
}

#[test]
fn session_start_creates_a_missing_file_and_fixes_a_missing_newline() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let file = env_file(&fx);
    let out = hook_env(
        &fx,
        "session-start",
        &session_payload(&wt(&fx, "a")),
        &[("CLAUDE_ENV_FILE", &file)],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .starts_with("# berth a\n")
    );

    let file2 = fx.root.path().join("claude-env2");
    fs::write(&file2, "export X=1").unwrap();
    hook_env(
        &fx,
        "session-start",
        &session_payload(&wt(&fx, "a")),
        &[("CLAUDE_ENV_FILE", &file2)],
    );
    assert!(
        fs::read_to_string(&file2)
            .unwrap()
            .starts_with("export X=1\n# berth a\n")
    );
}

#[test]
fn session_start_does_nothing_outside_a_worktree_or_without_the_variable() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let file = env_file(&fx);
    // Main checkout, and a directory that is no repository at all.
    for cwd in [fx.repo.clone(), fx.root.path().to_path_buf()] {
        let out = hook_env(
            &fx,
            "session-start",
            &session_payload(&cwd),
            &[("CLAUDE_ENV_FILE", &file)],
        );
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert_eq!(stdout(&out), "");
        assert!(!file.exists(), "nothing may be written for {cwd:?}");
    }
    // Inside a worktree but no CLAUDE_ENV_FILE.
    let out = hook(&fx, "session-start", &session_payload(&wt(&fx, "a")));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(!file.exists());
}

#[test]
fn session_start_unwritable_file_exits_3() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let file = fx.root.path().join("no-such-dir/env");
    let out = hook_env(
        &fx,
        "session-start",
        &session_payload(&wt(&fx, "a")),
        &[("CLAUDE_ENV_FILE", &file)],
    );
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

// ---- statusline ----

fn status_payload(dir: &Path) -> String {
    json!({
        "cwd": dir,
        "workspace": { "current_dir": dir, "project_dir": dir },
        "model": { "display_name": "x" }
    })
    .to_string()
}

#[test]
fn statusline_inside_and_outside() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let port = ports_of(&fx, "a");
    let before = fs::read(fx.repo.join(".agents/worktrees.json")).unwrap();
    let inside = hook(
        &fx,
        "statusline",
        &status_payload(&wt(&fx, "a").join("sub")),
    );
    assert_eq!(code(&inside), 0);
    // `sub` does not exist: the lookup still finds the worktree through its nearest ancestor.
    assert_eq!(
        stdout(&inside),
        format!("a :{port}\n"),
        "{}",
        stderr(&inside)
    );
    let exact = hook(&fx, "statusline", &status_payload(&wt(&fx, "a")));
    assert_eq!(stdout(&exact), format!("a :{port}\n"));
    // `cwd` is the fallback when `workspace.current_dir` is absent.
    let fallback = hook(
        &fx,
        "statusline",
        &json!({ "cwd": wt(&fx, "a") }).to_string(),
    );
    assert_eq!(stdout(&fallback), format!("a :{port}\n"));
    for dir in [fx.repo.clone(), fx.root.path().to_path_buf()] {
        let out = hook(&fx, "statusline", &status_payload(&dir));
        assert_eq!(code(&out), 0);
        assert_eq!(stdout(&out), "");
    }
    assert_eq!(
        fs::read(fx.repo.join(".agents/worktrees.json")).unwrap(),
        before
    );
}

#[test]
fn statusline_never_fails() {
    let fx = Fixture::new();
    for input in ["", "garbage", "[]", "{}", "{\"cwd\": 3}"] {
        let out = hook(&fx, "statusline", input);
        assert_eq!(code(&out), 0, "{input:?}");
        assert_eq!(stdout(&out), "");
    }
}

#[test]
fn statusline_ignores_a_held_lock() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&create(&fx, "a")), 0);
    let common = canon(&fx.repo).join(".git");
    let _guard = berth::lock::acquire(
        &berth::lock::repo_lock(&common),
        std::time::Duration::from_secs(1),
    )
    .expect("lock");
    let out = hook(&fx, "statusline", &status_payload(&wt(&fx, "a")));
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).starts_with("a :"), "{}", stdout(&out));
}

// ---- secrets ----

#[test]
fn canaries_never_reach_any_stream_or_the_env_file() {
    let fx = Fixture::new();
    write(&fx.repo, ".gitignore", ".env\n.hoard/\n");
    git(&fx.repo, &["add", ".gitignore"]);
    git(&fx.repo, &["commit", "-q", "-m", "ignore"]);
    write(&fx.repo, ".hoard/port", "41234\n");
    write(&fx.repo, ".hoard/token", HOARD_CANARY);
    write(&fx.repo, ".env", &format!("TOKEN={ENV_CANARY}\n"));
    fx.config(
        &fx.repo,
        "",
        "[cache]\nremote = \"hoard\"\n[copy]\ninclude = [\".env\"]",
    );
    let file = env_file(&fx);
    let outs = [
        create(&fx, "a"),
        hook_env(
            &fx,
            "session-start",
            &session_payload(&wt(&fx, "a")),
            &[("CLAUDE_ENV_FILE", &file)],
        ),
        hook(&fx, "statusline", &status_payload(&wt(&fx, "a"))),
        hook(&fx, "worktree-remove", &remove_payload(&fx, &wt(&fx, "a"))),
    ];
    for out in &outs {
        let text = format!("{}\n{}", stdout(out), stderr(out));
        assert!(
            !text.contains(HOARD_CANARY) && !text.contains(ENV_CANARY),
            "{text}"
        );
    }
    let env_text = fs::read_to_string(&file).unwrap();
    assert!(!env_text.contains(HOARD_CANARY) && !env_text.contains(ENV_CANARY));
    let token = canon(&fx.repo).join(".hoard/token");
    assert!(
        env_text.contains(&format!(
            "export TURBO_TOKEN=\"$(cat '{}')\"",
            token.display()
        )),
        "{env_text}"
    );
}
