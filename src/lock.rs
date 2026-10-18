//! Advisory file locks (flock) with a holder record. Lock order is always repo, then user.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::BerthError;
use crate::sys;

const POLL: Duration = Duration::from_millis(50);
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// A held lock. Dropping it empties the holder record, then releases the lock.
pub struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        let _ = self.file.unlock();
    }
}

fn io_err(path: &Path, source: std::io::Error) -> BerthError {
    BerthError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Take the exclusive lock at `path`, polling every 50 ms until `timeout`.
/// On timeout the error names the holder recorded in the file.
pub fn acquire(path: &Path, timeout: Duration) -> Result<LockGuard, BerthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(BerthError::Lock {
                        path: path.to_path_buf(),
                        holder: holder_of(path),
                    });
                }
                std::thread::sleep(POLL);
            }
            Err(TryLockError::Error(e)) => return Err(io_err(path, e)),
        }
    }
    let argv: Vec<String> = std::env::args().collect();
    let record = format!(
        "pid={} host={} cmd={}",
        std::process::id(),
        sys::hostname(),
        argv.join(" ")
    );
    file.set_len(0).map_err(|e| io_err(path, e))?;
    file.write_all(record.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| io_err(path, e))?;
    Ok(LockGuard { file })
}

fn holder_of(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).trim().to_string(),
        _ => "unknown (holder not yet recorded)".to_string(),
    }
}

/// Whether another descriptor holds the lock. Read-only: never creates or writes the file,
/// never blocks. A missing file is not held; an unexpected error counts as held.
pub fn is_held(path: &Path) -> bool {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(_) => true,
    }
}

/// Lock wait: `BERTH_LOCK_TIMEOUT_MS`, default 30000.
pub fn lock_timeout() -> Duration {
    let ms = std::env::var("BERTH_LOCK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(ms)
}

/// The per-repo lock: `<git-common-dir>/berth.lock`.
pub fn repo_lock(common_dir: &Path) -> PathBuf {
    common_dir.join("berth.lock")
}

/// The per-user lock guarding the port registry: `<state>/ports.lock`.
pub fn user_lock(state: &Path) -> PathBuf {
    state.join("ports.lock")
}
