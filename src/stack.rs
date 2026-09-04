use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::util;

/// Named stacks, each an ordered list of branches where every entry is based on
/// the one before it. Several stacks can sit on the trunk in parallel.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Stacks {
    #[serde(default)]
    pub stacks: BTreeMap<String, Vec<Entry>>,
    /// Pre-named-stacks format: a single unnamed stack.
    #[serde(default, skip_serializing)]
    entries: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Entry {
    pub branch: String,
    pub base: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub pr: Option<u64>,
}

/// State lives in the shared git dir so every worktree sees the same file.
fn state_path(root: &Path) -> Result<PathBuf> {
    let common = util::git(root, &["rev-parse", "--git-common-dir"])?;
    let common = if Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        root.join(common)
    };
    Ok(common.join("rigg").join("stack.json"))
}

impl Stacks {
    pub fn load(root: &Path) -> Result<Self> {
        let path = state_path(root)?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let mut s: Stacks = serde_json::from_str(&raw).unwrap_or_default();
        // Migrate the old single-stack file, naming it after its root branch.
        if !s.entries.is_empty() && s.stacks.is_empty() {
            let name = s.entries[0].branch.clone();
            s.stacks.insert(name, std::mem::take(&mut s.entries));
        }
        Ok(s)
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = state_path(root)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The stack a branch belongs to, if any.
    pub fn stack_of(&self, branch: &str) -> Option<&str> {
        self.stacks
            .iter()
            .find(|(_, es)| es.iter().any(|e| e.branch == branch))
            .map(|(n, _)| n.as_str())
    }

    pub fn tip_of(&self, name: &str) -> Option<&Entry> {
        self.stacks.get(name).and_then(|es| es.last())
    }

    pub fn find(&self, branch: &str) -> Option<&Entry> {
        self.stacks
            .values()
            .flat_map(|es| es.iter())
            .find(|e| e.branch == branch)
    }

    /// Drop branches that no longer exist, and any stack left empty.
    pub fn retain_branches(&mut self, mut keep: impl FnMut(&Entry) -> bool) {
        for entries in self.stacks.values_mut() {
            entries.retain(|e| keep(e));
        }
        self.stacks.retain(|_, es| !es.is_empty());
    }
}
