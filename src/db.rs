//! The task database (spec §4, §7): the pure plan. Provider I/O follows in a later task.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{DbCfg, Provider};
use crate::env::expand_with;
use crate::error::BerthError;
use crate::manifest::{DbEntry, PortBlock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbPlan {
    pub provider: Provider,
    /// Postgres database, compose project, or sqlite path relative to the worktree.
    pub name: String,
    pub url: Option<String>,
    /// The variable the URL is exported as; `None` when `[db].env = ""`.
    pub env_var: Option<String>,
    /// sqlite: absolute path of the database file in the worktree.
    pub abs_path: Option<PathBuf>,
}

fn cfg_err(message: String) -> BerthError {
    BerthError::Config {
        file: "berth.toml".into(),
        message,
    }
}

fn sanitize(repo: &str, keep_dash: bool) -> String {
    repo.to_lowercase()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '_' => c,
            '-' if keep_dash => c,
            _ => {
                if keep_dash {
                    '-'
                } else {
                    '_'
                }
            }
        })
        .collect()
}

/// `<repo>_<task_snake>`, repo lowercased with every char outside `[a-z0-9_]` turned into `_`.
pub fn default_pg_name(repo: &str, task: &str) -> String {
    format!("{}_{}", sanitize(repo, false), task.replace('-', "_"))
}

/// `<repo>-<task>`, repo lowercased, chars outside `[a-z0-9_-]` turned into `-`, leading `-`/`_`
/// trimmed.
pub fn default_project(repo: &str, task: &str) -> String {
    let repo = sanitize(repo, true);
    let repo = repo.trim_start_matches(['-', '_']);
    format!("{repo}-{task}")
}

fn valid_pg_name(n: &str) -> bool {
    (1..=63).contains(&n.len())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_project(n: &str) -> bool {
    let mut bytes = n.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Turn `[db]` plus task, repo and ports into the plan. Errors are `Config` (exit 3) and name the
/// key to set.
pub fn resolve(
    cfg: &DbCfg,
    repo_name: &str,
    task: &str,
    worktree: &Path,
    ports: &PortBlock,
) -> Result<DbPlan, BerthError> {
    let expand = |key: &str, template: &str, extra: &BTreeMap<String, String>| {
        expand_with(template, task, repo_name, worktree, ports, extra).map_err(|tok| {
            let what = if tok.starts_with('{') && tok.ends_with('}') && tok.len() > 1 {
                format!("unknown placeholder {tok}")
            } else {
                format!("unbalanced brace {tok}")
            };
            cfg_err(format!("{what} in [db].{key}"))
        })
    };
    let (name, abs_path, default_url) = match cfg.provider {
        Provider::Postgres => {
            let name = match &cfg.name {
                Some(t) => expand("name", t, &BTreeMap::new())?,
                None => default_pg_name(repo_name, task),
            };
            if !valid_pg_name(&name) {
                return Err(cfg_err(format!(
                    "database name {name:?} must match [a-z0-9_]{{1,63}}; set [db].name"
                )));
            }
            let user = cfg
                .user
                .as_deref()
                .map(|u| format!("{u}@"))
                .unwrap_or_default();
            let url = format!(
                "postgres://{user}{}:{}/{name}",
                cfg.pg_host(),
                cfg.pg_port()
            );
            (name, None, Some(url))
        }
        Provider::Sqlite => {
            let rel = cfg.sqlite_path().ok_or_else(|| {
                cfg_err("[db].template is required for provider = \"sqlite\"".into())
            })?;
            let abs = worktree.join(rel);
            let url = format!("file:{}", abs.display());
            (rel.to_string(), Some(abs), Some(url))
        }
        Provider::Compose => {
            let name = match &cfg.project {
                Some(t) => expand("project", t, &BTreeMap::new())?,
                None => default_project(repo_name, task),
            };
            if !valid_project(&name) {
                return Err(cfg_err(format!(
                    "compose project {name:?} must match [a-z0-9][a-z0-9_-]*; set [db].project"
                )));
            }
            (name, None, None)
        }
    };
    let url = match &cfg.url {
        Some(template) => {
            let mut extra = BTreeMap::new();
            match &abs_path {
                Some(p) => extra.insert("db.path".to_string(), p.display().to_string()),
                None => extra.insert("db.name".to_string(), name.clone()),
            };
            Some(expand("url", template, &extra)?)
        }
        None => default_url,
    };
    Ok(DbPlan {
        provider: cfg.provider,
        name,
        url,
        env_var: (!cfg.env.is_empty()).then(|| cfg.env.clone()),
        abs_path,
    })
}

/// The manifest record for a plan (`created: false`; postgres keeps its connection settings).
pub fn entry(plan: &DbPlan, cfg: &DbCfg) -> DbEntry {
    let pg = plan.provider == Provider::Postgres;
    DbEntry {
        provider: plan.provider.as_str().to_string(),
        name: plan.name.clone(),
        created: false,
        host: pg.then(|| cfg.pg_host().to_string()),
        port: pg.then(|| cfg.pg_port()),
        user: if pg { cfg.user.clone() } else { None },
    }
}
