use std::path::PathBuf;

/// Every failure berth reports as an error (not as a refusal `Item`).
#[derive(Debug, thiserror::Error)]
pub enum BerthError {
    #[error("{0}")]
    Usage(String),
    #[error("{}: {message}", file.display())]
    Config { file: PathBuf, message: String },
    #[error("{0}")]
    Env(String),
    #[error("git {args} failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("lock {} held by {holder}; wait or remove it if that process is gone", path.display())]
    Lock { path: PathBuf, holder: String },
    #[error(
        "no free block of {count} ports in {}-{}; widen [ports].range or `berth down` unused tasks",
        range.0, range.1
    )]
    NoPorts { range: (u16, u16), count: u16 },
    #[error("{}", hook_text(step, command, status, tail))]
    Hook {
        step: String,
        command: String,
        status: String,
        tail: Vec<String>,
    },
}

fn hook_text(step: &str, command: &str, status: &str, tail: &[String]) -> String {
    let mut text = format!("{step}: `{command}` {status}\nlast output:");
    for line in tail {
        text.push('\n');
        text.push_str(line);
    }
    text
}

impl BerthError {
    /// Usage errors exit 2; everything else is a config or environment error (3).
    pub fn exit_code(&self) -> i32 {
        match self {
            BerthError::Usage(_) => 2,
            _ => 3,
        }
    }

    /// Variant name in snake_case; the `key` of the `--json` error item.
    pub fn key(&self) -> &'static str {
        match self {
            BerthError::Usage(_) => "usage",
            BerthError::Config { .. } => "config",
            BerthError::Env(_) => "env",
            BerthError::Git { .. } => "git",
            BerthError::Io { .. } => "io",
            BerthError::Lock { .. } => "lock",
            BerthError::NoPorts { .. } => "no_ports",
            BerthError::Hook { .. } => "hook",
        }
    }

    /// The message for `--json`: the Display text, except a hook failure never carries its output.
    pub fn reason(&self) -> String {
        match self {
            BerthError::Hook {
                step,
                command,
                status,
                ..
            } => format!("{step}: `{command}` {status}"),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(tail: Vec<String>) -> BerthError {
        BerthError::Hook {
            step: "setup".into(),
            command: "pnpm install".into(),
            status: "exited 1".into(),
            tail,
        }
    }

    #[test]
    fn usage_is_2_rest_is_3() {
        let usage = BerthError::Usage("x".into());
        assert_eq!(usage.exit_code(), 2);
        let rest = [
            BerthError::Config {
                file: "berth.toml".into(),
                message: "x".into(),
            },
            BerthError::Env("x".into()),
            BerthError::Git {
                args: "status".into(),
                stderr: "x".into(),
            },
            BerthError::Io {
                path: "p".into(),
                source: std::io::Error::other("x"),
            },
            BerthError::Lock {
                path: "p".into(),
                holder: "pid 1".into(),
            },
            BerthError::NoPorts {
                range: (21000, 21009),
                count: 10,
            },
            hook(vec![]),
        ];
        for err in rest {
            assert_eq!(err.exit_code(), 3, "{}", err.key());
        }
    }

    #[test]
    fn hook_reason_has_no_tail() {
        let err = hook(vec!["SECRET".into()]);
        assert!(!err.reason().contains("SECRET"));
        assert_eq!(err.reason(), "setup: `pnpm install` exited 1");
        let shown = err.to_string();
        assert!(shown.contains("last output:") && shown.contains("SECRET"));
    }

    #[test]
    fn display_ends_with_the_fix() {
        let lock = BerthError::Lock {
            path: "/s/repo.lock".into(),
            holder: "pid 7 on box".into(),
        };
        assert!(
            lock.to_string()
                .ends_with("held by pid 7 on box; wait or remove it if that process is gone")
        );
        let ports = BerthError::NoPorts {
            range: (21000, 21009),
            count: 10,
        };
        assert_eq!(
            ports.to_string(),
            "no free block of 10 ports in 21000-21009; widen [ports].range or `berth down` unused tasks"
        );
        assert_eq!(ports.key(), "no_ports");
    }
}
