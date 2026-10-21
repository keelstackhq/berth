use std::collections::BTreeMap;
use std::path::Path;

use berth::BerthError;
use berth::config::{DbCfg, Provider};
use berth::db::{default_pg_name, default_project, entry, resolve};
use berth::manifest::PortBlock;

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
