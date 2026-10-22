mod common;

use common::{Fixture, write};
use serde_json::Value;
use std::fs;

const ENV_CANARY: &str = "BERTH_CANARY_5d1e";
const VARS_CANARY: &str = "BERTH_CANARY_9a7c";

fn run(fx: &Fixture, args: &[&str], seen: &mut Vec<(String, String)>) -> std::process::Output {
    let out = fx.berth().args(args).output().expect("run berth");
    seen.push((
        format!("{args:?}"),
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ));
    out
}

#[test]
fn canary_never_printed() {
    let fx = Fixture::new();
    fx.config(&fx.repo, "", "");
    write(&fx.repo, ".env", &format!("TOKEN={ENV_CANARY}\n"));
    write(&fx.repo, ".dev.vars", &format!("KEY={VARS_CANARY}\n"));
    fs::write(fx.repo.join(".gitignore"), ".env\n.dev.vars\n").unwrap();
    common::git(&fx.repo, &["add", ".gitignore"]);
    common::git(&fx.repo, &["commit", "-q", "-m", "ignore vars"]);

    let mut seen = Vec::new();
    let scan_files = |when: &str, must_exist: bool| {
        for file in [
            fx.repo.join(".agents/worktrees.json"),
            fx.state.join("ports.json"),
        ] {
            let text = fs::read_to_string(&file).unwrap_or_default();
            if must_exist {
                assert!(!text.is_empty(), "{file:?} is empty {when}");
            }
            for canary in [ENV_CANARY, VARS_CANARY] {
                assert!(!text.contains(canary), "canary in {file:?} {when}");
            }
        }
    };
    // Every plain run (no --json, no -v) must exit 0; a second plain `up` of a ready task is
    // idempotent. The --json / -v repeats (e.g. `down` after the entry is gone) may exit 1 or 3,
    // never 2 (usage).
    let mut ok = |args: &[&str]| {
        for json in [false, true] {
            for verbose in [false, true] {
                let mut full: Vec<&str> = args.to_vec();
                if json {
                    full.push("--json");
                }
                if verbose {
                    full.push("-v");
                }
                let out = run(&fx, &full, &mut seen);
                let code = out.status.code();
                if !json && !verbose {
                    assert_eq!(code, Some(0), "{full:?}: {}", seen.last().unwrap().1);
                } else {
                    assert!(matches!(code, Some(0 | 1 | 3)), "{full:?} exited {code:?}");
                }
            }
        }
    };
    ok(&["up", "a"]);
    scan_files("after up", true);
    ok(&["up", "a"]);
    ok(&["ls"]);
    for format in ["sh", "fish", "pwsh", "dotenv", "json"] {
        ok(&["env", "a", "--format", format]);
    }
    scan_files("after env", true);
    ok(&["doctor"]);
    scan_files("before down", true);
    ok(&["down", "a"]);
    scan_files("at end", false);

    for (args, text) in &seen {
        for canary in [ENV_CANARY, VARS_CANARY] {
            assert!(!text.contains(canary), "canary in output of {args}");
        }
    }

    fx.config(&fx.repo, "", "[setup]\nrun = [\"cat .env; exit 1\"]");
    let out = fx
        .berth()
        .args(["up", "b", "--json", "-v"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The hook's own output reaches stderr by design; this proves the canary was really copied.
    assert!(String::from_utf8_lossy(&out.stderr).contains(ENV_CANARY));
    let _: Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    assert!(!stdout.contains(ENV_CANARY) && !stdout.contains(VARS_CANARY));
}

const SQLITE_CANARY: &str = "BERTH_CANARY_sqlite_91ab";
const HOARD_CANARY: &str = "BERTH_CANARY_hoard_7c1f";
const DOTENV_CANARY: &str = "BERTH_CANARY_dotenv_44e0";

#[test]
fn m2_canaries_never_printed() {
    let fx = Fixture::new();
    fx.config(
        &fx.repo,
        "",
        "[db]\nprovider = \"sqlite\"\ntemplate = \"db/dev.sqlite\"\n\
         [cache]\nremote = \"hoard\"\nturbo_seed = \"link\"",
    );
    write(&fx.repo, ".env", &format!("TOKEN={DOTENV_CANARY}\n"));
    write(&fx.repo, "db/dev.sqlite", SQLITE_CANARY);
    write(&fx.repo, ".hoard/port", "41234");
    write(&fx.repo, ".hoard/token", HOARD_CANARY);
    write(&fx.repo, ".turbo/cache/h1.tar.zst", "cache entry");
    let exclude = fx.repo.join(".git/info/exclude");
    let old = fs::read_to_string(&exclude).unwrap_or_default();
    fs::write(
        &exclude,
        format!("{old}/db/dev.sqlite\n/.hoard/\n/.turbo/\n"),
    )
    .unwrap();

    let canaries = [SQLITE_CANARY, HOARD_CANARY, DOTENV_CANARY];
    let mut seen = Vec::new();
    let mut step = |args: &[&str]| {
        let out = run(&fx, args, &mut seen);
        assert!(
            matches!(out.status.code(), Some(0 | 1)),
            "{args:?} exited {:?}: {}",
            out.status.code(),
            seen.last().unwrap().1
        );
        out
    };
    step(&["up", "a", "--json"]);
    step(&["up", "a"]);
    step(&["-v", "up", "b"]);
    // The secrets really were copied and seeded, so their absence from the output means something.
    let a = fs::canonicalize(fx.root.path())
        .unwrap()
        .join("repo.berth/a");
    assert_eq!(
        fs::read_to_string(a.join("db/dev.sqlite")).unwrap(),
        SQLITE_CANARY
    );
    assert!(
        fs::read_to_string(a.join(".env"))
            .unwrap()
            .contains(DOTENV_CANARY)
    );
    assert!(a.join(".turbo/cache/h1.tar.zst").exists());
    let mut sh_text = String::new();
    for format in ["sh", "fish", "pwsh", "dotenv", "json"] {
        let out = step(&["env", "a", "--format", format]);
        if matches!(format, "sh" | "fish" | "pwsh") {
            sh_text.push_str(&String::from_utf8_lossy(&out.stdout));
        }
    }
    // The shell formats read the token from its file; they name the path, never the value.
    assert!(sh_text.contains(".hoard/token"), "{sh_text}");
    assert!(sh_text.contains("TURBO_API"), "{sh_text}");
    step(&["ls", "--json"]);
    step(&["doctor", "--json"]);
    step(&["gc", "--dry-run", "--json"]);
    for file in [
        fx.repo.join(".agents/worktrees.json"),
        fx.state.join("ports.json"),
    ] {
        let text = fs::read_to_string(&file).unwrap_or_default();
        assert!(!text.is_empty(), "{file:?} is empty");
        for canary in canaries {
            assert!(!text.contains(canary), "canary in {file:?}");
        }
    }
    step(&["down", "a"]);
    step(&["down", "b", "-v"]);

    for (args, text) in &seen {
        for canary in canaries {
            assert!(!text.contains(canary), "{canary} in output of {args}");
        }
    }
}
