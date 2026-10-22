use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use berth::cli::Format;
use berth::config::DbCfg;
use berth::db::{DbPlan, resolve};
use berth::env::{EnvInput, build_env, expand, format_env, next_command_hint};
use berth::manifest::PortBlock;

fn block() -> PortBlock {
    let names = BTreeMap::from([("web".to_string(), 0), ("api".to_string(), 1)]);
    PortBlock::new(21340, 10, &names)
}

#[test]
fn berth_vars_and_named_ports() {
    let ports = block();
    assert_eq!((ports.start, ports.end, ports.count()), (21340, 21349, 10));
    let extra = BTreeMap::new();
    let env = build_env(&EnvInput {
        repo_name: "hull",
        main: Path::new("/r/hull"),
        worktree: Path::new("/r/hull.berth/fix-login"),
        task: "fix-login",
        ports: &ports,
        extra: &extra,
        db: None,
    })
    .unwrap();
    assert_eq!(env["BERTH_TASK"], "fix-login");
    assert_eq!(env["BERTH_ROOT"], "/r/hull");
    assert_eq!(env["BERTH_WORKTREE"], "/r/hull.berth/fix-login");
    assert_eq!(env["BERTH_PORT_BASE"], "21340");
    assert_eq!(env["BERTH_PORT_COUNT"], "10");
    assert_eq!(env["PORT"], "21340");
    assert_eq!(env["WEB_PORT"], "21340");
    assert_eq!(env["API_PORT"], "21341");
    assert_eq!(
        env["TURBO_CACHE_DIR"],
        "/r/hull.berth/fix-login/.turbo/cache"
    );
    assert_eq!(
        env["VITE_CACHE_PATH"],
        "/r/hull.berth/fix-login/node_modules/.vite/task-cache"
    );
    assert_eq!(env["CARGO_TARGET_DIR"], "/r/hull.berth/fix-login/target");
    assert!(env["TMPDIR"].ends_with(".berth/tmp"));
}

#[test]
fn extra_values_are_expanded() {
    let ports = block();
    let extra = BTreeMap::from([(
        "DATABASE_URL".to_string(),
        "postgres://localhost:5432/app_{task_snake}?p={port.api}".to_string(),
    )]);
    let env = build_env(&EnvInput {
        repo_name: "hull",
        main: Path::new("/r/hull"),
        worktree: Path::new("/w"),
        task: "fix-login",
        ports: &ports,
        extra: &extra,
        db: None,
    })
    .unwrap();
    assert_eq!(
        env["DATABASE_URL"],
        "postgres://localhost:5432/app_fix_login?p=21341"
    );
}

#[test]
fn bad_extra_names_the_key() {
    let ports = block();
    let extra = BTreeMap::from([("K".to_string(), "{nope}".to_string())]);
    let err = build_env(&EnvInput {
        repo_name: "hull",
        main: Path::new("/r"),
        worktree: Path::new("/w"),
        task: "t",
        ports: &ports,
        extra: &extra,
        db: None,
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("berth.toml"), "{err}");
    assert!(
        err.contains("unknown placeholder {nope} in [env].K"),
        "{err}"
    );
}

fn ex(t: &str) -> Result<String, String> {
    expand(t, "fix-login", "hull", Path::new("/w"), &block())
}

#[test]
fn placeholders_expand() {
    assert_eq!(
        ex("postgres://localhost:5432/app_{task_snake}").unwrap(),
        "postgres://localhost:5432/app_fix_login"
    );
    assert_eq!(ex("{repo}-{task}").unwrap(), "hull-fix-login");
    assert_eq!(ex("{port.web}").unwrap(), "21340");
    assert_eq!(ex("{port.base}").unwrap(), "21340");
    assert_eq!(ex("{worktree}/x").unwrap(), "/w/x");
    assert_eq!(ex("a{{b}}c").unwrap(), "a{b}c");
}

#[test]
fn unknown_placeholder_is_config_error() {
    assert_eq!(ex("{nope}").unwrap_err(), "{nope}");
    assert_eq!(ex("{port.zzz}").unwrap_err(), "{port.zzz}");
    assert!(ex("a}b").is_err());
    assert!(ex("a{b").is_err());
    assert!(ex("a{b{c}").is_err());
}

fn sample() -> BTreeMap<String, String> {
    [
        ("A", "it's"),
        ("B", "a b"),
        ("C", "$HOME"),
        ("D", "x\"y"),
        ("E", "two\nlines"),
        ("F", "back\\slash"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

#[test]
fn sh_format_round_trips() {
    let env = sample();
    let out = format_env(&env, Format::Sh);
    assert!(out.ends_with('\n'));
    for (k, v) in &env {
        let script = format!("{out}printf '%s' \"${k}\"");
        let got = Command::new("sh").arg("-c").arg(&script).output().unwrap();
        assert!(got.status.success(), "{k}");
        assert_eq!(String::from_utf8(got.stdout).unwrap(), *v, "{k}");
    }
}

#[test]
fn fish_and_dotenv_escape() {
    let one = BTreeMap::from([("K".to_string(), "it's".to_string())]);
    assert_eq!(format_env(&one, Format::Fish), "set -gx K 'it\\'s'\n");
    assert_eq!(format_env(&one, Format::Dotenv), "K=\"it's\"\n");
    let two = BTreeMap::from([("K".to_string(), "a\"$b".to_string())]);
    assert_eq!(format_env(&two, Format::Dotenv), "K=\"a\\\"\\$b\"\n");
    let three = BTreeMap::from([("K".to_string(), "a\\b\nc".to_string())]);
    assert_eq!(format_env(&three, Format::Fish), "set -gx K 'a\\\\b\nc'\n");
    assert_eq!(format_env(&three, Format::Dotenv), "K=\"a\\\\b\\nc\"\n");
}

#[test]
fn pwsh_quotes_with_single_quotes_only() {
    let one = BTreeMap::from([("K".to_string(), "it's".to_string())]);
    assert_eq!(format_env(&one, Format::Pwsh), "$env:K = 'it''s'\n");
    // `$`, backtick, double quotes and backslashes are literal inside single quotes; a newline
    // stays a newline.
    let two = BTreeMap::from([("K".to_string(), "$HOME `x` \"q\" a\\b\nc".to_string())]);
    assert_eq!(
        format_env(&two, Format::Pwsh),
        "$env:K = '$HOME `x` \"q\" a\\b\nc'\n"
    );
    // PowerShell also treats the typographic single quotes as quote characters; double them.
    let three = BTreeMap::from([(
        "K".to_string(),
        "a\u{2018}b\u{2019}c\u{201a}d\u{201b}".to_string(),
    )]);
    assert_eq!(
        format_env(&three, Format::Pwsh),
        "$env:K = 'a\u{2018}\u{2018}b\u{2019}\u{2019}c\u{201a}\u{201a}d\u{201b}\u{201b}'\n"
    );
}

/// Round trip through a real `pwsh`; without one on PATH the test returns early (like the
/// APFS test in copy.rs).
#[test]
fn pwsh_format_round_trips() {
    if Command::new("pwsh")
        .args(["-NoProfile", "-Command", "exit 0"])
        .output()
        .is_err()
    {
        eprintln!("note: pwsh not on PATH; skipping the pwsh round trip");
        return;
    }
    let mut env = sample();
    env.insert("G".into(), "it\u{2019}s \u{2018}x\u{2019}".into());
    let out = format_env(&env, Format::Pwsh);
    let dir = tempfile::tempdir().unwrap();
    for (k, v) in &env {
        let script = dir.path().join("t.ps1");
        std::fs::write(&script, format!("{out}[Console]::Out.Write($env:{k})\n")).unwrap();
        let got = Command::new("pwsh")
            .args(["-NoProfile", "-File"])
            .arg(&script)
            .env_remove(k)
            .output()
            .unwrap();
        assert!(
            got.status.success(),
            "{k}: {}",
            String::from_utf8_lossy(&got.stderr)
        );
        // pwsh may translate `\n` to the platform newline when writing; compare normalized.
        let text = String::from_utf8(got.stdout).unwrap().replace("\r\n", "\n");
        assert_eq!(text, *v, "{k}");
    }
}

#[test]
fn json_is_one_sorted_object() {
    let out = format_env(&sample(), Format::Json);
    assert!(out.ends_with('\n'));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["A"], "it's");
    assert_eq!(v["E"], "two\nlines");
}

/// The default-name hash for the fixed main path `/main` used by `plan`.
fn h() -> String {
    format!("{:016x}", berth::ports::fnv1a64(b"/main"))[..6].to_string()
}

fn plan(toml_src: &str, repo: &str, task: &str, wt: &str) -> DbPlan {
    let cfg: DbCfg = toml::from_str(toml_src).unwrap();
    resolve(
        &cfg,
        repo,
        Path::new("/main"),
        task,
        Path::new(wt),
        &block(),
    )
    .unwrap()
}

fn env_with(
    plan: &DbPlan,
    extra: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, berth::BerthError> {
    build_env(&EnvInput {
        repo_name: "hull",
        main: Path::new("/r/hull"),
        worktree: Path::new("/w"),
        task: "fix-login",
        ports: &block(),
        extra,
        db: Some(plan),
    })
}

#[test]
fn db_url_and_compose_project_are_exported() {
    let pg = plan("provider = \"postgres\"", "hull", "fix-login", "/w");
    let env = env_with(&pg, &BTreeMap::new()).unwrap();
    assert_eq!(
        env["DATABASE_URL"],
        format!("postgres://localhost:5432/hull_{}_fix_login", h())
    );
    assert!(!env.contains_key("COMPOSE_PROJECT_NAME"));

    let c = plan(
        "provider = \"compose\"\nurl = \"postgres://u@localhost:{port.api}/{db.name}\"",
        "hull",
        "fix-login",
        "/w",
    );
    let env = env_with(&c, &BTreeMap::new()).unwrap();
    assert_eq!(
        env["COMPOSE_PROJECT_NAME"],
        format!("hull-{}-fix-login", h())
    );
    assert_eq!(
        env["DATABASE_URL"],
        format!("postgres://u@localhost:21341/hull-{}-fix-login", h())
    );

    let c = plan("provider = \"compose\"", "hull", "fix-login", "/w");
    let env = env_with(&c, &BTreeMap::new()).unwrap();
    assert!(!env.contains_key("DATABASE_URL"));

    let e = plan("provider = \"postgres\"\nenv = \"\"", "hull", "t", "/w");
    assert!(
        !env_with(&e, &BTreeMap::new())
            .unwrap()
            .contains_key("DATABASE_URL")
    );
}

#[test]
fn env_values_use_db_placeholders() {
    let pg = plan("provider = \"postgres\"", "hull", "fix-login", "/w");
    let extra = BTreeMap::from([
        ("A".to_string(), "{db.url}".to_string()),
        ("B".to_string(), "n={db.name}".to_string()),
    ]);
    let env = env_with(&pg, &extra).unwrap();
    assert_eq!(
        env["A"],
        format!("postgres://localhost:5432/hull_{}_fix_login", h())
    );
    assert_eq!(env["B"], format!("n=hull_{}_fix_login", h()));

    let sq = plan(
        "provider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"",
        "hull",
        "t",
        "/w",
    );
    let extra = BTreeMap::from([("P".to_string(), "{db.path}".to_string())]);
    assert_eq!(env_with(&sq, &extra).unwrap()["P"], "/w/db/dev.sqlite");
}

#[test]
fn db_path_with_postgres_is_unknown() {
    let pg = plan("provider = \"postgres\"", "hull", "t", "/w");
    let extra = BTreeMap::from([("P".to_string(), "{db.path}".to_string())]);
    let err = env_with(&pg, &extra).unwrap_err().to_string();
    assert!(
        err.contains("unknown placeholder {db.path} in [env].P"),
        "{err}"
    );
}

#[test]
fn db_placeholder_without_db_is_unknown() {
    let extra = BTreeMap::from([("P".to_string(), "{db.url}".to_string())]);
    let err = build_env(&EnvInput {
        repo_name: "hull",
        main: Path::new("/r"),
        worktree: Path::new("/w"),
        task: "t",
        ports: &block(),
        extra: &extra,
        db: None,
    })
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("unknown placeholder {db.url} in [env].P"),
        "{err}"
    );
}

#[test]
fn next_command_hint_per_shell() {
    assert_eq!(
        next_command_hint("/w/a b", false),
        "cd '/w/a b' && eval \"$(berth env)\""
    );
    assert_eq!(
        next_command_hint(r"C:\w\it's", true),
        r"Set-Location -LiteralPath 'C:\w\it''s'; berth env --format pwsh | Out-String | Invoke-Expression"
    );
}
