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
    let scan_files = |when: &str| {
        for file in [
            fx.repo.join(".agents/worktrees.json"),
            fx.state.join("ports.json"),
        ] {
            let text = fs::read_to_string(&file).unwrap_or_default();
            assert!(!text.is_empty(), "{file:?} is empty {when}");
            for canary in [ENV_CANARY, VARS_CANARY] {
                assert!(!text.contains(canary), "canary in {file:?} {when}");
            }
        }
    };
    // The first (plain) run of each command must succeed. Repeats (--json, -v, a second `up`,
    // `down` after the entry is gone) may legitimately exit 1 or 3, never 2 (usage).
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
    scan_files("after up");
    ok(&["up", "a"]);
    ok(&["ls"]);
    for format in ["sh", "fish", "dotenv", "json"] {
        ok(&["env", "a", "--format", format]);
    }
    scan_files("after env");
    ok(&["doctor"]);
    scan_files("before down");
    ok(&["down", "a"]);

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
