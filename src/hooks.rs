//! Setup and teardown hooks: shell commands run in their own process group, with a timeout
//! over the whole list. Output goes to berth's stderr only; the last 20 lines are kept for errors.

use crate::{BerthError, sys, vlog};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TAIL_LINES: usize = 20;
const READER_GRACE: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(50);

type Tail = Arc<Mutex<VecDeque<String>>>;

/// Run `cmds` in order via `sh -c` in `cwd` with `env` added to the inherited environment.
/// `timeout` covers the whole list. Returns the elapsed seconds, or `BerthError::Hook` naming
/// the failed command, its status and the last 20 output lines.
pub fn run_hooks(
    step: &str,
    cmds: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<f64, BerthError> {
    let start = Instant::now();
    let deadline = start + timeout;
    for cmd in cmds {
        vlog(&format!("hook {step}: {cmd}"));
        run_one(step, cmd, cwd, env, deadline, timeout)?;
    }
    Ok(start.elapsed().as_secs_f64())
}

fn run_one(
    step: &str,
    cmd: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    deadline: Instant,
    timeout: Duration,
) -> Result<(), BerthError> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|source| BerthError::Io {
            path: cwd.to_path_buf(),
            source,
        })?;
    let pgid = child.id();
    let tail: Tail = Arc::new(Mutex::new(VecDeque::new()));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, tail.clone(), done_tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, tail.clone(), done_tx.clone()));
    }
    drop(done_tx);

    let fail = |status: String| BerthError::Hook {
        step: step.to_string(),
        command: cmd.to_string(),
        status,
        tail: tail
            .lock()
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default(),
    };

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(source) => {
                sys::kill_group(pgid);
                let _ = child.wait();
                return Err(BerthError::Io {
                    path: cwd.to_path_buf(),
                    source,
                });
            }
        }
        if Instant::now() >= deadline {
            sys::kill_group(pgid);
            let _ = child.wait();
            break None;
        }
        thread::sleep(POLL);
    };

    if status.is_none_or(|s| !s.success()) {
        // Removes leftover background processes before any rollback.
        sys::kill_group(pgid);
    }
    // A background `cmd &` (or a process that left the group) may hold the pipes open: wait a
    // bounded time, then detach the readers.
    let grace_end = Instant::now() + READER_GRACE;
    for _ in 0..readers.len() {
        let left = grace_end.saturating_duration_since(Instant::now());
        match done_rx.recv_timeout(left) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(readers);

    let Some(status) = status else {
        return Err(fail(format!("timed out after {}s", timeout.as_secs_f64())));
    };
    if status.success() {
        return Ok(());
    }
    let text = match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited {code}"),
        (None, Some(sig)) => format!("killed by signal {sig}"),
        (None, None) => "failed".to_string(),
    };
    Err(fail(text))
}

fn spawn_reader<R: Read + Send + 'static>(
    pipe: R,
    tail: Tail,
    done: mpsc::Sender<()>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let _ = std::io::stderr().write_all(&line);
                    let text = String::from_utf8_lossy(&line).trim_end().to_string();
                    if let Ok(mut tail) = tail.lock() {
                        if tail.len() == TAIL_LINES {
                            tail.pop_front();
                        }
                        tail.push_back(text);
                    }
                }
            }
        }
        let _ = done.send(());
    })
}
