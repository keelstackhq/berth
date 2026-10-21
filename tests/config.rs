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

use berth::config::{Provider, Remote, Seed};

fn db_cfg(toml: &str) -> berth::config::DbCfg {
    load_toml(toml).1.unwrap().db.expect("db table")
}

#[test]
fn db_and_cache_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config::load(dir.path()).unwrap();
    assert!(cfg.db.is_none());
    assert_eq!(cfg.cache.remote, Remote::None);
    assert_eq!(cfg.cache.hoard_dir, ".hoard");
    assert_eq!(cfg.cache.turbo_seed, Seed::Off);
}

#[test]
fn db_minimal_tables_per_provider() {
    let sqlite = db_cfg("[db]\nprovider = \"sqlite\"\ntemplate = \"dev.db\"\n");
    assert_eq!(sqlite.provider, Provider::Sqlite);
    assert_eq!(sqlite.provider.as_str(), "sqlite");
    assert_eq!(sqlite.env, "DATABASE_URL");
    assert_eq!(sqlite.timeout(), std::time::Duration::from_secs(300));
    assert_eq!(sqlite.sqlite_path(), Some("dev.db"));
    let with_path = db_cfg("[db]\nprovider = \"sqlite\"\ntemplate = \"a.db\"\npath = \"b.db\"\n");
    assert_eq!(with_path.sqlite_path(), Some("b.db"));

    let pg = db_cfg("[db]\nprovider = \"postgres\"\n");
    assert_eq!(pg.provider.as_str(), "postgres");
    assert_eq!(pg.pg_host(), "localhost");
    assert_eq!(pg.pg_port(), 5432);
    let pg = db_cfg(
        "[db]\nprovider = \"postgres\"\nhost = \"db.local\"\nport = 5433\nuser = \"me\"\n\
         template = \"app_tpl\"\nname = \"x\"\n",
    );
    assert_eq!((pg.pg_host(), pg.pg_port()), ("db.local", 5433));

    let compose = db_cfg(
        "[db]\nprovider = \"compose\"\nservices = [\"db\"]\nproject = \"p\"\ntimeout_secs = 5\n",
    );
    assert_eq!(compose.provider.as_str(), "compose");
    assert_eq!(compose.timeout(), std::time::Duration::from_secs(5));
}

#[test]
fn db_provider_errors() {
    let text = err_text("[db]\nprovider = \"mysql\"\n");
    assert!(
        text.contains("provider") && text.contains("mysql"),
        "{text}"
    );
    let text = err_text("[db]\nenv = \"X\"\n");
    assert!(text.contains("provider"), "{text}");
}

#[test]
fn db_key_for_wrong_provider() {
    let cases = [
        ("postgres", "path = \"x.db\"", "path"),
        ("sqlite", "services = [\"db\"]", "services"),
        ("compose", "name = \"x\"", "name"),
        ("compose", "template = \"x\"", "template"),
        ("sqlite", "host = \"h\"", "host"),
        ("sqlite", "port = 1", "port"),
        ("compose", "user = \"u\"", "user"),
        ("postgres", "project = \"p\"", "project"),
    ];
    for (provider, line, key) in cases {
        let extra = if provider == "sqlite" && key != "template" {
            "template = \"t.db\"\n"
        } else {
            ""
        };
        let text = err_text(&format!("[db]\nprovider = \"{provider}\"\n{extra}{line}\n"));
        assert!(
            text.contains(&format!("[db].{key} applies only to")),
            "{provider} {line}: {text}"
        );
    }
    let text = err_text("[db]\nprovider = \"postgres\"\npath = \"x.db\"\n");
    assert!(
        text.contains("[db].path applies only to provider = \"sqlite\""),
        "{text}"
    );
}

#[test]
fn db_sqlite_template_rules() {
    let text = err_text("[db]\nprovider = \"sqlite\"\n");
    assert!(
        text.contains("[db].template is required for provider = \"sqlite\""),
        "{text}"
    );
    for bad in ["../x.db", "/abs.db", "a/../../x.db", ""] {
        let text = err_text(&format!(
            "[db]\nprovider = \"sqlite\"\ntemplate = \"{bad}\"\n"
        ));
        assert!(text.contains("template"), "{bad}: {text}");
        let text = err_text(&format!(
            "[db]\nprovider = \"sqlite\"\ntemplate = \"t.db\"\npath = \"{bad}\"\n"
        ));
        assert!(text.contains("path"), "{bad}: {text}");
    }
}

#[test]
fn db_env_name_rules() {
    for bad in ["database_url", "PORT", "BERTH_X", "TMPDIR", "1A"] {
        let text = err_text(&format!("[db]\nprovider = \"postgres\"\nenv = \"{bad}\"\n"));
        assert!(text.contains(bad) && text.contains("env"), "{bad}: {text}");
    }
    assert!(
        load_toml("[db]\nprovider = \"postgres\"\nenv = \"\"\n")
            .1
            .is_ok()
    );
}

#[test]
fn db_value_rules() {
    let bad = [
        ("postgres", "host = \"a b\"", "host"),
        ("postgres", "user = \"a;b\"", "user"),
        ("postgres", "template = \"a-b\"", "template"),
        ("compose", "services = [\"-x\"]", "services"),
        ("compose", "services = [\"a b\"]", "services"),
        ("compose", "timeout_secs = 0", "timeout_secs"),
    ];
    for (provider, line, key) in bad {
        let text = err_text(&format!("[db]\nprovider = \"{provider}\"\n{line}\n"));
        assert!(text.contains(key), "{line}: {text}");
    }
}

#[test]
fn env_table_conflicts_with_db_and_cache() {
    let text = err_text("[db]\nprovider = \"postgres\"\n[env]\nDATABASE_URL = \"x\"\n");
    assert!(
        text.contains("set the URL in [db].url, not [env]"),
        "{text}"
    );
    let text = err_text("[db]\nprovider = \"postgres\"\nenv = \"DB\"\n[env]\nDB = \"x\"\n");
    assert!(text.contains("[db].url"), "{text}");
    // another variable, or no variable, is fine
    assert!(
        load_toml("[db]\nprovider = \"postgres\"\nenv = \"\"\n[env]\nDATABASE_URL = \"x\"\n")
            .1
            .is_ok()
    );
    assert!(load_toml("[env]\nDATABASE_URL = \"x\"\n").1.is_ok());

    let text = err_text("[db]\nprovider = \"compose\"\n[env]\nCOMPOSE_PROJECT_NAME = \"x\"\n");
    assert!(
        text.contains(
            "berth sets COMPOSE_PROJECT_NAME for provider = \"compose\"; use [db].project"
        ),
        "{text}"
    );
    assert!(
        load_toml("[db]\nprovider = \"postgres\"\n[env]\nCOMPOSE_PROJECT_NAME = \"x\"\n")
            .1
            .is_ok()
    );

    for key in ["TURBO_API", "TURBO_TOKEN", "TURBO_TEAM"] {
        let text = err_text(&format!(
            "[cache]\nremote = \"hoard\"\n[env]\n{key} = \"x\"\n"
        ));
        assert!(
            text.contains(&format!("berth sets {key} for [cache].remote = \"hoard\"")),
            "{text}"
        );
        assert!(load_toml(&format!("[env]\n{key} = \"x\"\n")).1.is_ok());
    }
}

#[test]
fn cache_values_and_errors() {
    let (_d, cfg) =
        load_toml("[cache]\nremote = \"hoard\"\nhoard_dir = \"h\"\nturbo_seed = \"link\"\n");
    let cache = cfg.unwrap().cache;
    assert_eq!(cache.remote, Remote::Hoard);
    assert_eq!(cache.hoard_dir, "h");
    assert_eq!(cache.turbo_seed, Seed::Link);
    assert_eq!(
        load_toml("[cache]\nturbo_seed = \"copy\"\n")
            .1
            .unwrap()
            .cache
            .turbo_seed,
        Seed::Copy
    );
    let text = err_text("[cache]\nturbo_seed = \"hardlink\"\n");
    assert!(text.contains("hardlink"), "{text}");
    let text = err_text("[cache]\nunknown = 1\n");
    assert!(
        text.contains("unknown") && text.contains("berth.toml"),
        "{text}"
    );
    let text = err_text("[cache]\nremote = \"s3\"\n");
    assert!(text.contains("s3"), "{text}");
}
