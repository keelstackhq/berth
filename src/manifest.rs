//! Manifest types (`.agents/worktrees.json`, spec §6). Reading and writing come later.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
