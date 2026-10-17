//! Machine-wide port registry (`<state>/ports.json`, spec §6 and §9).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::BerthError;
use crate::manifest::{Manifest, write_atomic};
use crate::{git, lock};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub start: u16,
    pub end: u16,
    pub repo: PathBuf,
    pub task: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub blocks: Vec<Block>,
}

/// `$BERTH_STATE_DIR`, else `$XDG_STATE_HOME/berth`, else `$HOME/.local/state/berth`.
pub fn state_dir() -> Result<PathBuf, BerthError> {
    state_dir_from(|key| std::env::var(key).ok())
}

pub fn state_dir_from(get: impl Fn(&str) -> Option<String>) -> Result<PathBuf, BerthError> {
    let set = |key: &str| get(key).filter(|v| !v.is_empty());
    if let Some(dir) = set("BERTH_STATE_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return Err(BerthError::Config {
                file: PathBuf::from("BERTH_STATE_DIR"),
                message: format!(
                    "{} is not an absolute path; set BERTH_STATE_DIR to an absolute path",
                    dir.display()
                ),
            });
        }
        return Ok(dir);
    }
    // The XDG spec says a relative XDG_STATE_HOME must be ignored.
    if let Some(xdg) = set("XDG_STATE_HOME").filter(|v| Path::new(v).is_absolute()) {
        return Ok(PathBuf::from(xdg).join("berth"));
    }
    if let Some(home) = set("HOME") {
        return Ok(PathBuf::from(home).join(".local/state/berth"));
    }
    Err(BerthError::Env("set BERTH_STATE_DIR or HOME".into()))
}

impl Registry {
    pub fn empty() -> Registry {
        Registry {
            version: 1,
            blocks: Vec::new(),
        }
    }

    fn path(state: &Path) -> PathBuf {
        state.join("ports.json")
    }

    /// A missing file is an empty registry; an unreadable or invalid one is a `Config` error.
    pub fn load(state: &Path) -> Result<Registry, BerthError> {
        let path = Self::path(state);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::empty()),
            Err(source) => return Err(BerthError::Io { path, source }),
        };
        let reg: Registry = serde_json::from_slice(&bytes).map_err(|e| BerthError::Config {
            file: path.clone(),
            message: format!("invalid ports registry: {e}; fix or delete the file"),
        })?;
        if reg.version != 1 {
            return Err(BerthError::Config {
                file: path,
                message: format!(
                    "unsupported registry version {}; this berth reads version 1, upgrade berth",
                    reg.version
                ),
            });
        }
        Ok(reg)
    }

    pub fn save(&self, state: &Path) -> Result<(), BerthError> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("registry serializes");
        bytes.push(b'\n');
        write_atomic(&Self::path(state), &bytes)
    }

    pub fn add(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// Remove every block of `(repo, task)`; returns how many went.
    pub fn release(&mut self, repo: &Path, task: &str) -> usize {
        self.remove_where(|b| b.repo == repo && b.task == task)
    }

    /// Blocks that share at least one port with `start..=end`.
    pub fn overlaps(&self, start: u16, end: u16) -> Vec<&Block> {
        self.blocks
            .iter()
            .filter(|b| b.start <= end && start <= b.end)
            .collect()
    }

    pub fn remove_where(&mut self, f: impl Fn(&Block) -> bool) -> usize {
        let before = self.blocks.len();
        self.blocks.retain(|b| !f(b));
        before - self.blocks.len()
    }
}

/// Whether a block of another repo can be reclaimed. Read-only: never blocks, never creates a
/// file. A missing repo path is stale; a repo that cannot be read (not a repo any more, an
/// unreadable or invalid manifest) is not; a task still in its manifest is not; otherwise the
/// block is stale only when that repo's lock is free (an `up` there may be between ports and
/// manifest).
pub fn is_stale(b: &Block) -> bool {
    match std::fs::symlink_metadata(&b.repo) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
        Ok(_) => {}
    }
    let Ok(repo) = git::discover(&b.repo) else {
        return false;
    };
    let Ok(manifest) = Manifest::load(&repo.main) else {
        return false;
    };
    if manifest.find(&b.task).is_some() {
        return false;
    }
    !lock::is_held(&lock::repo_lock(&repo.common_dir))
}
