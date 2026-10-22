//! Small OS helpers: time, hostname, process liveness and tree kill, POSIX `sh` lookup, test
//! pause points. Unix and Windows differ only in here and in `hooks.rs`.

use crate::error::BerthError;
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Now as UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339(secs)
}

/// Seconds since the Unix epoch as UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant), valid for the whole proleptic Gregorian range.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// The machine name; `"unknown"` when it cannot be read. Unix: `uname -n`. Windows:
/// `COMPUTERNAME`, else `hostname`.
pub fn hostname() -> String {
    #[cfg(windows)]
    if let Some(name) = std::env::var("COMPUTERNAME")
        .ok()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
    {
        return name;
    }
    #[cfg(unix)]
    let mut cmd = Command::new("uname");
    #[cfg(unix)]
    cmd.arg("-n");
    #[cfg(windows)]
    let mut cmd = Command::new("hostname");
    cmd.stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Whether `pid` on `host` may still be running. Another host: assume yes. Spawn error: assume yes.
pub fn pid_alive(pid: u32, host: &str) -> bool {
    if host != hostname() {
        return true;
    }
    #[cfg(unix)]
    {
        match Command::new("sh")
            .args(["-c", r#"kill -0 "$1" 2>/dev/null"#, "sh", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(_) => true,
        }
    }
    #[cfg(windows)]
    {
        match Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            Ok(out) if out.status.success() => {
                tasklist_lists_pid(&String::from_utf8_lossy(&out.stdout), pid)
            }
            _ => true,
        }
    }
}

/// Whether `tasklist /FO CSV /NH` output has a row whose PID field (the second) is `pid`.
/// No match prints an `INFO:` line instead of a row.
pub fn tasklist_lists_pid(stdout: &str, pid: u32) -> bool {
    let want = pid.to_string();
    stdout.lines().any(|line| {
        line.trim()
            .trim_matches('"')
            .split("\",\"")
            .nth(1)
            .is_some_and(|field| field == want)
    })
}

/// Kill the process `pid` and everything it started. Errors are ignored (it may already be
/// gone). Unix: SIGKILL to the process group `pid` leads (hooks run in their own group).
/// Windows: `taskkill /T /F /PID`.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    let mut cmd = {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", r#"kill -s KILL -- "-$1""#, "sh", &pid.to_string()]);
        cmd
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()]);
        cmd
    };
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `fs::canonicalize` without Windows' verbatim prefix: `\\?\C:\x` becomes `C:\x` and
/// `\\?\UNC\srv\share` becomes `\\srv\share`, because git cannot use verbatim paths. Unix:
/// plain `fs::canonicalize`.
pub fn canonicalize(path: impl AsRef<std::path::Path>) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    if cfg!(windows)
        && let Some(s) = real.to_str()
    {
        return Ok(PathBuf::from(strip_verbatim(s)));
    }
    Ok(real)
}

/// The plain form of a verbatim Windows path (see [`canonicalize`]); anything else, and verbatim
/// paths with no plain form (`\\?\Volume{..}`, a bare drive), unchanged. Pure, any OS.
pub fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// Where Git for Windows keeps its `sh.exe`, derived from `git --exec-path`. Walks up from the
/// exec path (`<prefix>\mingw64\libexec\git-core`, also `mingw32` and `clangarm64`) to the first
/// directory with `bin\sh.exe`, else `usr\bin\sh.exe`. Pure: `exists` answers whether a candidate
/// file is there. Accepts `\` and `/`.
pub fn sh_from_exec_path(exec_path: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    let sep = if exec_path.contains('\\') { '\\' } else { '/' };
    let trimmed = exec_path.trim();
    let parts: Vec<&str> = trimmed
        .split(['\\', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    // A leading separator (`/c/Git/...`, `\\server\...`) belongs to the first piece.
    let lead = if trimmed.starts_with(['\\', '/']) {
        sep.to_string()
    } else {
        String::new()
    };
    for n in (1..parts.len()).rev() {
        let prefix = format!("{lead}{}", parts[..n].join(&sep.to_string()));
        for tail in [format!("bin{sep}sh.exe"), format!("usr{sep}bin{sep}sh.exe")] {
            let cand = format!("{prefix}{sep}{tail}");
            if exists(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// The POSIX `sh` that runs hooks and shell helpers. Unix: `sh`. Windows: `sh.exe` first on
/// `PATH`, else Git for Windows' own, found from `git --exec-path`; missing is exit 3.
pub fn find_sh() -> Result<PathBuf, BerthError> {
    #[cfg(unix)]
    {
        Ok(PathBuf::from("sh"))
    }
    #[cfg(windows)]
    {
        let on_path = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join("sh.exe"))
                .find(|c| c.is_file())
        });
        if let Some(p) = on_path {
            return Ok(p);
        }
        Command::new("git")
            .arg("--exec-path")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                sh_from_exec_path(String::from_utf8_lossy(&o.stdout).trim(), |p| {
                    Path::new(p).is_file()
                })
            })
            .map(PathBuf::from)
            .ok_or_else(|| {
                BerthError::Env(
                    "berth needs a POSIX sh on Windows and found none on PATH or next to git; \
                     install Git for Windows or put sh.exe on PATH"
                        .into(),
                )
            })
    }
}

/// Test hook: when `BERTH_TEST_PAUSE` equals `step`, write `<state_dir>/paused-<step>` and sleep
/// 120 s so a test can kill the process at a known point. No-op otherwise.
pub fn test_pause(step: &str) {
    if std::env::var("BERTH_TEST_PAUSE").as_deref() != Ok(step) {
        return;
    }
    if let Ok(dir) = crate::registry::state_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("paused-{step}")), b"");
    }
    std::thread::sleep(std::time::Duration::from_secs(120));
}
