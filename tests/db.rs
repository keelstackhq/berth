mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use berth::BerthError;
use berth::config::{DbCfg, Provider};
use berth::db::{
    check_sqlite, check_tools_in, create, default_pg_name, default_project, drop, entry, resolve,
};
use berth::manifest::DbEntry;
use berth::manifest::PortBlock;
use berth::report::Action;
use common::Fixture;

fn ports() -> PortBlock {
    let names = BTreeMap::from([("db".to_string(), 3)]);
    PortBlock::new(21340, 10, &names)
}

fn cfg(src: &str) -> DbCfg {
    toml::from_str(src).unwrap()
}

fn plan(src: &str, repo: &str, task: &str) -> Result<berth::db::DbPlan, BerthError> {
    resolve(&cfg(src), repo, task, Path::new("/wt"), &ports())
}

fn config_msg(err: BerthError) -> String {
    match err {
        BerthError::Config { file, message } => {
            assert_eq!(file, Path::new("berth.toml"));
            message
        }
        other => panic!("not a config error: {other}"),
    }
}

#[test]
fn postgres_default_name_and_url() {
    assert_eq!(
        default_pg_name("My-App.v2", "fix-login"),
        "my_app_v2_fix_login"
    );
    let p = plan("provider = \"postgres\"", "My-App.v2", "fix-login").unwrap();
    assert_eq!(p.provider, Provider::Postgres);
    assert_eq!(p.name, "my_app_v2_fix_login");
    assert_eq!(
        p.url.as_deref(),
        Some("postgres://localhost:5432/my_app_v2_fix_login")
    );
    assert_eq!(p.env_var.as_deref(), Some("DATABASE_URL"));
    assert_eq!(p.abs_path, None);
}

#[test]
fn postgres_user_and_port() {
    let p = plan(
        "provider = \"postgres\"\nuser = \"dev\"\nport = 5433\nhost = \"db.local\"",
        "app",
        "t",
    )
    .unwrap();
    assert_eq!(p.url.as_deref(), Some("postgres://dev@db.local:5433/app_t"));
}

#[test]
fn postgres_bad_names_name_the_key() {
    let msg = config_msg(
        plan(
            "provider = \"postgres\"\nname = \"{repo}_{task}\"",
            "x",
            "a-b",
        )
        .unwrap_err(),
    );
    assert!(msg.contains("[db].name"), "{msg}");
    assert!(msg.contains("x_a-b"), "{msg}");
    let long = "a".repeat(64);
    let msg = config_msg(
        plan(
            &format!("provider = \"postgres\"\nname = \"{long}\""),
            "x",
            "t",
        )
        .unwrap_err(),
    );
    assert!(msg.contains("[db].name"), "{msg}");
    let ok = "a".repeat(63);
    assert!(
        plan(
            &format!("provider = \"postgres\"\nname = \"{ok}\""),
            "x",
            "t"
        )
        .is_ok()
    );
    // A long derived default fails the same way.
    let msg = config_msg(plan("provider = \"postgres\"", &"r".repeat(70), "t").unwrap_err());
    assert!(msg.contains("[db].name"), "{msg}");
}

#[test]
fn postgres_name_template_and_url_template() {
    let p = plan(
        "provider = \"postgres\"\nname = \"{repo}_{task_snake}\"\nurl = \"pg://{db.name}:{port.db}\"",
        "x",
        "a-b",
    )
    .unwrap();
    assert_eq!(p.name, "x_a_b");
    assert_eq!(p.url.as_deref(), Some("pg://x_a_b:21343"));
}

#[test]
fn sqlite_plan() {
    let p = plan(
        "provider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"",
        "app",
        "t",
    )
    .unwrap();
    assert_eq!(p.name, "db/dev.sqlite");
    assert_eq!(p.url.as_deref(), Some("file:/wt/db/dev.sqlite"));
    assert_eq!(p.abs_path.as_deref(), Some(Path::new("/wt/db/dev.sqlite")));
    let p = plan(
        "provider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"\npath = \"data/t.db\"\nurl = \"sqlite://{db.path}\"",
        "app",
        "t",
    )
    .unwrap();
    assert_eq!(p.name, "data/t.db");
    assert_eq!(p.url.as_deref(), Some("sqlite:///wt/data/t.db"));
}

#[test]
fn compose_project_and_url() {
    assert_eq!(default_project("My Repo", "fix-login"), "my-repo-fix-login");
    assert_eq!(default_project("_.Repo", "t"), "repo-t");
    let p = plan("provider = \"compose\"", "My Repo", "fix").unwrap();
    assert_eq!(p.name, "my-repo-fix");
    assert_eq!(p.url, None);
    let p = plan(
        "provider = \"compose\"\nurl = \"postgres://postgres@localhost:{port.db}/{db.name}\"",
        "My Repo",
        "fix",
    )
    .unwrap();
    assert_eq!(
        p.url.as_deref(),
        Some("postgres://postgres@localhost:21343/my-repo-fix")
    );
}

#[test]
fn compose_bad_project() {
    let msg = config_msg(plan("provider = \"compose\"\nproject = \"-x\"", "r", "t").unwrap_err());
    assert!(msg.contains("[db].project"), "{msg}");
}

#[test]
fn empty_env_exports_nothing() {
    let p = plan("provider = \"postgres\"\nenv = \"\"", "app", "t").unwrap();
    assert_eq!(p.env_var, None);
    let p = plan("provider = \"postgres\"\nenv = \"PG_URL\"", "app", "t").unwrap();
    assert_eq!(p.env_var.as_deref(), Some("PG_URL"));
}

#[test]
fn unknown_url_placeholder_is_config_error() {
    let msg =
        config_msg(plan("provider = \"postgres\"\nurl = \"{db.path}\"", "a", "t").unwrap_err());
    assert!(
        msg.contains("{db.path}") && msg.contains("[db].url"),
        "{msg}"
    );
}

#[test]
fn entry_records_postgres_connection_only() {
    let c = cfg("provider = \"postgres\"\nuser = \"dev\"");
    let p = resolve(&c, "app", "t", Path::new("/wt"), &ports()).unwrap();
    let e = entry(&p, &c);
    assert_eq!(e.provider, "postgres");
    assert_eq!(e.name, "app_t");
    assert!(!e.created);
    assert_eq!(
        (e.host.as_deref(), e.port, e.user.as_deref()),
        (Some("localhost"), Some(5432), Some("dev"))
    );
    let c = cfg("provider = \"compose\"");
    let p = resolve(&c, "app", "t", Path::new("/wt"), &ports()).unwrap();
    let e = entry(&p, &c);
    assert_eq!(
        (e.provider.as_str(), e.host, e.port, e.user),
        ("compose", None, None, None)
    );
}

// ---- provider I/O (Task 4) ----

const T: Duration = Duration::from_secs(20);

/// Fake tool that logs `cwd: argv` to `<root>/bin/<name>.log`.
fn logging(fx: &Fixture, name: &str, extra: &str) {
    let log = fx.bin.join(format!("{name}.log"));
    fx.fake_bin(
        name,
        &format!("echo \"$(pwd): $*\" >> '{}'\n{extra}", log.display()),
    );
}

fn log_of(fx: &Fixture, name: &str) -> String {
    fs::read_to_string(fx.bin.join(format!("{name}.log"))).unwrap_or_default()
}

fn env_of(fx: &Fixture) -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".to_string(), fx.path_with_bin())])
}

fn pg_plan(c: &DbCfg, repo: &str) -> berth::db::DbPlan {
    resolve(c, repo, "a", Path::new("/wt"), &ports()).unwrap()
}

#[test]
fn createdb_argv_with_template() {
    let fx = Fixture::new();
    logging(&fx, "createdb", "");
    let c = cfg("provider = \"postgres\"\ntemplate = \"app_dev\"");
    let plan = pg_plan(&c, "repo");
    let item = create(&fx.repo, &fx.tmp, &c, &plan, &env_of(&fx)).unwrap();
    assert_eq!(item.action, Action::Create);
    assert_eq!(item.kind, "db");
    assert_eq!(item.key, "repo_a");
    assert_eq!(item.reason, "created from template app_dev");
    let log = log_of(&fx, "createdb");
    assert!(
        log.trim_end()
            .ends_with("--no-password --host localhost --port 5432 --template app_dev -- repo_a"),
        "{log}"
    );
    assert!(
        log.starts_with(fx.tmp.canonicalize().unwrap().to_str().unwrap()),
        "{log}"
    );
}

#[test]
fn createdb_with_user_without_template() {
    let fx = Fixture::new();
    logging(&fx, "createdb", "");
    let c = cfg("provider = \"postgres\"\nuser = \"dev\"\nhost = \"db.local\"\nport = 5433");
    let item = create(&fx.repo, &fx.tmp, &c, &pg_plan(&c, "repo"), &env_of(&fx)).unwrap();
    assert_eq!(item.reason, "created (empty)");
    let log = log_of(&fx, "createdb");
    assert!(
        log.trim_end()
            .ends_with("--no-password --host db.local --port 5433 --username dev -- repo_a"),
        "{log}"
    );
}

#[test]
fn createdb_failure_is_a_hook_error_with_tail() {
    let fx = Fixture::new();
    fx.fake_bin(
        "createdb",
        "echo 'ERROR: database \"repo_a\" already exists' >&2\nexit 1",
    );
    let c = cfg("provider = \"postgres\"");
    let err = create(&fx.repo, &fx.tmp, &c, &pg_plan(&c, "repo"), &env_of(&fx)).unwrap_err();
    assert!(err.reason().contains("createdb"), "{}", err.reason());
    assert!(!err.reason().contains("already exists"));
    match &err {
        BerthError::Hook { tail, .. } => {
            assert!(
                tail.iter().any(|l| l.contains("already exists")),
                "{tail:?}"
            );
        }
        other => panic!("not a hook error: {other}"),
    }
    assert_eq!(err.exit_code(), 3);
}

fn pg_entry(created: bool) -> DbEntry {
    DbEntry {
        provider: "postgres".into(),
        name: "repo_a".into(),
        created,
        host: Some("localhost".into()),
        port: Some(5432),
        user: None,
    }
}

#[test]
fn dropdb_argv_and_force() {
    let fx = Fixture::new();
    logging(&fx, "dropdb", "");
    let env = env_of(&fx);
    let item = drop(&fx.repo, &fx.tmp, &pg_entry(true), false, &env, T)
        .unwrap()
        .unwrap();
    assert_eq!(
        (item.action, item.reason.as_str()),
        (Action::Remove, "dropped")
    );
    assert_eq!(item.key, "repo_a");
    drop(&fx.repo, &fx.tmp, &pg_entry(true), true, &env, T).unwrap();
    let log = log_of(&fx, "dropdb");
    let lines: Vec<&str> = log.lines().collect();
    assert!(
        lines[0].ends_with("--no-password --host localhost --port 5432 --if-exists -- repo_a"),
        "{log}"
    );
    assert!(
        lines[1]
            .ends_with("--no-password --host localhost --port 5432 --if-exists --force -- repo_a"),
        "{log}"
    );
}

#[test]
fn dropdb_with_user_from_the_entry() {
    let fx = Fixture::new();
    logging(&fx, "dropdb", "");
    let mut e = pg_entry(true);
    e.user = Some("dev".into());
    e.host = Some("db.local".into());
    e.port = Some(5433);
    drop(&fx.repo, &fx.tmp, &e, false, &env_of(&fx), T).unwrap();
    assert!(log_of(&fx, "dropdb").trim_end().ends_with(
        "--no-password --host db.local --port 5433 --username dev --if-exists -- repo_a"
    ));
}

#[test]
fn dropdb_skips_a_database_berth_did_not_create() {
    let fx = Fixture::new();
    logging(&fx, "dropdb", "");
    let item = drop(&fx.repo, &fx.tmp, &pg_entry(false), true, &env_of(&fx), T)
        .unwrap()
        .unwrap();
    assert_eq!(item.action, Action::Skipped);
    assert_eq!(item.reason, "not created by berth; left in place");
    assert_eq!(log_of(&fx, "dropdb"), "");
}

#[test]
fn dropdb_failure_is_a_hook_error() {
    let fx = Fixture::new();
    fx.fake_bin("dropdb", "echo 'being accessed by other users' >&2\nexit 1");
    let err = drop(&fx.repo, &fx.tmp, &pg_entry(true), false, &env_of(&fx), T).unwrap_err();
    assert!(err.reason().contains("dropdb"));
}

#[test]
fn compose_up_and_down() {
    let fx = Fixture::new();
    logging(&fx, "docker", "");
    let c = cfg("provider = \"compose\"\nservices = [\"db\", \"redis\"]");
    let plan = resolve(&c, "My App", "fix-it", Path::new("/wt"), &ports()).unwrap();
    assert_eq!(plan.name, "my-app-fix-it");
    let wt = fx.tmp.canonicalize().unwrap();
    let item = create(&fx.repo, &wt, &c, &plan, &env_of(&fx)).unwrap();
    assert_eq!(
        (item.action, item.reason.as_str()),
        (Action::Create, "compose up")
    );
    assert_eq!(item.key, "my-app-fix-it");
    let e = DbEntry {
        provider: "compose".into(),
        name: plan.name.clone(),
        created: false,
        host: None,
        port: None,
        user: None,
    };
    let item = drop(&fx.repo, &wt, &e, false, &env_of(&fx), T)
        .unwrap()
        .unwrap();
    assert_eq!(
        (item.action, item.reason.as_str()),
        (Action::Remove, "compose down")
    );
    let log = log_of(&fx, "docker");
    let lines: Vec<&str> = log.lines().collect();
    let cwd = wt.display();
    assert_eq!(
        lines[0],
        format!("{cwd}: compose --project-name my-app-fix-it up --detach --wait db redis")
    );
    assert_eq!(
        lines[1],
        format!("{cwd}: compose --project-name my-app-fix-it down --volumes --remove-orphans")
    );
}

#[test]
fn compose_without_services_runs_all() {
    let fx = Fixture::new();
    logging(&fx, "docker", "");
    let c = cfg("provider = \"compose\"");
    create(&fx.repo, &fx.tmp, &c, &pg_plan(&c, "repo"), &env_of(&fx)).unwrap();
    assert!(
        log_of(&fx, "docker")
            .trim_end()
            .ends_with("compose --project-name repo-a up --detach --wait")
    );
}

#[test]
fn compose_failure_is_a_hook_error() {
    let fx = Fixture::new();
    fx.fake_bin("docker", "echo 'no daemon' >&2\nexit 1");
    let c = cfg("provider = \"compose\"");
    let err = create(&fx.repo, &fx.tmp, &c, &pg_plan(&c, "repo"), &env_of(&fx)).unwrap_err();
    assert!(err.reason().contains("docker"));
}

fn sqlite_fx(c: &DbCfg) -> (Fixture, berth::db::DbPlan, std::path::PathBuf) {
    let fx = Fixture::new();
    common::write(&fx.repo, ".gitignore", ".env\n*.db\n");
    common::git(&fx.repo, &["add", ".gitignore"]);
    common::git(&fx.repo, &["commit", "-q", "-m", "ignore db"]);
    let wt = fx.root.path().join("wt");
    fs::create_dir_all(&wt).unwrap();
    let plan = resolve(c, "repo", "a", &wt, &ports()).unwrap();
    (fx, plan, wt)
}

#[test]
fn sqlite_copies_bytes_and_mode_then_skips() {
    use std::os::unix::fs::PermissionsExt;
    let c = cfg("provider = \"sqlite\"\ntemplate = \"data/dev.db\"");
    let (fx, plan, wt) = sqlite_fx(&c);
    common::write(&fx.repo, "data/dev.db", "SQLite format 3\0rows");
    fs::set_permissions(
        fx.repo.join("data/dev.db"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let item = create(&fx.repo, &wt, &c, &plan, &BTreeMap::new()).unwrap();
    assert_eq!(
        (item.action, item.key.as_str(), item.reason.as_str()),
        (Action::Create, "data/dev.db", "copied from data/dev.db")
    );
    let target = wt.join("data/dev.db");
    assert_eq!(fs::read(&target).unwrap(), b"SQLite format 3\0rows");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(fs::read_dir(wt.join(".berth/tmp")).unwrap().count(), 0);
    let again = create(&fx.repo, &wt, &c, &plan, &BTreeMap::new()).unwrap();
    assert_eq!(
        (again.action, again.reason.as_str()),
        (Action::Skipped, "exists")
    );
}

#[test]
fn sqlite_refuses_target_through_a_symlink() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"data/dev.db\"");
    let (fx, plan, wt) = sqlite_fx(&c);
    common::write(&fx.repo, "data/dev.db", "x");
    let outside = fx.root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, wt.join("data")).unwrap();
    let item = create(&fx.repo, &wt, &c, &plan, &BTreeMap::new()).unwrap();
    assert_eq!(item.action, Action::Refused);
    assert!(!outside.join("dev.db").exists());
}

#[test]
fn check_sqlite_ok_when_template_is_fine_and_ignored() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"dev.db\"");
    let (fx, _, _) = sqlite_fx(&c);
    common::write(&fx.repo, "dev.db", "x");
    assert_eq!(check_sqlite(&fx.repo, &c).unwrap(), None);
    common::write(&fx.repo, "dev.db-wal", "");
    assert_eq!(check_sqlite(&fx.repo, &c).unwrap(), None);
}

fn env_msg(err: BerthError) -> String {
    match err {
        BerthError::Env(m) => m,
        other => panic!("not an env error: {other}"),
    }
}

#[test]
fn check_sqlite_missing_template() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"dev.db\"");
    let (fx, _, _) = sqlite_fx(&c);
    let msg = env_msg(check_sqlite(&fx.repo, &c).unwrap_err());
    assert!(
        msg.contains("[db].template") && msg.contains("dev.db"),
        "{msg}"
    );
}

#[test]
fn check_sqlite_symlink_and_directory_template() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"dev.db\"");
    let (fx, _, _) = sqlite_fx(&c);
    common::write(&fx.repo, "real.db", "x");
    std::os::unix::fs::symlink("real.db", fx.repo.join("dev.db")).unwrap();
    let msg = env_msg(check_sqlite(&fx.repo, &c).unwrap_err());
    assert!(
        msg.contains("[db].template") && msg.contains("symlink"),
        "{msg}"
    );
    fs::remove_file(fx.repo.join("dev.db")).unwrap();
    fs::create_dir(fx.repo.join("dev.db")).unwrap();
    let msg = env_msg(check_sqlite(&fx.repo, &c).unwrap_err());
    assert!(msg.contains("not a regular file"), "{msg}");
}

#[test]
fn check_sqlite_nonempty_wal() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"dev.db\"");
    let (fx, _, _) = sqlite_fx(&c);
    common::write(&fx.repo, "dev.db", "x");
    common::write(&fx.repo, "dev.db-wal", "pending");
    let msg = env_msg(check_sqlite(&fx.repo, &c).unwrap_err());
    assert!(msg.contains("non-empty -wal"), "{msg}");
    assert!(msg.contains("PRAGMA wal_checkpoint(TRUNCATE)"), "{msg}");
    assert!(!msg.contains("pending"));
}

#[test]
fn check_sqlite_unignored_target_is_refused() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"dev.sqlite3\"");
    let (fx, _, _) = sqlite_fx(&c);
    common::write(&fx.repo, "dev.sqlite3", "x");
    let item = check_sqlite(&fx.repo, &c).unwrap().unwrap();
    assert_eq!(
        (item.kind.as_str(), item.key.as_str()),
        ("db", "dev.sqlite3")
    );
    assert_eq!(item.action, Action::Refused);
    assert_eq!(
        item.reason,
        "add dev.sqlite3 to .gitignore: the agent would commit the database"
    );
}

#[test]
fn check_sqlite_checks_the_path_key_for_ignore() {
    let c = cfg("provider = \"sqlite\"\ntemplate = \"dev.db\"\npath = \"local.sqlite3\"");
    let (fx, _, _) = sqlite_fx(&c);
    common::write(&fx.repo, "dev.db", "x");
    let item = check_sqlite(&fx.repo, &c).unwrap().unwrap();
    assert_eq!(item.key, "local.sqlite3");
}

#[test]
fn check_tools_names_the_missing_tool() {
    let fx = Fixture::new();
    let only = fx.isolated_path();
    let pg = cfg("provider = \"postgres\"");
    let msg = env_msg(check_tools_in(&pg, only.as_ref()).unwrap_err());
    assert!(msg.starts_with("createdb not found on PATH"), "{msg}");
    fx.fake_bin("createdb", "exit 0");
    let only = fx.isolated_path();
    let msg = env_msg(check_tools_in(&pg, only.as_ref()).unwrap_err());
    assert!(msg.starts_with("dropdb not found on PATH"), "{msg}");
    fx.fake_bin("dropdb", "exit 0");
    let only = fx.isolated_path();
    check_tools_in(&pg, only.as_ref()).unwrap();
    let compose = cfg("provider = \"compose\"");
    let msg = env_msg(check_tools_in(&compose, only.as_ref()).unwrap_err());
    assert!(msg.starts_with("docker not found on PATH"), "{msg}");
    check_tools_in(&cfg("provider = \"sqlite\"\ntemplate = \"a\""), "".as_ref()).unwrap();
}

#[test]
fn check_tools_ignores_non_executable_files() {
    let fx = Fixture::new();
    fs::write(fx.bin.join("createdb"), "").unwrap();
    fs::write(fx.bin.join("dropdb"), "").unwrap();
    let pg = cfg("provider = \"postgres\"");
    assert!(check_tools_in(&pg, fx.bin.as_os_str()).is_err());
}

// ---- real PostgreSQL (optional) ----

struct Cluster {
    bin: std::path::PathBuf,
    dir: std::path::PathBuf,
    root: bool,
    started: bool,
}

impl Cluster {
    /// Run `sh -c script`, as the `postgres` user when we are root.
    fn sh(root: bool, script: &str) -> std::process::Output {
        let mut cmd = if root {
            let mut c = std::process::Command::new("su");
            c.args(["postgres", "-s", "/bin/sh", "-c", script]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", script]);
            c
        };
        cmd.output().expect("run sh")
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if self.started {
            let script = format!(
                "'{}/pg_ctl' -D '{}/data' -m immediate stop >/dev/null 2>&1",
                self.bin.display(),
                self.dir.display()
            );
            Cluster::sh(self.root, &script);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn is_root() -> bool {
    let out = std::process::Command::new("id").arg("-u").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim() == "0"
}

#[test]
fn real_postgres_create_and_drop() {
    let Some(pg_bin) = std::env::var_os("BERTH_TEST_PG_BIN") else {
        eprintln!("note: BERTH_TEST_PG_BIN unset; skipping the real PostgreSQL test");
        return;
    };
    let pg_bin = std::path::PathBuf::from(pg_bin);
    let root = is_root();
    let fx = Fixture::new();
    let port = fx.range.0 + 9;
    // The postgres user must be able to traverse the cluster dir, so it is not under TMPDIR.
    let base = if root { "/var/tmp" } else { "/tmp" };
    let dir = std::path::PathBuf::from(format!("{base}/berth-pg-{}-{port}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut cluster = Cluster {
        bin: pg_bin.clone(),
        dir: dir.clone(),
        root,
        started: false,
    };
    if root {
        let out = std::process::Command::new("chown")
            .args(["postgres"])
            .arg(&dir)
            .output()
            .unwrap();
        if !out.status.success() {
            eprintln!("note: cannot chown the cluster dir to postgres; skipping");
            return;
        }
    }
    let d = dir.display();
    let b = pg_bin.display();
    let init = Cluster::sh(
        root,
        &format!("'{b}/initdb' -D '{d}/data' -A trust -U postgres >'{d}/initdb.log' 2>&1"),
    );
    assert!(
        init.status.success(),
        "initdb failed: {}",
        fs::read_to_string(dir.join("initdb.log")).unwrap_or_default()
    );
    let start = Cluster::sh(
        root,
        &format!(
            "'{b}/pg_ctl' -D '{d}/data' -w -l '{d}/server.log' \
             -o \"-p {port} -c listen_addresses=127.0.0.1 -k {d}\" start >/dev/null 2>&1"
        ),
    );
    cluster.started = true;
    assert!(
        start.status.success(),
        "pg_ctl start failed: {}",
        fs::read_to_string(dir.join("server.log")).unwrap_or_default()
    );
    let psql = |sql: &str| -> String {
        let out = std::process::Command::new(pg_bin.join("psql"))
            .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres"])
            .args(["-d", "postgres", "-tAc", sql])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "psql: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let exists = |name: &str| {
        psql(&format!(
            "select count(*) from pg_database where datname='{name}'"
        )) == "1"
    };
    // Template with a table, so the copy is visible.
    let run = |tool: &str, args: &[&str]| {
        let out = std::process::Command::new(pg_bin.join(tool))
            .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{tool}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run("createdb", &["app_dev"]);
    run("psql", &["-d", "app_dev", "-c", "create table t(x int)"]);

    let c = cfg(&format!(
        "provider = \"postgres\"\ntemplate = \"app_dev\"\nuser = \"postgres\"\nhost = \"127.0.0.1\"\nport = {port}"
    ));
    let plan = resolve(&c, "repo", "a", &fx.repo, &ports()).unwrap();
    assert_eq!(plan.name, "repo_a");
    let env = BTreeMap::from([(
        "PATH".to_string(),
        format!(
            "{}:{}",
            pg_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )]);
    assert!(!exists("repo_a"));
    let item = create(&fx.repo, &fx.repo, &c, &plan, &env).unwrap();
    assert_eq!(item.action, Action::Create);
    assert!(exists("repo_a"));
    // A second create fails: an existing database is never adopted.
    assert!(create(&fx.repo, &fx.repo, &c, &plan, &env).is_err());
    let mut e = entry(&plan, &c);
    e.created = true;
    let item = drop(&fx.repo, &fx.repo, &e, false, &env, T)
        .unwrap()
        .unwrap();
    assert_eq!(item.action, Action::Remove);
    assert!(!exists("repo_a"));
    // dropdb --if-exists on a missing database succeeds.
    drop(&fx.repo, &fx.repo, &e, true, &env, T).unwrap();
    assert!(exists("app_dev"));
    eprintln!("real postgres on port {port}: create and drop verified");
}
