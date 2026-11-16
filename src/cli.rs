use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Parser)]
#[command(
    name = "berth",
    version,
    about = "An isolated, ready workspace for each parallel coding agent"
)]
pub struct Cli {
    /// Run as if started in this directory
    #[arg(short = 'C', global = true, value_name = "DIR")]
    pub dir: Option<PathBuf>,
    /// Print exactly one JSON object on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Never color output
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Log git and hook command lines to stderr
    #[arg(short = 'v', global = true)]
    pub verbose: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create (or confirm) the workspace for a task
    Up(UpArgs),
    /// Remove the workspace of a task
    Down(DownArgs),
    /// List workspaces
    Ls,
    /// Print the environment of a workspace
    Env(EnvArgs),
    /// Remove finished tasks (merged, clean, ready)
    Gc(GcArgs),
    /// Check berth's state against git and the machine; `--fix` repairs what a crash left
    Doctor(DoctorArgs),
    /// Handle a Claude Code hook event (JSON on stdin; see docs/integration.md)
    Hook(HookArgs),
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Up(_) => "up",
            Command::Down(_) => "down",
            Command::Ls => "ls",
            Command::Env(_) => "env",
            Command::Gc(_) => "gc",
            Command::Doctor(_) => "doctor",
            Command::Hook(_) => "hook",
        }
    }
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Remove interrupted or deleted tasks and stale port blocks, then re-check
    #[arg(long)]
    pub fix: bool,
}

#[derive(Debug, Args)]
pub struct HookArgs {
    pub event: HookEvent,
}

/// The hook events berth speaks (spec §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookEvent {
    WorktreeCreate,
    WorktreeRemove,
    SessionStart,
    Statusline,
}

#[derive(Debug, Args)]
pub struct UpArgs {
    pub task: Task,
    /// Ref to branch from (default: `[worktree].base`)
    #[arg(long)]
    pub base: Option<String>,
    /// Branch name (default: `[worktree].branch_prefix` + task)
    #[arg(long)]
    pub branch: Option<String>,
    /// Skip the setup hook
    #[arg(long)]
    pub no_setup: bool,
    /// Keep the workspace when setup fails
    #[arg(long)]
    pub keep_on_failure: bool,
    /// The hook name this task came from, when it differs from the task (`feat/x`).
    #[arg(skip)]
    pub hook_name: Option<String>,
}

#[derive(Debug, Args)]
pub struct DownArgs {
    /// Task (default: the worktree of the current directory)
    pub task: Option<Task>,
    /// Remove even with uncommitted changes
    #[arg(long)]
    pub force: bool,
    /// Also delete the task branch if it is merged
    #[arg(long)]
    pub delete_branch: bool,
}

#[derive(Debug, Args)]
pub struct GcArgs {
    /// Report what would be removed; change nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Also delete the branches of removed tasks (they are merged)
    #[arg(long)]
    pub delete_branch: bool,
}

#[derive(Debug, Args)]
pub struct EnvArgs {
    /// Task (default: the worktree of the current directory)
    pub task: Option<Task>,
    #[arg(long, value_enum, default_value_t = Format::Sh)]
    pub format: Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Sh,
    Fish,
    Pwsh,
    Dotenv,
    Json,
}

/// A task slug: `^[a-z0-9][a-z0-9-]{0,47}$`. It becomes a path segment, a branch suffix and a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task(String);

impl Task {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Task {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let mut chars = s.chars();
        let first_ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if first_ok && rest_ok && s.len() <= 48 {
            Ok(Task(s.to_string()))
        } else {
            Err(format!(
                "invalid task `{s}`: use 1-48 characters of a-z, 0-9 and `-`, starting with a letter or digit{}",
                match suggest(s) {
                    Some(slug) => format!("; try `{slug}`"),
                    None => String::new(),
                }
            ))
        }
    }
}

/// A valid slug close to `s` (lowercase, runs of other characters become one `-`), if any.
fn suggest(s: &str) -> Option<String> {
    // Dropping non-ASCII characters would suggest a different word, not a fix.
    if !s.is_ascii() {
        return None;
    }
    let mut out = String::new();
    for c in s.to_ascii_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let slug: String = out.trim_end_matches('-').chars().take(48).collect();
    let slug = slug.trim_end_matches('-').to_string();
    (!slug.is_empty() && slug != s).then_some(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_slug_gets_a_suggestion() {
        let err = |s: &str| s.parse::<Task>().unwrap_err();
        assert!(err("Fix Login").ends_with("; try `fix-login`"));
        assert!(err("feature/x_y").ends_with("; try `feature-x-y`"));
        assert!(!err("café").contains("try"));
        assert!(err("---").ends_with("starting with a letter or digit"));
    }

    #[test]
    fn slug_rules() {
        for ok in ["a", "0", "fix-login", "a-", &"a".repeat(48)] {
            assert!(ok.parse::<Task>().is_ok(), "{ok}");
        }
        for bad in ["", "-a", "A", "a_b", "a b", "a/b", "é", &"a".repeat(49)] {
            assert!(bad.parse::<Task>().is_err(), "{bad}");
        }
    }
}
