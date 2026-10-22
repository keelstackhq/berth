mod common;

use common::{Fixture, write};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
// The fake-binary tests (and their helpers) are Unix-only; see `Fixture::fake_bin`.
#[cfg(unix)]
use std::process::{Child, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const PG: &str = "[db]\nprovider = \"postgres\"\n";
const SETUP_URL: &str = "[setup]\nrun = [\"echo \\\"$DATABASE_URL\\\" > .db-url\"]";
#[cfg(unix)]
const SETUP_FAIL: &str = "[setup]\nrun = [\"exit 7\"]";

fn canon(p: &Path) -> PathBuf {
    berth::sys::canonicalize(p).expect("canonicalize")
}

fn root(fx: &Fixture) -> PathBuf {
    canon(fx.root.path())
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    root(fx).join(name).join(task)
}

fn run(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
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

fn entries(fx: &Fixture) -> Vec<Value> {
    match fs::read(fx.repo.join(".agents/worktrees.json")) {
        Ok(bytes) => {
            let v: Value = serde_json::from_slice(&bytes).unwrap();
            v["worktrees"].as_array().unwrap().clone()
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

// Unix-only: helper of the fake-binary tests
#[cfg(unix)]
fn log_path(fx: &Fixture, tool: &str) -> PathBuf {
    fx.root.path().join(format!("{tool}.log"))
}

// Unix-only: helper of the fake-binary tests
#[cfg(unix)]
fn log(fx: &Fixture, tool: &str) -> String {
    fs::read_to_string(log_path(fx, tool)).unwrap_or_default()
}

// Unix-only: helper of the fake-binary tests
#[cfg(unix)]
fn fail_flag(fx: &Fixture, tool: &str) -> PathBuf {
    fx.root.path().join(format!("{tool}.fail"))
}

/// Fake `tool`: appends its arguments to `<root>/<tool>.log`; exits 1 while `<root>/<tool>.fail`
/// exists.
// Unix-only: fake binaries are #!/bin/sh scripts with exec bits
#[cfg(unix)]
fn fake(fx: &Fixture, tool: &str) {
    let script = format!(
        "echo \"$*\" >> '{log}'\nif [ -e '{flag}' ]; then echo 'boom from {tool}' >&2; exit 1; fi",
        log = log_path(fx, tool).display(),
        flag = fail_flag(fx, tool).display(),
    );
    fx.fake_bin(tool, &script);
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
fn pg_fixture(rest: &str) -> Fixture {
    let fx = Fixture::new();
    fake(&fx, "createdb");
    fake(&fx, "dropdb");
    fx.config(&fx.repo, "", &format!("{PG}{rest}"));
    ignore(&fx, "/.db-url");
    fx
}

// Unix-only: helper of the fake-binary tests
#[cfg(unix)]
fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Unix-only: helper of the fake-binary tests
#[cfg(unix)]
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

// Unix-only: helper of the fake-binary tests
#[cfg(unix)]
fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn postgres_up_ok_down_roundtrip() {
    let fx = pg_fixture(SETUP_URL);
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert!(
        log(&fx, "createdb").contains(&format!("-- {}", fx.pg_name("a"))),
        "{}",
        log(&fx, "createdb")
    );
    assert_eq!(
        v["worktrees"][0]["db"],
        serde_json::json!({
            "provider": "postgres", "name": fx.pg_name("a"), "created": true,
            "host": "localhost", "port": 5432
        })
    );
    assert_eq!(
        fs::read_to_string(wt(&fx, "a").join(".db-url")).unwrap(),
        format!("postgres://localhost:5432/{}\n", fx.pg_name("a"))
    );
    let env = run(&fx, &["env", "a", "--json"]);
    assert_eq!(
        json(&env)["worktrees"][0]["env"]["DATABASE_URL"],
        format!("postgres://localhost:5432/{}", fx.pg_name("a"))
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"repo_[0-9a-f]{6}_a", "repo_[H]_a");
    settings.add_filter(&regex_escape(&root(&fx).display().to_string()), "[ROOT]");
    settings.add_filter(r#""pid": \d+"#, r#""pid": "[PID]""#);
    settings.add_filter(r#""host": "[^"]*""#, r#""host": "[HOST]""#);
    settings.add_filter(r#""secs": [0-9.eE+-]+"#, r#""secs": "[SECS]""#);
    settings.add_filter(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", "[TIME]");
    settings.add_filter(r"\b[0-9a-f]{40}\b", "[SHA]");
    settings.add_filter(r"\b(2[1-9]|30)\d{3}\b", "[PORT]");
    settings.bind(|| insta::assert_snapshot!("postgres_up_json", text));

    let again = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&again), 0, "{}", stderr(&again));
    let items = json(&again)["items"].as_array().unwrap().clone();
    assert!(items.iter().all(|i| i["action"] == "ok"), "{items:?}");
    assert!(
        items
            .iter()
            .any(|i| i["kind"] == "db" && i["key"] == fx.pg_name("a").as_str()),
        "{items:?}"
    );
    assert_eq!(log(&fx, "createdb").lines().count(), 1);

    let down = run(&fx, &["down", "a", "--json"]);
    assert_eq!(code(&down), 0, "{}", stderr(&down));
    let items = json(&down)["items"].as_array().unwrap().clone();
    assert!(
        items.iter().any(|i| i["kind"] == "db"
            && i["action"] == "remove"
            && i["key"] == fx.pg_name("a").as_str()),
        "{items:?}"
    );
    assert!(
        log(&fx, "dropdb").contains(&format!("--if-exists -- {}", fx.pg_name("a"))),
        "{}",
        log(&fx, "dropdb")
    );
    assert!(entries(&fx).is_empty());
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn createdb_failure_rolls_back_without_dropping() {
    let fx = pg_fixture("");
    fs::write(fail_flag(&fx, "createdb"), "").unwrap();
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("boom from createdb"),
        "{}",
        stderr(&out)
    );
    assert!(!wt(&fx, "a").exists());
    assert!(entries(&fx).is_empty());
    assert_eq!(reg_blocks(&fx), 0);
    assert_eq!(
        log(&fx, "dropdb"),
        "",
        "must not drop a db it did not create"
    );
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn setup_failure_drops_the_database() {
    let fx = pg_fixture(SETUP_FAIL);
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        log(&fx, "dropdb").contains(&format!("-- {}", fx.pg_name("a"))),
        "{}",
        log(&fx, "dropdb")
    );
    assert!(!wt(&fx, "a").exists());
    assert!(entries(&fx).is_empty());
    assert_eq!(reg_blocks(&fx), 0);
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn keep_on_failure_keeps_database_and_down_drops_it() {
    let fx = pg_fixture(SETUP_FAIL);
    let out = run(&fx, &["up", "a", "--keep-on-failure"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert_eq!(log(&fx, "dropdb"), "");
    let e = entries(&fx);
    assert_eq!(e[0]["state"], "failed");
    assert_eq!(e[0]["db"]["created"], true);
    let down = run(&fx, &["down", "a"]);
    assert_eq!(code(&down), 0, "{}", stderr(&down));
    assert!(log(&fx, "dropdb").contains(&format!("-- {}", fx.pg_name("a"))));
    assert!(entries(&fx).is_empty());
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn dropdb_failure_keeps_entry_removing() {
    let fx = pg_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    fs::write(fail_flag(&fx, "dropdb"), "").unwrap();
    let out = run(&fx, &["down", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.trim_end()
            .ends_with("fix it and run berth down a again, or pass --force"),
        "{err}"
    );
    assert_eq!(entries(&fx)[0]["state"], "removing");
    assert!(wt(&fx, "a").exists());
    // Fixed: a second down succeeds.
    fs::remove_file(fail_flag(&fx, "dropdb")).unwrap();
    let out = run(&fx, &["down", "a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(entries(&fx).is_empty());
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn down_force_with_failing_dropdb_warns() {
    let fx = pg_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    fs::write(fail_flag(&fx, "dropdb"), "").unwrap();
    let out = run(&fx, &["down", "a", "--force", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let items = json(&out)["items"].as_array().unwrap().clone();
    let db = items.iter().find(|i| i["kind"] == "db").expect("db item");
    assert_eq!(db["action"], "drift");
    assert_eq!(db["key"], fx.pg_name("a"));
    assert_eq!(db["reason"], "not dropped (ignored by --force)");
    assert!(log(&fx, "dropdb").lines().all(|l| l.contains("--force")));
    assert!(entries(&fx).is_empty());
    assert!(stderr(&out).contains("warning:"), "{}", stderr(&out));
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn crash_after_createdb_never_drops_on_down() {
    let fx = pg_fixture("");
    let mut child = spawn_paused(&fx, "after_db_create", "a");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(log(&fx, "createdb").contains(&format!("-- {}", fx.pg_name("a"))));
    let e = entries(&fx);
    assert_eq!(e[0]["state"], "creating");
    assert_eq!(e[0]["db"]["created"], false);
    let out = run(&fx, &["down", "a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        log(&fx, "dropdb"),
        "",
        "database berth may not own was dropped"
    );
    assert!(entries(&fx).is_empty());
    assert_eq!(reg_blocks(&fx), 0);
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn renamed_db_is_drift_and_down_drops_the_recorded_name() {
    let fx = pg_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    fx.config(&fx.repo, "", &format!("{PG}name = \"other_{{task}}\""));
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let items = json(&out)["items"].as_array().unwrap().clone();
    assert_eq!(items[0]["action"], "drift");
    let reason = items[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains(&fx.pg_name("a"))
            && reason.contains("other_a")
            && reason.contains("berth down a"),
        "{reason}"
    );
    let down = run(&fx, &["down", "a"]);
    assert_eq!(code(&down), 0, "{}", stderr(&down));
    let drop = log(&fx, "dropdb");
    assert!(
        drop.contains(&format!("-- {}", fx.pg_name("a"))) && !drop.contains("other_a"),
        "{drop}"
    );
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn db_added_after_up_is_drift() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    fake(&fx, "createdb");
    fake(&fx, "dropdb");
    fx.config(&fx.repo, "", PG);
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    assert_eq!(log(&fx, "createdb"), "");
}

// Unix-only: fake binaries are #!/bin/sh scripts with exec bits
#[cfg(unix)]
#[test]
fn missing_createdb_exits_3_before_any_lock() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", PG);
    let out = fx
        .berth()
        .env("PATH", fx.isolated_path())
        .args(["up", "a"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("createdb not found on PATH"),
        "{}",
        stderr(&out)
    );
    assert!(!fx.repo.join(".git/berth.lock").exists());
    assert!(!fx.state.join("ports.json").exists());
    assert!(entries(&fx).is_empty());
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn invalid_db_name_exits_3_before_any_lock() {
    let fx = pg_fixture("name = \"Bad-Name\"");
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(!fx.repo.join(".git/berth.lock").exists());
    assert_eq!(reg_blocks(&fx), 0);
    assert_eq!(log(&fx, "createdb"), "");
}

fn sqlite_fixture() -> Fixture {
    let fx = Fixture::new();
    write(&fx.repo, "db/dev.sqlite", "SQLITE-TEMPLATE");
    ignore(&fx, "/.db-url");
    fx.config(
        &fx.repo,
        "",
        &format!("[db]\nprovider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"\n{SETUP_URL}"),
    );
    fx
}

fn ignore(fx: &Fixture, line: &str) {
    let path = fx.repo.join(".git/info/exclude");
    let old = fs::read_to_string(&path).unwrap_or_default();
    fs::write(path, format!("{old}{line}\n")).unwrap();
}

#[test]
fn sqlite_up_copies_the_template() {
    let fx = sqlite_fixture();
    ignore(&fx, "/db/");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let w = wt(&fx, "a");
    assert_eq!(
        fs::read_to_string(w.join("db/dev.sqlite")).unwrap(),
        "SQLITE-TEMPLATE"
    );
    assert_eq!(
        fs::read_to_string(w.join(".db-url")).unwrap(),
        format!("file:{}\n", w.join("db/dev.sqlite").display())
    );
    let v = json(&out);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "db" && i["action"] == "create"),
        "{v}"
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("SQLITE-TEMPLATE"));
    let down = run(&fx, &["down", "a", "--json"]);
    assert_eq!(code(&down), 0, "{}", stderr(&down));
    assert!(!w.exists());
}

#[test]
fn sqlite_unignored_target_is_refused() {
    let fx = sqlite_fixture();
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let items = json(&out)["items"].as_array().unwrap().clone();
    assert!(
        items
            .iter()
            .any(|i| i["kind"] == "db" && i["action"] == "refused"),
        "{items:?}"
    );
    assert!(!wt(&fx, "a").exists());
    assert!(entries(&fx).is_empty());
    assert_eq!(reg_blocks(&fx), 0);
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn sqlite_nonempty_wal_exits_3_before_any_lock() {
    let fx = sqlite_fixture();
    ignore(&fx, "/db/");
    write(&fx.repo, "db/dev.sqlite-wal", "pending");
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(stderr(&out).contains("-wal"), "{}", stderr(&out));
    assert!(!wt(&fx, "a").exists());
    assert!(!fx.repo.join(".git/berth.lock").exists());
    assert_eq!(reg_blocks(&fx), 0);
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
fn compose_fixture() -> Fixture {
    let fx = Fixture::new();
    fake(&fx, "docker");
    ignore(&fx, "/.compose-env");
    fx.config(
        &fx.repo,
        "",
        "[db]\nprovider = \"compose\"\nservices = [\"db\"]\n[setup]\nrun = [\"echo \\\"$COMPOSE_PROJECT_NAME $DB_PORT\\\" > .compose-env\"]",
    );
    fx
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn compose_up_and_down() {
    let fx = compose_fixture();
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        log(&fx, "docker").trim(),
        format!(
            "compose --project-name {} up --detach --wait db",
            fx.project("a")
        )
    );
    let v = json(&out);
    let port = v["worktrees"][0]["env"]["DB_PORT"].as_str().unwrap();
    assert_eq!(
        v["worktrees"][0]["env"]["COMPOSE_PROJECT_NAME"],
        fx.project("a")
    );
    assert_eq!(v["worktrees"][0]["db"]["created"], true);
    assert_eq!(
        fs::read_to_string(wt(&fx, "a").join(".compose-env")).unwrap(),
        format!("{} {port}\n", fx.project("a"))
    );
    let down = run(&fx, &["down", "a"]);
    assert_eq!(code(&down), 0, "{}", stderr(&down));
    assert!(log(&fx, "docker").lines().any(|l| l
        == format!(
            "compose --project-name {} down --volumes --remove-orphans",
            fx.project("a")
        )));
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn compose_failure_rolls_back_with_compose_down() {
    let fx = compose_fixture();
    fs::write(fail_flag(&fx, "docker"), "").unwrap();
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(
        log(&fx, "docker").lines().any(|l| l
            == format!(
                "compose --project-name {} down --volumes --remove-orphans",
                fx.project("a")
            )),
        "{}",
        log(&fx, "docker")
    );
    assert!(!wt(&fx, "a").exists());
    assert!(entries(&fx).is_empty());
    assert_eq!(reg_blocks(&fx), 0);
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn compose_down_after_worktree_deleted_by_hand() {
    let fx = compose_fixture();
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    fs::remove_dir_all(wt(&fx, "a")).unwrap();
    let out = run(&fx, &["down", "a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(log(&fx, "docker").lines().any(|l| l
        == format!(
            "compose --project-name {} down --volumes --remove-orphans",
            fx.project("a")
        )));
    assert!(entries(&fx).is_empty());
}

#[test]
fn up_on_existing_task_ignores_nonempty_wal() {
    let fx = sqlite_fixture();
    ignore(&fx, "/db/");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    write(&fx.repo, "db/dev.sqlite-wal", "pending");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let items = json(&out)["items"].as_array().unwrap().clone();
    assert!(items.iter().all(|i| i["action"] == "ok"), "{items:?}");
}

// Unix-only: fake binaries are #!/bin/sh scripts with exec bits
#[cfg(unix)]
#[test]
fn up_on_existing_task_ignores_missing_createdb() {
    let fx = pg_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    fs::remove_file(fx.bin.join("createdb")).unwrap();
    let out = fx
        .berth()
        .env("PATH", fx.isolated_path())
        .args(["up", "a", "--json"])
        .output()
        .unwrap();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let items = json(&out)["items"].as_array().unwrap().clone();
    assert!(items.iter().all(|i| i["action"] == "ok"), "{items:?}");
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn createdb_failure_prints_a_recovery_hint() {
    let fx = pg_fixture("");
    fs::write(fail_flag(&fx, "createdb"), "").unwrap();
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    let name = fx.pg_name("a");
    assert!(
        stderr(&out).contains(&format!(
            "hint: if database {name} is a leftover, drop it with dropdb {name}, or set [db].name"
        )),
        "{}",
        stderr(&out)
    );
}

// Unix-only: needs Fixture::fake_bin (a #!/bin/sh script with an exec bit)
#[cfg(unix)]
#[test]
fn down_skip_reason_names_the_dropdb_recovery() {
    let fx = pg_fixture("");
    let mut child = spawn_paused(&fx, "after_db_create", "a");
    child.kill().unwrap();
    child.wait().unwrap();
    let out = run(&fx, &["down", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let name = fx.pg_name("a");
    let items = json(&out)["items"].as_array().unwrap().clone();
    let db = items.iter().find(|i| i["kind"] == "db").unwrap();
    assert_eq!(db["action"], "skipped");
    assert_eq!(
        db["reason"],
        format!(
            "not created by berth; left in place; drop it with dropdb {name} if it is a leftover"
        )
    );
}
