mod common;

use berth::BerthError;
use berth::hooks::run_hooks;
use berth::sys::{hostname, pid_alive};
use common::Fixture;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

fn cmds(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn run(
    dir: &Path,
    list: &[&str],
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<f64, BerthError> {
    run_hooks("setup", &cmds(list), dir, env, timeout)
}

fn hook_err(res: Result<f64, BerthError>) -> (String, String, Vec<String>) {
    match res {
        Err(BerthError::Hook {
            step, status, tail, ..
        }) => {
            assert_eq!(step, "setup");
            (step, status, tail)
        }
        other => panic!("expected Hook error, got {other:?}"),
    }
}

fn pid_from(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("sh")
        .args(["-c", r#"kill "$1""#, "sh", &pid.to_string()])
        .status();
}

#[test]
fn runs_in_order_with_env() {
    let fx = Fixture::new();
    let mut env = BTreeMap::new();
    env.insert("BERTH_TASK".to_string(), "t".to_string());
    run(
        &fx.repo,
        &["echo $BERTH_TASK > a", "cat a > b"],
        &env,
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(fx.repo.join("b")).unwrap().trim(),
        "t"
    );
}

#[test]
fn failure_keeps_last_20_lines() {
    let fx = Fixture::new();
    let res = run(
        &fx.repo,
        &["for i in $(seq 1 30); do echo line$i; done; exit 4"],
        &BTreeMap::new(),
        Duration::from_secs(10),
    );
    let (_, status, tail) = hook_err(res);
    assert_eq!(status, "exited 4");
    assert_eq!(tail.len(), 20);
    assert_eq!(tail[0], "line11");
    assert_eq!(tail[19], "line30");
}

#[test]
fn timeout_kills() {
    let fx = Fixture::new();
    let start = Instant::now();
    let res = run(
        &fx.repo,
        &["sleep 30"],
        &BTreeMap::new(),
        Duration::from_millis(300),
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    let (_, status, _) = hook_err(res);
    assert!(status.contains("timed out"), "{status}");
}

#[test]
fn timeout_kills_grandchildren() {
    let fx = Fixture::new();
    let start = Instant::now();
    let res = run(
        &fx.repo,
        &["sleep 30 & echo $! > pid; wait"],
        &BTreeMap::new(),
        Duration::from_millis(300),
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    hook_err(res);
    let pid = pid_from(&fx.repo);
    let host = hostname();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(pid, &host) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let alive = pid_alive(pid, &host);
    if alive {
        kill_pid(pid);
    }
    assert!(!alive, "grandchild {pid} survived the timeout");
}

#[test]
fn background_child_does_not_hang() {
    let fx = Fixture::new();
    let start = Instant::now();
    let res = run(
        &fx.repo,
        &["sleep 30 & echo $! > pid; exit 0"],
        &BTreeMap::new(),
        Duration::from_secs(10),
    );
    let elapsed = start.elapsed();
    kill_pid(pid_from(&fx.repo));
    res.unwrap();
    assert!(elapsed < Duration::from_millis(2500), "{elapsed:?}");
}

#[test]
fn non_utf8_output_does_not_hang() {
    let fx = Fixture::new();
    let start = Instant::now();
    run(
        &fx.repo,
        &[r"head -c 70000 /dev/zero | tr '\0' '\377'; echo; echo done"],
        &BTreeMap::new(),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn signal_death_status() {
    let fx = Fixture::new();
    let res = run(
        &fx.repo,
        &["kill -9 $$"],
        &BTreeMap::new(),
        Duration::from_secs(10),
    );
    let (_, status, _) = hook_err(res);
    assert_eq!(status, "killed by signal 9");
}

#[test]
fn empty_list_is_ok() {
    let fx = Fixture::new();
    run(&fx.repo, &[], &BTreeMap::new(), Duration::from_secs(1)).unwrap();
}

#[test]
fn timeout_with_detached_pipe_holder_does_not_hang() {
    let fx = Fixture::new();
    let start = Instant::now();
    let res = run(
        &fx.repo,
        &["setsid sleep 30 & echo $! > pid; sleep 30"],
        &BTreeMap::new(),
        Duration::from_millis(300),
    );
    let elapsed = start.elapsed();
    let pid_file = fx.repo.join("pid");
    if pid_file.exists() {
        kill_pid(pid_from(&fx.repo));
    }
    let (_, status, _) = hook_err(res);
    assert!(status.contains("timed out"), "{status}");
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}
