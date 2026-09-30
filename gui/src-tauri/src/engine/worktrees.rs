//! Worktrees: more than one checkout of the same repository, each in its own folder
//! on its own branch — fix something urgent on another branch without putting the
//! work in progress away in a stash.
//!
//! Graft has one window and one open repository, so "open a worktree" means
//! switching the window to that folder, the way "Open…" does; everything else in the
//! application then treats it as the repository (its own changelists and UI state
//! under `.git/worktrees/<name>/`, its own Undo chain and rebase plan keyed on its
//! root, the shared `refs/graft/discard` — see `CLAUDE.md`).
//!
//! What this module owns:
//!
//! * **The list** — `git worktree list --porcelain -z`: one record per worktree,
//!   attributes NUL-terminated, a record ended by an empty attribute. The first
//!   record is the main worktree (git's own order). An attribute before any
//!   `worktree` line is `Error::Parse`; an attribute this code does not know is
//!   skipped — git adds them over versions, and a new one is no reason to lose the
//!   list. "Current" is decided by comparing canonicalized paths: git prints real
//!   paths (`/private/var/…` on macOS) and the window may hold another spelling.
//! * **Add** — a new branch from a revision, or an existing local branch. The
//!   destination is an absolute path that is absent or an empty directory (git
//!   itself accepts both); a symlink there is refused, as in `remotes::clone`. The
//!   new branch name goes through `cli::check_branch_name` first, the start point
//!   must name a commit, and an existing branch must be a local branch: git reads a
//!   full `refs/heads/x` as "detach at that commit", so the short name is passed,
//!   after `--`. A branch already checked out in another worktree is refused before
//!   git runs, naming that worktree — git's own "is already used by worktree"
//!   stays the answer to a race.
//! * **Remove** — never the main worktree (it holds the repository) and never the one
//!   open in the window; a locked one only after unlocking; one whose folder is gone
//!   is pruned, not removed. `--force` throws away uncommitted and untracked files:
//!   the client asks [`dirty`] first and confirms that separately, and without
//!   `force` git refuses a dirty worktree. No backup is taken of what a forced
//!   removal discards — the confirmation says it is gone for good.
//! * **Lock / unlock / prune** — `git worktree lock|unlock|prune`, as they are.
//!
//! The worktree acted on is always looked up in a fresh list by the path the client
//! names, and git is given the path from that list — a stale dialog cannot point git
//! at a folder that is not a worktree.

use std::path::{Path, PathBuf};

use super::cli::check_branch_name;
use super::exec;
use crate::error::{Error, Result};
use crate::model::WorktreeInfo;

/// How many numbered variants of the suggested folder are tried before giving up.
const SUGGEST_TRIES: u32 = 100;

/// Longest folder name [`folder_name`] makes of a branch name.
const FOLDER_NAME_MAX: usize = 80;

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    exec::git(repo, args).run()?.checked_both()
}

/// Parse `git worktree list --porcelain -z`. `is_main` is set on the first record;
/// `is_current` is left false for [`list`] to decide.
pub(crate) fn parse_list(raw: &[u8]) -> Result<Vec<WorktreeInfo>> {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<WorktreeInfo> = Vec::new();
    let mut open = false;
    for attr in text.split('\0') {
        if attr.is_empty() {
            open = false;
            continue;
        }
        let (key, value) = match attr.split_once(' ') {
            Some((k, v)) => (k, Some(v)),
            None => (attr, None),
        };
        if key == "worktree" {
            let path = value
                .filter(|v| !v.is_empty())
                .ok_or_else(|| Error::Parse("worktree list: a record without a path".into()))?;
            out.push(WorktreeInfo {
                path: path.to_string(),
                head: None,
                branch: None,
                detached: false,
                bare: false,
                locked: false,
                lock_reason: None,
                prunable: false,
                prunable_reason: None,
                is_main: out.is_empty(),
                is_current: false,
            });
            open = true;
            continue;
        }
        let entry = match out.last_mut() {
            Some(e) if open => e,
            _ => {
                return Err(Error::Parse(format!(
                    "worktree list: {attr:?} outside a worktree record"
                )))
            }
        };
        let reason = || value.filter(|v| !v.is_empty()).map(str::to_string);
        match key {
            "HEAD" => {
                // An unborn branch prints the zero id: there is no commit.
                entry.head = value
                    .filter(|v| !v.is_empty() && v.bytes().any(|b| b != b'0'))
                    .map(str::to_string);
            }
            "branch" => {
                let full = value.unwrap_or_default();
                entry.branch = Some(full.strip_prefix("refs/heads/").unwrap_or(full).to_string());
            }
            "detached" => entry.detached = true,
            "bare" => entry.bare = true,
            "locked" => {
                entry.locked = true;
                entry.lock_reason = reason();
            }
            "prunable" => {
                entry.prunable = true;
                entry.prunable_reason = reason();
            }
            _ => {}
        }
    }
    Ok(out)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Every worktree of the repository, the main one first, the one at `repo` marked.
pub fn list(repo: &Path) -> Result<Vec<WorktreeInfo>> {
    let out = exec::git(repo, &["worktree", "list", "--porcelain", "-z"])
        .run()?
        .checked()?;
    let mut all = parse_list(&out)?;
    let here = canonical(repo);
    for w in &mut all {
        w.is_current = canonical(Path::new(&w.path)) == here;
    }
    Ok(all)
}

/// The main worktree's path when `repo` is a **linked** worktree, else `None`.
///
/// A `.git` directory answers without starting git (`build_state` asks on every
/// refresh). A `.git` file is a linked worktree **or a submodule** — the list tells
/// them apart: a submodule is the first (main) record of its own list.
pub fn main_of(repo: &Path) -> Result<Option<String>> {
    if repo.join(".git").is_dir() {
        return Ok(None);
    }
    let all = list(repo)?;
    Ok(match all.iter().find(|w| w.is_current) {
        Some(w) if !w.is_main => all.first().map(|m| m.path.clone()),
        _ => None,
    })
}

/// A folder name for a branch: `feature/login fix` → `feature-login-fix`. Letters
/// (any script), digits, `.`, `_`, `-` stay; runs of anything else become one `-`;
/// no leading or trailing `-` / `.`; at most [`FOLDER_NAME_MAX`] characters.
pub fn folder_name(branch: &str) -> String {
    let mut s = String::new();
    for c in branch.chars() {
        if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let trimmed: String = s
        .trim_matches(|c| c == '-' || c == '.')
        .chars()
        .take(FOLDER_NAME_MAX)
        .collect();
    let trimmed = trimmed.trim_end_matches(['-', '.']);
    if trimmed.is_empty() {
        "worktree".into()
    } else {
        trimmed.to_string()
    }
}

/// Where a new worktree for `branch` goes unless the user picks another folder:
/// next to the main worktree, `<main folder>-<branch folder name>`, numbered
/// (`-2`, `-3`, …) while that name is taken.
pub fn suggest_path(repo: &Path, branch: &str) -> Result<String> {
    let all = list(repo)?;
    let main = all
        .first()
        .map(|m| PathBuf::from(&m.path))
        .unwrap_or_else(|| repo.to_path_buf());
    let parent = main
        .parent()
        .ok_or_else(|| Error::Rule("the main worktree has no parent folder".into()))?;
    let stem = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let base = format!("{stem}-{}", folder_name(branch));
    for n in 1..SUGGEST_TRIES {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let candidate = parent.join(name);
        if std::fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate.display().to_string());
        }
    }
    Err(Error::Rule("choose a folder for the new worktree".into()))
}

/// Refuse a destination git would not take, or one that is not ours to fill.
fn check_destination(path: &str) -> Result<()> {
    let p = Path::new(path);
    if path.is_empty() || !p.is_absolute() {
        return Err(Error::Rule(format!(
            "\"{path}\" is not an absolute folder path"
        )));
    }
    match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(format!("{path}: {e}"))),
        Ok(m) if m.file_type().is_symlink() => Err(Error::Rule(format!(
            "{path} is a symbolic link; choose a real folder"
        ))),
        Ok(m) if !m.is_dir() => Err(Error::Rule(format!("{path} exists and is not a folder"))),
        Ok(_) => {
            let mut entries =
                std::fs::read_dir(p).map_err(|e| Error::Io(format!("{path}: {e}")))?;
            if entries.next().is_some() {
                Err(Error::Rule(format!(
                    "{path} already exists and is not empty; choose another folder"
                )))
            } else {
                Ok(())
            }
        }
    }
}

/// Whether a local branch `name` exists — `show-ref` answers 0 / 1, anything else is
/// a question git failed to answer.
fn local_branch_exists(repo: &Path, name: &str) -> Result<bool> {
    let full = format!("refs/heads/{name}");
    let out = exec::git(repo, &["show-ref", "--verify", "--quiet", full.as_str()]).run()?;
    match out.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(out.fail_stderr()),
    }
}

/// Add a worktree at `path`: on a new branch `branch` started at `start` (`None` —
/// `HEAD`) when `create`, else on the existing local branch `branch`.
pub fn add(repo: &Path, path: &str, branch: &str, create: bool, start: Option<&str>) -> Result<()> {
    check_destination(path)?;
    if create {
        check_branch_name(repo, branch)?;
        if local_branch_exists(repo, branch)? {
            return Err(Error::Rule(format!(
                "a branch named \"{branch}\" already exists; open it as an existing branch"
            )));
        }
        let start = start.filter(|s| !s.is_empty()).unwrap_or("HEAD");
        let probe = format!("{start}^{{commit}}");
        let out = exec::git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                probe.as_str(),
            ],
        )
        .run()?;
        if !out.success() {
            return Err(Error::Rule(format!("\"{start}\" does not name a commit")));
        }
        // Passed as given (after `--`, so it cannot be an option): a remote-tracking
        // start point then sets the new branch's upstream, as `git branch` would.
        git(repo, &["worktree", "add", "-b", branch, "--", path, start])?;
    } else {
        if branch.is_empty() || !local_branch_exists(repo, branch)? {
            return Err(Error::Rule(format!(
                "there is no local branch \"{branch}\""
            )));
        }
        if let Some(w) = list(repo)?
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(branch))
        {
            return Err(Error::Rule(format!(
                "\"{branch}\" is already checked out in the worktree at {}; a branch can be \
                 checked out in one worktree at a time",
                w.path
            )));
        }
        git(repo, &["worktree", "add", "--", path, branch])?;
    }
    Ok(())
}

/// The worktree the client named, looked up in the list git gives now.
fn find(repo: &Path, path: &str) -> Result<WorktreeInfo> {
    let wanted = canonical(Path::new(path));
    list(repo)?
        .into_iter()
        .find(|w| w.path == path || canonical(Path::new(&w.path)) == wanted)
        .ok_or_else(|| Error::Rule(format!("{path} is not a worktree of this repository")))
}

/// Whether the worktree at `path` has uncommitted changes or untracked files — what
/// makes `git worktree remove` refuse without `--force`. Ignored files do not count,
/// as they do not for git.
pub fn dirty(repo: &Path, path: &str) -> Result<bool> {
    let w = find(repo, path)?;
    if w.prunable || w.bare {
        return Ok(false);
    }
    let out = exec::git(
        Path::new(&w.path),
        &["status", "--porcelain", "-z", "--ignore-submodules=none"],
    )
    .run()?
    .checked()?;
    Ok(!out.is_empty())
}

/// Remove the worktree at `path` (its folder; the branch and its commits stay).
pub fn remove(repo: &Path, path: &str, force: bool) -> Result<()> {
    let w = find(repo, path)?;
    if w.is_main {
        return Err(Error::Rule(
            "the main worktree cannot be removed: it holds the repository itself".into(),
        ));
    }
    if w.is_current {
        return Err(Error::Rule(
            "this worktree is open in the window; open another one to remove it".into(),
        ));
    }
    if w.locked {
        return Err(Error::Rule(format!(
            "{} is locked; unlock it first",
            w.path
        )));
    }
    if w.prunable {
        return Err(Error::Rule(format!(
            "the folder of {} is gone; clean up missing worktrees instead",
            w.path
        )));
    }
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", w.path.as_str()]);
    git(repo, &args)?;
    Ok(())
}

/// Lock the worktree at `path` (`git worktree lock`), with an optional reason.
pub fn lock(repo: &Path, path: &str, reason: Option<&str>) -> Result<()> {
    let w = find(repo, path)?;
    let flag = reason
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| format!("--reason={r}"));
    let mut args = vec!["worktree", "lock"];
    if let Some(f) = flag.as_deref() {
        args.push(f);
    }
    args.extend(["--", w.path.as_str()]);
    git(repo, &args)?;
    Ok(())
}

/// Unlock the worktree at `path`.
pub fn unlock(repo: &Path, path: &str) -> Result<()> {
    let w = find(repo, path)?;
    git(repo, &["worktree", "unlock", "--", w.path.as_str()])?;
    Ok(())
}

/// Forget worktrees whose folders are gone (`git worktree prune`). Locked ones stay.
pub fn prune(repo: &Path) -> Result<()> {
    git(repo, &["worktree", "prune"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::{run_git, scratch_repo};

    struct Fixture {
        main: tempfile::TempDir,
        outer: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                main: scratch_repo(),
                outer: tempfile::tempdir().unwrap(),
            }
        }
        fn repo(&self) -> &Path {
            self.main.path()
        }
        fn at(&self, name: &str) -> String {
            self.outer
                .path()
                .canonicalize()
                .unwrap()
                .join(name)
                .display()
                .to_string()
        }
    }

    fn by_path<'a>(all: &'a [WorktreeInfo], path: &str) -> &'a WorktreeInfo {
        let want = canonical(Path::new(path));
        all.iter()
            .find(|w| canonical(Path::new(&w.path)) == want)
            .unwrap_or_else(|| panic!("{path} not in {all:?}"))
    }

    fn rule(r: Result<()>) -> String {
        match r {
            Err(Error::Rule(m)) => m,
            other => panic!("expected a rule refusal, got {other:?}"),
        }
    }

    #[test]
    fn parses_every_attribute_and_skips_unknown_ones() {
        let raw = b"worktree /r\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0\
worktree /w1\0HEAD 2222222222222222222222222222222222222222\0detached\0locked\0\0\
worktree /w2\0HEAD 0000000000000000000000000000000000000000\0branch refs/heads/x\0locked on usb\0prunable gitdir file points to non-existent location\0future-key 1\0\0";
        let all = parse_list(raw).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all[0].is_main && !all[1].is_main && !all[2].is_main);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
        assert!(all[1].detached && all[1].locked && all[1].lock_reason.is_none());
        assert_eq!(all[2].head, None, "the zero id is an unborn branch");
        assert_eq!(all[2].lock_reason.as_deref(), Some("on usb"));
        assert!(all[2].prunable);
        assert_eq!(
            all[2].prunable_reason.as_deref(),
            Some("gitdir file points to non-existent location")
        );
    }

    /// An attribute git put before any `worktree` line is not silently dropped.
    #[test]
    fn an_attribute_outside_a_record_is_a_parse_error() {
        assert!(matches!(parse_list(b"HEAD abc\0\0"), Err(Error::Parse(_))));
        assert!(matches!(
            parse_list(b"worktree /r\0\0detached\0\0"),
            Err(Error::Parse(_))
        ));
    }

    /// Main + linked branch + detached + locked + prunable (folder deleted by hand).
    #[test]
    fn lists_main_linked_detached_locked_and_prunable() {
        let f = Fixture::new();
        let r = f.repo();
        let (w1, w2, w3) = (f.at("w1"), f.at("w2"), f.at("w3"));
        run_git(r, &["worktree", "add", "-q", "-b", "side", "--", &w1]);
        run_git(r, &["worktree", "add", "-q", "--detach", "--", &w2, "HEAD"]);
        run_git(r, &["worktree", "add", "-q", "-b", "gone", "--", &w3]);
        run_git(r, &["worktree", "lock", "--reason", "on usb", "--", &w1]);
        std::fs::remove_dir_all(&w3).unwrap();

        let all = list(r).unwrap();
        assert_eq!(all.len(), 4);
        let main = &all[0];
        assert!(main.is_main && main.is_current);
        assert_eq!(main.branch.as_deref(), Some("main"));
        assert!(main.head.is_some());

        let a = by_path(&all, &w1);
        assert_eq!(a.branch.as_deref(), Some("side"));
        assert!(!a.is_main && !a.is_current && !a.detached);
        assert!(a.locked);
        assert_eq!(a.lock_reason.as_deref(), Some("on usb"));

        let b = by_path(&all, &w2);
        assert!(b.detached && b.branch.is_none() && b.head.is_some());

        let c = by_path(&all, &w3);
        assert!(c.prunable && c.prunable_reason.is_some());

        // Seen from a linked worktree, that one is current and the main is not.
        let from_linked = list(Path::new(&w2)).unwrap();
        assert!(by_path(&from_linked, &w2).is_current);
        assert!(!from_linked[0].is_current);
        let owner = main_of(Path::new(&w2)).unwrap();
        assert_eq!(owner.map(|p| canonical(Path::new(&p))), Some(canonical(r)));
        assert_eq!(main_of(r).unwrap(), None);
    }

    #[test]
    fn adds_a_new_branch_from_a_revision() {
        let f = Fixture::new();
        let r = f.repo();
        std::fs::write(r.join("b.txt"), "b\n").unwrap();
        run_git(r, &["add", "b.txt"]);
        run_git(r, &["commit", "-q", "-m", "second"]);
        let dest = f.at("hot");
        add(r, &dest, "hotfix", true, Some("HEAD~1")).unwrap();

        let w = by_path(&list(r).unwrap(), &dest).clone();
        assert_eq!(w.branch.as_deref(), Some("hotfix"));
        assert!(Path::new(&dest).join("a.txt").exists());
        assert!(
            !Path::new(&dest).join("b.txt").exists(),
            "started at HEAD~1"
        );
    }

    /// "Open in new worktree…" on a remote branch: a new local branch at the
    /// remote-tracking ref, which then tracks it — the start point is passed as
    /// given, not resolved to a hash (that would lose the upstream).
    #[test]
    fn a_remote_start_point_sets_the_upstream() {
        let upstream = scratch_repo();
        run_git(upstream.path(), &["branch", "feat"]);
        let outer = tempfile::tempdir().unwrap();
        let clone = outer.path().join("clone");
        let (src, dst) = (upstream.path().to_str().unwrap(), clone.to_str().unwrap());
        run_git(outer.path(), &["clone", "-q", src, dst]);

        let dest = outer.path().join("feat").display().to_string();
        add(
            &clone,
            &dest,
            "feat",
            true,
            Some("refs/remotes/origin/feat"),
        )
        .unwrap();
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "origin/feat");
    }

    #[test]
    fn adds_an_existing_branch_by_its_short_name() {
        let f = Fixture::new();
        let r = f.repo();
        run_git(r, &["branch", "feat"]);
        let dest = f.at("feat");
        add(r, &dest, "feat", false, None).unwrap();
        let w = by_path(&list(r).unwrap(), &dest).clone();
        assert_eq!(
            w.branch.as_deref(),
            Some("feat"),
            "checked out, not detached"
        );
        assert!(!w.detached);
    }

    /// A branch checked out elsewhere is refused before git runs, naming where.
    #[test]
    fn refuses_a_branch_checked_out_in_another_worktree() {
        let f = Fixture::new();
        let r = f.repo();
        run_git(r, &["branch", "feat"]);
        let first = f.at("one");
        add(r, &first, "feat", false, None).unwrap();

        let m = rule(add(r, &f.at("two"), "feat", false, None));
        assert!(
            m.contains("already checked out") && m.contains("one"),
            "{m}"
        );
        assert!(!Path::new(&f.at("two")).exists(), "nothing was created");

        // The main worktree's own branch too.
        let m = rule(add(r, &f.at("three"), "main", false, None));
        assert!(m.contains("already checked out"), "{m}");
    }

    #[test]
    fn refuses_bad_destinations_names_and_start_points() {
        let f = Fixture::new();
        let r = f.repo();
        assert!(rule(add(r, "relative/dir", "x", true, None)).contains("absolute"));

        let full = f.at("full");
        std::fs::create_dir_all(&full).unwrap();
        std::fs::write(Path::new(&full).join("f"), "x").unwrap();
        assert!(rule(add(r, &full, "x", true, None)).contains("not empty"));

        let empty = f.at("empty");
        std::fs::create_dir_all(&empty).unwrap();
        add(r, &empty, "into-empty", true, None).unwrap();

        assert!(rule(add(r, &f.at("n1"), "-x", true, None)).contains("valid branch name"));
        assert!(rule(add(r, &f.at("n2"), "main", true, None)).contains("already exists"));
        assert!(rule(add(r, &f.at("n3"), "y", true, Some("no-such-rev"))).contains("commit"));
        assert!(rule(add(r, &f.at("n4"), "nope", false, None)).contains("no local branch"));
    }

    #[test]
    fn removes_a_clean_worktree_and_refuses_a_dirty_one_without_force() {
        let f = Fixture::new();
        let r = f.repo();
        let clean = f.at("clean");
        let dirty_at = f.at("dirty");
        add(r, &clean, "c", true, None).unwrap();
        add(r, &dirty_at, "d", true, None).unwrap();
        std::fs::write(Path::new(&dirty_at).join("new.txt"), "untracked\n").unwrap();

        assert!(!dirty(r, &clean).unwrap());
        assert!(dirty(r, &dirty_at).unwrap());

        remove(r, &clean, false).unwrap();
        assert!(!Path::new(&clean).exists());
        assert_eq!(list(r).unwrap().len(), 2);

        match remove(r, &dirty_at, false) {
            Err(Error::Git { stderr, .. }) => {
                assert!(stderr.contains("modified or untracked"), "{stderr}")
            }
            other => panic!("expected git's refusal, got {other:?}"),
        }
        assert!(
            Path::new(&dirty_at).join("new.txt").exists(),
            "nothing lost"
        );
        remove(r, &dirty_at, true).unwrap();
        assert!(!Path::new(&dirty_at).exists());

        let branches = String::from_utf8(
            std::process::Command::new("git")
                .arg("-C")
                .arg(r)
                .args(["branch", "--format=%(refname:short)"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(
            branches.lines().any(|b| b == "d"),
            "the branch stays: {branches}"
        );
    }

    #[test]
    fn refuses_to_remove_the_main_current_locked_or_missing_one() {
        let f = Fixture::new();
        let r = f.repo();
        let main_path = list(r).unwrap()[0].path.clone();
        assert!(rule(remove(r, &main_path, true)).contains("main worktree"));

        let linked = f.at("linked");
        add(r, &linked, "l", true, None).unwrap();
        assert!(rule(remove(Path::new(&linked), &linked, true)).contains("open in the window"));

        lock(r, &linked, Some("usb")).unwrap();
        assert!(rule(remove(r, &linked, true)).contains("locked"));
        unlock(r, &linked).unwrap();
        assert!(!by_path(&list(r).unwrap(), &linked).locked);

        let gone = f.at("gone");
        add(r, &gone, "g", true, None).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        assert!(rule(remove(r, &gone, false)).contains("gone"));

        assert!(rule(remove(r, "/no/such/worktree", false)).contains("not a worktree"));
    }

    #[test]
    fn prune_forgets_missing_folders_but_keeps_locked_ones() {
        let f = Fixture::new();
        let r = f.repo();
        let (gone, kept) = (f.at("gone"), f.at("kept"));
        add(r, &gone, "g", true, None).unwrap();
        add(r, &kept, "k", true, None).unwrap();
        lock(r, &kept, None).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        std::fs::remove_dir_all(&kept).unwrap();

        prune(r).unwrap();
        let all = list(r).unwrap();
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(by_path(&all, &kept).locked);
    }

    #[test]
    fn folder_names_and_suggested_paths() {
        assert_eq!(folder_name("feature/login fix"), "feature-login-fix");
        assert_eq!(folder_name("-.x//y.-"), "x-y");
        assert_eq!(folder_name("фича/вход"), "фича-вход");
        assert_eq!(folder_name("///"), "worktree");

        let f = Fixture::new();
        let r = f.repo();
        let real = canonical(r);
        let stem = real.file_name().unwrap().to_string_lossy().to_string();
        let first = suggest_path(r, "feat/x").unwrap();
        assert_eq!(
            Path::new(&first),
            real.parent().unwrap().join(format!("{stem}-feat-x"))
        );
        std::fs::create_dir_all(&first).unwrap();
        let second = suggest_path(r, "feat/x").unwrap();
        assert!(second.ends_with(&format!("{stem}-feat-x-2")), "{second}");
        std::fs::remove_dir(&first).unwrap();
    }
}
