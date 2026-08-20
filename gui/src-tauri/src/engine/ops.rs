//! Operations that rewrite history, and the state of an unfinished one.
//!
//! Hides the sequence of git commands and the recognition of an in-progress
//! operation from files under `.git/`. Filled in by task 06 — except
//! [`detect_state`], which already answers "no operation" for a calm repository so
//! `RepoState.operation` is honest from the first task.

use std::path::Path;
use std::process::Command;

use crate::engine::cli::CliEngine;
use crate::error::{Error, Result};
use crate::model::{CommitFileEntry, OperationKind, OperationState, StashEntry};

/// Run `git -C <repo> <args>`; on failure carry **both** streams.
///
/// `git merge`, `git rebase`, `git cherry-pick` and `git revert` announce a conflict
/// partly on stdout ("CONFLICT (content): Merge conflict in a.txt") and partly on
/// stderr, so a helper keeping stderr alone would drop the very line naming the file.
/// `Error::Git` has one output field; both streams go into it, in the order git
/// wrote them (same convention as `engine::branches`).
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    if !out.status.success() {
        let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if !err.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(err);
        }
        return Err(Error::Git {
            command: args.join(" "),
            stderr: text,
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Which multi-step operation, if any, is in progress.
///
/// Recognised by the marker files git itself writes. Their location is resolved by
/// git (`rev-parse --git-path`), not by joining `.git` onto the worktree root: in a
/// linked worktree and in a submodule `.git` is a file and the markers live under
/// `.git/worktrees/<name>/`, so a concatenated path would quietly report "no
/// operation" and the banner would never appear. A path git cannot resolve is an
/// error, not silence.
///
/// `current` / `total` / `conflicted` are filled by task 06; a calm repository is
/// already reported exactly — `OperationKind::None`.
pub fn detect_state(repo: &Path) -> Result<OperationState> {
    const MARKERS: [&str; 5] = [
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ];
    let paths = CliEngine::new(repo).git_paths(&MARKERS)?;
    let present: Vec<bool> = paths.iter().map(|p| p.exists()).collect();

    let kind = if present[0] {
        OperationKind::Merge
    } else if present[1] || present[2] {
        OperationKind::Rebase
    } else if present[3] {
        OperationKind::CherryPick
    } else if present[4] {
        OperationKind::Revert
    } else {
        OperationKind::None
    };
    if kind == OperationKind::None {
        return Ok(OperationState {
            kind,
            current: None,
            total: None,
            conflicted: Vec::new(),
        });
    }

    // "N of M" exists only for a rebase, and the two backends spell it differently:
    // the merge backend keeps `msgnum`/`end`, the apply backend `next`/`last`.
    let (current, total) = if kind == OperationKind::Rebase {
        let (cur_name, tot_name) = if present[1] {
            ("rebase-merge/msgnum", "rebase-merge/end")
        } else {
            ("rebase-apply/next", "rebase-apply/last")
        };
        let counters = CliEngine::new(repo).git_paths(&[cur_name, tot_name])?;
        (read_counter(&counters[0]), read_counter(&counters[1]))
    } else {
        (None, None)
    };

    Ok(OperationState {
        kind,
        current,
        total,
        conflicted: conflicted_paths(repo)?,
    })
}

/// One number written by git into a rebase state file, or `None` when the file is
/// missing or unreadable — an absent counter is unknown, not zero.
fn read_counter(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Paths left unmerged by the stopped operation, in git's order.
///
/// `diff --diff-filter=U` names exactly the conflicted entries; `-z` keeps paths
/// with spaces or non-ASCII intact, so the split is on NUL, never on whitespace.
fn conflicted_paths(repo: &Path) -> Result<Vec<String>> {
    let out = git(repo, &["diff", "--name-only", "--diff-filter=U", "-z"])?;
    Ok(out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// git flag for a reset mode: `soft` | `mixed` | `hard` | `keep` (манифест G02,
/// История 57). Unknown values are rejected rather than folded into a default —
/// guessing here would discard the user's work under a different mode than asked.
pub fn reset_mode_flag(mode: &str) -> Result<&'static str> {
    match mode {
        "soft" => Ok("--soft"),
        "mixed" => Ok("--mixed"),
        "hard" => Ok("--hard"),
        "keep" => Ok("--keep"),
        other => Err(Error::Rule(format!(
            "unknown reset mode: {other} (expected soft, mixed, hard or keep)"
        ))),
    }
}

/// История 56 / G01: undo a commit with a new commit on top; history is left
/// intact. `--no-edit` takes git's own message instead of opening an editor —
/// stated as a flag rather than an `GIT_EDITOR` override so the intent survives
/// into `Error::Git { command, .. }`.
///
/// A conflict is an error carrying git's output whole; the repository is left in
/// the revert state, which [`detect_state`] then reports with its conflicted paths.
pub fn revert(repo: &Path, hash: &str) -> Result<()> {
    git(repo, &["revert", "--no-edit", hash])?;
    Ok(())
}

/// Reset to `hash` in one of git's modes: `soft` | `mixed` | `hard` | `keep`.
/// The mode is validated at the boundary before any git call.
pub fn reset(repo: &Path, hash: &str, mode: &str) -> Result<()> {
    let flag = reset_mode_flag(mode)?;
    git(repo, &["reset", flag, hash])?;
    Ok(())
}

/// How many commits on the current branch would be discarded by resetting to
/// `hash` — the number История 57 requires the `hard` confirmation to name.
///
/// A commit already reachable from `hash` is not lost, so this is exactly
/// `<hash>..HEAD`. Resetting to HEAD loses nothing and answers 0.
pub fn commits_after(repo: &Path, hash: &str) -> Result<u32> {
    let out = git(repo, &["rev-list", "--count", &format!("{hash}..HEAD")])?;
    out.trim()
        .parse()
        .map_err(|_| Error::Rule(format!("unexpected rev-list output: {}", out.trim())))
}

/// Whether the working tree or the index carries anything uncommitted — the other
/// half of the `hard` warning. Untracked files count: `hard` does not remove them,
/// but the user deciding between modes wants to know the tree is not clean.
pub fn has_local_changes(repo: &Path) -> Result<bool> {
    Ok(!git(repo, &["status", "--porcelain", "-z"])?
        .trim_matches('\0')
        .is_empty())
}

/// Whether the current branch already carries this commit — by ancestry, or by an
/// equivalent patch applied under a different hash.
///
/// История 58 disables cherry-pick for such a commit, so the answer is needed
/// before the menu is drawn, not as an error afterwards. Ancestry alone would say
/// "no" for a commit that was already cherry-picked once (the copy has its own
/// hash), which is exactly the case that produces a duplicate; `git cherry`
/// compares patch ids and catches it. A commit git cannot resolve is an error.
pub fn contains_commit(repo: &Path, hash: &str) -> Result<bool> {
    let ancestor = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", hash, "HEAD"])
        .output()?;
    if ancestor.status.success() {
        return Ok(true);
    }
    // git resolves the commit before answering; an unknown revision is an error,
    // not "not contained".
    git(repo, &["rev-parse", "--verify", &format!("{hash}^{{commit}}")])?;

    let parent = format!("{hash}^");
    let limited = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{parent}^{{commit}}")])
        .output()?
        .status
        .success();
    let listing = if limited {
        git(repo, &["cherry", "HEAD", hash, &parent])?
    } else {
        git(repo, &["cherry", "HEAD", hash])?
    };
    // `+` marks a commit whose patch is not in HEAD; `-` marks one that is.
    Ok(!listing.lines().any(|l| l.starts_with('+')))
}

/// История 58 / G03: replay a commit onto the current branch.
///
/// A commit the branch already carries is refused **before** git is started: a
/// cherry-pick of a contained commit either fails as an empty commit or silently
/// duplicates it, and neither is an answer the panel can explain.
pub fn cherry_pick(repo: &Path, hash: &str) -> Result<()> {
    if contains_commit(repo, hash)? {
        return Err(Error::Rule(format!(
            "commit {hash} is already contained in the current branch"
        )));
    }
    git(repo, &["cherry-pick", hash])?;
    Ok(())
}

/// История 55 / R24i.4: check out a revision, landing on a detached HEAD.
///
/// `--detach` is explicit: without it a hash that also happens to name a branch
/// would quietly check the branch out instead, and the dialog has just promised
/// the user a detached HEAD.
pub fn checkout_rev(repo: &Path, hash: &str) -> Result<()> {
    git(repo, &["checkout", "--detach", hash])?;
    Ok(())
}

/// История 54 / R24i.3: tag a commit — lightweight without a message, annotated
/// with one. Moving an existing tag is not offered: `git tag` refuses, and the
/// refusal reaches the user rather than a silently relocated tag.
pub fn tag_create(repo: &Path, hash: &str, name: &str, message: Option<&str>) -> Result<()> {
    match message {
        Some(m) => git(repo, &["tag", "-a", "-m", m, name, hash])?,
        None => git(repo, &["tag", name, hash])?,
    };
    Ok(())
}

/// The git subcommand that drives the operation in progress.
///
/// Which one it is has to be asked of the repository: `git rebase --abort` and
/// `git merge --abort` are different commands with different effects, and the
/// panel button says only "Abort".
fn in_progress_command(repo: &Path) -> Result<&'static str> {
    match detect_state(repo)?.kind {
        OperationKind::Merge => Ok("merge"),
        OperationKind::Rebase => Ok("rebase"),
        OperationKind::CherryPick => Ok("cherry-pick"),
        OperationKind::Revert => Ok("revert"),
        OperationKind::None => Err(Error::Rule("no operation in progress".into())),
    }
}

/// Run `git <op> <flag>` with no editor: `--continue` would otherwise open one to
/// confirm the message and hang a GUI process forever. `true` exits 0 without
/// touching the file, which keeps the message git already prepared.
fn drive(repo: &Path, op: &str, flag: &str) -> Result<()> {
    let args = [op, flag];
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .output()?;
    if !out.status.success() {
        let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if !err.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(err);
        }
        return Err(Error::Git {
            command: args.join(" "),
            stderr: text,
        });
    }
    Ok(())
}

/// История 30: carry on with the operation the repository is in.
pub fn op_continue(repo: &Path) -> Result<()> {
    let op = in_progress_command(repo)?;
    drive(repo, op, "--continue")
}

/// История 30: undo the operation, returning the branch to where it started.
pub fn op_abort(repo: &Path) -> Result<()> {
    let op = in_progress_command(repo)?;
    drive(repo, op, "--abort")
}

/// История 30: drop the current commit and carry on.
///
/// A merge has no `--skip` — there is no "next commit" to move to. Saying so is
/// better than running something else that happens to be spelled similarly.
pub fn op_skip(repo: &Path) -> Result<()> {
    let op = in_progress_command(repo)?;
    if op == "merge" {
        return Err(Error::Rule("a merge cannot skip a commit".into()));
    }
    drive(repo, op, "--skip")
}

/// The marker `CliEngine::checkout` writes into the stash message when it shelves
/// changes to let a branch switch through. It is what tells the application's own
/// stashes apart from the user's, so the two have to stay spelled the same; the
/// writer lives in `engine::cli`.
pub const APP_STASH_TAG: &str = "mygit: switching to";

/// История 21a / R13i.5: stashes the application made while switching branches,
/// newest first — the user's own stashes are none of this panel's business (a full
/// stash manager is R05a, out of scope).
///
/// Each entry is three NUL-separated fields — `stash@{N}`, the unix time it was
/// made, and git's own text (`On <branch>: <marker> <target>`). The contract is
/// `Vec<String>`, so the record is packed into the string rather than into a type;
/// the split is on NUL, the separator this project already uses for git output,
/// because a stash message may contain spaces, colons and newlines. `api.ts`
/// carries the matching `parseAppStash`.
///
/// The time is not formatted here: every visible string lives in `src/i18n.ts`.
pub fn stash_list_app(repo: &Path) -> Result<Vec<String>> {
    let raw = git(repo, &["stash", "list", "--format=%gd%x00%at%x00%gs%x01"])?;
    let mut out = Vec::new();
    for record in raw.split('\u{1}') {
        let record = record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let mut fields = record.split('\0');
        let (Some(gd), Some(at), Some(gs)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if gs.contains(APP_STASH_TAG) {
            out.push(format!("{gd}\0{at}\0{gs}"));
        }
    }
    Ok(out)
}

/// История 21a: put a shelved change back into the working tree.
///
/// Takes either the bare `stash@{N}` or a whole record from [`stash_list_app`].
/// Anything else is refused instead of being handed to git as a revision: `git
/// stash apply HEAD~1` is a real command with a very different meaning.
///
/// `apply`, not `pop`: the spec requires a failed restore to leave the stash
/// intact, and with no stash manager to recover a dropped entry, keeping it is the
/// recoverable choice.
pub fn stash_restore(repo: &Path, name: &str) -> Result<()> {
    let entry = name
        .split('\0')
        .next()
        .unwrap_or(name)
        .split(':')
        .next()
        .unwrap_or(name)
        .trim();
    if !(entry.starts_with("stash@{") && entry.ends_with('}')) {
        return Err(Error::Rule(format!("not a stash entry: {name}")));
    }
    git(repo, &["stash", "apply", entry])?;
    Ok(())
}

/// Split a stash entry's reflog text into branch and message.
///
/// git writes three shapes: `On <branch>: <message>` for a stash made with `-m`,
/// `WIP on <branch>: <sha> <subject>` for one made without, and — in detached HEAD
/// — the same with the literal `(no branch)` in place of a name. Splitting on the
/// **first colon**, not on whitespace: `(no branch)` carries a space, so a
/// space-based split would invent a branch named `(no`. A prefix this function does
/// not recognise leaves `branch: None` and the whole text as the message, which is
/// honest about not knowing rather than guessing.
fn split_stash_text(gs: &str) -> (Option<String>, String) {
    let rest = gs
        .strip_prefix("WIP on ")
        .or_else(|| gs.strip_prefix("On "))
        .or_else(|| gs.strip_prefix("wip on "))
        .or_else(|| gs.strip_prefix("on "));
    let Some(rest) = rest else {
        return (None, gs.trim().to_string());
    };
    let Some((branch, message)) = rest.split_once(':') else {
        return (None, gs.trim().to_string());
    };
    let branch = branch.trim();
    let branch = (!branch.is_empty() && branch != "(no branch)").then(|| branch.to_string());
    (branch, message.trim().to_string())
}

/// Every stash in the repository, newest first — the stash manager's list.
///
/// Unlike [`stash_list_app`] this hides nothing: the user's own stashes are what the
/// manager exists for, and `from_app` only lets the UI mark the ones the application
/// made while switching branches.
///
/// Fields are NUL-separated and records terminated by `%x01`, because a stash
/// message may contain spaces, colons and newlines. A record that does not parse is
/// an error, not a skipped line: a short list is indistinguishable from a complete
/// one and reads as "my stash is gone" (докблок CLAUDE.md). [`stash_list_app`]
/// keeps its older, forgiving loop so its own contract does not move.
pub fn stash_list(repo: &Path) -> Result<Vec<StashEntry>> {
    let raw = git(repo, &["stash", "list", "--format=%gd%x00%H%x00%at%x00%gs%x01"])?;
    let mut out = Vec::new();
    for record in raw.split('\u{1}') {
        let record = record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let mut fields = record.split('\0');
        let (Some(gd), Some(hash), Some(at), Some(gs)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(Error::Parse(format!("stash list record: {record:?}")));
        };
        let at: i64 = at
            .trim()
            .parse()
            .map_err(|_| Error::Parse(format!("stash list timestamp: {at:?}")))?;
        let (branch, message) = split_stash_text(gs);
        out.push(StashEntry {
            reference: gd.trim().to_string(),
            hash: hash.trim().to_string(),
            at,
            branch,
            message,
            from_app: gs.contains(APP_STASH_TAG),
        });
    }
    Ok(out)
}

/// Accept only a real `stash@{N}`, never an arbitrary revision: `git stash drop
/// HEAD~1` and `git stash apply HEAD~1` are real commands with very different
/// meanings, and one of them destroys.
fn stash_entry_ref(name: &str) -> Result<&str> {
    let entry = name.trim();
    if !(entry.starts_with("stash@{") && entry.ends_with('}')) {
        return Err(Error::Rule(format!("not a stash entry: {name}")));
    }
    Ok(entry)
}

/// Resolve `stash@{N}` and, when the caller says which stash commit it means, refuse
/// if the two disagree.
///
/// `stash@{N}` is a position in a stack, and every pop or drop renumbers everything
/// below it. A list the user is looking at goes stale the moment another window (or
/// the branch-switch dialog) drops an entry, and a stale index makes `drop` destroy
/// the *wrong* stash silently. Passing the hash from the listed entry turns that
/// into a refusal.
fn resolve_stash(repo: &Path, name: &str, expect: Option<&str>) -> Result<String> {
    let entry = stash_entry_ref(name)?;
    let hash = git(repo, &["rev-parse", entry])?.trim().to_string();
    if let Some(expect) = expect.map(str::trim).filter(|s| !s.is_empty()) {
        if !(hash.starts_with(expect) || expect.starts_with(&hash)) {
            return Err(Error::Rule(format!(
                "{entry} is no longer the stash {expect}; the list moved, reload it"
            )));
        }
    }
    Ok(entry.to_string())
}

/// Put a stash back into the working tree and **keep** the entry.
pub fn stash_apply(repo: &Path, name: &str, expect: Option<&str>) -> Result<()> {
    let entry = resolve_stash(repo, name, expect)?;
    git(repo, &["stash", "apply", &entry])?;
    Ok(())
}

/// Put a stash back and drop the entry. A failed `pop` leaves the entry in place —
/// that is git's own behaviour, and the error carries its reason.
pub fn stash_pop(repo: &Path, name: &str, expect: Option<&str>) -> Result<()> {
    let entry = resolve_stash(repo, name, expect)?;
    git(repo, &["stash", "pop", &entry])?;
    Ok(())
}

/// Discard a stash without applying it.
pub fn stash_drop(repo: &Path, name: &str, expect: Option<&str>) -> Result<()> {
    let entry = resolve_stash(repo, name, expect)?;
    git(repo, &["stash", "drop", &entry])?;
    Ok(())
}

/// What a stash changes, in the same shape as the file list of a commit.
///
/// A stash *is* a commit, so this is `commit::files` against its first parent — the
/// commit the stash was made on. Files stashed as untracked (`-u`) live in the
/// stash's third parent and are not part of that diff; the manager therefore shows
/// tracked changes only.
pub fn stash_files(repo: &Path, name: &str) -> Result<Vec<CommitFileEntry>> {
    let entry = stash_entry_ref(name)?;
    crate::engine::commit::files(repo, entry)
}

/// Stash the current changes, untracked files included, under `message`.
///
/// A clean tree is refused before git runs: `git stash push` on nothing to save
/// exits 0 and prints "No local changes to save", so without this the UI would
/// report a stash that does not exist.
pub fn stash_push(repo: &Path, message: Option<&str>) -> Result<()> {
    if !has_local_changes(repo)? {
        return Err(Error::Rule("nothing to stash: the working tree is clean".into()));
    }
    let mut args = vec!["stash", "push", "-u"];
    let message = message.map(str::trim).filter(|m| !m.is_empty());
    if let Some(m) = message {
        args.push("-m");
        args.push(m);
    }
    git(repo, &args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileState;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Where git itself keeps a marker for this worktree.
    fn marker_path(dir: &Path, name: &str) -> std::path::PathBuf {
        let p = std::path::PathBuf::from(git(dir, &["rev-parse", "--git-path", name]));
        if p.is_absolute() {
            p
        } else {
            dir.join(p)
        }
    }

    #[test]
    fn detect_state_calm_repository_reports_none() {
        let dir = scratch_repo();
        assert_eq!(detect_state(dir.path()).unwrap().kind, OperationKind::None);
    }

    #[test]
    fn detect_state_recognises_operation_markers() {
        let dir = scratch_repo();
        let p = dir.path();

        std::fs::write(marker_path(p, "MERGE_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::Merge);
        std::fs::remove_file(marker_path(p, "MERGE_HEAD")).unwrap();

        std::fs::create_dir_all(marker_path(p, "rebase-merge")).unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::Rebase);
        std::fs::remove_dir_all(marker_path(p, "rebase-merge")).unwrap();

        std::fs::write(marker_path(p, "CHERRY_PICK_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::CherryPick);
        std::fs::remove_file(marker_path(p, "CHERRY_PICK_HEAD")).unwrap();

        std::fs::write(marker_path(p, "REVERT_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::Revert);
    }

    /// In a linked worktree `.git` is a file and the markers live under
    /// `.git/worktrees/<name>/` — a concatenated `repo/.git/MERGE_HEAD` would miss
    /// them and report a calm repository.
    #[test]
    fn detect_state_sees_markers_in_a_linked_worktree() {
        let dir = scratch_repo();
        let outer = tempfile::tempdir().unwrap();
        let linked = outer.path().join("linked");
        git(
            dir.path(),
            &["worktree", "add", "-b", "wt", linked.to_str().unwrap()],
        );
        assert!(linked.join(".git").is_file(), "linked worktree has a .git file");

        assert_eq!(detect_state(&linked).unwrap().kind, OperationKind::None);

        let marker = marker_path(&linked, "MERGE_HEAD");
        assert!(
            marker.starts_with(
                dir.path()
                    .canonicalize()
                    .unwrap()
                    .join(".git")
                    .join("worktrees")
            ),
            "marker lives in the linked worktree's git dir: {}",
            marker.display()
        );
        std::fs::write(&marker, b"deadbeef\n").unwrap();
        assert_eq!(detect_state(&linked).unwrap().kind, OperationKind::Merge);
    }

    /// Outside a repository the state is unknown — that is an error, not "calm".
    #[test]
    fn detect_state_outside_a_repository_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_state(dir.path()).is_err());
    }

    /// Manifest G02 / История 57: four reset modes, unknown ones rejected.
    #[test]
    fn reset_accepts_four_modes_and_rejects_others() {
        for mode in ["soft", "mixed", "hard", "keep"] {
            assert!(reset_mode_flag(mode).is_ok(), "{mode} is a git reset mode");
        }
        match reset_mode_flag("nuke") {
            Err(Error::Rule(m)) => assert!(m.contains("nuke"), "{m}"),
            other => panic!("expected a rule error, got {other:?}"),
        }
        // the mode is checked before anything else, even while reset is a stub
        let dir = scratch_repo();
        match reset(dir.path(), "HEAD", "nuke") {
            Err(Error::Rule(m)) => assert!(m.contains("unknown reset mode"), "{m}"),
            other => panic!("expected the mode to be rejected, got {other:?}"),
        }
    }

    /// История 56 / G01: revert adds an inverse commit; the reverted commit stays
    /// reachable — history is not rewritten.
    #[test]
    fn revert_adds_an_inverse_commit_and_keeps_history() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        git(p, &["commit", "-am", "second"]);
        let target = git(p, &["rev-parse", "HEAD"]);

        revert(p, &target).unwrap();

        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "one\n");
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "3");
        assert_eq!(
            git(p, &["rev-parse", "HEAD~1"]),
            target,
            "the reverted commit is still in history"
        );
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::None);
    }

    /// История 30 / спека «Revert a commit»: a conflicting revert stops in a state
    /// the panel can recognise, and the conflicted path is listed.
    #[test]
    fn a_conflicting_revert_stops_in_a_recognisable_state() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        git(p, &["commit", "-am", "second"]);
        let second = git(p, &["rev-parse", "HEAD"]);
        std::fs::write(p.join("a.txt"), "three\n").unwrap();
        git(p, &["commit", "-am", "third"]);

        match revert(p, &second) {
            Err(Error::Git { stderr, .. }) => assert!(
                stderr.contains("a.txt"),
                "git names the conflicted file on stdout, and it reaches the caller: {stderr}"
            ),
            other => panic!("expected the conflict to surface, got {other:?}"),
        }

        let state = detect_state(p).unwrap();
        assert_eq!(state.kind, OperationKind::Revert);
        assert_eq!(state.conflicted, vec!["a.txt".to_string()]);
    }

    /// Three commits on `main`, the same file rewritten each time: c1 "one",
    /// c2 "two", c3 "three". Returns the repo and the hash of c1.
    fn repo_with_three_commits() -> (tempfile::TempDir, String) {
        let dir = scratch_repo();
        let p = dir.path();
        let first = git(p, &["rev-parse", "HEAD"]);
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        git(p, &["commit", "-am", "second"]);
        std::fs::write(p.join("a.txt"), "three\n").unwrap();
        git(p, &["commit", "-am", "third"]);
        (dir, first)
    }

    fn staged(p: &Path) -> String {
        git(p, &["diff", "--cached", "--name-only"])
    }

    fn unstaged(p: &Path) -> String {
        git(p, &["diff", "--name-only"])
    }

    fn worktree(p: &Path) -> String {
        std::fs::read_to_string(p.join("a.txt")).unwrap()
    }

    /// История 57 / G02: each mode leaves a different, named state of index and tree.
    #[test]
    fn reset_modes_leave_the_expected_index_and_tree() {
        // soft: history moves, index and tree keep the newest content
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        reset(p, &first, "soft").unwrap();
        assert_eq!(git(p, &["rev-parse", "HEAD"]), first);
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(staged(p), "a.txt", "soft keeps the difference staged");
        assert_eq!(worktree(p), "three\n");

        // mixed: index follows HEAD, the tree does not
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        reset(p, &first, "mixed").unwrap();
        assert_eq!(staged(p), "", "mixed unstages");
        assert_eq!(unstaged(p), "a.txt", "mixed leaves the tree alone");
        assert_eq!(worktree(p), "three\n");

        // hard: everything follows HEAD
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        reset(p, &first, "hard").unwrap();
        assert_eq!(staged(p), "");
        assert_eq!(unstaged(p), "");
        assert_eq!(worktree(p), "one\n");

        // keep on a clean tree: like hard, since there is nothing local to keep
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        reset(p, &first, "keep").unwrap();
        assert_eq!(staged(p), "");
        assert_eq!(unstaged(p), "");
        assert_eq!(worktree(p), "one\n");
    }

    /// `keep` differs from `hard` only when a locally modified file also differs
    /// between HEAD and the target: keep refuses, hard discards.
    #[test]
    fn keep_refuses_to_discard_local_edits_where_hard_discards_them() {
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "local\n").unwrap();
        match reset(p, &first, "keep") {
            Err(Error::Git { stderr, .. }) => assert!(
                stderr.contains("a.txt"),
                "git says which file it refuses to overwrite: {stderr}"
            ),
            other => panic!("keep must refuse to discard the local edit, got {other:?}"),
        }
        assert_eq!(worktree(p), "local\n", "the local edit survives");
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "3", "and so does history");

        reset(p, &first, "hard").unwrap();
        assert_eq!(worktree(p), "one\n", "hard discards it");
    }

    /// История 57: the confirmation has to name what a hard reset discards, so the
    /// numbers are available before the operation runs.
    #[test]
    fn reset_preview_counts_lost_commits_and_sees_local_changes() {
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        assert_eq!(commits_after(p, &first).unwrap(), 2, "c2 and c3 would be lost");
        assert!(!has_local_changes(p).unwrap(), "a fresh checkout is clean");

        std::fs::write(p.join("a.txt"), "local\n").unwrap();
        assert!(has_local_changes(p).unwrap());
        assert_eq!(commits_after(p, &first).unwrap(), 2, "an edit is not a commit");

        let head = git(p, &["rev-parse", "HEAD"]);
        assert_eq!(commits_after(p, &head).unwrap(), 0, "resetting to HEAD loses none");
    }

    /// A commit on a side branch `feat` touching `path` with `content`; the repo is
    /// left back on `main`. Returns its hash.
    fn commit_on_feat(p: &Path, path: &str, content: &str) -> String {
        git(p, &["checkout", "-b", "feat"]);
        std::fs::write(p.join(path), content).unwrap();
        git(p, &["add", path]);
        git(p, &["commit", "-m", "feat work"]);
        let hash = git(p, &["rev-parse", "HEAD"]);
        git(p, &["checkout", "main"]);
        hash
    }

    /// История 58 / G03: a commit from elsewhere is replayed onto the current branch.
    #[test]
    fn cherry_pick_replays_a_commit_onto_the_current_branch() {
        let dir = scratch_repo();
        let p = dir.path();
        let hash = commit_on_feat(p, "b.txt", "from feat\n");
        assert!(!p.join("b.txt").exists(), "main does not have it yet");

        cherry_pick(p, &hash).unwrap();

        assert_eq!(
            std::fs::read_to_string(p.join("b.txt")).unwrap(),
            "from feat\n"
        );
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "2");
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::None);
    }

    /// История 58: the menu item is disabled for a commit already in the branch, so
    /// the engine has to answer the question — and refuse before git is asked.
    #[test]
    fn cherry_pick_of_a_contained_commit_is_refused_before_git_runs() {
        let dir = scratch_repo();
        let p = dir.path();
        let hash = commit_on_feat(p, "b.txt", "from feat\n");
        assert!(!contains_commit(p, &hash).unwrap(), "not on main yet");

        cherry_pick(p, &hash).unwrap();
        assert!(contains_commit(p, &hash).unwrap(), "still identified by hash");

        let head = git(p, &["rev-parse", "HEAD"]);
        assert!(contains_commit(p, &head).unwrap(), "HEAD contains itself");
        match cherry_pick(p, &head) {
            Err(Error::Rule(m)) => assert!(m.contains(&head), "the message names the commit: {m}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "2", "nothing happened");
        assert_eq!(
            detect_state(p).unwrap().kind,
            OperationKind::None,
            "git was never started, so there is no half-finished operation"
        );
    }

    /// История 30: a conflicting cherry-pick leaves a state the panel recognises.
    #[test]
    fn a_conflicting_cherry_pick_stops_in_a_recognisable_state() {
        let dir = scratch_repo();
        let p = dir.path();
        let hash = commit_on_feat(p, "a.txt", "from feat\n");
        std::fs::write(p.join("a.txt"), "from main\n").unwrap();
        git(p, &["commit", "-am", "main work"]);

        match cherry_pick(p, &hash) {
            Err(Error::Git { stderr, .. }) => assert!(
                stderr.contains("a.txt"),
                "git names the conflicted file, and it reaches the caller: {stderr}"
            ),
            other => panic!("expected the conflict to surface, got {other:?}"),
        }

        let state = detect_state(p).unwrap();
        assert_eq!(state.kind, OperationKind::CherryPick);
        assert_eq!(state.conflicted, vec!["a.txt".to_string()]);
    }

    /// История 55 / R24i.4: checking out a revision lands on a detached HEAD, and
    /// the branch it came from is left where it was.
    #[test]
    fn checkout_rev_detaches_head_at_the_commit() {
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();
        let tip = git(p, &["rev-parse", "main"]);

        checkout_rev(p, &first).unwrap();

        assert_eq!(git(p, &["rev-parse", "HEAD"]), first);
        assert_eq!(worktree(p), "one\n");
        assert_eq!(
            git(p, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "HEAD",
            "HEAD is detached, not on a branch"
        );
        assert_eq!(git(p, &["rev-parse", "main"]), tip, "main did not move");
    }

    /// История 54 / R24i.3: a lightweight tag is a ref to the commit, an annotated
    /// one is a tag object carrying the message. Both land on the named commit, not
    /// on HEAD.
    #[test]
    fn tag_create_makes_lightweight_and_annotated_tags() {
        let (dir, first) = repo_with_three_commits();
        let p = dir.path();

        tag_create(p, &first, "light", None).unwrap();
        assert_eq!(git(p, &["cat-file", "-t", "light"]), "commit");
        assert_eq!(git(p, &["rev-parse", "light^{commit}"]), first);

        tag_create(p, &first, "annotated", Some("released here")).unwrap();
        assert_eq!(git(p, &["cat-file", "-t", "annotated"]), "tag");
        assert_eq!(git(p, &["rev-parse", "annotated^{commit}"]), first);
        assert!(
            git(p, &["tag", "-l", "--format=%(contents)", "annotated"])
                .contains("released here"),
            "the message is stored on the tag object"
        );

        assert!(
            tag_create(p, &first, "light", None).is_err(),
            "an existing tag is not silently moved"
        );
    }

    /// Run git without asserting success — for setting up an operation that is
    /// *meant* to stop on a conflict.
    fn git_may_fail(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git")
            .status
            .success()
    }

    /// `feat` carries two commits, the first of which conflicts with `main`.
    /// Returns the repo and the tip of `feat` before the rebase.
    fn repo_with_a_stopped_rebase() -> (tempfile::TempDir, String) {
        let dir = scratch_repo();
        let p = dir.path();
        git(p, &["checkout", "-b", "feat"]);
        std::fs::write(p.join("a.txt"), "feat one\n").unwrap();
        git(p, &["commit", "-am", "feat one"]);
        std::fs::write(p.join("b.txt"), "feat two\n").unwrap();
        git(p, &["add", "b.txt"]);
        git(p, &["commit", "-m", "feat two"]);
        let tip = git(p, &["rev-parse", "HEAD"]);
        git(p, &["checkout", "main"]);
        std::fs::write(p.join("a.txt"), "main one\n").unwrap();
        git(p, &["commit", "-am", "main one"]);
        git(p, &["checkout", "feat"]);
        assert!(!git_may_fail(p, &["rebase", "main"]), "the rebase is meant to stop");
        (dir, tip)
    }

    /// История 30 / спека «In-progress operation is drivable»: a stopped rebase
    /// reports "N of M" and every conflicted path.
    #[test]
    fn a_stopped_rebase_reports_its_step_and_conflicts() {
        let (dir, _tip) = repo_with_a_stopped_rebase();
        let state = detect_state(dir.path()).unwrap();
        assert_eq!(state.kind, OperationKind::Rebase);
        assert_eq!(state.current, Some(1), "stopped on the first of the two commits");
        assert_eq!(state.total, Some(2));
        assert_eq!(state.conflicted, vec!["a.txt".to_string()]);
    }

    /// Aborting puts the branch back exactly where it was.
    #[test]
    fn abort_returns_the_branch_to_its_state_before_the_operation() {
        let (dir, tip) = repo_with_a_stopped_rebase();
        let p = dir.path();

        op_abort(p).unwrap();

        assert_eq!(git(p, &["rev-parse", "HEAD"]), tip, "back at the old tip");
        assert_eq!(git(p, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat");
        assert_eq!(worktree(p), "feat one\n");
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::None);
    }

    /// Continuing after the conflict is resolved finishes the rebase.
    #[test]
    fn continue_finishes_a_rebase_after_the_conflict_is_resolved() {
        let (dir, _tip) = repo_with_a_stopped_rebase();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "resolved\n").unwrap();
        git(p, &["add", "a.txt"]);

        op_continue(p).unwrap();

        assert_eq!(detect_state(p).unwrap().kind, OperationKind::None);
        assert_eq!(git(p, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat");
        // c1 + "main one" + the two replayed feat commits
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "4");
        assert_eq!(worktree(p), "resolved\n");
        assert!(p.join("b.txt").exists(), "the second commit was replayed too");
    }

    /// Skipping drops the conflicting commit and replays the rest.
    #[test]
    fn skip_drops_the_conflicting_commit() {
        let (dir, _tip) = repo_with_a_stopped_rebase();
        let p = dir.path();

        op_skip(p).unwrap();

        assert_eq!(detect_state(p).unwrap().kind, OperationKind::None);
        // c1 + "main one" + only the second feat commit
        assert_eq!(git(p, &["rev-list", "--count", "HEAD"]), "3");
        assert_eq!(worktree(p), "main one\n", "the skipped commit left no trace");
        assert!(p.join("b.txt").exists());
    }

    /// With nothing in progress there is nothing to drive, and that is a rule
    /// error rather than a confusing git failure.
    #[test]
    fn driving_a_calm_repository_is_refused() {
        let dir = scratch_repo();
        for r in [
            op_continue(dir.path()),
            op_skip(dir.path()),
            op_abort(dir.path()),
        ] {
            match r {
                Err(Error::Rule(m)) => assert!(m.contains("no operation"), "{m}"),
                other => panic!("expected a rule error, got {other:?}"),
            }
        }
    }

    /// A merge has no "skip": pretending it does would silently do something else.
    #[test]
    fn skip_is_refused_for_a_merge() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(marker_path(p, "MERGE_HEAD"), b"deadbeef\n").unwrap();
        match op_skip(p) {
            Err(Error::Rule(m)) => assert!(m.contains("skip"), "{m}"),
            other => panic!("expected a rule error, got {other:?}"),
        }
    }

    /// История 21a / R13i.5: what the application shelved while switching branches
    /// is the only thing it offers back, and restoring returns it to the tree.
    #[test]
    fn stashes_made_while_switching_are_listed_and_restored() {
        let dir = scratch_repo();
        let p = dir.path();
        git(p, &["branch", "dev"]);

        // a stash the user made by hand — not the application's business
        std::fs::write(p.join("a.txt"), "handmade\n").unwrap();
        git(p, &["stash", "push", "-m", "mine"]);

        std::fs::write(p.join("a.txt"), "local\n").unwrap();
        CliEngine::new(p).checkout("dev", true).unwrap();
        assert_eq!(worktree(p), "one\n", "the switch left a clean tree");

        let listed = stash_list_app(p).unwrap();
        assert_eq!(listed.len(), 1, "only the application's own stash: {listed:?}");
        let f: Vec<&str> = listed[0].split('\0').collect();
        assert_eq!(f.len(), 3, "ref, time and text: {:?}", listed[0]);
        assert_eq!(f[0], "stash@{0}");
        let at: i64 = f[1].parse().expect("a unix timestamp");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            (now - at).abs() < 300,
            "the entry carries when it was made: {at} against {now}"
        );
        assert!(
            f[2].contains("main") && f[2].contains("dev"),
            "the entry names the branch it was made on and the one switched to: {}",
            f[2]
        );

        stash_restore(p, &listed[0]).unwrap();
        assert_eq!(worktree(p), "local\n", "the shelved change is back");
        assert_eq!(
            git(p, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "dev",
            "restoring does not switch branches"
        );
        assert_eq!(
            stash_list_app(p).unwrap().len(),
            1,
            "the entry stays until a stash manager offers to drop it"
        );
    }

    /// Спека branches: a restore that would clobber current work stops with git's
    /// own message and leaves the stash where it is.
    #[test]
    fn a_restore_onto_a_dirty_tree_fails_and_keeps_the_stash() {
        let dir = scratch_repo();
        let p = dir.path();
        git(p, &["branch", "dev"]);
        std::fs::write(p.join("a.txt"), "local\n").unwrap();
        CliEngine::new(p).checkout("dev", true).unwrap();

        std::fs::write(p.join("a.txt"), "in the way\n").unwrap();
        let entry = stash_list_app(p).unwrap().remove(0);
        match stash_restore(p, &entry) {
            Err(Error::Git { stderr, .. }) => assert!(
                stderr.contains("a.txt"),
                "git's own message reaches the caller: {stderr}"
            ),
            other => panic!("expected the restore to fail, got {other:?}"),
        }
        assert_eq!(worktree(p), "in the way\n", "current work untouched");
        assert_eq!(stash_list_app(p).unwrap().len(), 1, "the stash is intact");
    }

    /// A name that is not a stash entry is refused rather than handed to git as an
    /// arbitrary revision.
    #[test]
    fn stash_restore_rejects_a_name_that_is_not_a_stash_entry() {
        let dir = scratch_repo();
        match stash_restore(dir.path(), "HEAD~1") {
            Err(Error::Rule(m)) => assert!(m.contains("HEAD~1"), "{m}"),
            other => panic!("expected a rule error, got {other:?}"),
        }
    }

    /// Filtering the list must not renumber it: the refs come from git, so a user's
    /// stash sitting between two of the application's leaves a gap — and restoring
    /// the older entry still brings back its own content.
    #[test]
    fn listed_refs_survive_a_users_stash_sitting_between_them() {
        let dir = scratch_repo();
        let p = dir.path();
        git(p, &["branch", "dev"]);
        git(p, &["branch", "dev2"]);

        std::fs::write(p.join("a.txt"), "first local\n").unwrap();
        CliEngine::new(p).checkout("dev", true).unwrap();

        std::fs::write(p.join("a.txt"), "handmade\n").unwrap();
        git(p, &["stash", "push", "-m", "mine"]);

        std::fs::write(p.join("a.txt"), "second local\n").unwrap();
        CliEngine::new(p).checkout("dev2", true).unwrap();

        let listed = stash_list_app(p).unwrap();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert!(listed[0].starts_with("stash@{0}"), "newest first: {}", listed[0]);
        assert!(
            listed[1].starts_with("stash@{2}"),
            "the user's stash is stash@{{1}}, so the older entry keeps git's own ref: {}",
            listed[1]
        );

        stash_restore(p, &listed[1]).unwrap();
        assert_eq!(worktree(p), "first local\n");
    }

    // ── stash manager (R05a) ────────────────────────────────────────────────

    /// The manager lists **every** stash, the user's included, and marks only the
    /// application's own — that is the whole difference from `stash_list_app`.
    #[test]
    fn stash_list_shows_the_users_stashes_too() {
        let dir = scratch_repo();
        let p = dir.path();
        git(p, &["branch", "dev"]);

        std::fs::write(p.join("a.txt"), "mine\n").unwrap();
        git(p, &["stash", "push", "-m", "my own work"]);

        std::fs::write(p.join("a.txt"), "switching\n").unwrap();
        CliEngine::new(p).checkout("dev", true).unwrap();

        let all = stash_list(p).unwrap();
        assert_eq!(all.len(), 2, "{all:?}");
        assert_eq!(all[0].reference, "stash@{0}");
        assert!(all[0].from_app, "newest is the one the app made: {:?}", all[0]);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
        assert!(all[0].message.contains("mygit: switching to dev"), "{:?}", all[0]);

        assert_eq!(all[1].reference, "stash@{1}");
        assert!(!all[1].from_app, "the user's own stash: {:?}", all[1]);
        assert_eq!(all[1].branch.as_deref(), Some("main"));
        assert_eq!(all[1].message, "my own work");
        assert_eq!(all[1].hash.len(), 40, "the stash commit: {:?}", all[1]);
        assert!(all[1].at > 0, "creation time: {:?}", all[1]);

        // stash_list_app still sees only the application's, unchanged.
        assert_eq!(stash_list_app(p).unwrap().len(), 1);
    }

    /// A stash made in detached HEAD reads `WIP on (no branch): …`; the parser says
    /// "no branch" instead of inventing one out of the literal.
    #[test]
    fn a_stash_without_a_branch_reports_none() {
        let (branch, message) = split_stash_text("WIP on (no branch): 1a2b3c subject here");
        assert_eq!(branch, None);
        assert_eq!(message, "1a2b3c subject here");

        let (branch, message) = split_stash_text("On feature/x: keep: the colon");
        assert_eq!(branch.as_deref(), Some("feature/x"));
        assert_eq!(message, "keep: the colon");
    }

    /// apply keeps the entry, pop takes it away.
    #[test]
    fn apply_keeps_the_entry_and_pop_removes_it() {
        let dir = scratch_repo();
        let p = dir.path();

        std::fs::write(p.join("a.txt"), "work\n").unwrap();
        stash_push(p, Some("work in progress")).unwrap();
        assert_eq!(worktree(p), "one\n", "the stash took the change away");

        let entry = stash_list(p).unwrap().remove(0);
        assert_eq!(entry.message, "work in progress");

        stash_apply(p, &entry.reference, Some(&entry.hash)).unwrap();
        assert_eq!(worktree(p), "work\n");
        assert_eq!(stash_list(p).unwrap().len(), 1, "apply keeps the entry");

        git(p, &["checkout", "--", "a.txt"]);
        stash_pop(p, &entry.reference, Some(&entry.hash)).unwrap();
        assert_eq!(worktree(p), "work\n");
        assert!(stash_list(p).unwrap().is_empty(), "pop removed the entry");
    }

    /// drop discards without applying, and a `stash@{N}` that no longer holds the
    /// stash the caller named is refused instead of destroying its neighbour.
    #[test]
    fn drop_discards_and_a_moved_entry_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();

        for text in ["older\n", "middle\n", "newest\n"] {
            std::fs::write(p.join("a.txt"), text).unwrap();
            stash_push(p, Some(text.trim())).unwrap();
        }

        let middle = stash_list(p).unwrap().remove(1);
        assert_eq!(middle.message, "middle");

        // the newest stash is dropped elsewhere, so every index below it shifts up
        // and stash@{1} now names "older", not the entry the caller looked at.
        git(p, &["stash", "drop", "stash@{0}"]);
        match stash_drop(p, "stash@{1}", Some(&middle.hash)) {
            Err(Error::Rule(m)) => assert!(m.contains("the list moved"), "{m}"),
            other => panic!("a stale index must be refused: {other:?}"),
        }
        assert_eq!(stash_list(p).unwrap().len(), 2, "nothing was destroyed");

        stash_drop(p, "stash@{0}", Some(&middle.hash)).unwrap();
        let left = stash_list(p).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].message, "older");
        assert_eq!(worktree(p), "one\n", "drop applies nothing");
    }

    /// What is inside a stash, in the same shape as a commit's file list.
    #[test]
    fn stash_files_lists_what_the_stash_changes() {
        let dir = scratch_repo();
        let p = dir.path();

        std::fs::write(p.join("a.txt"), "changed\n").unwrap();
        std::fs::write(p.join("b.txt"), "added\n").unwrap();
        git(p, &["add", "b.txt"]);
        stash_push(p, Some("two files")).unwrap();

        let entry = stash_list(p).unwrap().remove(0);
        let mut files = stash_files(p, &entry.reference).unwrap();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[0].status, FileState::Modified);
        assert_eq!(files[1].path, "b.txt");
        assert_eq!(files[1].status, FileState::Added);
    }

    /// Stashing a clean tree is a domain refusal: git would exit 0 having made
    /// nothing, and the UI would show an entry that does not exist.
    #[test]
    fn stashing_a_clean_tree_is_refused() {
        let dir = scratch_repo();
        match stash_push(dir.path(), Some("nothing here")) {
            Err(Error::Rule(m)) => assert!(m.contains("nothing to stash"), "{m}"),
            other => panic!("expected a domain refusal: {other:?}"),
        }
        assert!(stash_list(dir.path()).unwrap().is_empty());
    }

    /// Nothing but a `stash@{N}` reaches git: `git stash drop HEAD~1` is a real
    /// command, and it destroys.
    #[test]
    fn stash_operations_reject_a_name_that_is_not_a_stash_entry() {
        let dir = scratch_repo();
        let p = dir.path();
        for r in [
            stash_apply(p, "HEAD~1", None),
            stash_pop(p, "HEAD~1", None),
            stash_drop(p, "HEAD~1", None),
            stash_files(p, "HEAD~1").map(|_| ()),
        ] {
            match r {
                Err(Error::Rule(m)) => assert!(m.contains("not a stash entry"), "{m}"),
                other => panic!("expected a domain refusal: {other:?}"),
            }
        }
    }
}
