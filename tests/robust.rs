//! Fuzz-ish robustness: corrupt or truncated state files never crash berth, never get
//! overwritten (so no worktree record is lost), and every error is one line with a fix.

mod common;

use common::Fixture;
use std::fs;
use std::path::PathBuf;
use std::process::Output;

fn run(fx: &Fixture, args: &[&str]) -> Output {
    fx.berth().args(args).output().expect("run berth")
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit code, not a signal")
}

fn files(fx: &Fixture) -> (PathBuf, PathBuf) {
    (
        fx.repo.join(".agents/worktrees.json"),
        fx.state.join("ports.json"),
    )
}

/// Every cut of a valid file, plus junk, as corrupt inputs.
fn corruptions(valid: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = vec![
        vec![],
        b"{".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"{\"version\":1}".to_vec(),
        vec![0xff, 0xfe, 0x00, 0x7b],
    ];
    for cut in (1..valid.len()).step_by(37) {
        out.push(valid[..cut].to_vec());
    }
    out
}

#[test]
fn corrupt_state_never_crashes_or_loses_records() {
    let fx = Fixture::new();
    let up = run(&fx, &["up", "a"]);
    assert_eq!(code(&up), 0, "{}", String::from_utf8_lossy(&up.stderr));
    let (manifest, registry) = files(&fx);
    for (which, path) in [("manifest", &manifest), ("registry", &registry)] {
        let valid = fs::read(path).expect("state file exists");
        for bad in corruptions(&valid) {
            fs::write(path, &bad).unwrap();
            for args in [
                vec!["up", "b"],
                vec!["ls"],
                vec!["down", "a"],
                vec!["gc"],
                vec!["doctor"],
                vec!["env", "a"],
            ] {
                let o = run(&fx, &args);
                let err = String::from_utf8_lossy(&o.stderr);
                assert!(matches!(code(&o), 0..=3), "{which} {args:?} {bad:?}: {err}");
                assert!(!err.contains("panicked"), "{which} {args:?}: {err}");
                if code(&o) == 3 {
                    assert_eq!(err.trim().lines().count(), 1, "{which} {args:?}: {err}");
                    assert!(err.contains("fix") || err.contains("delete"), "{err}");
                }
                if which == "manifest" {
                    assert_eq!(
                        fs::read(path).unwrap(),
                        bad,
                        "{args:?} rewrote the manifest"
                    );
                }
            }
        }
        fs::write(path, &valid).unwrap();
    }
    // Restored files work again and the record is still there.
    let ls = run(&fx, &["ls", "--json"]);
    assert_eq!(code(&ls), 0);
    assert!(String::from_utf8_lossy(&ls.stdout).contains("\"a\""));
}
