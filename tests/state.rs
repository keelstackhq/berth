use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use berth::BerthError;
use berth::manifest::{
    DbEntry, Entry, Manifest, Owner, PortBlock, SetupStatus, State, manifest_path, write_atomic,
};
use berth::registry::{Block, Registry, state_dir_from};
use berth::sys::{find_sh, kill_tree, pid_alive, rfc3339, tasklist_lists_pid};

fn entry(task: &str) -> Entry {
    Entry {
        task: task.into(),
        branch: format!("berth/{task}"),
        base: "a1b2c3d".into(),
        path: PathBuf::from(format!("/abs/hull.berth/{task}")),
        state: State::Ready,
        ports: PortBlock {
            start: 21340,
            end: 21349,
            named: BTreeMap::from([("web".to_string(), 21340)]),
        },
        env: BTreeMap::from([
            ("BERTH_TASK".to_string(), task.to_string()),
            ("PORT".to_string(), "21340".to_string()),
        ]),
        copied: vec![".env".into()],
        setup: SetupStatus {
            status: "ok".into(),
            secs: 14.2,
        },
        created_at: "2026-10-17T09:12:03Z".into(),
        owner: Owner {
            pid: 4242,
            host: "laptop".into(),
        },
        db: None,
    }
}

fn block(repo: &str, task: &str, start: u16) -> Block {
    Block {
        start,
        end: start + 9,
        repo: PathBuf::from(repo),
        task: task.into(),
        created_at: "2026-10-17T09:12:03Z".into(),
    }
}

#[test]
fn rfc3339_vectors() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(1792228323), "2026-10-17T09:12:03Z");
    assert_eq!(rfc3339(951782400), "2000-02-29T00:00:00Z");
}

#[test]
fn manifest_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path();
    let mut manifest = Manifest::load(main).unwrap();
    assert!(manifest.worktrees.is_empty());
    assert_eq!(manifest.repo, main);
    manifest.upsert(entry("fix-login"));
    manifest.save(main).unwrap();
    let text = std::fs::read_to_string(manifest_path(main)).unwrap();
    assert!(text.contains("\"version\": 1"), "{text}");
    assert!(text.ends_with("}\n"));
    assert_eq!(Manifest::load(main).unwrap(), manifest);
}

#[test]
fn manifest_parse_error_names_file() {
    let dir = tempfile::tempdir().unwrap();
    write_atomic(&manifest_path(dir.path()), b"{ nope").unwrap();
    match Manifest::load(dir.path()) {
        Err(BerthError::Config { file, .. }) => assert_eq!(file, manifest_path(dir.path())),
        other => panic!("expected Config, got {other:?}"),
    }
}

#[test]
fn upsert_replaces_and_sorts() {
    let mut manifest = Manifest {
        version: 1,
        repo: "/r".into(),
        worktrees: vec![],
    };
    manifest.upsert(entry("b"));
    manifest.upsert(entry("a"));
    let mut changed = entry("b");
    changed.branch = "other".into();
    manifest.upsert(changed);
    let tasks: Vec<_> = manifest.worktrees.iter().map(|e| e.task.as_str()).collect();
    assert_eq!(tasks, ["a", "b"]);
    assert_eq!(manifest.find("b").unwrap().branch, "other");
    assert!(manifest.find("c").is_none());
}

#[test]
fn remove_returns_entry() {
    let mut manifest = Manifest {
        version: 1,
        repo: "/r".into(),
        worktrees: vec![],
    };
    manifest.upsert(entry("a"));
    assert_eq!(manifest.remove("a").unwrap().task, "a");
    assert!(manifest.remove("a").is_none());
    assert!(manifest.worktrees.is_empty());
}

#[test]
fn atomic_write_leaves_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("sub").join("file.json");
    for i in 0..50 {
        write_atomic(&target, format!("{i}\n").as_bytes()).unwrap();
    }
    let names: Vec<_> = std::fs::read_dir(target.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["file.json"]);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "49\n");
}

#[test]
fn state_dir_precedence() {
    let all = |k: &str| match k {
        "BERTH_STATE_DIR" => Some("/b".to_string()),
        "XDG_STATE_HOME" => Some("/x".to_string()),
        "HOME" => Some("/h".to_string()),
        _ => None,
    };
    assert_eq!(state_dir_from(all).unwrap(), Path::new("/b"));
    let no_berth = |k: &str| if k == "BERTH_STATE_DIR" { None } else { all(k) };
    assert_eq!(state_dir_from(no_berth).unwrap(), Path::new("/x/berth"));
    let only_home = |k: &str| (k == "HOME").then(|| "/h".to_string());
    assert_eq!(
        state_dir_from(only_home).unwrap(),
        Path::new("/h/.local/state/berth")
    );
    assert!(matches!(state_dir_from(|_| None), Err(BerthError::Env(_))));
}

#[test]
fn registry_release_removes_all() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = Registry::load(dir.path()).unwrap();
    assert!(reg.blocks.is_empty());
    reg.add(block("/r", "a", 21000));
    reg.add(block("/r", "a", 21010));
    reg.add(block("/r", "b", 21020));
    assert_eq!(reg.overlaps(21005, 21015).len(), 2);
    assert!(reg.overlaps(21030, 21039).is_empty());
    assert_eq!(reg.release(Path::new("/r"), "a"), 2);
    reg.save(dir.path()).unwrap();
    let text = std::fs::read_to_string(dir.path().join("ports.json")).unwrap();
    assert!(text.contains("\"version\": 1"));
    let loaded = Registry::load(dir.path()).unwrap();
    assert_eq!(loaded.blocks.len(), 1);
    assert_eq!(loaded.blocks[0].task, "b");
    let mut loaded = loaded;
    assert_eq!(loaded.remove_where(|b| b.task == "b"), 1);
    assert!(loaded.blocks.is_empty());
}

#[test]
fn pid_alive_dead_and_live() {
    assert!(pid_alive(std::process::id(), &berth::sys::hostname()));
    let mut child = Command::new(find_sh().unwrap())
        .args(["-c", "true"])
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!pid_alive(pid, &berth::sys::hostname()));
    assert!(pid_alive(pid, "elsewhere"));
}

#[test]
fn tasklist_rows_are_matched_on_the_pid_field() {
    let row = "\"sh.exe\",\"4242\",\"Console\",\"1\",\"3,512 K\"\r\n";
    assert!(tasklist_lists_pid(row, 4242));
    assert!(!tasklist_lists_pid(row, 1));
    assert!(!tasklist_lists_pid(
        "INFO: No tasks are running which match the specified criteria.\r\n",
        4242
    ));
}

#[test]
fn kill_tree_kills_the_child() {
    let mut command = Command::new(find_sh().unwrap());
    command.args(["-c", "sleep 30"]);
    // On Unix kill_tree signals the group the pid leads, as hooks arrange.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().unwrap();
    kill_tree(child.id());
    let start = Instant::now();
    child.wait().unwrap();
    assert!(start.elapsed() < Duration::from_secs(10));
}

// Unix-only: needs `process_group(0)`, and `$!` is a POSIX pid that `tasklist` does not know.
#[cfg(unix)]
#[test]
fn kill_group_kills_group() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let mut child = Command::new("sh")
        .args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let sleep_pid: u32 = line.trim().parse().unwrap();
    let host = berth::sys::hostname();
    assert!(pid_alive(sleep_pid, &host));
    kill_tree(child.id());
    let start = Instant::now();
    child.wait().unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    // The orphaned sleep is reaped by init; give it a moment.
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(sleep_pid, &host) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !pid_alive(sleep_pid, &host),
        "sleep child survived kill_tree"
    );
}

#[test]
fn unknown_version_is_config_error() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path();
    write_atomic(
        &manifest_path(main),
        br#"{"version": 2, "repo": "/r", "worktrees": []}"#,
    )
    .unwrap();
    match Manifest::load(main) {
        Err(BerthError::Config { file, message }) => {
            assert_eq!(file, manifest_path(main));
            assert!(message.contains("version"), "{message}");
        }
        other => panic!("expected Config, got {other:?}"),
    }
    write_atomic(
        &dir.path().join("ports.json"),
        br#"{"version": 9, "blocks": []}"#,
    )
    .unwrap();
    match Registry::load(dir.path()) {
        Err(BerthError::Config { file, .. }) => assert_eq!(file, dir.path().join("ports.json")),
        other => panic!("expected Config, got {other:?}"),
    }
}

#[test]
fn relative_state_paths() {
    let xdg_rel = |k: &str| match k {
        "XDG_STATE_HOME" => Some("rel".to_string()),
        "HOME" => Some("/h".to_string()),
        _ => None,
    };
    assert_eq!(
        state_dir_from(xdg_rel).unwrap(),
        Path::new("/h/.local/state/berth")
    );
    let berth_rel = |k: &str| (k == "BERTH_STATE_DIR").then(|| "rel/dir".to_string());
    match state_dir_from(berth_rel) {
        Err(BerthError::Config { message, .. }) => assert!(message.contains("absolute")),
        other => panic!("expected Config, got {other:?}"),
    }
}

#[test]
fn m1_manifest_without_db_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path();
    let mut m = Manifest::load(main).unwrap();
    m.upsert(entry("fix-login"));
    m.save(main).unwrap();
    let text = std::fs::read_to_string(manifest_path(main)).unwrap();
    assert!(!text.contains("\"db\""), "{text}");
    let loaded = Manifest::load(main).unwrap();
    assert_eq!(loaded.worktrees[0].db, None);
    loaded.save(main).unwrap();
    assert_eq!(std::fs::read_to_string(manifest_path(main)).unwrap(), text);
}

#[test]
fn entry_with_db_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path();
    let mut m = Manifest::load(main).unwrap();
    let mut e = entry("a");
    e.db = Some(DbEntry {
        provider: "postgres".into(),
        name: "hull_a".into(),
        created: true,
        host: Some("localhost".into()),
        port: Some(5432),
        user: None,
    });
    let mut s = entry("b");
    s.db = Some(DbEntry {
        provider: "sqlite".into(),
        name: "db/dev.sqlite".into(),
        created: false,
        host: None,
        port: None,
        user: None,
    });
    m.upsert(e);
    m.upsert(s);
    m.save(main).unwrap();
    let text = std::fs::read_to_string(manifest_path(main)).unwrap();
    assert!(text.contains("\"host\": \"localhost\""), "{text}");
    assert_eq!(text.matches("\"user\"").count(), 0, "{text}");
    assert_eq!(Manifest::load(main).unwrap(), m);
}
