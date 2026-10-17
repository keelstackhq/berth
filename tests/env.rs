use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use berth::cli::Format;
use berth::env::{EnvInput, build_env, expand, format_env};
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
fn json_is_one_sorted_object() {
    let out = format_env(&sample(), Format::Json);
    assert!(out.ends_with('\n'));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["A"], "it's");
    assert_eq!(v["E"], "two\nlines");
}
