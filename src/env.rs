//! The per-worktree environment (spec §8) and its output formats.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::Format;
use crate::error::BerthError;
use crate::manifest::PortBlock;

pub struct EnvInput<'a> {
    pub repo_name: &'a str,
    pub main: &'a Path,
    pub worktree: &'a Path,
    pub task: &'a str,
    pub ports: &'a PortBlock,
    pub extra: &'a BTreeMap<String, String>,
}

/// berth's own variables, cache isolation, then each `[env]` value with placeholders expanded.
pub fn build_env(i: &EnvInput) -> Result<BTreeMap<String, String>, BerthError> {
    let wt = i.worktree.display().to_string();
    let mut env = BTreeMap::new();
    let mut set = |k: &str, v: String| {
        env.insert(k.to_string(), v);
    };
    set("BERTH_TASK", i.task.to_string());
    set("BERTH_ROOT", i.main.display().to_string());
    set("BERTH_WORKTREE", wt.clone());
    set("BERTH_PORT_BASE", i.ports.start.to_string());
    set("BERTH_PORT_COUNT", i.ports.count().to_string());
    set("PORT", i.ports.start.to_string());
    for (name, port) in &i.ports.named {
        set(&format!("{}_PORT", name.to_uppercase()), port.to_string());
    }
    set("TURBO_CACHE_DIR", format!("{wt}/.turbo/cache"));
    set(
        "VITE_CACHE_PATH",
        format!("{wt}/node_modules/.vite/task-cache"),
    );
    set("CARGO_TARGET_DIR", format!("{wt}/target"));
    set("TMPDIR", format!("{wt}/.berth/tmp"));
    for (key, template) in i.extra {
        let value = expand(template, i.task, i.repo_name, i.worktree, i.ports).map_err(|tok| {
            let message = if tok.starts_with('{') && tok.ends_with('}') && tok.len() > 1 {
                format!("unknown placeholder {tok} in [env].{key}")
            } else {
                format!("unbalanced brace in [env].{key}")
            };
            BerthError::Config {
                file: "berth.toml".into(),
                message,
            }
        })?;
        env.insert(key.clone(), value);
    }
    Ok(env)
}

/// Expand `{repo}`, `{task}`, `{task_snake}`, `{port.base}`, `{port.<name>}`, `{worktree}`.
/// `{{` and `}}` are literal braces. On a bad token the error is the token text: `{x}` for an
/// unknown placeholder, or the stray/unclosed fragment (`}`, `{x`) for unbalanced braces.
pub fn expand(
    template: &str,
    task: &str,
    repo_name: &str,
    worktree: &Path,
    ports: &PortBlock,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(pos) = rest.find(['{', '}']) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if let Some(after) = tail.strip_prefix("{{") {
            out.push('{');
            rest = after;
        } else if let Some(after) = tail.strip_prefix("}}") {
            out.push('}');
            rest = after;
        } else if tail.starts_with('}') {
            return Err("}".to_string());
        } else {
            let Some(close) = tail.find('}') else {
                return Err(tail.to_string());
            };
            let token = &tail[..=close];
            let name = &token[1..close];
            match name {
                _ if name.contains('{') => return Err("{".to_string()),
                "repo" => out.push_str(repo_name),
                "task" => out.push_str(task),
                "task_snake" => out.push_str(&task.replace('-', "_")),
                "worktree" => out.push_str(&worktree.display().to_string()),
                "port.base" => out.push_str(&ports.start.to_string()),
                _ => match name.strip_prefix("port.").and_then(|n| ports.named.get(n)) {
                    Some(port) => out.push_str(&port.to_string()),
                    None => return Err(token.to_string()),
                },
            }
            rest = &tail[close + 1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// POSIX single-quote: `'…'` with each `'` written as `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn fish_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn dotenv_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' | '"' | '$' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line per key, sorted (the map is ordered), trailing newline.
pub fn format_env(env: &BTreeMap<String, String>, f: Format) -> String {
    if f == Format::Json {
        let mut text = serde_json::to_string(env).expect("string map serializes");
        text.push('\n');
        return text;
    }
    let mut out = String::new();
    for (k, v) in env {
        match f {
            Format::Sh => out.push_str(&format!("export {k}={}\n", sh_quote(v))),
            Format::Fish => out.push_str(&format!("set -gx {k} {}\n", fish_quote(v))),
            Format::Dotenv => out.push_str(&format!("{k}={}\n", dotenv_quote(v))),
            Format::Json => unreachable!(),
        }
    }
    out
}
