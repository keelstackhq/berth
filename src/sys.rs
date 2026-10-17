//! Small OS helpers: time, hostname, process liveness and group kill, test pause points.

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

/// `uname -n`, trimmed; `"unknown"` when it cannot be read.
pub fn hostname() -> String {
    Command::new("uname")
        .arg("-n")
        .stdin(Stdio::null())
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

/// SIGKILL a whole process group. Errors are ignored (the group may already be gone).
pub fn kill_group(pgid: u32) {
    let _ = Command::new("sh")
        .args(["-c", r#"kill -s KILL -- "-$1""#, "sh", &pgid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
