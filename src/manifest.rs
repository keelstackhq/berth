//! Manifest types and I/O (`.agents/worktrees.json`, spec §6), plus the atomic writer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::BerthError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub repo: PathBuf,
    pub worktrees: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub task: String,
    pub branch: String,
    pub base: String,
    pub path: PathBuf,
    pub state: State,
    pub ports: PortBlock,
    pub env: BTreeMap<String, String>,
    pub copied: Vec<String>,
    pub setup: SetupStatus,
    pub created_at: String,
    pub owner: Owner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<DbEntry>,
}

/// The task's database (spec §6). `name` is the postgres database, the compose project, or the
/// sqlite path relative to the worktree. Postgres connection settings are recorded so `down`
/// works after config edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DbEntry {
    pub provider: String,
    pub name: String,
    pub created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Creating,
    Ready,
    Failed,
    Removing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortBlock {
    pub start: u16,
    pub end: u16,
    pub named: BTreeMap<String, u16>,
}

impl PortBlock {
    /// A block of `count` ports from `start`; each name maps to `start + offset`.
    pub fn new(start: u16, count: u16, names: &BTreeMap<String, u16>) -> PortBlock {
        PortBlock {
            start,
            end: start + count - 1,
            named: names
                .iter()
                .map(|(n, off)| (n.clone(), start + off))
                .collect(),
        }
    }

    pub fn count(&self) -> u16 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupStatus {
    /// `ok`, `failed`, `skipped` or `running`.
    pub status: String,
    pub secs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    pub host: String,
}

/// Write `bytes` to `path` atomically: temp file `.<name>.tmp.<pid>` in the same directory,
/// `sync_all`, `rename`. Parent directories are created; the temp file is removed on any error.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BerthError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| BerthError::Io { path, source }
    };
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = parent.unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        // Persist the rename itself. Unix only: Windows cannot open a directory as a file.
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if let Err(source) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(BerthError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

pub fn manifest_path(main: &Path) -> PathBuf {
    main.join(".agents").join("worktrees.json")
}

impl Manifest {
    /// A missing file is an empty manifest for `main`; a parse error is a `Config` error.
    pub fn load(main: &Path) -> Result<Manifest, BerthError> {
        let path = manifest_path(main);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Manifest {
                    version: 1,
                    repo: main.to_path_buf(),
                    worktrees: Vec::new(),
                });
            }
            Err(source) => return Err(BerthError::Io { path, source }),
        };
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| BerthError::Config {
                file: path.clone(),
                message: format!("invalid manifest: {e}; fix or delete the file"),
            })?;
        if manifest.version != 1 {
            return Err(BerthError::Config {
                file: path,
                message: format!(
                    "unsupported manifest version {}; this berth reads version 1, upgrade berth",
                    manifest.version
                ),
            });
        }
        Ok(manifest)
    }

    pub fn save(&self, main: &Path) -> Result<(), BerthError> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("manifest serializes");
        bytes.push(b'\n');
        write_atomic(&manifest_path(main), &bytes)
    }

    pub fn find(&self, task: &str) -> Option<&Entry> {
        self.worktrees.iter().find(|e| e.task == task)
    }

    /// Replace the entry with the same task, else add it; entries stay sorted by task.
    pub fn upsert(&mut self, entry: Entry) {
        match self.worktrees.iter_mut().find(|e| e.task == entry.task) {
            Some(slot) => *slot = entry,
            None => self.worktrees.push(entry),
        }
        self.worktrees.sort_by(|a, b| a.task.cmp(&b.task));
    }

    pub fn remove(&mut self, task: &str) -> Option<Entry> {
        let at = self.worktrees.iter().position(|e| e.task == task)?;
        Some(self.worktrees.remove(at))
    }
}
