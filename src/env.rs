//! The per-worktree environment (spec §8) and its output formats.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::Format;
use crate::config::Provider;
use crate::db::DbPlan;
use crate::error::BerthError;
use crate::manifest::PortBlock;

pub struct EnvInput<'a> {
    pub repo_name: &'a str,
    pub main: &'a Path,
    pub worktree: &'a Path,
    pub task: &'a str,
    pub ports: &'a PortBlock,
    pub extra: &'a BTreeMap<String, String>,
    pub db: Option<&'a DbPlan>,
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
    let placeholders = db_placeholders(i.db);
    if let Some(db) = i.db {
        if let (Some(var), Some(url)) = (&db.env_var, &db.url) {
            env.insert(var.clone(), url.clone());
        }
        if db.provider == Provider::Compose {
            env.insert("COMPOSE_PROJECT_NAME".into(), db.name.clone());
        }
    }
    for (key, template) in i.extra {
        let value = expand_with(
            template,
            i.task,
            i.repo_name,
            i.worktree,
            i.ports,
            &placeholders,
        )
        .map_err(|tok| {
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

/// The `{db.*}` placeholders a plan provides: `db.url` (when a URL exists), `db.name`
/// (postgres, compose), `db.path` (sqlite).
pub fn db_placeholders(db: Option<&DbPlan>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(db) = db {
        if let Some(url) = &db.url {
            map.insert("db.url".to_string(), url.clone());
        }
        match db.provider {
            Provider::Sqlite => {
                if let Some(path) = &db.abs_path {
                    map.insert("db.path".to_string(), path.display().to_string());
                }
            }
            Provider::Postgres | Provider::Compose => {
                map.insert("db.name".to_string(), db.name.clone());
            }
        }
    }
    map
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
    expand_with(template, task, repo_name, worktree, ports, &BTreeMap::new())
}

/// `expand` plus `extra` placeholders (keys such as `db.url`, without braces).
pub fn expand_with(
    template: &str,
    task: &str,
    repo_name: &str,
    worktree: &Path,
    ports: &PortBlock,
    extra: &BTreeMap<String, String>,
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
                _ if extra.contains_key(name) => out.push_str(&extra[name]),
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

/// `format_env` plus the hoard remote cache: `TURBO_API` and `TURBO_TEAM` join the sorted
/// variables; the token never does. `sh` and `fish` end with a line that reads `token_file` when
/// evaluated; `dotenv` and `json` leave the token out (the caller says so on stderr).
pub fn format_env_hoard(
    env: &BTreeMap<String, String>,
    f: Format,
    api: &str,
    token_file: &Path,
) -> String {
    let mut env = env.clone();
    env.insert("TURBO_API".into(), api.to_string());
    env.insert("TURBO_TEAM".into(), "hoard".into());
    let mut out = format_env(&env, f);
    let file = token_file.display().to_string();
    match f {
        Format::Sh => out.push_str(&format!(
            "export TURBO_TOKEN=\"$(cat {})\"\n",
            sh_quote(&file)
        )),
        Format::Fish => out.push_str(&format!(
            "set -gx TURBO_TOKEN (cat {})\n",
            fish_quote(&file)
        )),
        Format::Dotenv | Format::Json => {}
    }
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
