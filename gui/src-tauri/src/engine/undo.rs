//! Undo and Redo of the application's own git actions.
//!
//! Every mutating Tauri command runs through [`Undo::perform`] (in place of a bare
//! `exec::as_user`). It reads the repository before and after the action — a
//! [`Snapshot`] — and decides what the action was: a **step** with a known inverse,
//! nothing at all (the action changed nothing), or the end of the chain with a
//! reason (a push, a rebase, a failed merge, …). Undo and Redo walk the steps.
//!
//! ## The fingerprint is the guard
//!
//! A step is only ever reversed from the exact state it left: each snapshot carries a
//! digest of everything an inverse relies on, and Undo / Redo run only while the
//! repository still has the digest the chain expects. Anything else — a commit from a
//! terminal, a file saved in the editor, a background `git add` — ends the chain
//! with [`UndoReasonCode::External`] the next time anyone looks; no inverse ever runs
//! over state it did not record.
//!
//! The digest covers (FNV-1a, [`fnv1a`]: "did it change", not a security boundary):
//! the `git status` records with the branch headers but without `# branch.ab` (a
//! fetch moves the counters without touching anything local); every ref except
//! `refs/remotes/` (moved by any fetch, read by no inverse), `refs/prefetch/` (moved
//! by `git maintenance` in the background), `refs/graft/` (the
//! discard copies, moved forward by every restore, Undo's included) and `refs/stash`
//! (the stash list below says it better); the index as `ls-files --stage` (content,
//! mode and stage — not the stat data a status refresh rewrites); the stash list; the
//! unfinished operation; the bytes of every file `git status` lists, read the way
//! the discard copies read them ([`discard::describe`]).
//!
//! ## Kinds of inverse
//!
//! - [`Inverse::Soft`] — refs and index only, the working tree untouched by the
//!   action (checked: every path either snapshot lists has the same bytes): a commit
//!   (a changelist's, an amend, the first one), a reword of HEAD, `reset --soft` /
//!   `--mixed`, staging and unstaging lines. Undo moves the refs back with an
//!   `update-ref --stdin` transaction (each old value verified) and puts the index
//!   records back as they were — so a changelist commit returns its changes to the
//!   index **and** the working tree exactly as they were, "index wins" included.
//! - [`Inverse::Hard`] — `reset --hard` to the other side: merge, cherry-pick,
//!   revert, a hard reset. Only when no tracked change existed before or after
//!   (untracked files are left alone by both directions); a confirmation first.
//! - [`Inverse::Refs`] — checkout plus ref changes: switching branch or revision,
//!   creating a branch (switching to it), deleting local branches — one, or a group
//!   from the tree in one step (their upstreams are put back too), creating a tag. A switch that stashed first pops the stash back.
//! - [`Inverse::Rename`] — `branch -m` back and forth (moves config and reflog).
//! - [`Inverse::StashPush`], [`Inverse::StashRestore`], [`Inverse::StashDrop`] —
//!   the stash stack, top entry only, and restoring only onto a clean tree.
//! - [`Inverse::Discard`] — the rollback copies of `engine::discard`: Undo is
//!   `discard::restore` of the copy the discard took, Redo the restore of the copy
//!   that restore took. The index entries the rollback reset come back too.
//! - [`Inverse::Ignore`] — a rule "Ignore" appended to the root `.gitignore`
//!   ([`ignore::IgnoreEdit`]): Undo cuts it off again, or deletes the file the action
//!   created; Redo appends it again. Each only over the exact bytes it expects (an
//!   FNV-1a check of its own, on top of the chain's digest).
//!
//! ## Storage
//!
//! One chain per repository (keyed on the canonical root): in memory, and on disk
//! under `<data>/undo/<fnv1a of the root>.json` — the application data directory,
//! never the repository — written atomically (unique tmp + rename, mode 0600: it
//! names paths, branches and commit subjects). At most [`DEPTH`] steps. No file
//! content is stored: a step keeps object ids, which git keeps alive through the
//! reflog, the stash and `refs/graft/discard`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use super::cli::{fnv1a, CliEngine, TMP_COUNTER};
use super::ignore::{self, IgnoreEdit};
use super::{discard, exec, ops};
use crate::error::{Error, Result};
use crate::model::{OperationKind, UndoDirection, UndoReason, UndoReasonCode, UndoSide, UndoState};

/// Steps a repository's chain keeps; the oldest go first.
pub const DEPTH: usize = 100;

/// Version of the file format on disk. Another version reads as an empty chain.
const VERSION: u32 = 1;

/// Message of the throwaway stash an Undo of a stash restore takes the restored
/// changes away with (dropped right after).
const UNDO_STASH_MESSAGE: &str = "graft: undo";

/// The journal action name every Undo / Redo runs under.
const STEP_ACTION: &str = "undo_step";

// ── snapshot ────────────────────────────────────────────────────────────────

/// The repository as an inverse needs to know it. Built by [`capture`].
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    head: Option<String>,
    /// Short name of the branch HEAD points to; `None` when detached.
    branch: Option<String>,
    /// Every ref but remote-tracking ones, `refs/graft/` and `refs/stash`.
    refs: BTreeMap<String, String>,
    discard_tip: Option<String>,
    /// `mode oid stage` records by path, as `ls-files --stage` lists them.
    index: BTreeMap<String, Vec<String>>,
    /// (commit, reflog subject), newest first.
    stash: Vec<(String, String)>,
    operation: OperationKind,
    /// Some staged or unstaged change to a tracked path (conflicts included).
    tracked: bool,
    untracked: bool,
    unmerged: bool,
    /// Every path `git status` lists, rename sources included.
    paths: BTreeSet<String>,
    /// How paths lie on disk ([`discard::describe`]): those of `paths` and any extra.
    contents: BTreeMap<String, String>,
    digest: String,
}

fn text(out: Vec<u8>) -> String {
    String::from_utf8_lossy(&out).to_string()
}

/// Read the repository. `extra` are paths whose bytes are read on top of the listed
/// ones — the "before" paths, so that "did the action touch the working tree" can
/// be answered for a file the action made clean.
pub(crate) fn capture(repo: &Path, extra: &[String]) -> Result<Snapshot> {
    // `--no-optional-locks`: a read here must not refresh (write) the index under a
    // mutation running next to it.
    let status = exec::git(
        repo,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .run()?
    .checked()?;
    let refs = text(
        exec::git(
            repo,
            &["for-each-ref", "--format=%(refname)%00%(objectname)"],
        )
        .run()?
        .checked()?,
    );
    let index = exec::git(repo, &["ls-files", "--stage", "-z"])
        .run()?
        .checked()?;
    let stash = text(
        exec::git(repo, &["stash", "list", "--format=%H%x00%gs%x01"])
            .run()?
            .checked()?,
    );
    // The kind alone, from the markers: a bisect whose log does not parse must not
    // make every snapshot — and so every action — fail.
    let operation = ops::detect_kind(repo)?;

    let mut snap = Snapshot {
        head: None,
        branch: None,
        refs: BTreeMap::new(),
        discard_tip: None,
        index: BTreeMap::new(),
        stash: Vec::new(),
        operation,
        tracked: false,
        untracked: false,
        unmerged: false,
        paths: BTreeSet::new(),
        contents: BTreeMap::new(),
        digest: String::new(),
    };
    let mut digest: Vec<u8> = Vec::new();

    // status: NUL-separated records; a rename (`2`) is followed by its source.
    let status = text(status);
    let tokens: Vec<&str> = status.split('\0').collect();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        i += 1;
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t.strip_prefix("# ") {
            if h.starts_with("branch.ab ") {
                continue;
            }
            if let Some(o) = h.strip_prefix("branch.oid ") {
                snap.head = (o != "(initial)").then(|| o.to_string());
            } else if let Some(b) = h.strip_prefix("branch.head ") {
                snap.branch = (b != "(detached)").then(|| b.to_string());
            }
            digest.extend_from_slice(t.as_bytes());
            digest.push(0);
            continue;
        }
        digest.extend_from_slice(t.as_bytes());
        digest.push(0);
        let bad = || Error::Parse(format!("unexpected status record {t:?}"));
        match t.as_bytes()[0] {
            b'1' => {
                let path = t.splitn(9, ' ').nth(8).ok_or_else(bad)?;
                snap.tracked = true;
                snap.paths.insert(path.to_string());
            }
            b'2' => {
                let path = t.splitn(10, ' ').nth(9).ok_or_else(bad)?;
                let orig = tokens
                    .get(i)
                    .copied()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(bad)?;
                i += 1;
                digest.extend_from_slice(orig.as_bytes());
                digest.push(0);
                snap.tracked = true;
                snap.paths.insert(path.to_string());
                snap.paths.insert(orig.to_string());
            }
            b'u' => {
                let path = t.splitn(11, ' ').nth(10).ok_or_else(bad)?;
                snap.tracked = true;
                snap.unmerged = true;
                snap.paths.insert(path.to_string());
            }
            b'?' => {
                let path = t.get(2..).filter(|p| !p.is_empty()).ok_or_else(bad)?;
                snap.untracked = true;
                snap.paths.insert(path.to_string());
            }
            b'!' => {}
            _ => return Err(bad()),
        }
    }
    digest.push(0x1e);

    for line in refs.lines().filter(|l| !l.is_empty()) {
        let (name, oid) = line
            .split_once('\0')
            .ok_or_else(|| Error::Parse(format!("unexpected for-each-ref line {line:?}")))?;
        if name == discard::DISCARD_REF {
            snap.discard_tip = Some(oid.to_string());
        }
        // `refs/prefetch/`: `git maintenance` moves them in the background, hourly.
        if name.starts_with("refs/remotes/")
            || name.starts_with("refs/prefetch/")
            || name.starts_with("refs/graft/")
            || name == "refs/stash"
        {
            continue;
        }
        digest.extend_from_slice(line.as_bytes());
        digest.push(b'\n');
        snap.refs.insert(name.to_string(), oid.to_string());
    }
    digest.push(0x1e);

    for rec in index.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        let (meta, path) = rec
            .split_once('\t')
            .ok_or_else(|| Error::Parse(format!("unexpected ls-files record {rec:?}")))?;
        snap.index
            .entry(path.to_string())
            .or_default()
            .push(meta.to_string());
    }
    digest.extend_from_slice(&index);
    digest.push(0x1e);

    for record in stash.split('\u{1}') {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let (oid, subject) = record
            .split_once('\0')
            .ok_or_else(|| Error::Parse(format!("unexpected stash record {record:?}")))?;
        snap.stash.push((oid.to_string(), subject.to_string()));
    }
    digest.extend_from_slice(stash.as_bytes());
    digest.push(0x1e);
    digest.extend_from_slice(format!("{:?}", snap.operation).as_bytes());
    digest.push(0x1e);

    let mut want: Vec<String> = snap.paths.iter().cloned().collect();
    want.extend(extra.iter().filter(|p| !snap.paths.contains(*p)).cloned());
    for (path, entry) in discard::describe(repo, &want)? {
        snap.contents.insert(path, entry);
    }
    for p in &snap.paths {
        let e = snap
            .contents
            .get(p.trim_end_matches('/'))
            .map(String::as_str);
        digest.extend_from_slice(p.as_bytes());
        digest.push(0);
        digest.extend_from_slice(e.unwrap_or("?").as_bytes());
        digest.push(0);
    }
    snap.digest = fnv1a(&digest);
    Ok(snap)
}

impl Snapshot {
    fn paths_vec(&self) -> Vec<String> {
        self.paths.iter().cloned().collect()
    }

    fn side(&self) -> Side {
        Side {
            head: self.head.clone(),
            branch: self.branch.clone(),
            digest: self.digest.clone(),
        }
    }

    /// How `path` lay on disk, or `None` when this snapshot cannot tell: as read,
    /// when it was read; else as the index has it — a path `git status` did not list
    /// is byte for byte its index entry (a filter aside, which only makes the answer
    /// "changed", never a false "same").
    fn content(&self, path: &str) -> Option<String> {
        let p = path.trim_end_matches('/');
        if let Some(c) = self.contents.get(p) {
            return Some(c.clone());
        }
        if self.paths.contains(path) {
            return None;
        }
        let recs = self.index.get(p)?;
        recs.iter()
            .find_map(|r| r.strip_suffix(" 0"))
            .map(str::to_string)
    }
}

/// Did the working tree stay byte for byte as it was? Asked of every path either
/// snapshot lists **and** every path whose index entry moved: a hard reset on a
/// clean tree leaves nothing for `git status` to list on either side, and only the
/// index says which files it rewrote. `after` must have been captured with `before`'s
/// paths as extra.
fn untouched(before: &Snapshot, after: &Snapshot) -> bool {
    let moved = index_delta(before, after);
    let mut paths: BTreeSet<&str> = before
        .paths
        .iter()
        .chain(&after.paths)
        .map(String::as_str)
        .collect();
    paths.extend(moved.iter().map(|c| c.path.as_str()));
    let same = paths.into_iter().all(|p| {
        let was = before.content(p);
        was.is_some() && was == after.content(p)
    });
    same
}

// ── steps ───────────────────────────────────────────────────────────────────

/// Where HEAD was and the digest the repository had.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Side {
    head: Option<String>,
    branch: Option<String>,
    digest: String,
}

impl Side {
    fn position_differs(&self, other: &Side) -> bool {
        self.branch != other.branch || self.head != other.head
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefChange {
    name: String,
    before: Option<String>,
    after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexChange {
    path: String,
    before: Vec<String>,
    after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Upstream {
    branch: String,
    remote: String,
    merge: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Inverse {
    Soft,
    Hard,
    #[serde(rename_all = "camelCase")]
    Refs {
        /// The switch stashed local changes first (`branch_checkout` with stash);
        /// `target` is the name it switched to, for Redo.
        stash: bool,
        target: Option<String>,
        /// Written by chains saved before the group delete; read, never written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upstream: Option<Upstream>,
        /// The upstreams of the deleted local branches, one per branch that had one.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        upstreams: Vec<Upstream>,
    },
    Rename {
        from: String,
        to: String,
    },
    StashPush {
        message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    StashRestore {
        pop: bool,
        index: usize,
        oid: String,
        subject: String,
    },
    StashDrop {
        oid: String,
        subject: String,
    },
    #[serde(rename_all = "camelCase")]
    Discard {
        undo_id: String,
        redo_id: Option<String>,
    },
    Ignore {
        edit: IgnoreEdit,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    id: u64,
    action: String,
    detail: String,
    inverse: Inverse,
    before: Side,
    after: Side,
    /// The digest the last Undo of this step left — what Redo expects.
    undone: Option<String>,
    refs: Vec<RefChange>,
    index: Vec<IndexChange>,
    /// For a changelist commit: the list each committed path sat in. Undo hands
    /// them back so the files return to their lists, not to Default.
    #[serde(default)]
    lists: Vec<(String, String)>,
}

fn ref_delta(b: &Snapshot, a: &Snapshot) -> Vec<RefChange> {
    let names: BTreeSet<&String> = b.refs.keys().chain(a.refs.keys()).collect();
    names
        .into_iter()
        .filter_map(|n| {
            let (x, y) = (b.refs.get(n), a.refs.get(n));
            (x != y).then(|| RefChange {
                name: n.clone(),
                before: x.cloned(),
                after: y.cloned(),
            })
        })
        .collect()
}

fn index_delta(b: &Snapshot, a: &Snapshot) -> Vec<IndexChange> {
    let paths: BTreeSet<&String> = b.index.keys().chain(a.index.keys()).collect();
    paths
        .into_iter()
        .filter_map(|p| {
            let (x, y) = (b.index.get(p), a.index.get(p));
            (x != y).then(|| IndexChange {
                path: p.clone(),
                before: x.cloned().unwrap_or_default(),
                after: y.cloned().unwrap_or_default(),
            })
        })
        .collect()
}

/// What the call site knows that the snapshots cannot tell.
#[derive(Debug, Clone, Default)]
pub struct Hint {
    /// The command's own arguments that matter: a branch or stash name, a path, a
    /// reset mode. See each command in `commands.rs`.
    pub args: Vec<String>,
    /// For a changelist commit: (path, list id) of every committed path.
    pub lists: Vec<(String, String)>,
    /// For `file_ignore`: the rule the action writes, planned before it runs.
    pub ignore: Option<IgnoreEdit>,
}

impl Hint {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn args<S: Into<String>>(args: impl IntoIterator<Item = S>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    fn arg(&self, i: usize) -> Option<&str> {
        self.args.get(i).map(String::as_str)
    }
}

enum Verdict {
    /// The action changed nothing: the chain stays as it was.
    Nothing,
    Break(UndoReason),
    Record(Step),
}

fn reason(code: UndoReasonCode, action: Option<&str>) -> UndoReason {
    UndoReason {
        code,
        action: action.map(str::to_string),
    }
}

fn short(oid: Option<&str>) -> String {
    oid.map(|o| o.chars().take(7).collect()).unwrap_or_default()
}

/// First line of a commit's message, or empty when it cannot be read.
fn subject(repo: &Path, oid: Option<&str>) -> String {
    let Some(oid) = oid else {
        return String::new();
    };
    exec::git(
        repo,
        &["log", "-1", "--format=%s", "--end-of-options", oid, "--"],
    )
    .run()
    .ok()
    .filter(|o| o.success())
    .map(|o| o.stdout_text().trim().to_string())
    .unwrap_or_default()
}

/// The message of a stash's reflog subject: `On main: text` → `text`.
fn stash_text(subject: &str) -> String {
    subject
        .split_once(": ")
        .map(|(_, m)| m)
        .unwrap_or(subject)
        .to_string()
}

/// `N` of a `stash@{N}` at the start of `name` (a bare name or a whole
/// `stash_list_app` record).
fn stash_index(name: &str) -> Option<usize> {
    name.trim()
        .strip_prefix("stash@{")?
        .split('}')
        .next()?
        .parse()
        .ok()
}

fn upstream_of(repo: &Path, branch: &str) -> Option<Upstream> {
    let get = |key: &str| {
        exec::git(
            repo,
            &["config", "--get", &format!("branch.{branch}.{key}")],
        )
        .run()
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout_text().trim().to_string())
    };
    Some(Upstream {
        branch: branch.to_string(),
        remote: get("remote")?,
        merge: get("merge")?,
    })
}

fn heads_or_tags(name: &str) -> bool {
    name.starts_with("refs/heads/") || name.starts_with("refs/tags/")
}

struct Ctx<'a> {
    repo: &'a Path,
    action: &'a str,
    hint: &'a Hint,
    b: &'a Snapshot,
    a: &'a Snapshot,
}

impl Ctx<'_> {
    fn brk(&self, code: UndoReasonCode) -> Verdict {
        Verdict::Break(reason(code, Some(self.action)))
    }

    fn record(&self, inverse: Inverse, detail: String) -> Verdict {
        Verdict::Record(Step {
            id: 0,
            action: self.action.to_string(),
            detail,
            inverse,
            before: self.b.side(),
            after: self.a.side(),
            undone: None,
            refs: ref_delta(self.b, self.a),
            index: index_delta(self.b, self.a),
            lists: self.hint.lists.clone(),
        })
    }

    fn same_position(&self) -> bool {
        self.b.branch == self.a.branch && self.b.head == self.a.head
    }

    /// Refs and index only, the working tree left alone.
    fn soft(&self, detail: String) -> Verdict {
        let (b, a) = (self.b, self.a);
        if b.branch != a.branch || b.stash != a.stash || b.discard_tip != a.discard_tip {
            return self.brk(UndoReasonCode::Unsupported);
        }
        if !untouched(b, a) {
            return self.brk(UndoReasonCode::Worktree);
        }
        if ref_delta(b, a).iter().any(|r| !heads_or_tags(&r.name)) {
            return self.brk(UndoReasonCode::Unsupported);
        }
        self.record(Inverse::Soft, detail)
    }

    fn hard_ok(&self) -> bool {
        let (b, a) = (self.b, self.a);
        let own = b.branch.as_ref().map(|n| format!("refs/heads/{n}"));
        b.branch == a.branch
            && b.head.is_some()
            && a.head.is_some()
            && b.stash == a.stash
            && ref_delta(b, a)
                .iter()
                .all(|r| Some(&r.name) == own.as_ref())
    }

    /// `reset --hard` both ways — only with no tracked change on either side.
    fn hard(&self, detail: String) -> Verdict {
        if self.b.tracked || self.a.tracked || self.a.unmerged {
            return self.brk(UndoReasonCode::Dirty);
        }
        if !self.hard_ok() {
            return self.brk(UndoReasonCode::Unsupported);
        }
        self.record(Inverse::Hard, detail)
    }

    /// A reset: whichever of the two above fits what it did.
    fn reset(&self) -> Verdict {
        let detail = short(self.a.head.as_deref());
        match self.soft(detail.clone()) {
            Verdict::Break(r) if r.code == UndoReasonCode::Worktree => self.hard(detail),
            v => v,
        }
    }

    fn refs(&self, detail: String, upstreams: Vec<Upstream>) -> Verdict {
        let (b, a) = (self.b, self.a);
        let delta = ref_delta(b, a);
        if delta.iter().any(|r| !heads_or_tags(&r.name)) || b.discard_tip != a.discard_tip {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let stashed = a.stash.len() == b.stash.len() + 1 && a.stash[1..] == b.stash[..];
        if stashed {
            if a.tracked || a.untracked {
                return self.brk(UndoReasonCode::Unsupported);
            }
        } else if a.stash != b.stash {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let moved = !self.same_position();
        if moved && b.head.is_none() {
            return self.brk(UndoReasonCode::Unsupported);
        }
        if !moved {
            if delta.is_empty() {
                return self.brk(UndoReasonCode::Unsupported);
            }
            if !untouched(b, a) || !index_delta(b, a).is_empty() {
                return self.brk(UndoReasonCode::Worktree);
            }
        }
        let target = stashed
            .then(|| self.hint.arg(0).map(str::to_string))
            .flatten();
        if stashed && target.is_none() {
            return self.brk(UndoReasonCode::Unsupported);
        }
        self.record(
            Inverse::Refs {
                stash: stashed,
                target,
                upstream: None,
                upstreams,
            },
            detail,
        )
    }

    fn rename(&self) -> Verdict {
        let (Some(from), Some(to)) = (self.hint.arg(0), self.hint.arg(1)) else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        let delta = ref_delta(self.b, self.a);
        let (f, t) = (format!("refs/heads/{from}"), format!("refs/heads/{to}"));
        let ok = delta.len() == 2
            && delta
                .iter()
                .any(|r| r.name == f && r.before.is_some() && r.after.is_none())
            && delta
                .iter()
                .any(|r| r.name == t && r.before.is_none() && r.after.is_some())
            && self.b.stash == self.a.stash
            && self.b.head == self.a.head
            && index_delta(self.b, self.a).is_empty();
        if !ok {
            return self.brk(UndoReasonCode::Unsupported);
        }
        self.record(
            Inverse::Rename {
                from: from.to_string(),
                to: to.to_string(),
            },
            format!("{from} → {to}"),
        )
    }

    /// Nothing but the stash (and what it takes from or gives to the tree) moved.
    fn stash_only(&self) -> bool {
        self.same_position()
            && ref_delta(self.b, self.a).is_empty()
            && self.b.discard_tip == self.a.discard_tip
    }

    fn stash_push(&self) -> Verdict {
        let (b, a) = (self.b, self.a);
        let pushed = a.stash.len() == b.stash.len() + 1 && a.stash[1..] == b.stash[..];
        if !pushed || !self.stash_only() || a.tracked || a.untracked {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let message = self.hint.arg(0).filter(|m| !m.trim().is_empty());
        self.record(
            Inverse::StashPush {
                message: message.map(str::to_string),
            },
            stash_text(&a.stash[0].1),
        )
    }

    fn stash_restore(&self, pop: bool) -> Verdict {
        let (b, a) = (self.b, self.a);
        let Some(n) = self.hint.arg(0).and_then(stash_index) else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        if b.tracked || b.untracked || a.unmerged {
            return self.brk(UndoReasonCode::StashDirty);
        }
        if !self.stash_only() {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let Some((oid, subject)) = b.stash.get(n).cloned() else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        if pop {
            if n != 0 {
                return self.brk(UndoReasonCode::StashPosition);
            }
            if a.stash[..] != b.stash[1..] {
                return self.brk(UndoReasonCode::Unsupported);
            }
        } else if a.stash != b.stash {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let detail = stash_text(&subject);
        self.record(
            Inverse::StashRestore {
                pop,
                index: n,
                oid,
                subject,
            },
            detail,
        )
    }

    fn stash_drop(&self) -> Verdict {
        let (b, a) = (self.b, self.a);
        let Some(n) = self.hint.arg(0).and_then(stash_index) else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        if n != 0 {
            return self.brk(UndoReasonCode::StashPosition);
        }
        let dropped = !b.stash.is_empty() && a.stash[..] == b.stash[1..];
        if !dropped || !self.stash_only() || !index_delta(b, a).is_empty() || !untouched(b, a) {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let (oid, subject) = b.stash[0].clone();
        let detail = stash_text(&subject);
        self.record(Inverse::StashDrop { oid, subject }, detail)
    }

    fn discard(&self) -> Verdict {
        let (b, a) = (self.b, self.a);
        let Some(tip) = a
            .discard_tip
            .clone()
            .filter(|_| a.discard_tip != b.discard_tip)
        else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        if !self.same_position() || !ref_delta(b, a).is_empty() || b.stash != a.stash {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let entry = match discard::list(self.repo, 1) {
            Ok(list) => list.into_iter().next().filter(|e| e.id == tip),
            Err(_) => None,
        };
        let Some(entry) = entry else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        let detail = match entry.paths.as_slice() {
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} +{}", rest.len()),
            [] => String::new(),
        };
        self.record(
            Inverse::Discard {
                undo_id: tip,
                redo_id: None,
            },
            detail,
        )
    }
}

impl Ctx<'_> {
    /// A rule appended to `.gitignore`: nothing may have moved but that file (and
    /// the paths the rule now hides, which leave the status list byte for byte as
    /// they were). Anything else happened alongside, and Undo would reverse it blind.
    fn ignore(&self) -> Verdict {
        let (b, a) = (self.b, self.a);
        let Some(edit) = self.hint.ignore.clone() else {
            return self.brk(UndoReasonCode::Unsupported);
        };
        if !self.same_position()
            || !ref_delta(b, a).is_empty()
            || !index_delta(b, a).is_empty()
            || b.stash != a.stash
            || b.discard_tip != a.discard_tip
        {
            return self.brk(UndoReasonCode::Unsupported);
        }
        let others = b
            .paths
            .iter()
            .chain(&a.paths)
            .filter(|p| p.as_str() != ignore::IGNORE_FILE)
            .all(|p| {
                let was = b.content(p);
                was.is_some() && was == a.content(p)
            });
        if !others {
            return self.brk(UndoReasonCode::Worktree);
        }
        let detail = edit.pattern.clone();
        self.record(Inverse::Ignore { edit }, detail)
    }
}

/// What the action `action` did, going from `b` to `a`.
fn classify(
    repo: &Path,
    action: &str,
    hint: &Hint,
    ok: bool,
    b: &Snapshot,
    a: &Snapshot,
    upstreams: Vec<Upstream>,
) -> Verdict {
    let cx = Ctx {
        repo,
        action,
        hint,
        b,
        a,
    };
    let remote_delete = match action {
        "branch_delete" => hint.arg(1) == Some("remote"),
        "branch_delete_many" => hint.arg(0) == Some("remote"),
        _ => false,
    };
    // Remotes live in `.git/config` and `refs/remotes/`, neither of which the digest
    // reads, so their arms come before the "nothing changed" answer — otherwise the
    // verdict would depend on whether HEAD's branch happened to track the remote (the
    // `# branch.upstream` header is in the digest). Adding a remote and changing its
    // address touch nothing an inverse relies on: the chain goes on. A rename or a
    // removal re-points or unsets the upstream of every branch tracking it, and an
    // inverse that puts an upstream back (a deleted branch's) would then name a remote
    // that is gone — the chain ends, with that reason.
    //
    // Worktrees: the snapshot is of the open worktree, and adding, removing, locking
    // or pruning another one changes nothing in it — except that an added worktree
    // has a branch checked out now (a new one, or one this chain may have created
    // or renamed). The inverses move branches with `update-ref`, which does not ask
    // other worktrees, so an Undo could delete or move that branch from under the
    // new folder: an added worktree ends the chain, whatever the digest says. The
    // rest leaves it as it is — removing another worktree frees a branch, it does
    // not take one.
    match action {
        "remote_add" | "remote_set_url" => return Verdict::Nothing,
        "remote_rename" | "remote_remove" if ok => return cx.brk(UndoReasonCode::Remotes),
        "worktree_add" if ok => return cx.brk(UndoReasonCode::Worktrees),
        "worktree_remove" | "worktree_lock" | "worktree_unlock" | "worktree_prune" => {
            return Verdict::Nothing
        }
        _ => {}
    }
    if b.digest == a.digest {
        // Publishing changes nothing here and everything for whoever pulls: the
        // commits Undo would take back are someone else's history now.
        return if ok && (action == "push" || remote_delete) {
            cx.brk(UndoReasonCode::Published)
        } else {
            Verdict::Nothing
        };
    }
    // A bisect moves HEAD through history on git's schedule and keeps its own
    // marks; nothing here can be reversed step by step, before, during or after.
    if action.starts_with("op_bisect")
        || b.operation == OperationKind::Bisect
        || a.operation == OperationKind::Bisect
    {
        return cx.brk(UndoReasonCode::Bisect);
    }
    if !ok {
        return cx.brk(UndoReasonCode::Failed);
    }
    if b.operation != OperationKind::None || a.operation != OperationKind::None {
        return cx.brk(UndoReasonCode::Operation);
    }
    let head_subject = || subject(repo, a.head.as_deref());
    let arg0 = || hint.arg(0).unwrap_or_default().to_string();
    match action {
        "commit_list" => cx.soft(head_subject()),
        "lines_stage" | "lines_unstage" => cx.soft(arg0()),
        "commit_reword" => {
            // A HEAD reword keeps the parents; an older one is a rebase underneath.
            let parents = |oid: Option<&str>| {
                oid.and_then(|o| {
                    exec::git(
                        repo,
                        &["log", "-1", "--format=%P", "--end-of-options", o, "--"],
                    )
                    .run()
                    .ok()
                    .filter(|x| x.success())
                    .map(|x| x.stdout_text())
                })
            };
            let (pb, pa) = (parents(b.head.as_deref()), parents(a.head.as_deref()));
            if pb.is_some() && pb == pa {
                cx.soft(head_subject())
            } else {
                cx.brk(UndoReasonCode::History)
            }
        }
        "commit_reset" => cx.reset(),
        "branch_merge" => cx.hard(arg0()),
        "commit_cherry_pick" | "commit_revert" => cx.hard(head_subject()),
        "branch_checkout" => cx.refs(
            a.branch.clone().unwrap_or_else(|| short(a.head.as_deref())),
            Vec::new(),
        ),
        "commit_checkout" => cx.refs(short(a.head.as_deref()), Vec::new()),
        "branch_create" | "tag_create" => cx.refs(arg0(), Vec::new()),
        "branch_delete" | "branch_delete_many" if remote_delete => {
            cx.brk(UndoReasonCode::Published)
        }
        "branch_delete" => cx.refs(arg0(), upstreams),
        // The names, joined: `, ` cannot occur inside a branch name (no spaces).
        "branch_delete_many" => {
            cx.refs(hint.args.get(1..).unwrap_or_default().join(", "), upstreams)
        }
        "branch_rename" => cx.rename(),
        "stash_push" => cx.stash_push(),
        "stash_pop" => cx.stash_restore(true),
        "stash_apply" | "stash_restore" => cx.stash_restore(false),
        "stash_drop" => cx.stash_drop(),
        "file_rollback" | "list_rollback" | "lines_revert" | "discard_restore" => cx.discard(),
        "file_ignore" => cx.ignore(),
        "push" => cx.brk(UndoReasonCode::Published),
        "fetch" => cx.brk(UndoReasonCode::Fetched),
        // Only here when the digest moved: remote-tracking refs are not in it, so
        // that is new tags (git's default tag following). Not `Nothing` — the
        // chain would then fail its next check and end as "External", the wrong
        // reason.
        "fetch_background" => cx.brk(UndoReasonCode::Fetched),
        "pull" | "branch_update" => cx.brk(UndoReasonCode::Integrated),
        "branch_rebase_onto" | "op_rebase_start" | "commits_squash" => {
            cx.brk(UndoReasonCode::History)
        }
        "git_exec" => cx.brk(UndoReasonCode::Console),
        _ => cx.brk(UndoReasonCode::Unsupported),
    }
}

// ── running an inverse ──────────────────────────────────────────────────────

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    exec::git(repo, args).run()?.checked_both()
}

/// Move refs from one side of `changes` to the other in one transaction, each old
/// value verified: a ref someone moved since makes the whole move fail untouched.
fn set_refs(repo: &Path, changes: &[RefChange], back: bool, why: &str) -> Result<()> {
    let mut input = Vec::new();
    for c in changes {
        let (from, to) = if back {
            (&c.after, &c.before)
        } else {
            (&c.before, &c.after)
        };
        let line = match (from, to) {
            (Some(f), Some(t)) => format!("update {}\0{t}\0{f}\0", c.name),
            (None, Some(t)) => format!("create {}\0{t}\0", c.name),
            (Some(f), None) => format!("delete {}\0{f}\0", c.name),
            (None, None) => continue,
        };
        input.extend_from_slice(line.as_bytes());
    }
    if input.is_empty() {
        return Ok(());
    }
    exec::git(repo, &["update-ref", "-m", why, "--stdin", "-z"])
        .input(&input)
        .run()?
        .checked_both()?;
    Ok(())
}

/// Put the index records of `changes` back on one side. Two batches — every
/// changed path removed, then the wanted records added: a removal and an addition of
/// one path in the same `--index-info` batch leave the old entry in place.
fn set_index(repo: &Path, changes: &[IndexChange], back: bool) -> Result<()> {
    let oid_len = changes
        .iter()
        .flat_map(|c| c.before.iter().chain(&c.after))
        .find_map(|r| r.split(' ').nth(1).map(str::len))
        .unwrap_or(40);
    let zero = "0".repeat(oid_len);
    let mut removals = Vec::new();
    let mut additions = Vec::new();
    for c in changes {
        let (from, to) = if back {
            (&c.after, &c.before)
        } else {
            (&c.before, &c.after)
        };
        if !from.is_empty() {
            removals.extend_from_slice(format!("0 {zero}\t{}\0", c.path).as_bytes());
        }
        for r in to {
            additions.extend_from_slice(format!("{r}\t{}\0", c.path).as_bytes());
        }
    }
    for batch in [removals, additions] {
        if !batch.is_empty() {
            exec::git(repo, &["update-index", "--add", "-z", "--index-info"])
                .input(&batch)
                .run()?
                .checked_both()?;
        }
    }
    Ok(())
}

fn checkout(repo: &Path, side: &Side) -> Result<()> {
    match (&side.branch, &side.head) {
        (Some(b), _) => git(repo, &["checkout", "-q", "--end-of-options", b, "--"])?,
        (None, Some(h)) => git(
            repo,
            &["checkout", "-q", "--detach", "--end-of-options", h, "--"],
        )?,
        (None, None) => return Err(Error::Rule("there is no revision to switch back to".into())),
    };
    Ok(())
}

fn run_inverse(repo: &Path, step: &mut Step, dir: UndoDirection) -> Result<()> {
    let back = dir == UndoDirection::Undo;
    let why = if back {
        format!("graft: undo {}", step.action)
    } else {
        format!("graft: redo {}", step.action)
    };
    let (from, to) = if back {
        (step.after.clone(), step.before.clone())
    } else {
        (step.before.clone(), step.after.clone())
    };
    match &mut step.inverse {
        Inverse::Soft => {
            set_refs(repo, &step.refs, back, &why)?;
            if from.branch.is_none() && to.branch.is_none() && from.head != to.head {
                let (Some(f), Some(t)) = (&from.head, &to.head) else {
                    return Err(Error::Rule("a detached HEAD without a revision".into()));
                };
                git(
                    repo,
                    &["update-ref", "--no-deref", "-m", &why, "HEAD", t, f],
                )?;
            }
            set_index(repo, &step.index, back)?;
        }
        Inverse::Hard => {
            let target = to
                .head
                .as_deref()
                .ok_or_else(|| Error::Rule("no revision to reset to".into()))?;
            git(
                repo,
                &["reset", "--hard", "-q", "--end-of-options", target, "--"],
            )?;
        }
        Inverse::Refs {
            stash,
            target,
            upstream,
            upstreams,
        } => {
            let upstreams = upstream.iter().chain(upstreams.iter());
            let moved = from.position_differs(&to);
            if back {
                if moved {
                    checkout(repo, &to)?;
                }
                set_refs(repo, &step.refs, true, &why)?;
                for u in upstreams {
                    let (r, m) = (
                        format!("branch.{}.remote", u.branch),
                        format!("branch.{}.merge", u.branch),
                    );
                    git(repo, &["config", &r, &u.remote])?;
                    git(repo, &["config", &m, &u.merge])?;
                }
                if *stash {
                    git(repo, &["stash", "pop", "--index", "-q", "stash@{0}"])?;
                }
            } else {
                set_refs(repo, &step.refs, false, &why)?;
                for u in upstreams {
                    let section = format!("branch.{}", u.branch);
                    let _ = exec::git(repo, &["config", "--remove-section", &section]).run();
                }
                match (stash, target) {
                    (true, Some(t)) => CliEngine::new(repo).checkout(t, true)?,
                    _ if moved => checkout(repo, &to)?,
                    _ => {}
                }
            }
        }
        Inverse::Rename { from: f, to: t } => {
            let (a, b) = if back { (&*t, &*f) } else { (&*f, &*t) };
            git(repo, &["branch", "-m", "--", a, b])?;
        }
        Inverse::StashPush { message } => {
            if back {
                git(repo, &["stash", "pop", "--index", "-q", "stash@{0}"])?;
            } else {
                ops::stash_push(repo, message.as_deref())?;
            }
        }
        Inverse::StashRestore {
            pop,
            index,
            oid,
            subject,
        } => {
            if back {
                // The tree was clean before the restore, so everything in it now is
                // what the restore brought: taken away whole, the stash kept (or put
                // back, for a pop) holds the same content.
                git(
                    repo,
                    &["stash", "push", "-u", "-q", "-m", UNDO_STASH_MESSAGE],
                )?;
                git(repo, &["stash", "drop", "-q", "stash@{0}"])?;
                if *pop {
                    git(repo, &["stash", "store", "-q", "-m", subject, oid])?;
                }
            } else if *pop {
                ops::stash_pop(repo, "stash@{0}", Some(oid))?;
            } else {
                ops::stash_apply(repo, &format!("stash@{{{index}}}"), Some(oid))?;
            }
        }
        Inverse::StashDrop { oid, subject } => {
            if back {
                git(repo, &["stash", "store", "-q", "-m", subject, oid])?;
            } else {
                ops::stash_drop(repo, "stash@{0}", Some(oid))?;
            }
        }
        Inverse::Discard { undo_id, redo_id } => {
            let restored = |id: &str| -> Result<String> {
                discard::restore(repo, id, false)?
                    .map(|e| e.id)
                    .ok_or_else(|| Error::Rule("the restore changed no file".into()))
            };
            if back {
                *redo_id = Some(restored(undo_id)?);
            } else {
                let id = redo_id
                    .clone()
                    .ok_or_else(|| Error::Rule("no copy to discard again from".into()))?;
                *undo_id = restored(&id)?;
            }
            set_index(repo, &step.index, back)?;
        }
        Inverse::Ignore { edit } => {
            if back {
                ignore::revert(repo, edit)?;
            } else {
                ignore::apply(repo, edit)?;
            }
        }
    }
    Ok(())
}

// ── the chains ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chain {
    version: u32,
    repo: String,
    next_id: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    reason: Option<UndoReason>,
}

impl Chain {
    fn fresh(repo: &Path) -> Self {
        Chain {
            version: VERSION,
            repo: repo.display().to_string(),
            next_id: 1,
            ..Default::default()
        }
    }

    /// The digest the repository must have for the chain to go on.
    fn expected(&self) -> Option<&str> {
        match self.redo.last() {
            Some(s) => s.undone.as_deref(),
            None => self.undo.last().map(|s| s.after.digest.as_str()),
        }
    }

    fn cut(&mut self, why: UndoReason) {
        self.undo.clear();
        self.redo.clear();
        self.reason = Some(why);
    }

    fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

fn key_of(repo: &Path) -> PathBuf {
    std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf())
}

/// `<data>/undo/<fnv1a of the canonical root>.json`.
fn chain_file(data: &Path, key: &Path) -> PathBuf {
    data.join("undo")
        .join(format!("{}.json", fnv1a(key.to_string_lossy().as_bytes())))
}

fn load(data: Option<&Path>, key: &Path) -> Chain {
    let fresh = Chain::fresh(key);
    let Some(data) = data else {
        return fresh;
    };
    let Ok(bytes) = std::fs::read(chain_file(data, key)) else {
        return fresh;
    };
    match serde_json::from_slice::<Chain>(&bytes) {
        // Two roots with one hash, or an old format: nothing of it is ours.
        Ok(c) if c.version == VERSION && c.repo == fresh.repo => c,
        _ => fresh,
    }
}

fn save(data: Option<&Path>, key: &Path, chain: &Chain) {
    let Some(data) = data else {
        return;
    };
    if let Err(e) = write_chain(data, key, chain) {
        // The chain in memory is right; the file lags until the next write.
        eprintln!("graft: undo journal not saved: {e}");
    }
}

fn write_chain(data: &Path, key: &Path, chain: &Chain) -> Result<()> {
    let file = chain_file(data, key);
    let dir = file
        .parent()
        .ok_or_else(|| Error::Io("the undo journal has no folder".into()))?;
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(chain).map_err(|e| Error::Io(format!("undo journal: {e}")))?;
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.tmp.{}.{n}",
        file.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if let Err(e) = std::fs::rename(&tmp, &file) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[derive(Default)]
struct Inner {
    chains: HashMap<PathBuf, Chain>,
    /// Repositories an action (or an Undo / Redo) is running on.
    active: HashSet<PathBuf>,
    /// Repositories where a second action started while one was running.
    tainted: HashSet<PathBuf>,
    /// The subset of `active` whose action is the application's own background
    /// work ([`Undo::perform_background`]). A user action, a step or a state read
    /// arriving then **waits** for it on [`Undo::idle`] instead of tainting it or
    /// being refused: the person did nothing concurrent, and a fetch nobody asked
    /// for must not end their chain.
    background: HashSet<PathBuf>,
}

impl Inner {
    fn chain(&mut self, data: Option<&Path>, key: &Path) -> &mut Chain {
        self.chains
            .entry(key.to_path_buf())
            .or_insert_with(|| load(data, key))
    }
}

/// The undo journal of the process: every repository's chain.
#[derive(Default)]
pub struct Undo {
    inner: Mutex<Inner>,
    /// Signalled when a background action ends.
    idle: Condvar,
}

fn unavailable(why: UndoReason) -> UndoSide {
    UndoSide {
        reason: Some(why),
        ..Default::default()
    }
}

fn both(why: UndoReason) -> UndoState {
    UndoState {
        undo: unavailable(why.clone()),
        redo: unavailable(why),
    }
}

/// Commits reachable from `from` but not from `to` — what leaves the branch when a
/// hard reset moves it from `from` to `to`.
fn leaving(repo: &Path, from: Option<&str>, to: Option<&str>) -> u32 {
    let (Some(f), Some(t)) = (from, to) else {
        return 0;
    };
    exec::git(repo, &["rev-list", "--count", &format!("{t}..{f}"), "--"])
        .run()
        .ok()
        .filter(|o| o.success())
        .and_then(|o| o.stdout_text().trim().parse().ok())
        .unwrap_or(0)
}

fn side_of(repo: &Path, step: &Step, dir: UndoDirection) -> UndoSide {
    let destructive = step.inverse == Inverse::Hard;
    let lost_commits = if destructive {
        let (from, to) = match dir {
            UndoDirection::Undo => (&step.after.head, &step.before.head),
            UndoDirection::Redo => (&step.before.head, &step.after.head),
        };
        leaving(repo, from.as_deref(), to.as_deref())
    } else {
        0
    };
    UndoSide {
        id: Some(step.id),
        action: Some(step.action.clone()),
        detail: Some(step.detail.clone()).filter(|d| !d.is_empty()),
        destructive,
        lost_commits,
        reason: None,
    }
}

impl Undo {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The lock, once no background action runs on `key`.
    ///
    /// Waiting, not cancelling: the only way to stop a running git is a kill, and
    /// a git killed while it updates refs leaves `*.lock` files behind that fail
    /// the person's next pull ("cannot lock ref"). The wait is bounded by the
    /// network run's own stall limits (`exec::network_env`).
    fn lock_foreground(&self, key: &Path) -> MutexGuard<'_, Inner> {
        let mut g = self.lock();
        while g.background.contains(key) {
            g = self.idle.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g
    }

    /// Run the user action `action` (journaled under that name, as `exec::as_user`
    /// does) and record what it did to `repo`'s chain. The action's own result is
    /// returned untouched: recording never fails an action.
    pub fn perform<T>(
        &self,
        data: Option<&Path>,
        repo: &Path,
        action: &'static str,
        hint: Hint,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let key = key_of(repo);
        let solo = {
            let mut g = self.lock_foreground(&key);
            if g.active.contains(&key) {
                // Its effects would be read into the other action's "after": that one
                // ends the chain when it finishes.
                g.tainted.insert(key.clone());
                false
            } else {
                g.active.insert(key.clone());
                true
            }
        };
        if !solo {
            return exec::as_user(action, f);
        }
        self.record(data, repo, &key, action, hint, false, || {
            exec::as_user(action, f)
        })
    }

    /// Run the application's own background action `action` (`fetch_background`)
    /// and record it like any other — journaled with origin `background`, since no
    /// person asked for it (no `exec::as_user`, so not counted in
    /// `exec::OWN_ACTIONS` either: the git-dir watcher reports what it moved, and
    /// that is the one refresh the window does).
    ///
    /// `None`, without running `f`, when anything is already running on the
    /// repository: background work gives way, it never taints a person's action.
    /// Anyone arriving while it runs waits for it ([`Undo::lock_foreground`]).
    pub fn perform_background<T>(
        &self,
        data: Option<&Path>,
        repo: &Path,
        action: &'static str,
        f: impl FnOnce() -> Result<T>,
    ) -> Option<Result<T>> {
        let key = key_of(repo);
        {
            let mut g = self.lock();
            if g.active.contains(&key) {
                return None;
            }
            g.active.insert(key.clone());
            g.background.insert(key.clone());
        }
        // Released however `record` ends — a panic inside would otherwise leave
        // every later action of this repository waiting forever.
        struct Release<'a>(&'a Undo, PathBuf);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                let mut g = self.0.lock();
                g.active.remove(&self.1);
                g.background.remove(&self.1);
                drop(g);
                self.0.idle.notify_all();
            }
        }
        let _release = Release(self, key.clone());
        Some(self.record(data, repo, &key, action, Hint::none(), true, f))
    }

    /// Snapshot, run, snapshot, classify, and put the verdict on the chain. The
    /// caller has marked `key` active; this unmarks it — except for a background
    /// action, whose guard does that together with waking the waiters.
    #[allow(clippy::too_many_arguments)]
    fn record<T>(
        &self,
        data: Option<&Path>,
        repo: &Path,
        key: &Path,
        action: &'static str,
        hint: Hint,
        background: bool,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let key = key.to_path_buf();
        // Read before the action: deleting a branch removes its config section.
        let upstreams: Vec<Upstream> = match action {
            "branch_delete" if hint.arg(1) != Some("remote") => {
                hint.arg(0).and_then(|n| upstream_of(repo, n)).into_iter().collect()
            }
            "branch_delete_many" if hint.arg(0) != Some("remote") => hint
                .args
                .iter()
                .skip(1)
                .filter_map(|n| upstream_of(repo, n))
                .collect(),
            _ => Vec::new(),
        };
        let before = capture(repo, &[]);
        let out = f();
        let after = before.as_ref().ok().map(|b| capture(repo, &b.paths_vec()));
        let verdict = match (&before, &after) {
            (Ok(b), Some(Ok(a))) => classify(repo, action, &hint, out.is_ok(), b, a, upstreams),
            _ => Verdict::Break(reason(UndoReasonCode::Unverifiable, Some(action))),
        };

        let mut g = self.lock();
        if !background {
            g.active.remove(&key);
        }
        let tainted = g.tainted.remove(&key);
        let chain = g.chain(data, &key);
        if let (Some(e), Ok(b)) = (chain.expected(), &before) {
            if e != b.digest {
                chain.cut(reason(UndoReasonCode::External, None));
            }
        }
        let verdict = if tainted {
            Verdict::Break(reason(UndoReasonCode::Concurrent, Some(action)))
        } else {
            verdict
        };
        match verdict {
            Verdict::Nothing => {}
            Verdict::Break(why) => chain.cut(why),
            Verdict::Record(mut step) => {
                step.id = chain.next_id;
                chain.next_id += 1;
                chain.undo.push(step);
                if chain.undo.len() > DEPTH {
                    let extra = chain.undo.len() - DEPTH;
                    chain.undo.drain(..extra);
                }
                chain.redo.clear();
                chain.reason = None;
            }
        }
        let chain = chain.clone();
        drop(g);
        save(data, &key, &chain);
        out
    }

    /// What Undo and Redo would do now, or why they cannot. Reading it checks the
    /// repository against the chain and ends the chain on a mismatch — which is how
    /// a change made elsewhere shows up as a reason instead of a surprise.
    pub fn state(&self, data: Option<&Path>, repo: &Path) -> Result<UndoState> {
        let key = key_of(repo);
        let mut g = self.lock_foreground(&key);
        if g.active.contains(&key) {
            return Ok(both(reason(UndoReasonCode::Busy, None)));
        }
        let chain = g.chain(data, &key);
        if chain.is_empty() {
            let why = chain
                .reason
                .clone()
                .unwrap_or_else(|| reason(UndoReasonCode::Empty, None));
            return Ok(both(why));
        }
        let expected = chain.expected().map(str::to_string);
        let current = capture(repo, &[]).map(|s| s.digest);
        let broken = match &current {
            Ok(d) if Some(d) == expected.as_ref() => None,
            Ok(_) => Some(reason(UndoReasonCode::External, None)),
            Err(_) => Some(reason(UndoReasonCode::Unverifiable, None)),
        };
        if let Some(why) = broken {
            chain.cut(why.clone());
            let chain = chain.clone();
            drop(g);
            save(data, &key, &chain);
            return Ok(both(why));
        }
        let undo = match chain.undo.last() {
            Some(s) => side_of(repo, s, UndoDirection::Undo),
            None => unavailable(reason(UndoReasonCode::NoEarlier, None)),
        };
        let redo = match chain.redo.last() {
            Some(s) => side_of(repo, s, UndoDirection::Redo),
            None => unavailable(reason(UndoReasonCode::NoNext, None)),
        };
        Ok(UndoState { undo, redo })
    }

    /// Undo or redo the step `id` — refused unless it is the one on top and the
    /// repository is exactly as the chain left it. Returns, for an undone changelist
    /// commit, the (path, list) pairs to put the files back in.
    pub fn step(
        &self,
        data: Option<&Path>,
        repo: &Path,
        dir: UndoDirection,
        id: u64,
    ) -> Result<Vec<(String, String)>> {
        let key = key_of(repo);
        let (mut step, expected) = {
            let mut g = self.lock_foreground(&key);
            if g.active.contains(&key) {
                return Err(Error::Rule(
                    "another action is running on this repository".into(),
                ));
            }
            let chain = g.chain(data, &key);
            let top = match dir {
                UndoDirection::Undo => chain.undo.last(),
                UndoDirection::Redo => chain.redo.last(),
            };
            let Some(top) = top.filter(|s| s.id == id).cloned() else {
                return Err(Error::Rule(
                    "the undo history moved since it was shown; look again".into(),
                ));
            };
            let expected = chain.expected().map(str::to_string);
            g.active.insert(key.clone());
            (top, expected)
        };

        let mut external = false;
        let result = (|| -> Result<(Snapshot, Snapshot)> {
            let current = capture(repo, &[])?;
            if Some(&current.digest) != expected.as_ref() {
                external = true;
                return Err(Error::Rule(
                    "the repository changed outside the recorded action; nothing was reversed"
                        .into(),
                ));
            }
            exec::as_user(STEP_ACTION, || run_inverse(repo, &mut step, dir))?;
            let after = capture(repo, &[])?;
            Ok((current, after))
        })();

        let mut g = self.lock();
        g.active.remove(&key);
        let tainted = g.tainted.remove(&key);
        let chain = g.chain(data, &key);
        let out = match result {
            Ok((current, after)) => {
                match dir {
                    UndoDirection::Undo => {
                        chain.undo.pop();
                        step.undone = Some(after.digest);
                        chain.redo.push(step.clone());
                    }
                    UndoDirection::Redo => {
                        chain.redo.pop();
                        step.before = current.side();
                        step.after = after.side();
                        step.undone = None;
                        chain.undo.push(step.clone());
                    }
                }
                if tainted {
                    chain.cut(reason(UndoReasonCode::Concurrent, None));
                }
                Ok(match dir {
                    UndoDirection::Undo => step.lists,
                    UndoDirection::Redo => Vec::new(),
                })
            }
            Err(e) => {
                let code = if external {
                    UndoReasonCode::External
                } else {
                    UndoReasonCode::InverseFailed
                };
                chain.cut(reason(code, Some(&step.action)));
                Err(e)
            }
        };
        let chain = chain.clone();
        drop(g);
        save(data, &key, &chain);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::engine::{branches, rebase};
    use crate::model::DiscardKind;

    fn g(p: &Path, args: &[&str]) -> String {
        let out = exec::git(p, args).run().unwrap();
        assert!(
            out.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout_text()
    }

    fn fp(p: &Path) -> String {
        capture(p, &[]).unwrap().digest
    }

    struct Rig {
        repo: tempfile::TempDir,
        data: tempfile::TempDir,
        undo: Undo,
    }

    impl Rig {
        fn new() -> Self {
            Rig {
                repo: scratch_repo(),
                data: tempfile::tempdir().unwrap(),
                undo: Undo::default(),
            }
        }
        fn p(&self) -> &Path {
            self.repo.path()
        }
        fn d(&self) -> Option<&Path> {
            Some(self.data.path())
        }
        fn act<T>(&self, action: &'static str, hint: Hint, f: impl FnOnce() -> Result<T>) -> T {
            self.undo
                .perform(self.d(), self.p(), action, hint, f)
                .unwrap()
        }
        fn state(&self) -> UndoState {
            self.undo.state(self.d(), self.p()).unwrap()
        }
        fn go(&self, dir: UndoDirection) -> Result<Vec<(String, String)>> {
            let s = self.state();
            let side = match dir {
                UndoDirection::Undo => s.undo,
                UndoDirection::Redo => s.redo,
            };
            let id = side
                .id
                .unwrap_or_else(|| panic!("{dir:?} unavailable: {:?}", side.reason));
            self.undo.step(self.d(), self.p(), dir, id)
        }
        fn undo(&self) {
            self.go(UndoDirection::Undo).unwrap();
        }
        fn redo(&self) {
            self.go(UndoDirection::Redo).unwrap();
        }
        fn reason(&self) -> UndoReasonCode {
            self.state().undo.reason.expect("undo is available").code
        }
        /// action → undo → as before → redo → as after.
        fn round_trip<T>(&self, action: &'static str, hint: Hint, f: impl FnOnce() -> Result<T>) {
            let before = fp(self.p());
            self.act(action, hint, f);
            let after = fp(self.p());
            assert_ne!(before, after, "{action} changed nothing");
            let s = self.state();
            assert_eq!(
                s.undo.action.as_deref(),
                Some(action),
                "{:?}",
                s.undo.reason
            );
            self.undo();
            assert_eq!(fp(self.p()), before, "undo of {action} restores the state");
            self.redo();
            assert_eq!(fp(self.p()), after, "redo of {action} restores the result");
        }
    }

    #[test]
    fn a_changelist_commit_comes_back_with_its_index() {
        let r = Rig::new();
        let p = r.p();
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        g(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap(); // staged + unstaged
        std::fs::write(p.join("b.txt"), "new\n").unwrap();
        g(p, &["add", "b.txt"]);
        std::fs::write(p.join("c.txt"), "other list\n").unwrap();
        g(p, &["add", "c.txt"]); // staged for another list, not committed
        let index_before = g(p, &["ls-files", "--stage"]);
        let head_before = g(p, &["rev-parse", "HEAD"]);

        let hint = Hint {
            lists: vec![("a.txt".into(), "feature".into())],
            ..Hint::default()
        };
        let paths = vec!["a.txt".to_string(), "b.txt".to_string()];
        r.round_trip("commit_list", hint.clone(), || {
            CliEngine::new(p).commit_paths(&paths, "partial", false)
        });
        assert_eq!(r.state().undo.detail.as_deref(), Some("partial"));

        let lists = r.go(UndoDirection::Undo).unwrap();
        assert_eq!(lists, hint.lists, "the lists come back with an undo");
        assert_eq!(g(p, &["rev-parse", "HEAD"]), head_before);
        assert_eq!(
            g(p, &["ls-files", "--stage"]),
            index_before,
            "index wins, as before"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).unwrap(),
            "one\ntwo\nthree\n"
        );
    }

    #[test]
    fn amend_and_first_commit() {
        let r = Rig::new();
        let p = r.p();
        std::fs::write(p.join("a.txt"), "amended\n").unwrap();
        let paths = vec!["a.txt".to_string()];
        r.round_trip("commit_list", Hint::none(), || {
            CliEngine::new(p).commit_paths(&paths, "init again", true)
        });

        // An unborn branch: Undo makes it unborn again.
        let r = Rig::new();
        let p = r.p();
        g(p, &["checkout", "-q", "--orphan", "fresh"]);
        g(p, &["rm", "-q", "--cached", "a.txt"]);
        let before = fp(p);
        let paths = vec!["a.txt".to_string()];
        r.act("commit_list", Hint::none(), || {
            CliEngine::new(p).commit_paths(&paths, "first", false)
        });
        r.undo();
        assert_eq!(fp(p), before);
        assert!(
            exec::git(p, &["rev-parse", "--verify", "-q", "fresh"])
                .run()
                .unwrap()
                .code
                == Some(1)
        );
        r.redo();
        assert_eq!(g(p, &["log", "-1", "--format=%s"]).trim(), "first");
    }

    #[test]
    fn a_commit_on_a_detached_head() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["checkout", "-q", "--detach"]);
        std::fs::write(p.join("a.txt"), "detached\n").unwrap();
        let paths = vec!["a.txt".to_string()];
        r.round_trip("commit_list", Hint::none(), || {
            CliEngine::new(p).commit_paths(&paths, "on a detached head", false)
        });
        assert_eq!(
            g(p, &["log", "-1", "--format=%s"]).trim(),
            "on a detached head"
        );
        assert!(
            exec::git(p, &["symbolic-ref", "-q", "HEAD"])
                .run()
                .unwrap()
                .code
                == Some(1)
        );
    }

    #[test]
    fn prefetch_refs_do_not_end_the_chain() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        g(
            p,
            &["update-ref", "refs/prefetch/remotes/origin/main", "HEAD"],
        );
        assert!(r.state().undo.id.is_some());
    }

    #[test]
    fn reword_of_head_and_reset_soft() {
        let r = Rig::new();
        let p = r.p();
        let head = g(p, &["rev-parse", "HEAD"]).trim().to_string();
        let data = r.data.path().to_path_buf();
        r.round_trip("commit_reword", Hint::none(), || {
            rebase::reword(p, &data, &head, "renamed")
        });

        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        g(p, &["commit", "-qam", "second"]);
        r.round_trip("commit_reset", Hint::args([head.as_str(), "mixed"]), || {
            ops::reset(p, &head, "mixed")
        });
        g(p, &["commit", "-qam", "second again"]);
        r.round_trip("commit_reset", Hint::args([head.as_str(), "hard"]), || {
            ops::reset(p, &head, "hard")
        });
        assert!(r.state().undo.destructive);
    }

    #[test]
    fn a_hard_reset_over_local_changes_ends_the_chain() {
        let r = Rig::new();
        let p = r.p();
        let head = g(p, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        g(p, &["commit", "-qam", "second"]);
        std::fs::write(p.join("a.txt"), "dirty\n").unwrap();
        r.act("commit_reset", Hint::none(), || {
            ops::reset(p, &head, "hard")
        });
        assert_eq!(r.reason(), UndoReasonCode::Dirty);
    }

    #[test]
    fn checkout_create_delete_rename_and_tag() {
        let r = Rig::new();
        let p = r.p();
        let eng = CliEngine::new(p);
        g(p, &["branch", "side"]);
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        g(p, &["commit", "-qam", "second"]);
        let first = g(p, &["rev-parse", "HEAD~1"]).trim().to_string();

        r.round_trip("branch_checkout", Hint::args(["side"]), || {
            eng.checkout("side", false)
        });
        g(p, &["checkout", "-q", "main"]);
        r.round_trip("commit_checkout", Hint::args([&first]), || {
            ops::checkout_rev(p, &first)
        });
        g(p, &["checkout", "-q", "main"]);
        r.round_trip("branch_create", Hint::args(["topic"]), || {
            eng.create_branch("topic", Some(&first))
        });
        assert_eq!(r.state().undo.detail.as_deref(), Some("topic"));
        g(p, &["checkout", "-q", "main"]);

        g(p, &["config", "branch.side.remote", "origin"]);
        g(p, &["config", "branch.side.merge", "refs/heads/side"]);
        r.act("branch_delete", Hint::args(["side", "local"]), || {
            branches::delete(p, "side", false, true)
        });
        r.undo();
        assert_eq!(g(p, &["rev-parse", "side"]).trim(), first);
        assert_eq!(
            g(p, &["config", "branch.side.merge"]).trim(),
            "refs/heads/side"
        );
        r.redo();
        assert!(
            exec::git(p, &["rev-parse", "--verify", "-q", "side"])
                .run()
                .unwrap()
                .code
                == Some(1)
        );
        r.undo();

        r.round_trip("branch_rename", Hint::args(["main", "trunk"]), || {
            branches::rename(p, "main", "trunk")
        });
        assert_eq!(g(p, &["symbolic-ref", "--short", "HEAD"]).trim(), "trunk");
        r.round_trip("tag_create", Hint::args(["v1"]), || {
            ops::tag_create(p, &first, "v1", Some("annotated"))
        });
    }

    #[test]
    fn a_group_delete_is_one_step_that_brings_every_branch_back() {
        let r = Rig::new();
        let p = r.p();
        let head = g(p, &["rev-parse", "HEAD"]).trim().to_string();
        for b in ["x", "y", "z"] {
            g(p, &["branch", b]);
        }
        g(p, &["config", "branch.x.remote", "origin"]);
        g(p, &["config", "branch.x.merge", "refs/heads/x"]);
        g(p, &["config", "branch.z.remote", "origin"]);
        g(p, &["config", "branch.z.merge", "refs/heads/zz"]);
        let exists = |b: &str| {
            exec::git(p, &["rev-parse", "--verify", "-q", b])
                .run()
                .unwrap()
                .code
                == Some(0)
        };

        let names: Vec<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        r.act(
            "branch_delete_many",
            Hint::args(["local", "x", "y", "z"]),
            || branches::delete_many(p, &names, false, false),
        );
        assert!(!exists("x") && !exists("y") && !exists("z"));
        assert_eq!(r.state().undo.detail.as_deref(), Some("x, y, z"));

        r.undo();
        for b in ["x", "y", "z"] {
            assert_eq!(g(p, &["rev-parse", b]).trim(), head, "{b} is back, one Undo");
        }
        assert_eq!(g(p, &["config", "branch.x.merge"]).trim(), "refs/heads/x");
        assert_eq!(g(p, &["config", "branch.z.merge"]).trim(), "refs/heads/zz");

        r.redo();
        assert!(!exists("x") && !exists("y") && !exists("z"));
        assert!(
            exec::git(p, &["config", "--get", "branch.z.merge"])
                .run()
                .unwrap()
                .code
                == Some(1),
            "redo drops the upstreams again"
        );
    }

    #[test]
    fn a_remote_group_delete_ends_the_chain_as_published() {
        let r = Rig::new();
        let p = r.p();
        r.act("branch_create", Hint::args(["other"]), || {
            CliEngine::new(p).create_branch("other", None)
        });
        // No server here: the push itself is not the point, its verdict is. A
        // remote deletion moves nothing the digest reads, and still has to end the
        // chain — the branch is gone for everyone who fetches.
        r.act("branch_delete_many", Hint::args(["remote", "origin/x"]), || {
            Ok(())
        });
        assert_eq!(r.reason(), UndoReasonCode::Published);
    }

    #[test]
    fn a_chain_saved_with_a_single_upstream_still_reads() {
        let old = r#"{"type":"refs","stash":false,"target":null,
            "upstream":{"branch":"side","remote":"origin","merge":"refs/heads/side"}}"#;
        match serde_json::from_str::<Inverse>(old).unwrap() {
            Inverse::Refs {
                upstream,
                upstreams,
                ..
            } => {
                assert_eq!(upstream.map(|u| u.branch).as_deref(), Some("side"));
                assert!(upstreams.is_empty());
            }
            other => panic!("{other:?}"),
        }
        let none = r#"{"type":"refs","stash":false,"target":null,"upstream":null}"#;
        assert!(matches!(
            serde_json::from_str::<Inverse>(none).unwrap(),
            Inverse::Refs { upstream: None, .. }
        ));
    }

    #[test]
    fn a_switch_that_stashed_pops_the_stash_back() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["branch", "side"]);
        std::fs::write(p.join("a.txt"), "local\n").unwrap();
        std::fs::write(p.join("u.txt"), "untracked\n").unwrap();
        let before = fp(p);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", true)
        });
        assert_eq!(g(p, &["stash", "list"]).lines().count(), 1);
        r.undo();
        assert_eq!(fp(p), before);
        assert_eq!(g(p, &["stash", "list"]), "");
        r.redo();
        assert_eq!(g(p, &["symbolic-ref", "--short", "HEAD"]).trim(), "side");
        assert_eq!(g(p, &["stash", "list"]).lines().count(), 1);
    }

    #[test]
    fn merge_cherry_pick_and_revert() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["checkout", "-q", "-b", "side"]);
        std::fs::write(p.join("s.txt"), "side\n").unwrap();
        g(p, &["add", "s.txt"]);
        g(p, &["commit", "-qm", "side work"]);
        let pick = g(p, &["rev-parse", "HEAD"]).trim().to_string();
        g(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("m.txt"), "main\n").unwrap();
        g(p, &["add", "m.txt"]);
        g(p, &["commit", "-qm", "main work"]);
        std::fs::write(p.join("junk.txt"), "untracked stays\n").unwrap();

        r.round_trip("branch_merge", Hint::args(["side"]), || {
            branches::merge(p, "side")
        });
        let s = r.state();
        assert!(s.undo.destructive);
        assert_eq!(
            s.undo.lost_commits, 2,
            "the merge commit and the side commit"
        );
        r.undo();
        r.round_trip("commit_cherry_pick", Hint::args([&pick]), || {
            ops::cherry_pick(p, &pick)
        });
        let head = g(p, &["rev-parse", "HEAD"]).trim().to_string();
        r.round_trip("commit_revert", Hint::args([&head]), || {
            ops::revert(p, &head)
        });
        assert!(p.join("junk.txt").exists());
    }

    #[test]
    fn stash_push_pop_apply_drop() {
        let r = Rig::new();
        let p = r.p();
        std::fs::write(p.join("a.txt"), "local\n").unwrap();
        g(p, &["add", "a.txt"]);
        std::fs::write(p.join("n.txt"), "new\n").unwrap();
        let dirty = fp(p);
        r.act("stash_push", Hint::args(["wip"]), || {
            ops::stash_push(p, Some("wip"))
        });
        assert_eq!(r.state().undo.detail.as_deref(), Some("wip"));
        r.undo();
        assert_eq!(fp(p), dirty, "the index comes back too");
        r.redo();
        assert_eq!(g(p, &["stash", "list"]).lines().count(), 1);

        r.round_trip("stash_pop", Hint::args(["stash@{0}"]), || {
            ops::stash_pop(p, "stash@{0}", None)
        });
        g(p, &["stash", "push", "-q", "-u", "-m", "again"]);
        r.round_trip("stash_apply", Hint::args(["stash@{0}"]), || {
            ops::stash_apply(p, "stash@{0}", None)
        });
        g(p, &["reset", "-q", "--hard"]);
        g(p, &["clean", "-qfd"]);
        r.round_trip("stash_drop", Hint::args(["stash@{0}"]), || {
            ops::stash_drop(p, "stash@{0}", None)
        });
    }

    #[test]
    fn a_popped_stash_below_the_top_ends_the_chain() {
        let r = Rig::new();
        let p = r.p();
        for n in ["x1", "x2"] {
            std::fs::write(p.join("a.txt"), format!("{n}\n")).unwrap();
            g(p, &["stash", "push", "-q", "-m", n]);
        }
        r.act("stash_pop", Hint::args(["stash@{1}"]), || {
            ops::stash_pop(p, "stash@{1}", None)
        });
        assert_eq!(r.reason(), UndoReasonCode::StashPosition);
    }

    #[test]
    fn a_rollback_comes_back_from_its_copy_with_the_index() {
        let r = Rig::new();
        let p = r.p();
        std::fs::write(p.join("a.txt"), "staged\n").unwrap();
        g(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "staged\nand more\n").unwrap();
        std::fs::write(p.join("n.txt"), "added\n").unwrap();
        g(p, &["add", "n.txt"]);
        let paths = vec!["a.txt".to_string(), "n.txt".to_string()];
        r.round_trip("file_rollback", Hint::none(), || {
            discard::with_backup(p, DiscardKind::Files, &paths, || {
                CliEngine::new(p).rollback(&paths)
            })
        });
        assert_eq!(r.state().undo.detail.as_deref(), Some("a.txt +1"));
        r.undo();
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).unwrap(),
            "staged\nand more\n"
        );
        assert!(g(p, &["ls-files", "--stage", "n.txt"]).contains("n.txt"));
    }

    #[test]
    fn staging_lines_is_undone_on_the_index() {
        let r = Rig::new();
        let p = r.p();
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        let paths = vec!["a.txt".to_string()];
        r.round_trip("lines_stage", Hint::args(["a.txt"]), || {
            CliEngine::new(p).stage_paths(&paths)
        });
        r.round_trip("lines_unstage", Hint::args(["a.txt"]), || {
            g(p, &["reset", "-q", "--", "a.txt"]);
            Ok(())
        });
    }

    #[test]
    fn an_external_change_refuses_undo() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        let id = r.state().undo.id.unwrap();
        std::fs::write(p.join("a.txt"), "edited in a terminal\n").unwrap();
        let err = r.undo.step(r.d(), p, UndoDirection::Undo, id).unwrap_err();
        assert!(matches!(err, Error::Rule(_)), "{err:?}");
        assert_eq!(
            g(p, &["symbolic-ref", "--short", "HEAD"]).trim(),
            "side",
            "nothing reversed"
        );
        assert_eq!(r.reason(), UndoReasonCode::External);

        // Seen by `state` alone, too.
        g(p, &["checkout", "-q", "--", "a.txt"]);
        r.act("branch_checkout", Hint::args(["main"]), || {
            CliEngine::new(p).checkout("main", false)
        });
        assert!(r.state().undo.id.is_some());
        g(p, &["tag", "outside"]);
        assert_eq!(r.reason(), UndoReasonCode::External);
    }

    #[test]
    fn push_and_rebase_end_the_chain() {
        let r = Rig::new();
        let p = r.p();
        let remote = tempfile::tempdir().unwrap();
        g(remote.path(), &["init", "-q", "--bare"]);
        g(
            p,
            &[
                "remote",
                "add",
                "origin",
                &remote.path().display().to_string(),
            ],
        );
        g(p, &["push", "-q", "-u", "origin", "main"]);
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        g(p, &["checkout", "-q", "main"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        assert!(r.state().undo.id.is_some());
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        g(p, &["commit", "-qam", "on side"]);
        r.act("push", Hint::args(["upstream"]), || {
            CliEngine::new(p).push("upstream")
        });
        let s = r.state();
        assert_eq!(
            s.undo.reason.as_ref().unwrap().code,
            UndoReasonCode::Published
        );
        assert_eq!(s.undo.reason.unwrap().action.as_deref(), Some("push"));

        // A rebase that rewrote the branch.
        g(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("m.txt"), "m\n").unwrap();
        g(p, &["add", "m.txt"]);
        g(p, &["commit", "-qm", "main moves"]);
        g(p, &["checkout", "-q", "side"]);
        r.act("branch_checkout", Hint::args(["main"]), || {
            CliEngine::new(p).checkout("main", false)
        });
        r.undo();
        r.act("branch_rebase_onto", Hint::args(["main"]), || {
            branches::rebase_onto(p, "main")
        });
        assert_eq!(r.reason(), UndoReasonCode::History);
    }

    /// Starting, answering and ending a bisect each end the chain with the bisect
    /// named as the reason — not "an unfinished merge, rebase…" — and an action
    /// taken during a bisect cannot start a new chain either.
    #[test]
    fn a_bisect_ends_the_chain() {
        use crate::engine::bisect;
        let r = Rig::new();
        let p = r.p();
        for i in 2..=5 {
            std::fs::write(p.join("a.txt"), format!("{i}\n")).unwrap();
            g(p, &["commit", "-qam", &format!("c{i}")]);
        }
        let first = g(p, &["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        assert!(r.state().undo.id.is_some(), "a chain to end");

        r.act("op_bisect_start", Hint::none(), || {
            bisect::start(p, None, &[first.clone()])
        });
        let s = r.state();
        assert!(s.undo.id.is_none());
        assert_eq!(s.undo.reason.as_ref().unwrap().code, UndoReasonCode::Bisect);
        assert_eq!(
            s.undo.reason.unwrap().action.as_deref(),
            Some("op_bisect_start")
        );

        r.act("op_bisect_mark", Hint::args(["good"]), || {
            bisect::mark(p, "good", None)
        });
        assert_eq!(r.reason(), UndoReasonCode::Bisect);

        g(p, &["tag", "during"]);
        r.act("tag_create", Hint::args(["during2"]), || {
            ops::tag_create(p, "HEAD", "during2", None)
        });
        assert_eq!(r.reason(), UndoReasonCode::Bisect, "no step is recorded mid-bisect");

        r.act("op_bisect_reset", Hint::none(), || bisect::reset(p));
        assert_eq!(r.reason(), UndoReasonCode::Bisect);
        assert_eq!(g(p, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "side");
    }

    #[test]
    fn the_chain_survives_a_restart() {
        let r = Rig::new();
        let root = r.p().to_path_buf();
        let p = root.as_path();
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        let r = Rig {
            undo: Undo::default(),
            ..r
        }; // a new process, same data dir
        let s = r.state();
        assert_eq!(s.undo.action.as_deref(), Some("branch_checkout"));
        assert_eq!(s.undo.detail.as_deref(), Some("side"));
        r.undo();
        assert_eq!(g(p, &["symbolic-ref", "--short", "HEAD"]).trim(), "main");
        let r = Rig {
            undo: Undo::default(),
            ..r
        };
        assert!(r.state().redo.id.is_some(), "the redo side is on disk too");

        // Without a data directory nothing is kept.
        let u = Undo::default();
        assert_eq!(
            u.state(None, p).unwrap().undo.reason.unwrap().code,
            UndoReasonCode::Empty
        );
    }

    #[test]
    fn an_action_that_changed_nothing_keeps_the_chain() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        let refused = r.undo.perform(r.d(), p, "commit_list", Hint::none(), || {
            CliEngine::new(p).commit_paths(&["a.txt".to_string()], "nothing", false)
        });
        assert!(refused.is_err());
        assert_eq!(r.state().undo.action.as_deref(), Some("branch_checkout"));
        r.act("fetch", Hint::none(), || Ok(()));
        assert_eq!(r.state().undo.action.as_deref(), Some("branch_checkout"));
    }

    #[test]
    fn a_stale_id_is_refused() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["branch", "side"]);
        r.act("branch_checkout", Hint::args(["side"]), || {
            CliEngine::new(p).checkout("side", false)
        });
        let id = r.state().undo.id.unwrap();
        assert!(r.undo.step(r.d(), p, UndoDirection::Undo, id + 1).is_err());
        assert!(r.undo.step(r.d(), p, UndoDirection::Redo, id).is_err());
        r.undo.step(r.d(), p, UndoDirection::Undo, id).unwrap();
    }

    #[test]
    fn depth_is_capped() {
        let r = Rig::new();
        let p = r.p();
        g(p, &["branch", "side"]);
        for i in 0..(DEPTH + 3) {
            let to = if i % 2 == 0 { "side" } else { "main" };
            r.act("branch_checkout", Hint::args([to]), || {
                CliEngine::new(p).checkout(to, false)
            });
        }
        let g = r.undo.lock();
        assert_eq!(g.chains.values().next().unwrap().undo.len(), DEPTH);
    }

    /// Adding a remote or changing its address leaves the chain alone; renaming or
    /// removing one ends it with `Remotes` — whether or not HEAD's branch tracked it
    /// (with it, the `# branch.upstream` header changes the digest; without, nothing
    /// the digest reads does, and the verdict must not differ).
    #[test]
    fn remotes_keep_or_end_the_chain_the_same_way_on_any_branch() {
        use crate::engine::remotes;
        for tracked in [false, true] {
            let r = Rig::new();
            let p = r.p();
            g(p, &["branch", "side"]);
            r.act("branch_checkout", Hint::args(["side"]), || {
                CliEngine::new(p).checkout("side", false)
            });
            r.act("remote_add", Hint::args(["origin"]), || {
                remotes::add(p, "origin", "https://h/a.git")
            });
            r.act("remote_set_url", Hint::args(["origin"]), || {
                remotes::set_url(p, "origin", "https://h/b.git", true)
            });
            assert_eq!(
                r.state().undo.action.as_deref(),
                Some("branch_checkout"),
                "add and set-url leave the chain alone"
            );
            if tracked {
                g(p, &["update-ref", "refs/remotes/origin/side", "HEAD"]);
                g(p, &["branch", "--set-upstream-to=origin/side"]);
                // The upstream set from a terminal: re-read, so the chain ends here
                // as External and starts over with the next recorded step.
                r.state();
                r.act("branch_create", Hint::args(["x"]), || {
                    CliEngine::new(p).create_branch("x", None)
                });
                r.act("branch_checkout", Hint::args(["side"]), || {
                    CliEngine::new(p).checkout("side", false)
                });
            }
            let before = fp(p);
            r.act("remote_rename", Hint::args(["origin", "up"]), || {
                remotes::rename(p, "origin", "up")
            });
            assert_eq!(
                fp(p) != before,
                tracked,
                "the digest moved only when tracked"
            );
            assert_eq!(r.reason(), UndoReasonCode::Remotes, "tracked: {tracked}");

            r.act("branch_create", Hint::args(["y"]), || {
                CliEngine::new(p).create_branch("y", None)
            });
            r.act("remote_remove", Hint::args(["up"]), || {
                remotes::remove(p, "up")
            });
            assert_eq!(r.reason(), UndoReasonCode::Remotes, "tracked: {tracked}");
        }
    }

    /// Removing, locking, unlocking and pruning another worktree leave the chain
    /// alone; adding one ends it with `Worktrees` — also for an existing branch,
    /// where the digest does not move: the step before created that branch, and its
    /// Undo would delete it from under the new folder.
    #[test]
    fn an_added_worktree_ends_the_chain_the_rest_leave_it() {
        use crate::engine::worktrees;
        let r = Rig::new();
        let p = r.p();
        let outer = tempfile::tempdir().unwrap();
        let at = |n: &str| outer.path().join(n).display().to_string();

        r.act("branch_create", Hint::args(["x"]), || {
            CliEngine::new(p).create_branch("x", None)
        });
        let (a, b) = (at("a"), at("b"));
        r.act("worktree_add", Hint::args(["y", b.as_str()]), || {
            worktrees::add(p, &b, "y", true, None)
        });
        assert_eq!(r.reason(), UndoReasonCode::Worktrees);

        r.act("branch_create", Hint::args(["z"]), || {
            CliEngine::new(p).create_branch("z", None)
        });
        r.act("worktree_lock", Hint::args([b.as_str()]), || {
            worktrees::lock(p, &b, None)
        });
        r.act("worktree_unlock", Hint::args([b.as_str()]), || {
            worktrees::unlock(p, &b)
        });
        r.act("worktree_remove", Hint::args([b.as_str()]), || {
            worktrees::remove(p, &b, false)
        });
        r.act("worktree_prune", Hint::none(), || worktrees::prune(p));
        assert_eq!(
            r.state().undo.action.as_deref(),
            Some("branch_create"),
            "the chain goes on"
        );

        let before = fp(p);
        // `create_branch` switched to each new branch: `x` is free again.
        r.act("worktree_add", Hint::args(["x", a.as_str()]), || {
            worktrees::add(p, &a, "x", false, None)
        });
        assert_eq!(fp(p), before, "an existing branch moves nothing here");
        assert_eq!(r.reason(), UndoReasonCode::Worktrees);
    }

    /// "Ignore" writes one line into `.gitignore`: Undo takes it out (or deletes the
    /// file it created), Redo puts it back — the hidden file reappears and vanishes
    /// with it. An edit of `.gitignore` afterwards ends the chain instead.
    #[test]
    fn an_ignore_rule_comes_and_goes_with_undo() {
        use crate::model::IgnoreKind;
        for existing in [false, true] {
            let r = Rig::new();
            let p = r.p();
            if existing {
                std::fs::write(p.join(".gitignore"), "*.tmp\r\n").unwrap();
                g(p, &["add", ".gitignore"]);
                g(p, &["commit", "-q", "-m", "rules"]);
            }
            std::fs::write(p.join("x.log"), "noise\n").unwrap();
            let edit = ignore::plan(p, "x.log", IgnoreKind::File).unwrap();
            let hint = Hint {
                ignore: Some(edit.clone()),
                ..Hint::default()
            };
            r.round_trip("file_ignore", hint, || ignore::apply(p, &edit));
            assert_eq!(r.state().undo.detail.as_deref(), Some("/x.log"));

            r.undo();
            let listed = g(p, &["status", "--porcelain", "--untracked-files=all"]);
            assert!(listed.contains("x.log"), "visible again: {listed}");
            assert_eq!(p.join(".gitignore").exists(), existing, "a created file goes, an existing one stays");
            if existing {
                assert_eq!(std::fs::read(p.join(".gitignore")).unwrap(), b"*.tmp\r\n");
            }
            r.redo();
            std::fs::write(p.join(".gitignore"), "edited by hand\n").unwrap();
            assert_eq!(r.reason(), UndoReasonCode::External, "existing: {existing}");
        }
    }

    /// Something else changing the working tree in the same action is not an ignore
    /// the chain can take back.
    #[test]
    fn an_ignore_with_other_changes_ends_the_chain() {
        use crate::model::IgnoreKind;
        let r = Rig::new();
        let p = r.p();
        std::fs::write(p.join("x.log"), "noise\n").unwrap();
        let edit = ignore::plan(p, "x.log", IgnoreKind::File).unwrap();
        let hint = Hint {
            ignore: Some(edit.clone()),
            ..Hint::default()
        };
        r.act("file_ignore", hint, || {
            ignore::apply(p, &edit)?;
            std::fs::write(p.join("a.txt"), "changed alongside\n").map_err(|e| Error::Io(e.to_string()))
        });
        assert_eq!(r.reason(), UndoReasonCode::Worktree);
    }

    // ---- background actions ----

    #[test]
    fn a_background_action_gives_way_to_a_running_one() {
        let rig = Rig::new();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let r = &rig;
        std::thread::scope(|s| {
            s.spawn(move || {
                r.act("branch_create", Hint::args(["x"]), || {
                    started_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                    CliEngine::new(r.p()).create_branch("x", None)
                })
            });
            started_rx.recv().unwrap();
            let bg = rig
                .undo
                .perform_background(rig.d(), rig.p(), "fetch_background", || Ok(()));
            assert!(
                bg.is_none(),
                "background work never taints a person's action"
            );
            go_tx.send(()).unwrap();
        });
        assert!(rig.state().undo.id.is_some());
    }

    #[test]
    fn a_person_arriving_during_a_background_action_waits_instead_of_ending_the_chain() {
        let rig = Rig::new();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let r = &rig;
        std::thread::scope(|s| {
            let bg = s.spawn(move || {
                r.undo
                    .perform_background(r.d(), r.p(), "fetch_background", || {
                        started_tx.send(()).unwrap();
                        go_rx.recv().unwrap();
                        Ok(())
                    })
            });
            started_rx.recv().unwrap();
            let person = s.spawn(|| {
                rig.act("branch_create", Hint::args(["y"]), || {
                    CliEngine::new(rig.p()).create_branch("y", None)
                })
            });
            // Give the person's action time to reach the wait, then let the
            // background one finish.
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(!person.is_finished(), "it waits for the background action");
            go_tx.send(()).unwrap();
            assert!(matches!(bg.join().unwrap(), Some(Ok(()))));
            person.join().unwrap();
        });
        let st = rig.state();
        assert!(
            st.undo.id.is_some(),
            "recorded, not Concurrent: {:?}",
            st.undo.reason
        );
        assert_eq!(st.undo.action.as_deref(), Some("branch_create"));
    }
}
