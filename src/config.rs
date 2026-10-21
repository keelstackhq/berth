//! `berth.toml` loading and validation (spec §4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::BerthError;

const FILE: &str = "berth.toml";
const DEFAULT_INCLUDE: [&str; 4] = [".env", ".env.local", "apps/*/.env.local", ".dev.vars"];
const DEFAULT_NAMES: [(&str, u16); 4] = [("web", 0), ("api", 1), ("storybook", 2), ("db", 3)];
/// Variables berth sets itself (spec §7), besides `BERTH_*`, `PORT` and `<NAME>_PORT`.
const RESERVED_ENV: [&str; 4] = [
    "TURBO_CACHE_DIR",
    "VITE_CACHE_PATH",
    "CARGO_TARGET_DIR",
    "TMPDIR",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub version: u32,
    pub worktree: WorktreeCfg,
    pub copy: Option<CopyCfg>,
    pub ports: PortsCfg,
    pub env: BTreeMap<String, String>,
    pub db: Option<DbCfg>,
    pub cache: CacheCfg,
    pub setup: HookCfg,
    pub teardown: HookCfg,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,
            worktree: WorktreeCfg::default(),
            copy: None,
            ports: PortsCfg::default(),
            env: BTreeMap::new(),
            db: None,
            cache: CacheCfg::default(),
            setup: HookCfg::default(),
            teardown: HookCfg::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WorktreeCfg {
    pub root: String,
    pub branch_prefix: String,
    pub base: String,
    pub max: u32,
}

impl Default for WorktreeCfg {
    fn default() -> Self {
        WorktreeCfg {
            root: "../{repo}.berth".into(),
            branch_prefix: "berth/".into(),
            base: "HEAD".into(),
            max: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CopyCfg {
    pub include: Vec<String>,
    pub max_bytes: u64,
}

impl Default for CopyCfg {
    fn default() -> Self {
        CopyCfg {
            include: default_include(),
            max_bytes: 1_048_576,
        }
    }
}

fn default_include() -> Vec<String> {
    DEFAULT_INCLUDE.iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PortsCfg {
    pub count: u16,
    pub range: [u16; 2],
    pub names: Option<BTreeMap<String, u16>>,
}

impl Default for PortsCfg {
    fn default() -> Self {
        PortsCfg {
            count: 10,
            range: [21000, 30999],
            names: None,
        }
    }
}

impl PortsCfg {
    /// Explicit `names` as written; otherwise the defaults that fit in `count`.
    pub fn names(&self) -> BTreeMap<String, u16> {
        match &self.names {
            Some(names) => names.clone(),
            None => DEFAULT_NAMES
                .iter()
                .filter(|(_, offset)| *offset < self.count)
                .map(|(name, offset)| (name.to_string(), *offset))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Sqlite,
    Postgres,
    Compose,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Sqlite => "sqlite",
            Provider::Postgres => "postgres",
            Provider::Compose => "compose",
        }
    }
}

impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        match text.as_str() {
            "sqlite" => Ok(Provider::Sqlite),
            "postgres" => Ok(Provider::Postgres),
            "compose" => Ok(Provider::Compose),
            other => Err(serde::de::Error::custom(format!(
                "[db].provider `{other}` is not supported; use \"sqlite\", \"postgres\" or \"compose\""
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbCfg {
    pub provider: Provider,
    #[serde(default = "default_db_env")]
    pub env: String,
    pub url: Option<String>,
    pub template: Option<String>,
    pub path: Option<String>,
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub services: Option<Vec<String>>,
    pub project: Option<String>,
    #[serde(default = "default_db_timeout")]
    pub timeout_secs: u64,
}

fn default_db_env() -> String {
    "DATABASE_URL".into()
}

fn default_db_timeout() -> u64 {
    300
}

impl DbCfg {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    pub fn pg_host(&self) -> &str {
        self.host.as_deref().unwrap_or("localhost")
    }

    pub fn pg_port(&self) -> u16 {
        self.port.unwrap_or(5432)
    }

    /// The sqlite file inside the worktree: `path`, else `template`.
    pub fn sqlite_path(&self) -> Option<&str> {
        self.path.as_deref().or(self.template.as_deref())
    }

    fn validate(&self) -> Result<(), String> {
        use Provider::{Compose, Postgres, Sqlite};
        let p = self.provider;
        let applies: [(&str, bool, &[Provider]); 8] = [
            ("template", self.template.is_some(), &[Sqlite, Postgres]),
            ("path", self.path.is_some(), &[Sqlite]),
            ("name", self.name.is_some(), &[Postgres]),
            ("host", self.host.is_some(), &[Postgres]),
            ("port", self.port.is_some(), &[Postgres]),
            ("user", self.user.is_some(), &[Postgres]),
            ("services", self.services.is_some(), &[Compose]),
            ("project", self.project.is_some(), &[Compose]),
        ];
        for (key, given, providers) in applies {
            if given && !providers.contains(&p) {
                return Err(wrong_provider(key, providers));
            }
        }
        if !self.env.is_empty() && !is_env_name(&self.env) {
            return Err(format!(
                "[db].env `{}` must be empty or match [A-Z_][A-Z0-9_]*",
                self.env
            ));
        }
        if !self.env.is_empty() && is_reserved_env(&self.env) {
            return Err(format!(
                "[db].env `{}` is set by berth itself; choose another name",
                self.env
            ));
        }
        if self.timeout_secs == 0 {
            return Err("[db].timeout_secs = 0 would time out at once; use 1 or more".into());
        }
        if p == Sqlite {
            if self.template.is_none() {
                return Err("[db].template is required for provider = \"sqlite\"; \
                     name the database file to copy, relative to the repo"
                    .into());
            }
            for (key, value) in [("template", &self.template), ("path", &self.path)] {
                if let Some(value) = value {
                    check_relative(key, value)?;
                }
            }
        }
        if let Some(host) = &self.host {
            check_chars("host", host, true)?;
        }
        if let Some(user) = &self.user {
            check_chars("user", user, true)?;
        }
        if p == Postgres
            && let Some(t) = &self.template
            && (t.is_empty() || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err(format!(
                "[db].template `{t}` must be a database name matching [A-Za-z0-9_]+"
            ));
        }
        for service in self.services.iter().flatten() {
            check_chars("services", service, true)?;
            if service.starts_with('-') {
                return Err(format!(
                    "[db].services entry `{service}` must not start with `-`"
                ));
            }
        }
        Ok(())
    }
}

fn wrong_provider(key: &str, providers: &[Provider]) -> String {
    let list: Vec<String> = providers
        .iter()
        .map(|p| format!("\"{}\"", p.as_str()))
        .collect();
    format!(
        "[db].{key} applies only to provider = {}; remove it or change the provider",
        list.join(" or ")
    )
}

fn check_chars(key: &str, value: &str, dots: bool) -> Result<(), String> {
    let ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || (dots && b == b'.'));
    if ok {
        Ok(())
    } else {
        Err(format!("[db].{key} `{value}` must match [A-Za-z0-9._-]+"))
    }
}

fn check_relative(key: &str, value: &str) -> Result<(), String> {
    let path = Path::new(value);
    let bad = value.is_empty()
        || path.is_absolute()
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
    if bad {
        Err(format!(
            "[db].{key} `{value}` must be a non-empty relative path without `..`"
        ))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Remote {
    #[default]
    None,
    Hoard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seed {
    #[default]
    Off,
    Link,
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CacheCfg {
    pub remote: Remote,
    pub hoard_dir: String,
    pub turbo_seed: Seed,
}

impl Default for CacheCfg {
    fn default() -> Self {
        CacheCfg {
            remote: Remote::None,
            hoard_dir: ".hoard".into(),
            turbo_seed: Seed::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HookCfg {
    pub run: Vec<String>,
    pub timeout_secs: u64,
}

impl Default for HookCfg {
    fn default() -> Self {
        HookCfg {
            run: Vec::new(),
            timeout_secs: 900,
        }
    }
}

/// Where the list of files to copy comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopySource {
    /// Glob patterns relative to the main checkout.
    Include(Vec<String>),
    /// A `.worktreeinclude` file (gitignore syntax; only gitignored matches are copied).
    WorktreeInclude(PathBuf),
}

impl Config {
    pub fn copy_source(&self, main: &Path) -> CopySource {
        if let Some(copy) = &self.copy {
            return CopySource::Include(copy.include.clone());
        }
        let file = main.join(".worktreeinclude");
        if file.is_file() {
            CopySource::WorktreeInclude(file)
        } else {
            CopySource::Include(default_include())
        }
    }

    pub fn max_bytes(&self) -> u64 {
        self.copy
            .as_ref()
            .map_or_else(|| CopyCfg::default().max_bytes, |c| c.max_bytes)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "version = {} is not supported; set version = 1",
                self.version
            ));
        }
        let [low, high] = self.ports.range;
        if low > high {
            return Err(format!(
                "[ports].range = [{low}, {high}] is reversed; put the lower port first"
            ));
        }
        let size = u32::from(high) - u32::from(low) + 1;
        let count = self.ports.count;
        if count == 0 || u32::from(count) > size {
            return Err(format!(
                "[ports].count = {count} must be between 1 and the range size {size}"
            ));
        }
        if let Some(names) = &self.ports.names {
            for (name, offset) in names {
                if !is_port_name(name) {
                    return Err(format!(
                        "[ports].names key `{name}` must match [a-z][a-z0-9_]*"
                    ));
                }
                if *offset >= count {
                    return Err(format!(
                        "[ports].names `{name}` = {offset} is outside the block of {count} ports; \
                         use an offset below count or raise count"
                    ));
                }
            }
        }
        if self.worktree.max == 0 {
            return Err("[worktree].max = 0 would refuse every worktree; use 1 or more".into());
        }
        if self.worktree.branch_prefix.starts_with('-') {
            return Err(format!(
                "[worktree].branch_prefix `{}` must not start with `-`",
                self.worktree.branch_prefix
            ));
        }
        let port_vars: Vec<String> = self
            .ports
            .names()
            .keys()
            .map(|n| format!("{}_PORT", n.to_uppercase()))
            .collect();
        for key in self.env.keys() {
            if !is_env_name(key) {
                return Err(format!("[env] key `{key}` must match [A-Z_][A-Z0-9_]*"));
            }
            if is_reserved_env(key) || port_vars.contains(key) {
                return Err(format!(
                    "[env] key `{key}` is set by berth itself; choose another name"
                ));
            }
        }
        if let Some(db) = &self.db {
            db.validate()?;
            if !db.env.is_empty() && self.env.contains_key(&db.env) {
                return Err(format!(
                    "[env] sets `{}`, which is [db].env; set the URL in [db].url, not [env]",
                    db.env
                ));
            }
            if db.provider == Provider::Compose && self.env.contains_key("COMPOSE_PROJECT_NAME") {
                return Err(
                    "[env] COMPOSE_PROJECT_NAME: berth sets COMPOSE_PROJECT_NAME for \
                     provider = \"compose\"; use [db].project"
                        .into(),
                );
            }
        }
        if self.cache.remote == Remote::Hoard {
            for key in ["TURBO_API", "TURBO_TOKEN", "TURBO_TEAM"] {
                if self.env.contains_key(key) {
                    return Err(format!(
                        "[env] {key}: berth sets {key} for [cache].remote = \"hoard\"; remove it"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn is_reserved_env(key: &str) -> bool {
    key.starts_with("BERTH_") || key == "PORT" || RESERVED_ENV.contains(&key)
}

fn is_port_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'_'))
        && bytes.all(|b| matches!(b, b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}

fn config_error(message: String) -> BerthError {
    BerthError::Config {
        file: PathBuf::from(FILE),
        message,
    }
}

/// Load `<main>/berth.toml`; a missing file means the defaults.
pub fn load(main: &Path) -> Result<Config, BerthError> {
    let path = main.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(source) => return Err(BerthError::Io { path, source }),
    };
    let cfg: Config = toml::from_str(&text).map_err(|e| {
        let line = e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        match line {
            Some(line) => config_error(format!("line {line}: {}", e.message())),
            None => config_error(e.message().to_string()),
        }
    })?;
    cfg.validate().map_err(config_error)?;
    Ok(cfg)
}
