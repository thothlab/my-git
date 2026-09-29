//! Watching the git directory, so that a commit, checkout, fetch or stash made in a
//! terminal shows up without the user coming back to the window.
//!
//! One recursive watch (two for a worktree whose git directory lies outside the
//! common one) on the open repository, debounced here, not by a debouncer crate: every
//! raw event is tagged "Graft's own action or not" **the moment it arrives**, and a
//! debounced batch no longer knows when each of its members happened. The watcher never
//! starts git: its one `rev-parse` runs when it is created, and what it reports is only
//! "something moved" — the window decides when to re-read (`gui/src/repoWatch.ts`).
//!
//! ## What counts
//!
//! The on-disk layout of a git directory is git's documented contract: ref tips
//! (`refs/`, `packed-refs`), `HEAD` and the reflogs, and the markers of an unfinished
//! operation. Deliberately **not**:
//!
//! - `index` — `git status`, which every re-read runs, rewrites it opportunistically:
//!   watching it turns each refresh into the cause of the next one. An external
//!   `git add` is picked up on focus instead.
//! - `FETCH_HEAD` — rewritten by every fetch, including one that brought nothing; a
//!   fetch that brought something moves `refs/remotes/`.
//! - `*.lock` — the half of an update that is not the update.
//! - `refs/graft/` and `logs/refs/graft/` — Graft's own rollback copies
//!   (`engine::discard`), which the log hides anyway.
//! - `changelists.json`, `graft-ui.json` — Graft's own files, written on every state
//!   build and every panel setting.
//!
//! Paths are matched by component from the **root of the git directory**, never as a
//! suffix: `worktrees/<other>/HEAD` and `modules/<sub>/HEAD` end in `HEAD` too, and
//! belong to another worktree and a submodule.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::engine::cli::CliEngine;
use crate::error::{Error, Result};

/// Quiet time that closes a burst: one `git commit` writes the object, the ref, the
/// reflog and `HEAD` within a few milliseconds.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// A burst that never goes quiet (a long rebase running hooks between its commits) is
/// still reported this often, so the window is not frozen on the state before it.
pub const MAX_BATCH: Duration = Duration::from_secs(2);

/// How long after Graft's own action its events are still taken for its own. The
/// macOS stream is created with zero latency, so events trail the command by
/// milliseconds; the margin is for the post-action state build. Kept short on purpose:
/// an external change inside this window is only seen on the next event or on focus.
pub const OWN_GRACE: Duration = Duration::from_millis(1000);

/// Names at the root of a git directory that are never an external change, whatever
/// the allowlist says. Prefix match: the atomic writes of the two JSON files go
/// through `<name>.tmp.<pid>.<n>` next to them.
const DENY_ROOT_PREFIX: &[&str] = &["changelists.json", "graft-ui.json"];
const DENY_ROOT: &[&str] = &["index", "FETCH_HEAD"];

/// Single files at the root of a git directory whose change the window shows.
const WATCH_ROOT_FILES: &[&str] = &[
    "HEAD",
    "ORIG_HEAD",
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "packed-refs",
];

/// Directories of an unfinished operation: everything inside counts.
const WATCH_OP_DIRS: &[&str] = &["rebase-merge", "rebase-apply", "sequencer"];

/// Components of a relative path as strings; `None` when one is not plain UTF-8 or
/// not a normal name (`..`, a root) — nothing git writes looks like that.
fn parts(rel: &Path) -> Option<Vec<&str>> {
    rel.components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect()
}

/// Is a change at `rel`, relative to the **repository's own git directory**, one the
/// window should show? In an ordinary repository this directory is also the common
/// one, so refs are judged here too.
pub fn relevant(rel: &Path) -> bool {
    let Some(p) = parts(rel) else { return false };
    let Some(first) = p.first().copied() else {
        return false;
    };
    let last = p[p.len() - 1];
    if last.ends_with(".lock") {
        return false;
    }
    if p.len() == 1 && DENY_ROOT.contains(&first) {
        return false;
    }
    if DENY_ROOT_PREFIX.iter().any(|d| first.starts_with(d)) {
        return false;
    }
    match first {
        "refs" => p.get(1) != Some(&"graft"),
        "logs" => match p.get(1).copied() {
            Some("HEAD") => p.len() == 2,
            Some("refs") => p.get(2) != Some(&"graft"),
            _ => false,
        },
        f if WATCH_OP_DIRS.contains(&f) => true,
        f => p.len() == 1 && WATCH_ROOT_FILES.contains(&f),
    }
}

/// Is a change at `rel`, relative to the **common** directory of a linked worktree,
/// one this worktree should show? Only what worktrees share: refs, `packed-refs` and
/// the reflogs of refs. `HEAD`, `logs/HEAD` and the operation markers there belong to
/// the main worktree, and `worktrees/<name>/` to each of the others.
pub fn relevant_common(rel: &Path) -> bool {
    let Some(p) = parts(rel) else { return false };
    let shared = match p.first().copied() {
        Some("refs") | Some("packed-refs") => true,
        Some("logs") => p.get(1) == Some(&"refs"),
        _ => false,
    };
    shared && relevant(rel)
}

/// The directories a repository keeps its state in, canonical: the macOS stream
/// reports `/private/var/...` for a watch set on `/var/...`, and a prefix comparison
/// of two spellings of one directory never matches.
#[derive(Debug, Clone)]
pub struct GitDirs {
    /// `HEAD`, the index, the operation markers — per worktree.
    pub git_dir: PathBuf,
    /// Refs and `packed-refs` — shared by all worktrees. Equal to `git_dir` in an
    /// ordinary repository.
    pub common_dir: PathBuf,
}

impl GitDirs {
    /// Ask git where they are (`engine::cli::git_paths`, never `.git` joined onto the
    /// root: in a linked worktree `.git` is a file).
    pub fn resolve(repo: &Path) -> Result<GitDirs> {
        let paths = CliEngine::new(repo).git_paths(&["HEAD", "packed-refs"])?;
        let dir_of = |p: &Path| -> Result<PathBuf> {
            let parent = p
                .parent()
                .ok_or_else(|| Error::Parse(format!("no git directory above {}", p.display())))?;
            parent
                .canonicalize()
                .map_err(|e| Error::Io(format!("{}: {e}", parent.display())))
        };
        Ok(GitDirs {
            git_dir: dir_of(&paths[0])?,
            common_dir: dir_of(&paths[1])?,
        })
    }

    /// Does a change at this absolute path count? The own git directory is asked
    /// first: a linked worktree's lives inside the common one, under `worktrees/`.
    pub fn counts(&self, path: &Path) -> bool {
        if let Ok(rel) = path.strip_prefix(&self.git_dir) {
            return relevant(rel);
        }
        if let Ok(rel) = path.strip_prefix(&self.common_dir) {
            return relevant_common(rel);
        }
        false
    }

    /// Directories to watch recursively: the common one, plus the own one when it is
    /// not already inside it (`git worktree add` with a separate `--git-dir`, a
    /// submodule-like layout).
    fn roots(&self) -> Vec<&Path> {
        let mut r = vec![self.common_dir.as_path()];
        if !self.git_dir.starts_with(&self.common_dir) {
            r.push(self.git_dir.as_path());
        }
        r
    }
}

/// Does this event say anything about the repository at all? Reads are dropped by
/// kind: inotify reports every *open*, and a watcher that woke on the `git status` of
/// its own refresh would never go quiet. Metadata-only changes are not an update of
/// anything git keeps.
fn event_counts(ev: &notify::Event, dirs: &GitDirs) -> bool {
    match ev.kind {
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)) => return false,
        _ => {}
    }
    // The platform lost track (queue overflow, a rescan): assume it mattered, as
    // a missing file name is taken to.
    if ev.need_rescan() || ev.paths.is_empty() {
        return true;
    }
    ev.paths.iter().any(|p| dirs.counts(p))
}

/// A running watch. Dropping it stops it: the notify handle goes, the channel it held
/// disconnects, and the debounce thread returns without reporting what it held.
pub struct RepoWatcher {
    repo: PathBuf,
    stopped: Arc<AtomicBool>,
    _handle: RecommendedWatcher,
}

impl RepoWatcher {
    /// The repository root this watcher reports for.
    pub fn repo(&self) -> &Path {
        &self.repo
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        // A batch already closed must not be reported for a repository no longer open.
        self.stopped.store(true, Ordering::SeqCst);
    }
}

/// Watch `repo`, calling `report(repo)` once per burst of changes that were not
/// Graft's own. `own()` says whether Graft is inside an action right now (production:
/// `exec::OWN_ACTIONS` with [`OWN_GRACE`]); it is a parameter so that a test is not
/// silenced by another test running an action on a parallel thread.
pub fn start(
    repo: &Path,
    own: impl Fn() -> bool + Send + 'static,
    report: impl Fn(&Path) + Send + 'static,
) -> Result<RepoWatcher> {
    let dirs = GitDirs::resolve(repo)?;
    let (tx, rx) = mpsc::channel::<bool>();

    let watch_dirs = dirs.clone();
    let mut handle = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // An error from the platform names no path; the next real event, or focus,
        // still arrives.
        let Ok(ev) = res else { return };
        if event_counts(&ev, &watch_dirs) {
            // `true` = external. Tagged now: by the time the burst closes, the
            // action that caused it may have ended.
            let _ = tx.send(!own());
        }
    })
    .map_err(|e| Error::Io(format!("watch {}: {e}", repo.display())))?;
    for root in dirs.roots() {
        handle
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| Error::Io(format!("watch {}: {e}", root.display())))?;
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    let owner = repo.to_path_buf();
    std::thread::Builder::new()
        .name("graft-repo-watch".into())
        .spawn(move || debounce(rx, &flag, || report(&owner)))
        .map_err(|e| Error::Io(e.to_string()))?;

    Ok(RepoWatcher {
        repo: repo.to_path_buf(),
        stopped,
        _handle: handle,
    })
}

/// Close bursts: report once per quiet [`DEBOUNCE`], or every [`MAX_BATCH`] of an
/// endless one — and only if some member of the burst was external.
fn debounce(rx: mpsc::Receiver<bool>, stopped: &AtomicBool, report: impl Fn()) {
    loop {
        let Ok(first) = rx.recv() else { return };
        let opened = Instant::now();
        let mut external = first;
        loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(x) => {
                    external |= x;
                    if opened.elapsed() >= MAX_BATCH {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if external && !stopped.load(Ordering::SeqCst) {
            report();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn yes(rel: &str) -> bool {
        relevant(Path::new(rel))
    }

    #[test]
    fn refs_head_and_operation_markers_count() {
        for p in [
            "HEAD",
            "ORIG_HEAD",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "packed-refs",
            "refs",
            "refs/heads/main",
            "refs/heads/feature/x",
            "refs/remotes/origin/HEAD",
            "refs/tags/v1",
            "refs/stash",
            "logs/HEAD",
            "logs/refs/heads/main",
            "logs/refs/stash",
            "rebase-merge",
            "rebase-merge/done",
            "rebase-apply/next",
            "sequencer/todo",
        ] {
            assert!(yes(p), "{p} must count");
        }
    }

    /// Each of these, if it counted, would be a refresh nobody asked for — and the
    /// first three a refresh that causes the next one.
    #[test]
    fn graft_own_files_index_fetch_head_and_locks_do_not() {
        for p in [
            "index",
            "index.lock",
            "changelists.json",
            "changelists.json.tmp.123.4",
            "graft-ui.json",
            "graft-ui.json.tmp.123.4",
            "FETCH_HEAD",
            "HEAD.lock",
            "refs/heads/main.lock",
            "packed-refs.lock",
            "refs/graft/discard",
            "refs/graft/discard.lock",
            "logs/refs/graft/discard",
            "objects/ab/cdef",
            "objects/pack/pack-1.pack",
            "config",
            "description",
            "COMMIT_EDITMSG",
            "fsmonitor--daemon/cookies/x",
            "hooks/pre-commit",
            "logs",
            "logs/other",
            "",
        ] {
            assert!(!yes(p), "{p} must not count");
        }
    }

    /// Matching by suffix would take another worktree's or a submodule's `HEAD` for
    /// this repository's.
    #[test]
    fn nested_git_dirs_are_not_this_repository() {
        for p in [
            "worktrees/other/HEAD",
            "worktrees/other/rebase-merge/done",
            "modules/sub/HEAD",
            "modules/sub/refs/heads/main",
            "refs/heads/HEAD.lock",
        ] {
            assert!(!yes(p), "{p} must not count");
        }
    }

    #[test]
    fn common_dir_of_a_linked_worktree_shares_only_refs() {
        let c = |p: &str| relevant_common(Path::new(p));
        assert!(c("refs/heads/main"));
        assert!(c("packed-refs"));
        assert!(c("logs/refs/heads/main"));
        assert!(!c("HEAD"), "the main worktree's HEAD");
        assert!(!c("logs/HEAD"));
        assert!(!c("rebase-merge/done"));
        assert!(!c("worktrees/other/HEAD"));
        assert!(!c("refs/graft/discard"));
        assert!(!c("index"));
    }

    #[test]
    fn counts_resolves_against_the_own_dir_first() {
        let dirs = GitDirs {
            git_dir: PathBuf::from("/r/.git/worktrees/me"),
            common_dir: PathBuf::from("/r/.git"),
        };
        assert!(dirs.counts(Path::new("/r/.git/worktrees/me/HEAD")));
        assert!(dirs.counts(Path::new("/r/.git/worktrees/me/rebase-merge/done")));
        assert!(!dirs.counts(Path::new("/r/.git/worktrees/other/HEAD")));
        assert!(dirs.counts(Path::new("/r/.git/refs/heads/x")));
        assert!(!dirs.counts(Path::new("/r/.git/HEAD")));
        assert!(!dirs.counts(Path::new("/elsewhere/HEAD")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_separators() {
        assert!(yes(r"refs\heads\main"));
        assert!(!yes(r"refs\graft\discard"));
        assert!(!yes(r"worktrees\other\HEAD"));
    }

    #[test]
    fn resolves_the_git_dirs_of_a_linked_worktree() {
        let dir = scratch_repo();
        let p = dir.path();
        let wt = tempfile::tempdir().unwrap();
        let wt_path = wt.path().join("wt");
        run(
            p,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                wt_path.to_str().unwrap(),
            ],
        );

        let main = GitDirs::resolve(p).unwrap();
        assert_eq!(main.git_dir, main.common_dir);
        assert_eq!(main.git_dir, p.join(".git").canonicalize().unwrap());

        let linked = GitDirs::resolve(&wt_path).unwrap();
        assert_eq!(linked.common_dir, main.common_dir);
        assert!(linked
            .git_dir
            .starts_with(main.common_dir.join("worktrees")));
        assert_eq!(
            linked.roots().len(),
            1,
            "own dir lies inside the common one"
        );
    }

    // ---- the watcher itself, on a real repository ----
    //
    // File-system events are asynchronous and the platform streams take a moment to
    // start, so the waits below are generous: a positive case waits up to 5 s for its
    // report (it normally comes ~300 ms after the write), a negative one waits the
    // debounce plus a full second. `own` is a local flag, not `exec::OWN_ACTIONS`:
    // another test running an action on a parallel thread would otherwise silence
    // these.

    const ARRIVES: Duration = Duration::from_secs(5);
    const SILENCE: Duration = Duration::from_millis(1300);

    struct Probe {
        rx: mpsc::Receiver<PathBuf>,
        own: Arc<AtomicBool>,
        _w: RepoWatcher,
    }

    fn probe(repo: &Path) -> Probe {
        let (tx, rx) = mpsc::channel();
        let own = Arc::new(AtomicBool::new(false));
        let o = own.clone();
        let w = start(
            repo,
            move || o.load(Ordering::SeqCst),
            move |r| {
                let _ = tx.send(r.to_path_buf());
            },
        )
        .unwrap();
        // The stream is not live the instant `watch` returns.
        std::thread::sleep(Duration::from_millis(400));
        Probe { rx, own, _w: w }
    }

    impl Probe {
        fn expect_report(&self, repo: &Path, what: &str) {
            let got = self
                .rx
                .recv_timeout(ARRIVES)
                .unwrap_or_else(|_| panic!("{what}: no report"));
            assert_eq!(got, repo, "{what}: reported for the open repository");
            self.drain();
        }

        fn expect_silence(&self, what: &str) {
            if let Ok(r) = self.rx.recv_timeout(SILENCE) {
                panic!("{what}: unexpected report for {}", r.display());
            }
        }

        /// Wait out a burst still closing, and forget what it reported.
        fn drain(&self) {
            std::thread::sleep(DEBOUNCE + Duration::from_millis(200));
            while self.rx.try_recv().is_ok() {}
        }
    }

    #[test]
    fn external_commit_and_checkout_are_reported_once_per_burst() {
        let dir = scratch_repo();
        let p = dir.path();
        let w = probe(p);

        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-q", "-m", "second"]);
        w.expect_report(p, "commit");

        run(p, &["checkout", "-q", "-b", "side"]);
        w.expect_report(p, "checkout");
        w.expect_silence("one checkout, one report");
    }

    #[test]
    fn graft_own_files_and_the_index_are_not_reported() {
        let dir = scratch_repo();
        let p = dir.path();
        let w = probe(p);

        // The rollback copies: written the way `engine::discard` writes them.
        run(
            p,
            &["update-ref", "-m", "backup", "refs/graft/discard", "HEAD"],
        );
        w.expect_silence("refs/graft/discard");

        // The index: rewritten by the very `git status` a refresh runs.
        std::fs::write(p.join("a.txt"), "changed\n").unwrap();
        run(p, &["add", "a.txt"]);
        run(p, &["status", "--porcelain"]);
        w.expect_silence("index");

        std::fs::write(p.join(".git/changelists.json"), "{}").unwrap();
        std::fs::write(p.join(".git/graft-ui.json"), "{}").unwrap();
        w.expect_silence("changelists.json / graft-ui.json");
    }

    #[test]
    fn a_change_during_an_own_action_is_not_reported() {
        let dir = scratch_repo();
        let p = dir.path();
        let w = probe(p);

        w.own.store(true, Ordering::SeqCst);
        run(p, &["branch", "own"]);
        // Long enough for the events of the write to arrive while "own" holds.
        std::thread::sleep(Duration::from_millis(500));
        w.own.store(false, Ordering::SeqCst);
        w.expect_silence("own action");

        run(p, &["branch", "theirs"]);
        w.expect_report(p, "the next external change still arrives");
    }

    #[test]
    fn a_dropped_watcher_is_silent() {
        let dir = scratch_repo();
        let p = dir.path();
        let Probe { rx, own: _own, _w } = probe(p);
        drop(_w);
        run(p, &["branch", "after"]);
        assert!(rx.recv_timeout(SILENCE).is_err(), "no report after drop");
    }

    #[test]
    fn activity_counts_nested_actions_and_the_grace_after() {
        let a = crate::engine::exec::Activity::new();
        assert!(!a.within(Duration::from_secs(10)));
        a.enter();
        a.enter();
        a.leave();
        assert!(a.within(Duration::ZERO), "one action still running");
        a.leave();
        assert!(a.within(Duration::from_secs(10)), "inside the grace");
        assert!(!a.within(Duration::ZERO), "past a zero grace");
    }
}
