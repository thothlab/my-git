//! The branch tree and operations on branches.
//!
//! Hides `for-each-ref`, the parse of `%(upstream:track)` and the grouping order.
//!
//! Two conventions the frontend has to know:
//!
//! * A detached HEAD is returned as one extra node placed **first**, recognised by
//!   `full_ref == "HEAD"` ([`DETACHED_REF`]); its `name` is the short revision. The
//!   engine emits no display text — every visible string lives in `src/i18n.ts`.
//! * `is_favorite` is always `false` here: favourites live in `.git/graft-ui.json`
//!   and are applied by the tree component, not by git.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::model::BranchNode;

/// `full_ref` of the synthetic node standing for a detached HEAD.
pub const DETACHED_REF: &str = "HEAD";

/// Fields of one `for-each-ref` record, in the order the format asks for them.
const REF_FORMAT: &str = "%(refname:short)%00%(refname)%00%(HEAD)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:unix)%01";
const REF_FIELDS: usize = 6;

/// Run `git -C <repo> <args>`; on failure carry **both** streams.
///
/// `git merge` and `git rebase` announce a conflict partly on stdout ("CONFLICT
/// (content): Merge conflict in f.txt") and partly on stderr, so a helper that kept
/// stderr alone would drop the very line naming the file. `Error::Git` has one
/// output field; both streams go into it, in the order git wrote them.
fn both_streams(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(stdout).trim().to_string();
    let err = String::from_utf8_lossy(stderr);
    let err = err.trim();
    if !err.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err);
    }
    text
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    if !out.status.success() {
        return Err(Error::Git {
            command: args.join(" "),
            stderr: both_streams(&out.stdout, &out.stderr),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// `%(upstream:track)` → ahead / behind.
///
/// Empty string with an upstream present means "in sync" — that is `(0, 0)`, a
/// real answer. `[gone]` means the upstream ref no longer exists, so the counters
/// are unknown, which is `None` and not zero. A branch with no upstream never
/// reaches here.
fn parse_track(track: &str) -> (Option<u32>, Option<u32>) {
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.is_empty() {
        return (Some(0), Some(0));
    }
    if inner == "gone" {
        return (None, None);
    }
    let (mut ahead, mut behind) = (Some(0), Some(0));
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().ok();
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().ok();
        }
    }
    (ahead, behind)
}

/// Local and remote-tracking branches with ahead/behind, ready for the tree.
///
/// Order: the detached HEAD node (if any), then local branches, then remote ones,
/// each group alphabetically with the current branch first. `refs/remotes/*/HEAD`
/// is a pointer at another branch, not a branch, and is dropped.
pub fn tree(repo: &Path) -> Result<Vec<BranchNode>> {
    let raw = git(
        repo,
        &[
            "for-each-ref",
            &format!("--format={REF_FORMAT}"),
            "refs/heads",
            "refs/remotes",
        ],
    )?;

    let mut local: Vec<BranchNode> = Vec::new();
    let mut remote: Vec<BranchNode> = Vec::new();
    for record in raw.split('\u{1}') {
        let record = record.trim_start_matches(['\n', '\r']);
        if record.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = record.split('\u{0}').collect();
        if f.len() != REF_FIELDS {
            return Err(Error::Parse(format!(
                "for-each-ref record has {} fields, expected {REF_FIELDS}",
                f.len()
            )));
        }
        let (name, full_ref, head, upstream, track, date) = (f[0], f[1], f[2], f[3], f[4], f[5]);
        let is_remote = full_ref.starts_with("refs/remotes/");
        if is_remote && full_ref.ends_with("/HEAD") {
            continue;
        }
        let upstream = (!upstream.is_empty()).then(|| upstream.to_string());
        let (ahead, behind) = match upstream {
            Some(_) => parse_track(track),
            None => (None, None),
        };
        let node = BranchNode {
            name: name.to_string(),
            full_ref: full_ref.to_string(),
            is_remote,
            is_current: head.trim() == "*",
            upstream,
            ahead,
            behind,
            is_favorite: false,
            last_commit_at: date.trim().parse().unwrap_or(0),
        };
        if is_remote {
            remote.push(node);
        } else {
            local.push(node);
        }
    }
    let by_name = |a: &BranchNode, b: &BranchNode| {
        b.is_current.cmp(&a.is_current).then_with(|| a.name.cmp(&b.name))
    };
    local.sort_by(by_name);
    remote.sort_by(by_name);

    let mut out = Vec::with_capacity(local.len() + remote.len() + 1);
    if let Some(head) = detached_head(repo)? {
        out.push(head);
    }
    out.append(&mut local);
    out.append(&mut remote);
    Ok(out)
}

/// The synthetic node for a detached HEAD, or `None` while HEAD points at a branch.
/// An empty repository (HEAD pointing at an unborn branch) is not detached.
fn detached_head(repo: &Path) -> Result<Option<BranchNode>> {
    if current_branch(repo)?.is_some() {
        return Ok(None);
    }
    let short = git(repo, &["rev-parse", "--short", "HEAD"])?.trim().to_string();
    let at: i64 = git(repo, &["log", "-1", "--format=%ct", "HEAD"])?
        .trim()
        .parse()
        .unwrap_or(0);
    Ok(Some(BranchNode {
        name: short,
        full_ref: DETACHED_REF.to_string(),
        is_remote: false,
        is_current: true,
        upstream: None,
        ahead: None,
        behind: None,
        is_favorite: false,
        last_commit_at: at,
    }))
}

/// Whether a ref exists under `refs/heads/` (`local = true`) or `refs/remotes/`.
///
/// "git could not answer" is not folded into "no such ref": `show-ref` exits 1 for a
/// missing ref and anything else means the question itself failed, and a guard built
/// on a swallowed failure silently stops guarding (докблок `error.rs`).
fn ref_exists(repo: &Path, name: &str, local: bool) -> Result<bool> {
    let prefix = if local { "refs/heads/" } else { "refs/remotes/" };
    let full = format!("{prefix}{name}");
    let args = ["show-ref", "--verify", "--quiet", full.as_str()];
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Git {
            command: args.join(" "),
            stderr: both_streams(&out.stdout, &out.stderr),
        }),
    }
}

/// Short name of the checked-out branch, or `None` in detached HEAD. A git that
/// failed to answer is an error, never a silent "not the current branch".
fn current_branch(repo: &Path) -> Result<Option<String>> {
    let args = ["symbolic-ref", "--short", "-q", "HEAD"];
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    match out.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None),
        _ => Err(Error::Git {
            command: args.join(" "),
            stderr: both_streams(&out.stdout, &out.stderr),
        }),
    }
}

/// Upstream of a branch in `<remote>/<branch>` form, or `None` when it tracks
/// nothing — that is git's own answer (a non-zero exit), not a hidden failure.
fn upstream_of(repo: &Path, name: &str) -> Result<Option<String>> {
    let spec = format!("{name}@{{upstream}}");
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", spec.as_str()])
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let up = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!up.is_empty()).then_some(up))
}

/// Rename a local branch. git carries `branch.<name>.remote/merge` across, so the
/// upstream survives. A remote-tracking branch has no local name to move and is
/// refused before git is called (спека branches: «Remote branch cannot be renamed»).
pub fn rename(repo: &Path, from: &str, to: &str) -> Result<()> {
    if !ref_exists(repo, from, true)? && ref_exists(repo, from, false)? {
        return Err(Error::Rule(format!(
            "{from} is a remote branch; only local branches can be renamed"
        )));
    }
    git(repo, &["branch", "-m", from, to])?;
    Ok(())
}

/// How many commits would be lost by deleting `name` — commits reachable from it
/// and from neither HEAD nor its upstream.
///
/// This is deliberately **git's own** definition of "not fully merged", the one
/// `git branch -d` applies: with two criteria in play the guard fires where git
/// would not (and stays silent where git refuses), and the user gets a raw "use -D"
/// instead of the confirmation dialog the guard exists to feed. The wider reading
/// ("merged into some other branch") was dropped for that reason.
pub fn unmerged_count(repo: &Path, name: &str) -> Result<u32> {
    let mut args: Vec<String> = ["rev-list", "--count", name, "--not", "HEAD"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(up) = upstream_of(repo, name)? {
        args.push(up);
    }
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git(repo, &borrowed)?;
    out.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("rev-list --count returned {:?}", out.trim())))
}

/// Delete a branch: local (`remote = false`) or on its remote.
///
/// Two refusals happen before git runs, so nothing is destroyed and no git output
/// is swallowed: the current branch, and — unless `force` — a branch holding
/// commits found nowhere else, whose number the message names for the dialog.
pub fn delete(repo: &Path, name: &str, remote: bool, force: bool) -> Result<()> {
    if remote {
        let (remote_name, branch) = name.split_once('/').ok_or_else(|| {
            Error::Rule(format!(
                "{name} is not a remote branch name (expected <remote>/<branch>)"
            ))
        })?;
        git(repo, &["push", remote_name, "--delete", branch])?;
        return Ok(());
    }
    if current_branch(repo)?.as_deref() == Some(name) {
        return Err(Error::Rule(format!(
            "{name} is the current branch; check out another branch first"
        )));
    }
    if !force {
        let n = unmerged_count(repo, name)?;
        if n > 0 {
            return Err(Error::Rule(format!(
                "{name} has {n} commits that are on no other branch; deleting it loses them"
            )));
        }
    }
    git(repo, &["branch", if force { "-D" } else { "-d" }, name])?;
    Ok(())
}

/// Merge `name` into the current branch. A conflict is a git failure whose output
/// (both streams) reaches the caller whole, and leaves the repository in the
/// unfinished merge that `ops::detect_state` reports.
pub fn merge(repo: &Path, name: &str) -> Result<()> {
    git(repo, &["merge", "--no-edit", name])?;
    Ok(())
}

/// Replay the current branch on top of `name`. Conflicts behave as in [`merge`].
pub fn rebase_onto(repo: &Path, name: &str) -> Result<()> {
    git(repo, &["rebase", name])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;

    /// Local `run`: cli.rs's test helper is private and cli.rs is not this task's file.
    fn run(dir: &Path, args: &[&str]) {
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
    }

    /// scratch repo + a bare "origin" it has pushed `main` to.
    fn repo_with_origin() -> (tempfile::TempDir, tempfile::TempDir) {
        let bare = tempfile::tempdir().unwrap();
        run(bare.path(), &["init", "--bare", "-b", "main"]);
        let work = scratch_repo();
        run(
            work.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        run(work.path(), &["push", "-u", "origin", "main"]);
        // create refs/remotes/origin/HEAD — otherwise "the pointer is skipped" is
        // an assertion about a ref the fixture does not have
        run(work.path(), &["remote", "set-head", "origin", "main"]);
        (work, bare)
    }

    fn node<'a>(nodes: &'a [BranchNode], name: &str) -> &'a BranchNode {
        nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("no node {name} in {:?}", nodes.iter().map(|n| &n.name).collect::<Vec<_>>()))
    }

    #[test]
    fn tree_reports_ahead_and_behind_against_upstream() {
        let (work, bare) = repo_with_origin();
        let p = work.path();

        assert_eq!(
            {
                let n = tree(p).unwrap();
                let m = node(&n, "main");
                (m.ahead, m.behind)
            },
            (Some(0), Some(0)),
            "in sync with the upstream is zeros, not absent counters"
        );

        // origin advances by two commits, from a second clone
        let other = tempfile::tempdir().unwrap();
        run(other.path(), &["clone", bare.path().to_str().unwrap(), "c"]);
        let c = other.path().join("c");
        run(&c, &["config", "user.email", "t@example.com"]);
        run(&c, &["config", "user.name", "Test"]);
        run(&c, &["config", "commit.gpgsign", "false"]);
        for n in ["r1", "r2"] {
            std::fs::write(c.join(n), "x\n").unwrap();
            run(&c, &["add", n]);
            run(&c, &["commit", "-m", n]);
        }
        run(&c, &["push", "origin", "main"]);

        // and the local side makes one commit of its own
        std::fs::write(p.join("local.txt"), "l\n").unwrap();
        run(p, &["add", "local.txt"]);
        run(p, &["commit", "-m", "local"]);
        run(p, &["fetch", "origin"]);

        let nodes = tree(p).unwrap();
        let main = node(&nodes, "main");
        assert!(main.is_current);
        assert!(!main.is_remote);
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (Some(1), Some(2)));
        assert!(main.last_commit_at > 0);

        let remote = node(&nodes, "origin/main");
        assert!(remote.is_remote);
        assert!(!remote.is_current);
        // `%(refname:short)` shortens refs/remotes/origin/HEAD to plain `origin`,
        // so the check has to name what actually arrives: the full ref and that name.
        assert!(
            !nodes.iter().any(|n| n.full_ref.ends_with("/HEAD") || n.name == "origin"),
            "the remote HEAD pointer is not a branch, got {:?}",
            nodes.iter().map(|n| (&n.name, &n.full_ref)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn branch_without_upstream_has_no_counters() {
        let dir = scratch_repo();
        run(dir.path(), &["branch", "solo"]);
        let nodes = tree(dir.path()).unwrap();
        let solo = node(&nodes, "solo");
        assert_eq!(solo.upstream, None);
        assert_eq!(solo.ahead, None, "absence of counters, not zeros");
        assert_eq!(solo.behind, None, "absence of counters, not zeros");
        assert_eq!(solo.full_ref, "refs/heads/solo");
    }

    #[test]
    fn detached_head_is_reported_as_its_own_node() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "--detach", "HEAD"]);

        let nodes = tree(p).unwrap();
        let head = &nodes[0];
        assert_eq!(head.full_ref, DETACHED_REF, "detached HEAD comes first");
        assert!(head.is_current);
        assert!(!head.is_remote);
        assert_eq!(head.upstream, None);
        let full = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(p)
                .args(["log", "-1", "--format=%H"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert!(head.name.len() >= 7, "the node names the revision: {}", head.name);
        assert!(
            head.name.len() < full.len(),
            "an abbreviated revision, not the whole hash"
        );
        assert!(
            full.starts_with(&head.name),
            "{} is not a prefix of {full}",
            head.name
        );
        assert!(
            !node(&nodes, "main").is_current,
            "no branch is current while HEAD is detached"
        );
    }

    /// two commits on `feat` that live nowhere else
    fn repo_with_unmerged_feat() -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "-b", "feat"]);
        for n in ["f1", "f2"] {
            std::fs::write(p.join(n), "x\n").unwrap();
            run(p, &["add", n]);
            run(p, &["commit", "-m", n]);
        }
        run(p, &["checkout", "main"]);
        dir
    }

    /// both branches touch the same line of the same file
    fn repo_with_conflict() -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "-b", "feat"]);
        std::fs::write(p.join("a.txt"), "from feat\n").unwrap();
        run(p, &["commit", "-am", "feat edit"]);
        run(p, &["checkout", "main"]);
        std::fs::write(p.join("a.txt"), "from main\n").unwrap();
        run(p, &["commit", "-am", "main edit"]);
        dir
    }

    #[test]
    fn rename_keeps_upstream_and_refuses_a_remote_branch() {
        let (work, _bare) = repo_with_origin();
        let p = work.path();

        rename(p, "main", "trunk").unwrap();
        let nodes = tree(p).unwrap();
        assert!(!nodes.iter().any(|n| n.name == "main" && !n.is_remote));
        let trunk = node(&nodes, "trunk");
        assert!(trunk.is_current);
        assert_eq!(
            trunk.upstream.as_deref(),
            Some("origin/main"),
            "renaming keeps the upstream it tracked"
        );

        let err = rename(p, "origin/main", "origin/other").unwrap_err();
        assert!(
            matches!(err, Error::Rule(ref m) if m.contains("origin/main")),
            "a remote branch is refused with a reason, got {err:?}"
        );
        assert!(
            tree(p).iter().flatten().any(|n| n.name == "origin/main"),
            "and is left alone"
        );
    }

    /// `git branch -d <name>` succeeds — i.e. git itself calls the branch merged.
    fn git_would_delete(dir: &Path, name: &str) -> bool {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["branch", "-d", name])
            .output()
            .unwrap();
        if out.status.success() {
            run(dir, &["branch", name, "@{-1}"]); // put it back for the next check
        }
        out.status.success()
    }

    #[test]
    fn unmerged_count_follows_gits_own_definition_of_merged() {
        let dir = repo_with_unmerged_feat();
        let p = dir.path();
        // hand-counted: f1 and f2 are reachable from `feat` and from nothing else
        assert_eq!(unmerged_count(p, "feat").unwrap(), 2);
        assert!(!git_would_delete(p, "feat"));

        // merged into another branch while HEAD is elsewhere: git still refuses,
        // so the count must still be 2 — one criterion, not two
        run(p, &["checkout", "-b", "release"]);
        run(p, &["merge", "--no-edit", "feat"]);
        run(p, &["checkout", "main"]);
        assert_eq!(
            unmerged_count(p, "feat").unwrap(),
            2,
            "merged into release but not into HEAD: git refuses, so must the count"
        );
        assert!(!git_would_delete(p, "feat"));

        // merged into HEAD: git deletes it, and the count says nothing is lost
        run(p, &["merge", "--no-edit", "feat"]);
        assert_eq!(unmerged_count(p, "feat").unwrap(), 0);
        assert!(git_would_delete(p, "feat"));
    }

    #[test]
    fn upstream_gone_leaves_counters_absent() {
        let (work, _bare) = repo_with_origin();
        let p = work.path();
        run(p, &["checkout", "-b", "side"]);
        run(p, &["push", "-u", "origin", "side"]);
        run(p, &["push", "origin", "--delete", "side"]);
        run(p, &["fetch", "--prune", "origin"]);

        let nodes = tree(p).unwrap();
        let side = node(&nodes, "side");
        assert_eq!(side.upstream.as_deref(), Some("origin/side"), "still configured");
        assert_eq!(side.ahead, None, "a gone upstream is unknown, not zero");
        assert_eq!(side.behind, None, "a gone upstream is unknown, not zero");
    }

    #[test]
    fn order_puts_the_current_branch_first_and_remotes_last() {
        let (work, _bare) = repo_with_origin();
        let p = work.path();
        run(p, &["branch", "aaa"]);
        run(p, &["branch", "zzz"]);

        let names: Vec<&str> = tree(p).unwrap().iter().map(|n| n.name.clone()).collect::<Vec<_>>().leak().iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            vec!["main", "aaa", "zzz", "origin/main"],
            "current first, then locals by name, remotes last"
        );
    }

    #[test]
    fn a_git_that_cannot_answer_is_an_error_not_a_lifted_guard() {
        let outside = tempfile::tempdir().unwrap(); // not a repository at all
        let err = delete(outside.path(), "main", false, false).unwrap_err();
        assert!(
            matches!(err, Error::Git { .. } | Error::Io(_)),
            "a failed git call must not read as \"not the current branch\": {err:?}"
        );
        // the two guard questions themselves: a git that could not answer errors,
        // it does not return the permissive answer
        assert!(
            current_branch(outside.path()).is_err(),
            "unanswerable HEAD must not read as \"detached, so any name may be deleted\""
        );
        assert!(
            ref_exists(outside.path(), "main", false).is_err(),
            "unanswerable ref lookup must not read as \"no such remote branch\""
        );
    }

    #[test]
    fn delete_refuses_current_and_unmerged_but_obeys_force() {
        let dir = repo_with_unmerged_feat();
        let p = dir.path();

        let err = delete(p, "main", false, false).unwrap_err();
        assert!(
            matches!(err, Error::Rule(ref m) if m.contains("main")),
            "the current branch cannot be deleted, got {err:?}"
        );

        let err = delete(p, "feat", false, false).unwrap_err();
        match err {
            Error::Rule(m) => assert!(m.contains('2'), "the count is available to the caller: {m}"),
            other => panic!("expected a domain rule, got {other:?}"),
        }
        assert!(tree(p).unwrap().iter().any(|n| n.name == "feat"));

        delete(p, "feat", false, true).unwrap();
        assert!(!tree(p).unwrap().iter().any(|n| n.name == "feat"));
    }

    #[test]
    fn delete_removes_a_remote_branch() {
        let (work, _bare) = repo_with_origin();
        let p = work.path();
        run(p, &["checkout", "-b", "side"]);
        run(p, &["push", "-u", "origin", "side"]);
        run(p, &["checkout", "main"]);
        assert!(tree(p).unwrap().iter().any(|n| n.name == "origin/side"));

        delete(p, "origin/side", true, false).unwrap();
        run(p, &["fetch", "--prune", "origin"]);
        assert!(!tree(p).unwrap().iter().any(|n| n.name == "origin/side"));
    }

    #[test]
    fn merge_joins_a_branch_and_a_conflict_stops_recognisably() {
        let clean = scratch_repo();
        let p = clean.path();
        run(p, &["checkout", "-b", "feat"]);
        std::fs::write(p.join("new.txt"), "n\n").unwrap();
        run(p, &["add", "new.txt"]);
        run(p, &["commit", "-m", "add new"]);
        run(p, &["checkout", "main"]);
        merge(p, "feat").unwrap();
        assert!(p.join("new.txt").exists(), "the merge landed");
        assert_eq!(
            crate::engine::ops::detect_state(p).unwrap().kind,
            crate::model::OperationKind::None
        );

        let dirty = repo_with_conflict();
        let q = dirty.path();
        let err = merge(q, "feat").unwrap_err();
        match err {
            Error::Git { stderr, .. } => assert!(
                stderr.contains("a.txt"),
                "git's own output reaches the caller whole: {stderr}"
            ),
            other => panic!("expected a git failure, got {other:?}"),
        }
        assert_eq!(
            crate::engine::ops::detect_state(q).unwrap().kind,
            crate::model::OperationKind::Merge,
            "the repository is left in an unfinished merge"
        );
    }

    #[test]
    fn rebase_replays_current_onto_a_branch_and_a_conflict_stops_recognisably() {
        let clean = scratch_repo();
        let p = clean.path();
        run(p, &["checkout", "-b", "feat"]);
        std::fs::write(p.join("f.txt"), "f\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["commit", "-m", "feat work"]);
        run(p, &["checkout", "main"]);
        std::fs::write(p.join("m.txt"), "m\n").unwrap();
        run(p, &["add", "m.txt"]);
        run(p, &["commit", "-m", "main work"]);
        run(p, &["checkout", "feat"]);
        rebase_onto(p, "main").unwrap();
        assert!(p.join("m.txt").exists(), "feat now sits on top of main");
        assert_eq!(
            crate::engine::ops::detect_state(p).unwrap().kind,
            crate::model::OperationKind::None
        );

        let dirty = repo_with_conflict();
        let q = dirty.path();
        run(q, &["checkout", "feat"]);
        let err = rebase_onto(q, "main").unwrap_err();
        match err {
            Error::Git { stderr, .. } => assert!(
                stderr.contains("a.txt"),
                "git's own output reaches the caller whole: {stderr}"
            ),
            other => panic!("expected a git failure, got {other:?}"),
        }
        assert_eq!(
            crate::engine::ops::detect_state(q).unwrap().kind,
            crate::model::OperationKind::Rebase,
            "the repository is left in an unfinished rebase"
        );
    }
}
