use std::path::Path;

use berth::config::{self, Config, CopySource};

const DEFAULT_INCLUDE: [&str; 4] = [".env", ".env.local", "apps/*/.env.local", ".dev.vars"];

fn defaults() -> Vec<String> {
    DEFAULT_INCLUDE.iter().map(|s| s.to_string()).collect()
}

fn load_toml(toml: &str) -> (tempfile::TempDir, Result<Config, berth::BerthError>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("berth.toml"), toml).unwrap();
    let result = config::load(dir.path());
    (dir, result)
}

fn err_text(toml: &str) -> String {
    load_toml(toml).1.unwrap_err().to_string()
}

#[test]
fn missing_file_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config::load(dir.path()).unwrap();
    assert_eq!(cfg.version, 1);
    assert_eq!(cfg.worktree.root, "../{repo}.berth");
    assert_eq!(cfg.worktree.branch_prefix, "berth/");
    assert_eq!(cfg.worktree.base, "HEAD");
    assert_eq!(cfg.worktree.max, 6);
    assert_eq!(cfg.ports.count, 10);
    assert_eq!(cfg.ports.range, [21000, 30999]);
    assert_eq!(cfg.ports.names()["web"], 0);
    assert_eq!(cfg.setup.timeout_secs, 900);
    assert_eq!(cfg.teardown.timeout_secs, 900);
    assert!(cfg.setup.run.is_empty() && cfg.env.is_empty());
    assert_eq!(cfg.max_bytes(), 1_048_576);
    match cfg.copy_source(dir.path()) {
        CopySource::Include(list) => assert_eq!(list, defaults()),
        other => panic!("expected Include, got {other:?}"),
    }
}

#[test]
fn unknown_key_is_error() {
    let text = err_text("[ports]\nfoo = 1\n");
    assert!(
        text.contains("berth.toml") && text.contains("foo"),
        "{text}"
    );
}

#[test]
fn invalid_toml_names_line() {
    let text = err_text("[ports");
    assert!(
        text.contains("berth.toml") && text.contains("line 1"),
        "{text}"
    );
}

#[test]
fn name_offset_outside_block() {
    let text = err_text("[ports]\ncount = 2\nnames = { web = 5 }\n");
    assert!(text.contains("web"), "{text}");
}

#[test]
fn default_names_trimmed_to_count() {
    let (_d, cfg) = load_toml("[ports]\ncount = 2\n");
    let names = cfg.unwrap().ports.names();
    let want: std::collections::BTreeMap<String, u16> =
        [("web".to_string(), 0), ("api".to_string(), 1)].into();
    assert_eq!(names, want);
}

#[test]
fn env_cannot_shadow_berth_vars() {
    for key in ["PORT", "WEB_PORT", "BERTH_X", "TMPDIR", "CARGO_TARGET_DIR"] {
        let text = err_text(&format!("[env]\n{key} = \"1\"\n"));
        assert!(text.contains(key), "{key}: {text}");
    }
    assert!(load_toml("[env]\nSMTP_PORT = \"1025\"\n").1.is_ok());
}

#[test]
fn env_shadow_follows_explicit_names() {
    let toml = "[ports]\nnames = { mail = 0 }\n[env]\nWEB_PORT = \"1\"\n";
    assert!(load_toml(toml).1.is_ok());
    let text = err_text("[ports]\nnames = { mail = 0 }\n[env]\nMAIL_PORT = \"1\"\n");
    assert!(text.contains("MAIL_PORT"), "{text}");
}

#[test]
fn copy_max_bytes_only_keeps_default_include() {
    let (d, cfg) = load_toml("[copy]\nmax_bytes = 10\n");
    let cfg = cfg.unwrap();
    assert_eq!(cfg.max_bytes(), 10);
    match cfg.copy_source(d.path()) {
        CopySource::Include(list) => assert_eq!(list, defaults()),
        other => panic!("expected Include, got {other:?}"),
    }
}

#[test]
fn worktreeinclude_used_without_copy_table() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".worktreeinclude"), ".env\n").unwrap();
    let cfg = config::load(dir.path()).unwrap();
    assert_eq!(
        cfg.copy_source(dir.path()),
        CopySource::WorktreeInclude(dir.path().join(".worktreeinclude"))
    );

    std::fs::write(
        dir.path().join("berth.toml"),
        "[copy]\ninclude = [\".env\"]\n",
    )
    .unwrap();
    let cfg = config::load(dir.path()).unwrap();
    assert_eq!(
        cfg.copy_source(dir.path()),
        CopySource::Include(vec![".env".to_string()])
    );
}

#[test]
fn validation_rules() {
    let bad = [
        ("version = 2\n", "version"),
        ("[ports]\nrange = [30, 20]\n", "range"),
        ("[ports]\ncount = 0\n", "count"),
        ("[ports]\ncount = 20\nrange = [100, 105]\n", "count"),
        ("[ports]\nnames = { Web = 0 }\n", "Web"),
        ("[env]\nlower = \"x\"\n", "lower"),
        ("[worktree]\nmax = 0\n", "max"),
        ("[worktree]\nbranch_prefix = \"-x\"\n", "branch_prefix"),
    ];
    for (toml, needle) in bad {
        let text = err_text(toml);
        assert!(
            text.contains("berth.toml") && text.contains(needle),
            "{toml}: {text}"
        );
    }
    assert!(load_toml("[worktree]\nbranch_prefix = \"\"\n").1.is_ok());
}

#[test]
fn load_reads_only_from_given_dir() {
    assert!(config::load(Path::new("/nonexistent-berth-dir")).is_ok());
}
