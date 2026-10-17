//! `berth.toml` loading and validation (spec §4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
            if key.starts_with("BERTH_")
                || key == "PORT"
                || RESERVED_ENV.contains(&key.as_str())
                || port_vars.contains(key)
            {
                return Err(format!(
                    "[env] key `{key}` is set by berth itself; choose another name"
                ));
            }
        }
        Ok(())
    }
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
