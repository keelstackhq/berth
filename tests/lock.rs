use std::path::Path;
use std::time::{Duration, Instant};

use berth::BerthError;
use berth::lock::{acquire, is_held, repo_lock, user_lock};

#[test]
fn second_acquire_times_out_naming_holder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("berth.lock");
    let _a = acquire(&path, Duration::from_millis(200)).unwrap();
    let started = Instant::now();
    let err = match acquire(&path, Duration::from_millis(200)) {
        Ok(_) => panic!("second acquire must time out"),
        Err(err) => err,
    };
    assert!(started.elapsed() >= Duration::from_millis(150));
    match err {
        BerthError::Lock { path: p, holder } => {
            assert_eq!(p, path);
            assert!(
                holder.contains(&format!("pid={}", std::process::id())),
                "{holder}"
            );
            assert!(holder.contains("host="), "{holder}");
            assert!(holder.contains("cmd="), "{holder}");
        }
        other => panic!("expected Lock, got {other:?}"),
    }
}

#[test]
fn released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("berth.lock");
    let a = acquire(&path, Duration::from_millis(200)).unwrap();
    assert!(!std::fs::read(&path).unwrap().is_empty());
    drop(a);
    assert!(std::fs::read(&path).unwrap().is_empty());
    // A zero timeout tries once: success proves the lock was free right away,
    // without a wall-clock assertion that flakes under load.
    let _b = acquire(&path, Duration::ZERO).expect("lock free immediately after drop");
}

#[test]
fn is_held_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("berth.lock");
    assert!(!is_held(&path));
    assert!(!path.exists(), "is_held must not create the file");

    let a = acquire(&path, Duration::from_millis(200)).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(is_held(&path));
    assert_eq!(std::fs::read(&path).unwrap(), before);

    drop(a);
    assert!(!is_held(&path));
    assert!(std::fs::read(&path).unwrap().is_empty());
}

#[test]
fn empty_holder_is_reported_as_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("berth.lock");
    // Hold the flock without recording a holder, as a racing acquirer would between lock and write.
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .unwrap();
    f.lock().unwrap();
    match acquire(&path, Duration::from_millis(100)) {
        Err(BerthError::Lock { holder, .. }) => {
            assert_eq!(holder, "unknown (holder not yet recorded)")
        }
        _ => panic!("expected Lock error"),
    }
}

#[test]
fn lock_paths() {
    assert_eq!(
        repo_lock(Path::new("/r/.git")),
        Path::new("/r/.git/berth.lock")
    );
    assert_eq!(user_lock(Path::new("/s")), Path::new("/s/ports.lock"));
}
