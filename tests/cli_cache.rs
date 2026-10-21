mod common;

use common::{Fixture, git, write};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Output;

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).expect("canonicalize")
}

fn wt(fx: &Fixture, task: &str) -> PathBuf {
    let name = format!("{}.berth", fx.repo.file_name().unwrap().to_string_lossy());
    canon(fx.root.path()).join(name).join(task)
}

fn run(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("stdout is not one JSON object ({e}): {}", stdout(out));
    })
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn items(out: &Output, kind: &str) -> Vec<Value> {
    json(out)["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == kind)
        .cloned()
        .collect()
}

/// A repo whose committed `.gitignore` ignores `.turbo/`, with a seedable cache in main.
fn seeded(mode: &str) -> Fixture {
    let fx = Fixture::new();
    write(&fx.repo, ".gitignore", ".env\n.turbo/\n");
    git(&fx.repo, &["add", ".gitignore"]);
    git(&fx.repo, &["commit", "-q", "-m", "ignore turbo"]);
    let cache = fx.repo.join(".turbo/cache");
    write(&cache, "h1.tar.zst", "archive one");
    write(&cache, "h1-meta.json", "{\"hash\":\"h1\"}");
    write(&cache, "h2.tar.zst", "archive two");
    write(&cache, "x.tmp", "partial");
    write(&cache, "sub/deep.tar.zst", "nested");
    if !mode.is_empty() {
        fx.config(&fx.repo, "", &format!("[cache]\nturbo_seed = \"{mode}\""));
    }
    fx
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn link_mode_shares_inodes_and_skips_tmp_and_subdirs() {
    let fx = seeded("link");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache.len(), 1, "{cache:?}");
    assert_eq!(cache[0]["action"], "create");
    assert_eq!(cache[0]["reason"], "seeded 3 entries (3 linked, 0 copied)");
    let seeded_dir = wt(&fx, "a").join(".turbo/cache");
    assert_eq!(
        names(&seeded_dir),
        ["h1-meta.json", "h1.tar.zst", "h2.tar.zst"]
    );
    for name in ["h1.tar.zst", "h1-meta.json", "h2.tar.zst"] {
        let a = fs::metadata(fx.repo.join(".turbo/cache").join(name)).unwrap();
        let b = fs::metadata(seeded_dir.join(name)).unwrap();
        assert_eq!(a.ino(), b.ino(), "{name} should be hard linked");
    }
}

#[test]
fn turbo_style_rewrite_in_worktree_leaves_main_untouched() {
    let fx = seeded("link");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let dir = wt(&fx, "a").join(".turbo/cache");
    // turbo writes a temp file and renames it over the entry.
    fs::write(dir.join("h1.tar.zst.tmp"), "rewritten").unwrap();
    fs::rename(dir.join("h1.tar.zst.tmp"), dir.join("h1.tar.zst")).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("h1.tar.zst")).unwrap(),
        "rewritten"
    );
    assert_eq!(
        fs::read_to_string(fx.repo.join(".turbo/cache/h1.tar.zst")).unwrap(),
        "archive one"
    );
}

#[test]
fn copy_mode_copies_bytes_with_new_inodes() {
    let fx = seeded("copy");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache[0]["action"], "create");
    assert_eq!(cache[0]["reason"], "seeded 3 entries (0 linked, 3 copied)");
    let seeded_dir = wt(&fx, "a").join(".turbo/cache");
    assert_eq!(
        names(&seeded_dir),
        ["h1-meta.json", "h1.tar.zst", "h2.tar.zst"]
    );
    for name in ["h1.tar.zst", "h1-meta.json", "h2.tar.zst"] {
        let src = fx.repo.join(".turbo/cache").join(name);
        let dst = seeded_dir.join(name);
        assert_ne!(
            fs::metadata(&src).unwrap().ino(),
            fs::metadata(&dst).unwrap().ino()
        );
        assert_eq!(fs::read(&src).unwrap(), fs::read(&dst).unwrap());
    }
}

#[test]
fn turbo_not_ignored_is_skipped_and_leaves_no_directory() {
    let fx = seeded("link");
    // Undo the ignore rule in the committed .gitignore and in main.
    write(&fx.repo, ".gitignore", ".env\n");
    git(&fx.repo, &["add", ".gitignore"]);
    git(&fx.repo, &["commit", "-q", "-m", "unignore"]);
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache[0]["action"], "skipped");
    assert_eq!(
        cache[0]["reason"],
        ".turbo/ is not gitignored; add it to .gitignore"
    );
    assert!(!wt(&fx, "a").join(".turbo").exists());
}

#[test]
fn missing_source_is_skipped() {
    let fx = seeded("link");
    fs::remove_dir_all(fx.repo.join(".turbo")).unwrap();
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let cache = items(&out, "cache");
    assert_eq!(cache[0]["action"], "skipped");
    assert_eq!(cache[0]["reason"], "no .turbo/cache in the main checkout");
    assert!(!wt(&fx, "a").join(".turbo").exists());
}

#[test]
fn default_config_has_no_cache_item_and_no_turbo_dir() {
    let fx = seeded("");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(items(&out, "cache").is_empty());
    assert!(!wt(&fx, "a").join(".turbo").exists());
}

#[test]
fn existing_targets_are_kept() {
    let fx = seeded("copy");
    // A file the worktree already has (tracked) must not be replaced.
    write(&fx.repo, ".turbo/cache/h1.tar.zst", "archive one");
    git(&fx.repo, &["add", "-f", ".turbo/cache/h1.tar.zst"]);
    git(&fx.repo, &["commit", "-q", "-m", "track one entry"]);
    write(&fx.repo, ".turbo/cache/h1.tar.zst", "changed in main");
    let out = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let dir = wt(&fx, "a").join(".turbo/cache");
    assert_eq!(
        fs::read_to_string(dir.join("h1.tar.zst")).unwrap(),
        "archive one"
    );
    assert_eq!(
        items(&out, "cache")[0]["reason"],
        "seeded 2 entries (0 linked, 2 copied)"
    );
}

#[test]
fn down_after_seeding_is_not_dirty() {
    let fx = seeded("link");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let out = run(&fx, &["down", "a"]);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    assert!(!wt(&fx, "a").exists());
}

// ---- Task 7: hoard remote-cache export ----

const CANARY: &str = "BERTH_CANARY_hoard_7c1f";

fn hoard_fixture(rest: &str) -> Fixture {
    let fx = Fixture::new();
    write(&fx.repo, ".gitignore", ".env\n.hoard/\n.hoard-seen\n");
    git(&fx.repo, &["add", ".gitignore"]);
    git(&fx.repo, &["commit", "-q", "-m", "ignore hoard"]);
    write(&fx.repo, ".hoard/port", "41234\n");
    write(&fx.repo, ".hoard/token", CANARY);
    fx.config(
        &fx.repo,
        "",
        &format!("[cache]\nremote = \"hoard\"\n{rest}"),
    );
    fx
}

fn token_path(fx: &Fixture) -> String {
    canon(&fx.repo).join(".hoard/token").display().to_string()
}

#[test]
fn hooks_get_the_hoard_variables() {
    let fx = hoard_fixture("");
    let seen = canon(fx.root.path()).join("teardown-seen");
    fx.config(
        &fx.repo,
        "",
        &format!(
            "[cache]\nremote = \"hoard\"\n[setup]\nrun = [\"echo \\\"$TURBO_API $TURBO_TEAM $TURBO_TOKEN\\\" > .hoard-seen\"]\n\
             [teardown]\nrun = [\"echo \\\"$TURBO_API $TURBO_TEAM $TURBO_TOKEN\\\" > '{}'\"]",
            seen.display()
        ),
    );
    let out = run(&fx, &["up", "a", "-v"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(wt(&fx, "a").join(".hoard-seen")).unwrap(),
        format!("http://127.0.0.1:41234 hoard {CANARY}\n")
    );
    assert!(!stdout(&out).contains(CANARY) && !stderr(&out).contains(CANARY));
    let down = run(&fx, &["down", "a", "-v"]);
    assert_eq!(code(&down), 0, "{}", stderr(&down));
    assert_eq!(
        fs::read_to_string(&seen).unwrap(),
        format!("http://127.0.0.1:41234 hoard {CANARY}\n")
    );
    assert!(!stdout(&down).contains(CANARY) && !stderr(&down).contains(CANARY));
}

#[test]
fn rollback_teardown_gets_the_hoard_variables() {
    let fx = hoard_fixture("");
    let seen = canon(fx.root.path()).join("rollback-seen");
    fx.config(
        &fx.repo,
        "",
        &format!(
            "[cache]\nremote = \"hoard\"\n[setup]\nrun = [\"exit 4\"]\n\
             [teardown]\nrun = [\"echo \\\"$TURBO_TOKEN\\\" > '{}'\"]",
            seen.display()
        ),
    );
    let out = run(&fx, &["up", "a"]);
    assert_eq!(code(&out), 3);
    assert_eq!(fs::read_to_string(&seen).unwrap(), format!("{CANARY}\n"));
    assert!(!stdout(&out).contains(CANARY) && !stderr(&out).contains(CANARY));
}

#[test]
fn env_sh_prints_a_token_substitution_not_the_token() {
    let fx = hoard_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let out = run(&fx, &["env", "a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(!text.contains(CANARY) && !stderr(&out).contains(CANARY));
    assert!(
        text.contains("export TURBO_API='http://127.0.0.1:41234'\n"),
        "{text}"
    );
    assert!(text.contains("export TURBO_TEAM='hoard'\n"), "{text}");
    let last = text.lines().last().unwrap();
    assert_eq!(
        last,
        format!("export TURBO_TOKEN=\"$(cat '{}')\"", token_path(&fx))
    );
    // Sorted block: TURBO_API sits among the other exports, before the token line.
    let keys: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("export "))
        .filter_map(|l| l.split('=').next())
        .filter(|k| *k != "TURBO_TOKEN")
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);

    // Evaluating the output really yields the token.
    let file = fx.root.path().join("env.sh");
    fs::write(&file, &text).unwrap();
    let eval = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "eval \"$(cat '{}')\"; printf %s \"$TURBO_TOKEN\"",
            file.display()
        ))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&eval.stdout), CANARY);
}

#[test]
fn env_fish_reads_the_token_file() {
    let fx = hoard_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let out = run(&fx, &["env", "a", "--format", "fish"]);
    let text = stdout(&out);
    assert!(!text.contains(CANARY));
    assert!(
        text.contains("set -gx TURBO_API 'http://127.0.0.1:41234'\n"),
        "{text}"
    );
    assert_eq!(
        text.lines().last().unwrap(),
        format!("set -gx TURBO_TOKEN (cat '{}')", token_path(&fx))
    );
}

#[test]
fn env_dotenv_and_json_omit_the_token_and_say_so() {
    let fx = hoard_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    for format in ["dotenv", "json"] {
        let out = run(&fx, &["env", "a", "--format", format]);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        let text = stdout(&out);
        assert!(
            !text.contains("TURBO_TOKEN") && !text.contains(CANARY),
            "{text}"
        );
        assert!(
            text.contains("TURBO_API") && text.contains("TURBO_TEAM"),
            "{text}"
        );
        assert_eq!(
            stderr(&out).trim(),
            format!(
                "note: TURBO_TOKEN is not printed; read it from {}",
                token_path(&fx)
            )
        );
    }
    let out = run(&fx, &["env", "a", "--format", "json"]);
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["TURBO_API"], "http://127.0.0.1:41234");
    assert_eq!(v["TURBO_TEAM"], "hoard");
}

#[test]
fn manifest_and_json_reports_never_contain_hoard_variables_or_the_token() {
    let fx = hoard_fixture("");
    let up = run(&fx, &["up", "a", "--json"]);
    assert_eq!(code(&up), 0, "{}", stderr(&up));
    let env = run(&fx, &["env", "a", "--json"]);
    let manifest = fs::read_to_string(fx.repo.join(".agents/worktrees.json")).unwrap();
    for text in [
        stdout(&up),
        stdout(&env),
        manifest,
        stderr(&up),
        stderr(&env),
    ] {
        // TURBO_CACHE_DIR is berth's own cache variable; the hoard ones must be absent.
        for name in ["TURBO_API", "TURBO_TEAM", "TURBO_TOKEN", CANARY] {
            assert!(!text.contains(name), "{name} in {text}");
        }
    }
}

#[test]
fn no_server_warns_once_and_exports_nothing() {
    let fx = hoard_fixture("");
    fs::remove_file(fx.repo.join(".hoard/port")).unwrap();
    let up = run(&fx, &["up", "a"]);
    assert_eq!(code(&up), 0, "{}", stderr(&up));
    assert_eq!(
        stderr(&up)
            .matches("warning: no running hoard server")
            .count(),
        1
    );
    let out = run(&fx, &["env", "a"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!stdout(&out).contains("TURBO_API") && !stdout(&out).contains("TURBO_TEAM"));
    assert!(!stdout(&out).contains("TURBO_TOKEN"));
    assert_eq!(
        stderr(&out)
            .matches("warning: no running hoard server")
            .count(),
        1
    );
    assert!(
        stderr(&out).contains("start hoard serve --storage fs in"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn bad_port_or_empty_token_counts_as_no_server() {
    let fx = hoard_fixture("");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    write(&fx.repo, ".hoard/port", "not a port");
    let out = run(&fx, &["env", "a"]);
    assert!(stderr(&out).contains("warning: no running hoard server"));
    write(&fx.repo, ".hoard/port", "41234");
    write(&fx.repo, ".hoard/token", "\n");
    let out = run(&fx, &["env", "a"]);
    assert!(stderr(&out).contains("warning: no running hoard server"));
    assert!(!stdout(&out).contains("TURBO_API") && !stdout(&out).contains("TURBO_TOKEN"));
}

#[test]
fn remote_none_prints_no_turbo_and_no_warning() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let out = run(&fx, &["env", "a"]);
    assert_eq!(code(&out), 0);
    assert!(!stdout(&out).contains("TURBO_API") && !stdout(&out).contains("TURBO_TOKEN"));
    assert!(!stderr(&out).contains("warning"), "{}", stderr(&out));
}

#[test]
fn hoard_dir_is_relative_to_main() {
    let fx = hoard_fixture("hoard_dir = \"state/hoard\"\n");
    write(&fx.repo, "state/hoard/port", "5555");
    write(&fx.repo, "state/hoard/token", "tok");
    assert_eq!(code(&run(&fx, &["up", "a"])), 0);
    let out = run(&fx, &["env", "a"]);
    assert!(
        stdout(&out).contains("TURBO_API='http://127.0.0.1:5555'"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("state/hoard/token"),
        "{}",
        stdout(&out)
    );
}
