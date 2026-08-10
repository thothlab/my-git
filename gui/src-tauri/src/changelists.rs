//! Named-changelist metadata layer: the on-disk store, its sync against real git
//! status, and the mutating operations. The on-disk schema is byte-compatible with
//! the TUI version so both tools share one `<repo>/.git/changelists.json`.
//!
//! Contract source is the TUI **living spec**, not ТЗ §5 (which predates the TUI and
//! diverges). Two rules that a naive reading gets wrong (Правка `e61e291`):
//!   * an unassigned *tracked* file goes to **Default**, never the active list;
//!   * *untracked* files live in a synthetic "Unversioned Files" list that is **never
//!     persisted** — that non-persistence is exactly why the two tools never fight
//!     over untracked placement.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{ChangelistView, FileState, FileStatus, RepoSnapshot};

const DEFAULT_ID: &str = "default";
const UNVERSIONED_ID: &str = "unversioned";

/// One changelist as stored on disk. Field names/casing are fixed by the shared
/// schema — do NOT add fields (e.g. `isNotForCommit`); that would break byte-compat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredChangelist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default, rename = "isDefault")]
    pub is_default: bool,
    #[serde(default)]
    pub files: Vec<String>,
}

/// The `.git/changelists.json` document (`version: 1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    #[serde(rename = "activeChangelistId")]
    pub active_changelist_id: String,
    pub changelists: Vec<StoredChangelist>,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: 1,
            active_changelist_id: DEFAULT_ID.to_string(),
            changelists: vec![StoredChangelist {
                id: DEFAULT_ID.into(),
                name: "Default".into(),
                comment: String::new(),
                is_default: true,
                files: Vec::new(),
            }],
        }
    }
}

pub fn store_path(repo: &Path) -> PathBuf {
    // Inside .git/ ⇒ automatically outside version control and `git status`, matching
    // the TUI and JetBrains' workspace.xml placement.
    repo.join(".git").join("changelists.json")
}

/// Load the store, or a fresh Default store on first run (missing file is not an error).
pub fn load(repo: &Path) -> Result<Store> {
    match std::fs::read(store_path(repo)) {
        Ok(bytes) => {
            let mut store: Store = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Parse(format!("changelists.json: {e}")))?;
            ensure_default(&mut store);
            Ok(store)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(Error::Io(e.to_string())),
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Persist atomically: write a temp file whose name is unique **per call** (an atomic
/// counter, not just the pid — Правка `716da3a`), then rename over the target so a
/// concurrent TUI reader never sees a partial file.
pub fn save(repo: &Path, store: &Store) -> Result<()> {
    let path = store_path(repo);
    let dir = path
        .parent()
        .ok_or_else(|| Error::Io("no .git directory".into()))?;
    let mut json =
        serde_json::to_string_pretty(store).map_err(|e| Error::Parse(e.to_string()))?;
    json.push('\n');

    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("changelists.json.tmp.{}.{}", std::process::id(), n));
    std::fs::write(&tmp, json).map_err(|e| Error::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

fn ensure_default(store: &mut Store) {
    if store.changelists.iter().any(|c| c.is_default) {
        return;
    }
    if let Some(c) = store.changelists.iter_mut().find(|c| c.id == DEFAULT_ID) {
        c.is_default = true;
    } else {
        store.changelists.insert(
            0,
            StoredChangelist {
                id: DEFAULT_ID.into(),
                name: "Default".into(),
                comment: String::new(),
                is_default: true,
                files: Vec::new(),
            },
        );
    }
}

/// Reconcile the store against a git-status snapshot. Returns whether anything changed
/// (so the caller can persist only when needed). Rules per the living spec:
///   1. drop stored entries for files no longer tracked-changed;
///   2. a file belongs to at most one list (first wins);
///   3. Default always exists;
///   4. unassigned tracked files → Default (NOT the active list);
///   5. untracked files are never stored (they surface as the synthetic list in views);
///   6. active id must reference an existing list.
pub fn sync(store: &mut Store, snap: &RepoSnapshot) -> bool {
    let tracked: HashSet<&str> = snap
        .files
        .iter()
        .filter(|f| f.status != FileState::Untracked)
        .map(|f| f.path.as_str())
        .collect();
    let mut changed = false;

    // (1) prune vanished / now-untracked entries
    for cl in &mut store.changelists {
        let before = cl.files.len();
        cl.files.retain(|p| tracked.contains(p.as_str()));
        changed |= cl.files.len() != before;
    }

    // (2) a file lives in at most one list
    let mut seen: HashSet<String> = HashSet::new();
    for cl in &mut store.changelists {
        let before = cl.files.len();
        cl.files.retain(|p| seen.insert(p.clone()));
        changed |= cl.files.len() != before;
    }

    // (3) Default must exist
    if !store.changelists.iter().any(|c| c.is_default) {
        ensure_default(store);
        changed = true;
    }

    // (4) unassigned tracked files → Default
    let assigned: HashSet<&str> = seen.iter().map(|s| s.as_str()).collect();
    let unassigned: Vec<String> = snap
        .files
        .iter()
        .filter(|f| f.status != FileState::Untracked)
        .map(|f| f.path.clone())
        .filter(|p| !assigned.contains(p.as_str()))
        .collect();
    if !unassigned.is_empty() {
        let def = store.changelists.iter_mut().find(|c| c.is_default).unwrap();
        def.files.extend(unassigned);
        changed = true;
    }

    // (6) active id sane
    if !store
        .changelists
        .iter()
        .any(|c| c.id == store.active_changelist_id)
    {
        store.active_changelist_id = DEFAULT_ID.to_string();
        changed = true;
    }

    changed
}

/// Build the UI view: each stored list resolved to file statuses, plus the synthetic
/// (non-persisted) "Unversioned Files" list for untracked files.
pub fn build_views(store: &Store, snap: &RepoSnapshot) -> Vec<ChangelistView> {
    let by_path: HashMap<&str, &FileStatus> =
        snap.files.iter().map(|f| (f.path.as_str(), f)).collect();

    let mut views: Vec<ChangelistView> = store
        .changelists
        .iter()
        .map(|cl| ChangelistView {
            id: cl.id.clone(),
            name: cl.name.clone(),
            comment: cl.comment.clone(),
            is_default: cl.is_default,
            is_unversioned: false,
            files: cl
                .files
                .iter()
                .filter_map(|p| by_path.get(p.as_str()).map(|f| (*f).clone()))
                .collect(),
        })
        .collect();

    let untracked: Vec<FileStatus> = snap
        .files
        .iter()
        .filter(|f| f.status == FileState::Untracked)
        .cloned()
        .collect();
    if !untracked.is_empty() {
        views.push(ChangelistView {
            id: UNVERSIONED_ID.into(),
            name: "Unversioned Files".into(),
            comment: String::new(),
            is_default: false,
            is_unversioned: true,
            files: untracked,
        });
    }
    views
}

// ── Mutating operations (validated) ──────────────────────────────────────────

fn slugify(name: &str, store: &Store) -> String {
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "list".into() } else { base };
    let mut id = base.clone();
    let mut n = 1;
    while store.changelists.iter().any(|c| c.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn name_taken(store: &Store, name: &str, except: Option<&str>) -> bool {
    store
        .changelists
        .iter()
        .any(|c| Some(c.id.as_str()) != except && c.name.eq_ignore_ascii_case(name))
}

pub fn create(store: &mut Store, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Rule("имя списка не может быть пустым".into()));
    }
    if name_taken(store, name, None) {
        return Err(Error::Rule(format!("список \"{name}\" уже существует")));
    }
    let id = slugify(name, store);
    store.changelists.push(StoredChangelist {
        id: id.clone(),
        name: name.into(),
        comment: String::new(),
        is_default: false,
        files: Vec::new(),
    });
    Ok(id)
}

pub fn rename(store: &mut Store, id: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Rule("имя списка не может быть пустым".into()));
    }
    if name_taken(store, name, Some(id)) {
        return Err(Error::Rule(format!("список \"{name}\" уже существует")));
    }
    let cl = store
        .changelists
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| Error::Rule("список не найден".into()))?;
    cl.name = name.into();
    Ok(())
}

pub fn set_comment(store: &mut Store, id: &str, comment: &str) -> Result<()> {
    let cl = store
        .changelists
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| Error::Rule("список не найден".into()))?;
    cl.comment = comment.to_string();
    Ok(())
}

pub fn delete(store: &mut Store, id: &str) -> Result<()> {
    let idx = store
        .changelists
        .iter()
        .position(|c| c.id == id)
        .ok_or_else(|| Error::Rule("список не найден".into()))?;
    if store.changelists[idx].is_default {
        return Err(Error::Rule("список Default нельзя удалить".into()));
    }
    let files = std::mem::take(&mut store.changelists[idx].files);
    store.changelists.remove(idx);
    let def = store
        .changelists
        .iter_mut()
        .find(|c| c.is_default)
        .ok_or_else(|| Error::Rule("нет списка Default".into()))?;
    def.files.extend(files); // files return to Default, not dropped
    if store.active_changelist_id == id {
        store.active_changelist_id = DEFAULT_ID.into();
    }
    Ok(())
}

pub fn set_active(store: &mut Store, id: &str) -> Result<()> {
    if !store.changelists.iter().any(|c| c.id == id) {
        return Err(Error::Rule("список не найден".into()));
    }
    store.active_changelist_id = id.into();
    Ok(())
}

pub fn move_files(store: &mut Store, paths: &[String], to: &str) -> Result<()> {
    // Unversioned is synthetic and not in the store, so it is rejected as a target.
    if !store.changelists.iter().any(|c| c.id == to) {
        return Err(Error::Rule("целевой список не найден".into()));
    }
    let set: HashSet<&str> = paths.iter().map(|s| s.as_str()).collect();
    for cl in &mut store.changelists {
        cl.files.retain(|p| !set.contains(p.as_str()));
    }
    let dest = store.changelists.iter_mut().find(|c| c.id == to).unwrap();
    for p in paths {
        if !dest.files.contains(p) {
            dest.files.push(p.clone());
        }
    }
    Ok(())
}

/// Resolve the concrete file paths of a list (used by the commit path).
pub fn list_paths(store: &Store, id: &str) -> Vec<String> {
    store
        .changelists
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.files.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fstat(path: &str, status: FileState) -> FileStatus {
        FileStatus {
            path: path.into(),
            status,
            old_path: None,
            staged: false,
            unstaged: true,
        }
    }
    fn snap(files: Vec<FileStatus>) -> RepoSnapshot {
        RepoSnapshot {
            branch: "main".into(),
            upstream: None,
            ahead: 0,
            behind: 0,
            detached: false,
            files,
        }
    }
    fn default_list(store: &Store) -> &StoredChangelist {
        store.changelists.iter().find(|c| c.is_default).unwrap()
    }

    #[test]
    fn tracked_goes_to_default_untracked_not_persisted() {
        let mut store = Store::default();
        let s = snap(vec![
            fstat("a.rs", FileState::Modified),
            fstat("new.rs", FileState::Untracked),
        ]);
        sync(&mut store, &s);

        assert!(default_list(&store).files.contains(&"a.rs".to_string()));
        assert!(
            !store
                .changelists
                .iter()
                .any(|c| c.files.iter().any(|p| p == "new.rs")),
            "untracked file must never be persisted"
        );

        let views = build_views(&store, &s);
        let unv = views.iter().find(|v| v.is_unversioned).unwrap();
        assert!(unv.files.iter().any(|f| f.path == "new.rs"));
    }

    #[test]
    fn active_list_does_not_govern_placement() {
        let mut store = Store::default();
        let wip = create(&mut store, "WIP").unwrap();
        set_active(&mut store, &wip).unwrap();
        sync(&mut store, &snap(vec![fstat("a.rs", FileState::Modified)]));
        assert!(default_list(&store).files.contains(&"a.rs".to_string()));
        assert!(!store
            .changelists
            .iter()
            .find(|c| c.id == wip)
            .unwrap()
            .files
            .contains(&"a.rs".to_string()));
    }

    #[test]
    fn vanished_and_duplicate_pruned() {
        let mut store = Store::default();
        store.changelists[0].files.push("gone.rs".into());
        let wip = create(&mut store, "WIP").unwrap();
        // same path in two lists ⇒ dedup keeps first (Default)
        store
            .changelists
            .iter_mut()
            .find(|c| c.id == wip)
            .unwrap()
            .files
            .push("a.rs".into());
        store.changelists[0].files.push("a.rs".into());

        sync(&mut store, &snap(vec![fstat("a.rs", FileState::Modified)]));
        assert!(!default_list(&store).files.contains(&"gone.rs".to_string()));
        let count = store
            .changelists
            .iter()
            .filter(|c| c.files.iter().any(|p| p == "a.rs"))
            .count();
        assert_eq!(count, 1, "a.rs must be in exactly one list");
    }

    #[test]
    fn delete_reassigns_and_default_protected() {
        let mut store = Store::default();
        let wip = create(&mut store, "WIP").unwrap();
        move_files(&mut store, &["a.rs".to_string()], &wip).unwrap();
        delete(&mut store, &wip).unwrap();
        assert!(default_list(&store).files.contains(&"a.rs".to_string()));
        assert!(delete(&mut store, DEFAULT_ID).is_err());
    }

    #[test]
    fn duplicate_name_rejected_case_insensitive() {
        let mut store = Store::default();
        create(&mut store, "WIP").unwrap();
        assert!(create(&mut store, "WIP").is_err());
        assert!(create(&mut store, "wip").is_err());
    }

    #[test]
    fn byte_compat_with_tui_fixture() {
        // Exactly the shape the TUI writes (ТЗ §5.1 / TUI living spec).
        let fixture = r#"{"version":1,"activeChangelistId":"default","changelists":[
            {"id":"default","name":"Default","comment":"","isDefault":true,"files":["src/main.rs"]},
            {"id":"not-for-commit","name":"Not for commit","comment":"cfg","isDefault":false,"files":["config/local.xml"]}]}"#;
        let store: Store = serde_json::from_str(fixture).unwrap();
        assert_eq!(store.version, 1);
        assert_eq!(store.active_changelist_id, "default");
        let nfc = store
            .changelists
            .iter()
            .find(|c| c.name == "Not for commit")
            .unwrap();
        assert_eq!(nfc.files, vec!["config/local.xml".to_string()]);

        let json = serde_json::to_string(&store).unwrap();
        assert!(json.contains("\"isDefault\""));
        assert!(json.contains("\"activeChangelistId\""));
        assert!(!json.contains("isNotForCommit"), "no extra schema fields");
        assert!(!json.contains("isUnversioned"), "synthetic marker never stored");
    }

    #[test]
    fn save_load_roundtrip_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let mut store = Store::default();
        create(&mut store, "Not for commit").unwrap();
        save(dir.path(), &store).unwrap();

        let loaded = load(dir.path()).unwrap();
        assert!(loaded.changelists.iter().any(|c| c.name == "Not for commit"));

        let leftovers = std::fs::read_dir(dir.path().join(".git"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftovers, 0, "atomic save must not leave temp files");
    }

    #[test]
    fn first_run_without_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let store = load(dir.path()).unwrap();
        assert_eq!(store.changelists.len(), 1);
        assert!(store.changelists[0].is_default);
    }
}
