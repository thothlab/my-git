//! Named changelists — the metadata layer over git (ТЗ §6).
//!
//! The `.git/changelists.json` document is the contract **shared with the GUI
//! version**: both tools operate on one repository and see the same lists. This
//! module owns the model, load, startup/refresh sync against real `git status`,
//! and atomic conflict-tolerant persistence. It is pure logic — `sync` takes the
//! current changed files as input, so it is decoupled from the engine backend.

use crate::engine::ChangedFile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Schema version of `changelists.json`. Bumped only on a backward-incompatible
/// change to a stable field; additive fields do not bump it. Guarded by the
/// freeze test below (pattern from gwm-cli `contract.rs`).
pub const STORE_VERSION: u32 = 1;
/// Stable id of the always-present, non-deletable Default list.
pub const DEFAULT_ID: &str = "default";

/// One changelist. Field order matches the ТЗ §6.1 schema so serialized output
/// stays byte-shape-compatible with the GUI version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changelist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(rename = "isDefault", default)]
    pub is_default: bool,
    #[serde(default)]
    pub files: Vec<String>,
}

/// The `changelists.json` document. Unknown fields are tolerated on read
/// (forward-compat: a newer GUI may add fields); the `version` integer gates
/// backward-incompatible changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelistStore {
    pub version: u32,
    #[serde(rename = "activeChangelistId")]
    pub active_changelist_id: String,
    pub changelists: Vec<Changelist>,
}

impl Default for ChangelistStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            active_changelist_id: DEFAULT_ID.to_string(),
            changelists: vec![Changelist {
                id: DEFAULT_ID.to_string(),
                name: "Default".to_string(),
                comment: String::new(),
                is_default: true,
                files: Vec::new(),
            }],
        }
    }
}

/// `<repo_root>/.git/changelists.json`.
pub fn store_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".git").join("changelists.json")
}

impl ChangelistStore {
    /// Load the store, or return a fresh Default store when the file is absent
    /// (first run). A present-but-invalid file is a hard error.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let store: ChangelistStore = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))?;
                Ok(store)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Atomic write (temp file + rename) so a concurrent GUI writer never sees a
    /// partial file; the rename makes the last writer win at file granularity.
    pub fn persist(&self, path: &Path) -> Result<()> {
        let dir = path.parent().context("store path has no parent")?;
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_file_name(format!("changelists.json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, json.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| {
            let _ = std::fs::remove_file(&tmp);
            format!("renaming into {}", path.display())
        })?;
        let _ = dir; // parent existence is implied by a working repo (.git present)
        Ok(())
    }

    /// Reconcile the store with the current working tree (ТЗ §6.2 rules 1–4):
    /// prune vanished files, enforce one-list membership, and route unassigned
    /// changed files into the active list (or Default). Returns `true` if the
    /// store was modified (so callers can skip persisting on an idle tick).
    pub fn sync(&mut self, changed: &[ChangedFile]) -> bool {
        let before = self.changelists.clone();
        let changed_paths: BTreeSet<&str> = changed.iter().map(|f| f.path.as_str()).collect();

        // Prune vanished files; enforce at-most-one-list by tracking what we keep.
        let mut kept: BTreeSet<String> = BTreeSet::new();
        for list in &mut self.changelists {
            list.files
                .retain(|f| changed_paths.contains(f.as_str()) && kept.insert(f.clone()));
        }

        // Unassigned changed files → active list (or Default).
        let target = self.active_index();
        for f in changed {
            if !kept.contains(&f.path) {
                self.changelists[target].files.push(f.path.clone());
                kept.insert(f.path.clone());
            }
        }
        before != self.changelists
    }

    /// Index of the active list, falling back to the Default list.
    fn active_index(&self) -> usize {
        self.changelists
            .iter()
            .position(|c| c.id == self.active_changelist_id)
            .or_else(|| self.changelists.iter().position(|c| c.is_default))
            .unwrap_or(0)
    }

    fn default_index(&self) -> usize {
        self.changelists
            .iter()
            .position(|c| c.is_default)
            .unwrap_or(0)
    }

    /// Create a new (non-default) list. Rejects a duplicate name.
    pub fn create(&mut self, name: &str) -> Result<String> {
        anyhow::ensure!(
            !self.changelists.iter().any(|c| c.name == name),
            "a changelist named {name:?} already exists"
        );
        let id = self.unique_id(&slugify(name));
        self.changelists.push(Changelist {
            id: id.clone(),
            name: name.to_string(),
            comment: String::new(),
            is_default: false,
            files: Vec::new(),
        });
        Ok(id)
    }

    /// Rename a list (id stays stable). Rejects a duplicate name.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<()> {
        anyhow::ensure!(
            !self
                .changelists
                .iter()
                .any(|c| c.name == name && c.id != id),
            "a changelist named {name:?} already exists"
        );
        let list = self.list_mut(id)?;
        list.name = name.to_string();
        Ok(())
    }

    /// Delete a non-default list; its files fall back to Default. The Default
    /// list cannot be deleted.
    pub fn delete(&mut self, id: &str) -> Result<()> {
        let idx = self
            .changelists
            .iter()
            .position(|c| c.id == id)
            .with_context(|| format!("no changelist {id:?}"))?;
        anyhow::ensure!(
            !self.changelists[idx].is_default,
            "the Default changelist cannot be deleted"
        );
        let orphaned = std::mem::take(&mut self.changelists[idx].files);
        self.changelists.remove(idx);
        let def = self.default_index();
        self.changelists[def].files.extend(orphaned);
        if self.active_changelist_id == id {
            self.active_changelist_id = self.changelists[self.default_index()].id.clone();
        }
        Ok(())
    }

    /// Set the active list (unassigned changed files land here on sync).
    pub fn set_active(&mut self, id: &str) -> Result<()> {
        anyhow::ensure!(
            self.changelists.iter().any(|c| c.id == id),
            "no changelist {id:?}"
        );
        self.active_changelist_id = id.to_string();
        Ok(())
    }

    /// Move files to `target_id`, removing them from every other list so each
    /// file belongs to at most one list.
    pub fn move_files(&mut self, paths: &[String], target_id: &str) -> Result<()> {
        let target = self
            .changelists
            .iter()
            .position(|c| c.id == target_id)
            .with_context(|| format!("no changelist {target_id:?}"))?;
        let moving: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
        for list in &mut self.changelists {
            list.files.retain(|f| !moving.contains(f.as_str()));
        }
        for p in paths {
            if !self.changelists[target].files.iter().any(|f| f == p) {
                self.changelists[target].files.push(p.clone());
            }
        }
        Ok(())
    }

    /// Remove committed files from whatever list holds them (post-commit, ТЗ §6.2 rule 6).
    pub fn remove_files(&mut self, paths: &[String]) {
        let removing: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
        for list in &mut self.changelists {
            list.files.retain(|f| !removing.contains(f.as_str()));
        }
    }

    fn list_mut(&mut self, id: &str) -> Result<&mut Changelist> {
        self.changelists
            .iter_mut()
            .find(|c| c.id == id)
            .with_context(|| format!("no changelist {id:?}"))
    }

    fn unique_id(&self, base: &str) -> String {
        if !self.changelists.iter().any(|c| c.id == base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|cand| !self.changelists.iter().any(|c| &c.id == cand))
            .expect("infinite range yields a free id")
    }
}

/// Slugify a display name into a stable id (matches GUI-style ids like
/// "not-for-commit" from "Not for commit").
fn slugify(name: &str) -> String {
    let mut s = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() {
        s.push_str("list");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::FileStatus;

    fn changed(paths: &[&str]) -> Vec<ChangedFile> {
        paths
            .iter()
            .map(|p| ChangedFile {
                path: p.to_string(),
                status: FileStatus::Modified,
            })
            .collect()
    }

    // A GUI-authored fixture in the exact ТЗ §6.1 schema.
    const GUI_FIXTURE: &str = r#"{
      "version": 1,
      "activeChangelistId": "default",
      "changelists": [
        { "id": "default", "name": "Default", "comment": "", "isDefault": true, "files": ["src/main.rs"] },
        { "id": "not-for-commit", "name": "Not for commit", "comment": "local", "isDefault": false, "files": ["config/local.xml"] }
      ]
    }"#;

    #[test]
    fn store_written_in_shared_schema() {
        let mut s = ChangelistStore::default();
        s.changelists[0].files.push("src/main.rs".to_string());
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"activeChangelistId\":\"default\""));
        assert!(json.contains("\"isDefault\":true"));
        assert!(json.contains("\"files\":[\"src/main.rs\"]"));
    }

    #[test]
    fn gui_written_file_is_read_without_loss() {
        let s: ChangelistStore = serde_json::from_str(GUI_FIXTURE).unwrap();
        let nfc = s
            .changelists
            .iter()
            .find(|c| c.name == "Not for commit")
            .unwrap();
        assert_eq!(nfc.id, "not-for-commit");
        assert_eq!(nfc.files, vec!["config/local.xml"]);
    }

    #[test]
    fn tolerates_unknown_future_fields() {
        // Forward-compat: a newer GUI may add fields we don't know.
        let json = r#"{"version":1,"activeChangelistId":"default","changelists":[
            {"id":"default","name":"Default","isDefault":true,"files":[],"color":"blue"}],"extra":42}"#;
        let s: ChangelistStore = serde_json::from_str(json).unwrap();
        assert_eq!(s.changelists.len(), 1);
    }

    #[test]
    fn first_run_without_store_file() {
        let dir = std::env::temp_dir().join(format!("mygit-cl-{}", std::process::id()));
        let path = dir.join("nope.json");
        let s = ChangelistStore::load(&path).unwrap();
        assert_eq!(s.active_changelist_id, DEFAULT_ID);
        assert!(s.changelists.iter().any(|c| c.is_default));
        assert_eq!(s.changelists.len(), 1);
    }

    #[test]
    fn active_list_captures_unassigned() {
        let mut s = ChangelistStore::default();
        let wip = s.create("WIP").unwrap();
        s.set_active(&wip).unwrap();
        s.sync(&changed(&["src/ui.rs"]));
        let wip_list = s.changelists.iter().find(|c| c.id == wip).unwrap();
        assert_eq!(wip_list.files, vec!["src/ui.rs"]);
    }

    #[test]
    fn move_reassigns_exclusively() {
        let mut s = ChangelistStore::default();
        s.sync(&changed(&["config/local.xml"]));
        let nfc = s.create("Not for commit").unwrap();
        s.move_files(&["config/local.xml".to_string()], &nfc)
            .unwrap();
        let in_lists: Vec<&str> = s
            .changelists
            .iter()
            .filter(|c| c.files.iter().any(|f| f == "config/local.xml"))
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(in_lists, vec![nfc.as_str()]);
    }

    #[test]
    fn default_cannot_be_deleted() {
        let mut s = ChangelistStore::default();
        assert!(s.delete(DEFAULT_ID).is_err());
        assert!(s.changelists.iter().any(|c| c.is_default));
    }

    #[test]
    fn files_survive_list_deletion() {
        let mut s = ChangelistStore::default();
        let wip = s.create("WIP").unwrap();
        s.set_active(&wip).unwrap();
        s.sync(&changed(&["src/ui.rs"]));
        s.delete(&wip).unwrap();
        let def = s.changelists.iter().find(|c| c.is_default).unwrap();
        assert!(def.files.iter().any(|f| f == "src/ui.rs"));
    }

    #[test]
    fn vanished_file_is_pruned() {
        let mut s = ChangelistStore::default();
        s.changelists[0].files.push("old.txt".to_string());
        s.sync(&changed(&["src/main.rs"])); // old.txt no longer changed
        assert!(!s.changelists[0].files.iter().any(|f| f == "old.txt"));
        assert!(s.changelists[0].files.iter().any(|f| f == "src/main.rs"));
    }

    #[test]
    fn duplicate_name_rejected() {
        let mut s = ChangelistStore::default();
        s.create("WIP").unwrap();
        assert!(s.create("WIP").is_err());
        let id = s.changelists.last().unwrap().id.clone();
        assert!(s.rename(&id, "Default").is_err());
    }

    #[test]
    fn persist_is_atomic_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("mygit-persist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("changelists.json");
        let mut s = ChangelistStore::default();
        s.sync(&changed(&["a.rs", "b.rs"]));
        s.persist(&path).unwrap();
        // No leftover temp files in the directory.
        let temps = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(temps, 0, "atomic write must not leave temp files");
        let reloaded = ChangelistStore::load(&path).unwrap();
        assert_eq!(reloaded, s);
    }

    #[test]
    fn schema_freeze() {
        // Freeze the serialized shape the GUI depends on (AC#2). A backward-
        // incompatible change to field names/order/nesting fails this test on
        // purpose — bump STORE_VERSION and update the expectation deliberately.
        let s = ChangelistStore {
            version: 1,
            active_changelist_id: "default".to_string(),
            changelists: vec![Changelist {
                id: "default".to_string(),
                name: "Default".to_string(),
                comment: String::new(),
                is_default: true,
                files: vec!["src/main.rs".to_string()],
            }],
        };
        let expected = r#"{"version":1,"activeChangelistId":"default","changelists":[{"id":"default","name":"Default","comment":"","isDefault":true,"files":["src/main.rs"]}]}"#;
        assert_eq!(serde_json::to_string(&s).unwrap(), expected);
    }
}
