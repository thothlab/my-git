//! Git engine (ТЗ §2.1).
//!
//! The engine boundary is the `GitEngine` trait. `GixEngine` uses `gix` to
//! discover the repository and shells out to the system `git` CLI for the
//! operations — the pragmatic, maximally-real backend ("работа на реальном
//! git"). The trait keeps this invisible to callers, so hot read paths can move
//! to `gix`/`git2` later without touching the UI. Shelling out to short-lived
//! `git` processes does not affect the resident-memory / instant-start goal
//! (AC#7): the TUI process stays tiny.

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// How many recent git invocations the command log keeps.
const CMD_LOG_CAP: usize = 400;

/// One recorded `git` invocation, for the command-log window. stdout is not
/// stored (diffs/logs are large and irrelevant to diagnosing failures).
#[derive(Debug, Clone)]
pub struct CmdEntry {
    /// The arguments, e.g. `rebase develop`.
    pub command: String,
    pub ok: bool,
    pub code: Option<i32>,
    /// Trimmed stderr (the detailed error on failure).
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl FileStatus {
    /// One-letter code shown in the Changes panel.
    pub fn letter(self) -> char {
        match self {
            FileStatus::Modified => 'M',
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Untracked => '?',
            FileStatus::Conflicted => 'C',
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub hash: String,
    pub summary: String,
    pub author: String,
    pub refs: Vec<String>,
}

/// One changed file within a commit (name-status vs its first parent).
#[derive(Debug, Clone)]
pub struct CommitFile {
    pub status: char,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, Default)]
pub struct PushOpts {
    pub set_upstream: bool,
    pub force: bool,
    pub force_with_lease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseState {
    pub current: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default)]
pub struct BranchState {
    pub current_branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub rebase: Option<RebaseState>,
    pub detached: bool,
}

/// The operation surface from the PRD "API list". Callers depend on observable
/// behaviour, not on which backend services a call.
pub trait GitEngine {
    fn status(&self) -> Result<Vec<ChangedFile>>;
    fn diff(&self, path: &str) -> Result<String>;
    fn stage(&self, paths: &[String]) -> Result<()>;
    fn commit(&self, paths: &[String], message: &str, amend: bool) -> Result<String>;
    fn log(&self, limit: usize) -> Result<Vec<Commit>>;
    fn revert(&self, hash: &str) -> Result<()>;
    fn reset(&self, hash: &str, mode: ResetMode) -> Result<()>;
    fn checkout_file(&self, path: &str) -> Result<()>;
    fn branch_state(&self) -> Result<BranchState>;
    fn branches(&self) -> Result<Vec<String>>;
    fn remote_branches(&self) -> Result<Vec<String>>;
    fn log_for(&self, refname: &str, limit: usize) -> Result<Vec<Commit>>;
    fn commit_files(&self, hash: &str) -> Result<Vec<CommitFile>>;
    fn commit_body(&self, hash: &str) -> Result<String>;
    fn commit_file_diff(&self, hash: &str, path: &str) -> Result<String>;
    fn checkout_branch(&self, name: &str) -> Result<()>;
    fn create_branch(&self, name: &str, from: &str) -> Result<()>;
    fn push(&self, branch: &str, opts: &PushOpts) -> Result<()>;
    fn fetch(&self) -> Result<()>;
    /// Reserved API (PRD "API list"); the UI currently uses fetch for the safe
    /// incremental path. Wired to a key in a later iteration.
    #[allow(dead_code)]
    fn pull(&self) -> Result<()>;
    /// Stash the working tree (incl. untracked) under a name — the "shelve" op.
    fn stash_push(&self, message: &str) -> Result<()>;
    /// True if `hash` is reachable from HEAD (on the current branch's history).
    fn is_on_head(&self, hash: &str) -> bool;
    /// Reword (change the message of) any commit on the current branch via an
    /// interactive rebase. Saves an undo point first.
    fn reword_commit(&self, hash: &str, message: &str) -> Result<()>;
    /// Squash a commit into its parent (message = the combined message). Saves an
    /// undo point first.
    fn squash_into_parent(&self, hash: &str, message: &str) -> Result<()>;
    /// Squash a contiguous set of commits on the current branch into one (the
    /// oldest is kept as the base; message = combined). Saves an undo point first.
    fn squash_commits(&self, hashes: &[String], message: &str) -> Result<()>;
    /// Drop a commit from the current branch's history via interactive rebase.
    /// Saves an undo point first; aborts cleanly if dropping would conflict.
    fn drop_commit(&self, hash: &str) -> Result<()>;
    /// Cherry-pick a commit (from any branch) onto HEAD. Saves an undo point
    /// first; on conflict the cherry-pick is left in progress for the conflict flow.
    fn cherry_pick(&self, hash: &str) -> Result<()>;
    /// Save the current HEAD as the undo point before a history rewrite.
    fn backup_head(&self) -> Result<()>;
    /// Whether an undo point exists.
    fn has_backup(&self) -> bool;
    /// Reset HEAD back to the saved undo point.
    fn restore_backup(&self) -> Result<()>;
    /// List stashes as `(selector, subject)` pairs, e.g. `("stash@{0}", "On main: wip")`.
    fn stash_list(&self) -> Result<Vec<(String, String)>>;
    fn stash_apply(&self, stash: &str) -> Result<()>;
    fn stash_pop(&self, stash: &str) -> Result<()>;
    fn stash_drop(&self, stash: &str) -> Result<()>;
    /// Rebase the current branch onto `target`, auto-stashing any local (tracked)
    /// changes first and restoring them after (so a dirty tree doesn't block it).
    fn rebase_onto(&self, target: &str) -> Result<()>;
    /// Fetch a remote-tracking branch (e.g. `origin/develop`) and rebase the
    /// current branch onto the freshly-updated ref (auto-stash, like `rebase_onto`).
    fn fetch_and_rebase_onto(&self, remote_ref: &str) -> Result<()>;
    fn rebase_continue(&self) -> Result<()>;
    fn rebase_skip(&self) -> Result<()>;
    fn rebase_abort(&self) -> Result<()>;
    fn conflicts(&self) -> Result<Vec<String>>;
    /// A snapshot of recent `git` invocations (oldest first) for the command-log
    /// window. Default returns nothing so lightweight engines need not track it.
    fn command_log(&self) -> Vec<CmdEntry> {
        Vec::new()
    }
    /// The sequencer operation currently stopped in the working tree, if any:
    /// `"rebase"`, `"cherry-pick"`, or `"revert"`. Used to drive the LOG conflict flow.
    fn op_in_progress(&self) -> Option<&'static str>;
    /// Continue / skip / abort whichever sequencer op is in progress.
    fn op_continue(&self) -> Result<()>;
    fn op_skip(&self) -> Result<()>;
    fn op_abort(&self) -> Result<()>;
    fn repo_root(&self) -> &Path;
}

/// Engine backed by `gix` discovery + `git` CLI operations.
pub struct GixEngine {
    #[allow(dead_code)]
    repo: gix::Repository,
    root: PathBuf,
    /// Ring buffer of recent git invocations for the command-log window.
    cmdlog: RefCell<VecDeque<CmdEntry>>,
}

impl GixEngine {
    /// Discover the repository containing `dir`. Errors when `dir` is not inside
    /// a git repository — the caller turns this into the AC#1 non-repo message.
    pub fn discover(dir: &Path) -> Result<Self> {
        let repo = gix::discover(dir)?;
        let root = repo
            .work_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repo.git_dir().to_path_buf());
        Ok(Self {
            repo,
            root,
            cmdlog: RefCell::new(VecDeque::new()),
        })
    }

    /// Record one git invocation in the ring buffer.
    fn record(&self, command: String, out: &std::process::Output) {
        let mut log = self.cmdlog.borrow_mut();
        if log.len() >= CMD_LOG_CAP {
            log.pop_front();
        }
        log.push_back(CmdEntry {
            command,
            ok: out.status.success(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }

    /// Run `git` in the repo. `GIT_EDITOR`/`GIT_SEQUENCE_EDITOR` are neutralised
    /// (we supply messages ourselves) and terminal prompts are disabled so a
    /// missing credential fails fast instead of hanging the TUI.
    fn git(&self, args: &[&str]) -> Result<std::process::Output> {
        let out = std::process::Command::new("git")
            .current_dir(&self.root)
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .output()
            .with_context(|| format!("running `git {}`", args.join(" ")))?;
        self.record(args.join(" "), &out);
        Ok(out)
    }

    /// Run `git`, error on non-zero exit, return stdout as a `String`.
    fn git_check(&self, args: &[&str]) -> Result<String> {
        let out = self.git(args)?;
        anyhow::ensure!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn git_dir(&self) -> Option<PathBuf> {
        self.git_check(&["rev-parse", "--absolute-git-dir"])
            .ok()
            .map(|s| PathBuf::from(s.trim()))
    }

    fn write_temp(&self, name: &str, content: &str) -> Result<PathBuf> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("mygit-{}-{n}-{name}", std::process::id()));
        std::fs::write(&p, content).with_context(|| format!("writing {}", p.display()))?;
        Ok(p)
    }

    /// Run a non-interactive `git rebase -i` by feeding a prebuilt todo file via
    /// `GIT_SEQUENCE_EDITOR=cp <todo>` and (optionally) a commit message via
    /// `GIT_EDITOR=cp <msg>`. On failure the rebase is aborted so the tree is
    /// left clean.
    /// `base` is the rebase base ref, or the literal `"--root"` to rewrite from the
    /// root commit.
    fn run_rebase_todo(&self, base: &str, todo: &str, message: Option<&str>) -> Result<()> {
        let todo_path = self.write_temp("rebase-todo", todo)?;
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(&self.root)
            .env(
                "GIT_SEQUENCE_EDITOR",
                format!("cp \"{}\"", todo_path.display()),
            )
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["rebase", "-i", "--autostash", base]);
        let msg_path = match message {
            Some(m) => {
                let p = self.write_temp("rebase-msg", m)?;
                cmd.env("GIT_EDITOR", format!("cp \"{}\"", p.display()));
                Some(p)
            }
            None => {
                cmd.env("GIT_EDITOR", "true");
                None
            }
        };
        let out = cmd.output().context("running git rebase -i")?;
        self.record(
            format!("rebase -i --autostash {base} (scripted todo)"),
            &out,
        );
        let _ = std::fs::remove_file(&todo_path);
        if let Some(p) = &msg_path {
            let _ = std::fs::remove_file(p);
        }
        if !out.status.success() {
            let _ = self.git(&["rebase", "--abort"]);
            anyhow::bail!(
                "rebase failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Compute the interactive-rebase base and the ordered (oldest-first) commit
    /// list for a rewrite that includes `target`. Returns base `"--root"` when
    /// `target` has no parent (rewrite from the root commit).
    fn rebase_span(&self, target: &str) -> Result<(String, Vec<String>)> {
        let parent = format!("{target}^");
        let has_parent = self
            .git(&["rev-parse", "--verify", "--quiet", &parent])?
            .status
            .success();
        let (base, range) = if has_parent {
            (parent.clone(), format!("{parent}..HEAD"))
        } else {
            ("--root".to_string(), "HEAD".to_string())
        };
        let list = self.git_check(&["rev-list", "--reverse", &range])?;
        let commits = list
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        Ok((base, commits))
    }

    fn detect_rebase(&self) -> Option<RebaseState> {
        let git = self.git_dir()?;
        let rm = git.join("rebase-merge");
        if rm.exists() {
            return Some(RebaseState {
                current: read_num(&rm.join("msgnum")).unwrap_or(0),
                total: read_num(&rm.join("end")).unwrap_or(0),
            });
        }
        let ra = git.join("rebase-apply");
        if ra.exists() {
            return Some(RebaseState {
                current: read_num(&ra.join("next")).unwrap_or(0),
                total: read_num(&ra.join("last")).unwrap_or(0),
            });
        }
        None
    }
}

impl GitEngine for GixEngine {
    fn status(&self) -> Result<Vec<ChangedFile>> {
        let out = self.git(&["status", "--porcelain", "-z", "--untracked-files=all"])?;
        anyhow::ensure!(
            out.status.success(),
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(parse_porcelain_z(&out.stdout))
    }

    fn diff(&self, path: &str) -> Result<String> {
        // vs HEAD covers staged+unstaged; fall back to worktree diff, then to an
        // untracked file rendered as an addition.
        for args in [
            vec!["diff", "--no-color", "HEAD", "--", path],
            vec!["diff", "--no-color", "--", path],
        ] {
            let out = self.git(&args)?;
            if out.status.success() && !out.stdout.is_empty() {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
        // Untracked: `--no-index` returns exit 1 when files differ; take stdout.
        let out = self.git(&["diff", "--no-color", "--no-index", "--", "/dev/null", path])?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn stage(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.git_check(&args)?;
        Ok(())
    }

    fn commit(&self, paths: &[String], message: &str, amend: bool) -> Result<String> {
        self.stage(paths)?;
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
        }
        self.git_check(&args)?;
        Ok(self.git_check(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        self.log_for("HEAD", limit)
    }

    fn log_for(&self, refname: &str, limit: usize) -> Result<Vec<Commit>> {
        let n = format!("-n{limit}");
        let pretty = "--pretty=format:%H%x1f%s%x1f%an%x1f%D";
        // A missing ref / repo with no commits makes `git log` fail; treat as empty.
        let out = self.git(&["log", &n, pretty, refname])?;
        if !out.status.success() {
            return Ok(Vec::new());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let commits = text
            .lines()
            .filter_map(|line| {
                let mut f = line.split('\u{1f}');
                let hash = f.next()?.to_string();
                let summary = f.next().unwrap_or("").to_string();
                let author = f.next().unwrap_or("").to_string();
                let refs = f
                    .next()
                    .unwrap_or("")
                    .split(", ")
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                Some(Commit {
                    hash,
                    summary,
                    author,
                    refs,
                })
            })
            .collect();
        Ok(commits)
    }

    fn commit_files(&self, hash: &str) -> Result<Vec<CommitFile>> {
        // name-status of the commit vs its first parent.
        let text = self.git_check(&["show", "--no-color", "--name-status", "--format=", hash])?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                let mut parts = line.split('\t');
                let status = parts.next()?.chars().next().unwrap_or('?');
                // Rename/copy rows are "R100\told\tnew" — the new path is last.
                let path = parts.next_back()?.to_string();
                (!path.is_empty()).then_some(CommitFile { status, path })
            })
            .collect())
    }

    fn commit_body(&self, hash: &str) -> Result<String> {
        self.git_check(&["show", "-s", "--format=%B", hash])
    }

    fn commit_file_diff(&self, hash: &str, path: &str) -> Result<String> {
        let out = self.git(&["show", "--no-color", "--format=", hash, "--", path])?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn revert(&self, hash: &str) -> Result<()> {
        self.git_check(&["revert", "--no-edit", hash])?;
        Ok(())
    }

    fn reset(&self, hash: &str, mode: ResetMode) -> Result<()> {
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.git_check(&["reset", flag, hash])?;
        Ok(())
    }

    fn checkout_file(&self, path: &str) -> Result<()> {
        self.git_check(&["checkout", "HEAD", "--", path])?;
        Ok(())
    }

    fn branch_state(&self) -> Result<BranchState> {
        let mut st = BranchState::default();
        let head = self.git(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if head.status.success() {
            st.current_branch = Some(String::from_utf8_lossy(&head.stdout).trim().to_string());
        } else {
            st.detached = true;
            if let Ok(h) = self.git_check(&["rev-parse", "--short", "HEAD"]) {
                st.current_branch = Some(format!("detached@{}", h.trim()));
            }
        }
        if let Ok(up) = self.git_check(&[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ]) {
            st.upstream = Some(up.trim().to_string());
            if let Ok(counts) =
                self.git_check(&["rev-list", "--left-right", "--count", "@{upstream}...HEAD"])
            {
                let mut it = counts.split_whitespace();
                st.behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                st.ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
        st.rebase = self.detect_rebase();
        Ok(st)
    }

    fn branches(&self) -> Result<Vec<String>> {
        let text = self.git_check(&["branch", "--format=%(refname:short)"])?;
        Ok(text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    fn remote_branches(&self) -> Result<Vec<String>> {
        let text = self.git_check(&["branch", "-r", "--format=%(refname:short)"])?;
        Ok(text
            .lines()
            .map(|l| l.trim().to_string())
            // Drop empties and the "origin/HEAD -> origin/main" alias row.
            .filter(|l| !l.is_empty() && !l.ends_with("/HEAD") && !l.contains("->"))
            .collect())
    }

    fn checkout_branch(&self, name: &str) -> Result<()> {
        self.git_check(&["checkout", name])?;
        Ok(())
    }

    fn create_branch(&self, name: &str, from: &str) -> Result<()> {
        self.git_check(&["checkout", "-b", name, from])?;
        Ok(())
    }

    fn push(&self, branch: &str, opts: &PushOpts) -> Result<()> {
        let mut args: Vec<&str> = vec!["push"];
        if opts.set_upstream {
            args.push("-u");
        }
        if opts.force_with_lease {
            args.push("--force-with-lease");
        }
        if opts.force {
            args.push("--force");
        }
        args.push("origin");
        args.push(branch);
        self.git_check(&args)?;
        Ok(())
    }

    fn fetch(&self) -> Result<()> {
        self.git_check(&["fetch"])?;
        Ok(())
    }

    fn pull(&self) -> Result<()> {
        self.git_check(&["pull"])?;
        Ok(())
    }

    fn stash_push(&self, message: &str) -> Result<()> {
        self.git_check(&["stash", "push", "-u", "-m", message])?;
        Ok(())
    }

    fn is_on_head(&self, hash: &str) -> bool {
        self.git(&["merge-base", "--is-ancestor", hash, "HEAD"])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn backup_head(&self) -> Result<()> {
        self.git_check(&["update-ref", "refs/mygit/undo", "HEAD"])?;
        Ok(())
    }

    fn has_backup(&self) -> bool {
        self.git(&["rev-parse", "--verify", "--quiet", "refs/mygit/undo"])
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn restore_backup(&self) -> Result<()> {
        anyhow::ensure!(self.has_backup(), "nothing to undo");
        self.git_check(&["reset", "--hard", "refs/mygit/undo"])?;
        Ok(())
    }

    fn stash_list(&self) -> Result<Vec<(String, String)>> {
        let text = self.git_check(&["stash", "list", "--format=%gd%x1f%s"])?;
        Ok(text
            .lines()
            .filter_map(|l| {
                let mut f = l.splitn(2, '\u{1f}');
                let sel = f.next()?.trim().to_string();
                let msg = f.next().unwrap_or("").trim().to_string();
                (!sel.is_empty()).then_some((sel, msg))
            })
            .collect())
    }

    fn stash_apply(&self, stash: &str) -> Result<()> {
        self.git_check(&["stash", "apply", stash])?;
        Ok(())
    }

    fn stash_pop(&self, stash: &str) -> Result<()> {
        self.git_check(&["stash", "pop", stash])?;
        Ok(())
    }

    fn stash_drop(&self, stash: &str) -> Result<()> {
        self.git_check(&["stash", "drop", stash])?;
        Ok(())
    }

    fn reword_commit(&self, hash: &str, message: &str) -> Result<()> {
        anyhow::ensure!(self.is_on_head(hash), "commit is not on the current branch");
        let target = self.git_check(&["rev-parse", hash])?.trim().to_string();
        let (base, commits) = self.rebase_span(&target)?;
        let mut todo = String::new();
        for h in &commits {
            let action = if *h == target { "reword" } else { "pick" };
            todo.push_str(&format!("{action} {h}\n"));
        }
        self.backup_head()?;
        self.run_rebase_todo(&base, &todo, Some(message))
    }

    fn squash_into_parent(&self, hash: &str, message: &str) -> Result<()> {
        anyhow::ensure!(self.is_on_head(hash), "commit is not on the current branch");
        let target = self.git_check(&["rev-parse", hash])?.trim().to_string();
        // Squash the target into its parent: rewrite the parent's span, keeping the
        // parent as the base `pick` and turning the target into a `squash`. The
        // parent may itself be the root commit (base becomes `--root`).
        let parent = self.git(&["rev-parse", "--verify", "--quiet", &format!("{target}^")])?;
        anyhow::ensure!(
            parent.status.success(),
            "the root commit has no parent to squash into"
        );
        let parent = String::from_utf8_lossy(&parent.stdout).trim().to_string();
        let (base, commits) = self.rebase_span(&parent)?;
        let mut todo = String::new();
        for h in &commits {
            let action = if *h == target { "squash" } else { "pick" };
            todo.push_str(&format!("{action} {h}\n"));
        }
        self.backup_head()?;
        self.run_rebase_todo(&base, &todo, Some(message))
    }

    fn squash_commits(&self, hashes: &[String], message: &str) -> Result<()> {
        anyhow::ensure!(hashes.len() >= 2, "select at least two commits to squash");
        let mut set = std::collections::HashSet::new();
        for h in hashes {
            anyhow::ensure!(
                self.is_on_head(h),
                "a selected commit is not on the current branch"
            );
            set.insert(self.git_check(&["rev-parse", h])?.trim().to_string());
        }
        // The oldest selected commit anchors the rebase span; it stays a `pick`
        // and the rest squash into it.
        let all = self.git_check(&["rev-list", "--reverse", "HEAD"])?;
        let oldest = all
            .lines()
            .map(str::trim)
            .find(|h| set.contains(*h))
            .context("selected commits are not on the current branch")?
            .to_string();
        let (base, commits) = self.rebase_span(&oldest)?;
        // Contiguity: selected commits must occupy consecutive positions, else a
        // `pick` would land between `squash`es and squash into the wrong base.
        let positions: Vec<usize> = commits
            .iter()
            .enumerate()
            .filter(|(_, h)| set.contains(*h))
            .map(|(i, _)| i)
            .collect();
        anyhow::ensure!(
            positions.len() == set.len(),
            "some selected commits are outside the squash range"
        );
        anyhow::ensure!(
            positions.windows(2).all(|w| w[1] == w[0] + 1),
            "select adjacent commits to squash"
        );
        let mut todo = String::new();
        for h in &commits {
            let action = if *h != oldest && set.contains(h) {
                "squash"
            } else {
                "pick"
            };
            todo.push_str(&format!("{action} {h}\n"));
        }
        self.backup_head()?;
        self.run_rebase_todo(&base, &todo, Some(message))
    }

    fn drop_commit(&self, hash: &str) -> Result<()> {
        anyhow::ensure!(self.is_on_head(hash), "commit is not on the current branch");
        let target = self.git_check(&["rev-parse", hash])?.trim().to_string();
        let head = self.git_check(&["rev-parse", "HEAD"])?.trim().to_string();
        // Dropping the tip is just moving HEAD back to its parent.
        if target == head {
            let parent = self.git(&["rev-parse", "--verify", "--quiet", &format!("{target}^")])?;
            anyhow::ensure!(parent.status.success(), "cannot drop the only commit");
            let parent = String::from_utf8_lossy(&parent.stdout).trim().to_string();
            self.backup_head()?;
            self.git_check(&["reset", "--hard", &parent])?;
            return Ok(());
        }
        let (base, commits) = self.rebase_span(&target)?;
        let mut todo = String::new();
        for h in &commits {
            let action = if *h == target { "drop" } else { "pick" };
            todo.push_str(&format!("{action} {h}\n"));
        }
        self.backup_head()?;
        self.run_rebase_todo(&base, &todo, None)
    }

    fn cherry_pick(&self, hash: &str) -> Result<()> {
        self.backup_head()?;
        let out = self.git(&["cherry-pick", hash])?;
        if !out.status.success() {
            // On conflict the cherry-pick stays in progress for the LOG conflict
            // flow; a non-conflict failure leaves nothing to drive, so report it.
            if self.op_in_progress().is_none() {
                anyhow::bail!(
                    "cherry-pick failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            anyhow::bail!("cherry-pick stopped on conflict — resolve, then continue");
        }
        Ok(())
    }

    fn rebase_onto(&self, target: &str) -> Result<()> {
        // --autostash: stash local (tracked) changes before the rebase and pop
        // them afterwards, so a dirty working tree doesn't block the rebase
        // (matches the JetBrains "rebase with uncommitted changes" behaviour).
        self.git_check(&["rebase", "--autostash", target])?;
        Ok(())
    }

    fn fetch_and_rebase_onto(&self, remote_ref: &str) -> Result<()> {
        // Update the remote-tracking ref first so we rebase onto the latest state,
        // not a stale local copy. `origin/develop` → `git fetch origin develop`;
        // the branch part keeps any embedded slashes (e.g. `origin/feature/x`).
        match remote_ref.split_once('/') {
            Some((remote, branch)) => self.git_check(&["fetch", remote, branch])?,
            None => self.git_check(&["fetch"])?,
        };
        self.git_check(&["rebase", "--autostash", remote_ref])?;
        Ok(())
    }

    fn rebase_continue(&self) -> Result<()> {
        self.git_check(&["rebase", "--continue"])?;
        Ok(())
    }

    fn rebase_skip(&self) -> Result<()> {
        self.git_check(&["rebase", "--skip"])?;
        Ok(())
    }

    fn rebase_abort(&self) -> Result<()> {
        self.git_check(&["rebase", "--abort"])?;
        Ok(())
    }

    fn conflicts(&self) -> Result<Vec<String>> {
        let text = self.git_check(&["diff", "--name-only", "--diff-filter=U"])?;
        Ok(text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    fn command_log(&self) -> Vec<CmdEntry> {
        self.cmdlog.borrow().iter().cloned().collect()
    }

    fn op_in_progress(&self) -> Option<&'static str> {
        // A stopped interactive rebase also leaves CHERRY_PICK_HEAD behind, so
        // detect rebase first — otherwise a rebase would be mis-driven with
        // `git cherry-pick --continue`.
        if self.detect_rebase().is_some() {
            return Some("rebase");
        }
        let git = self.git_dir()?;
        if git.join("CHERRY_PICK_HEAD").exists() {
            return Some("cherry-pick");
        }
        if git.join("REVERT_HEAD").exists() {
            return Some("revert");
        }
        None
    }

    fn op_continue(&self) -> Result<()> {
        let op = self.op_in_progress().context("no operation in progress")?;
        self.git_check(&[op, "--continue"])?;
        Ok(())
    }

    fn op_skip(&self) -> Result<()> {
        let op = self.op_in_progress().context("no operation in progress")?;
        self.git_check(&[op, "--skip"])?;
        Ok(())
    }

    fn op_abort(&self) -> Result<()> {
        let op = self.op_in_progress().context("no operation in progress")?;
        self.git_check(&[op, "--abort"])?;
        Ok(())
    }

    fn repo_root(&self) -> &Path {
        &self.root
    }
}

/// Parse `git status --porcelain -z` into `ChangedFile`s. Paths are repo-relative
/// with `/` separators (git's convention) and `-z` leaves them unquoted.
fn parse_porcelain_z(bytes: &[u8]) -> Vec<ChangedFile> {
    let tokens: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok.len() < 3 {
            continue; // trailing empty token
        }
        let x = tok[0] as char;
        let y = tok[1] as char;
        let path = String::from_utf8_lossy(&tok[3..]).into_owned();
        if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            i += 1; // skip the original path of a rename/copy
        }
        out.push(ChangedFile {
            path,
            status: map_status(x, y),
        });
    }
    out
}

/// Collapse a porcelain XY status pair into a single displayable status.
/// Precedence: conflict > untracked > renamed > added > deleted > modified.
fn map_status(x: char, y: char) -> FileStatus {
    let unmerged = matches!((x, y), ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D'));
    if unmerged {
        FileStatus::Conflicted
    } else if x == '?' && y == '?' {
        FileStatus::Untracked
    } else if x == 'R' || y == 'R' {
        FileStatus::Renamed
    } else if x == 'A' || y == 'A' {
        FileStatus::Added
    } else if x == 'D' || y == 'D' {
        FileStatus::Deleted
    } else {
        FileStatus::Modified
    }
}

fn read_num(path: &Path) -> Option<usize> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mygit-it-{}-{}",
            std::process::id(),
            // vary by nanos-free counter: use an atomic
            next_id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                ok.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&ok.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        dir
    }

    fn next_id() -> usize {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn parses_porcelain_statuses() {
        let raw = b" M a.rs\0?? b.txt\0A  c.rs\0 D d.rs\0UU e.rs\0";
        let got: Vec<(String, FileStatus)> = parse_porcelain_z(raw)
            .into_iter()
            .map(|f| (f.path, f.status))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a.rs".into(), FileStatus::Modified),
                ("b.txt".into(), FileStatus::Untracked),
                ("c.rs".into(), FileStatus::Added),
                ("d.rs".into(), FileStatus::Deleted),
                ("e.rs".into(), FileStatus::Conflicted),
            ]
        );
    }

    #[test]
    fn rename_consumes_original_path() {
        let raw = b"R  new.rs\0old.rs\0 M keep.rs\0";
        let files = parse_porcelain_z(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "new.rs");
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!(files[1].path, "keep.rs");
    }

    #[test]
    fn nested_paths_use_forward_slash() {
        let files = parse_porcelain_z(b" M src/ui/panel.rs\0");
        assert_eq!(files[0].path, "src/ui/panel.rs");
    }

    #[test]
    fn status_reports_untracked_on_real_repo() {
        let dir = init_repo();
        std::fs::write(dir.join("hello.txt"), b"hi").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        let files = engine.status().unwrap();
        assert!(files
            .iter()
            .any(|f| f.path == "hello.txt" && f.status == FileStatus::Untracked));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_then_log_and_branch_state() {
        let dir = init_repo();
        std::fs::write(dir.join("a.txt"), b"one").unwrap();
        let engine = GixEngine::discover(&dir).unwrap();
        let hash = engine
            .commit(&["a.txt".to_string()], "first", false)
            .unwrap();
        assert_eq!(hash.len(), 40);
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "first");
        // working tree is clean after commit
        assert!(engine.status().unwrap().is_empty());
        // branch_state: a branch name, no upstream, no rebase
        let bs = engine.branch_state().unwrap();
        assert!(bs.current_branch.is_some());
        assert!(bs.upstream.is_none());
        assert!(bs.rebase.is_none());
        assert!(!bs.detached);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn three_commits(engine: &GixEngine, dir: &Path) {
        for (f, m) in [("a.txt", "c1"), ("b.txt", "c2"), ("c.txt", "c3")] {
            std::fs::write(dir.join(f), m).unwrap();
            engine.commit(&[f.to_string()], m, false).unwrap();
        }
    }

    #[test]
    fn reword_older_commit_and_undo() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        three_commits(&engine, &dir);
        let c2 = engine.log(10).unwrap()[1].hash.clone(); // [c3, c2, c1]
        engine.reword_commit(&c2, "c2 reworded").unwrap();
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 3, "reword keeps the commit count");
        assert!(log.iter().any(|c| c.summary == "c2 reworded"));
        assert!(log.iter().any(|c| c.summary == "c1"));
        assert!(log.iter().any(|c| c.summary == "c3"));
        // undo restores the original message
        engine.restore_backup().unwrap();
        let log = engine.log(10).unwrap();
        assert!(log.iter().any(|c| c.summary == "c2"));
        assert!(!log.iter().any(|c| c.summary == "c2 reworded"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn squash_commit_into_parent() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        three_commits(&engine, &dir);
        // Squash c3 into c2 (c2's parent c1 is the root, so squashing c2 itself
        // isn't supported — squashing into a non-root parent is).
        let c3 = engine.log(10).unwrap()[0].hash.clone();
        engine.squash_into_parent(&c3, "c2 + c3").unwrap();
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2, "3 commits -> 2 after squash");
        assert!(log.iter().any(|c| c.summary == "c2 + c3"));
        assert!(log.iter().any(|c| c.summary == "c1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_log_records_failures() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        // A doomed rebase onto a nonexistent ref fails; the log captures stderr.
        assert!(engine.rebase_onto("no-such-branch-xyz").is_err());
        let log = engine.command_log();
        let failed = log.iter().find(|e| !e.ok).expect("a failed entry recorded");
        assert!(failed.command.contains("rebase"), "cmd: {}", failed.command);
        assert!(!failed.stderr.is_empty(), "stderr captured");
        // successful commands are recorded too (the commit above).
        assert!(log.iter().any(|e| e.ok));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reword_root_commit() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        three_commits(&engine, &dir);
        let c1 = engine.log(10).unwrap()[2].hash.clone(); // [c3, c2, c1]
        engine.reword_commit(&c1, "root reworded").unwrap();
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 3, "reword keeps the commit count");
        assert!(log.iter().any(|c| c.summary == "root reworded"));
        assert!(log.iter().any(|c| c.summary == "c2"));
        assert!(log.iter().any(|c| c.summary == "c3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn squash_second_into_root() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        three_commits(&engine, &dir);
        let c2 = engine.log(10).unwrap()[1].hash.clone();
        engine.squash_into_parent(&c2, "c1 + c2").unwrap();
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2, "3 commits -> 2 after squashing into root");
        assert!(log.iter().any(|c| c.summary == "c1 + c2"));
        assert!(log.iter().any(|c| c.summary == "c3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn squash_marked_range_and_reject_gaps() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        three_commits(&engine, &dir);
        let log = engine.log(10).unwrap(); // [c3, c2, c1]
        let (c1, c2, c3) = (
            log[2].hash.clone(),
            log[1].hash.clone(),
            log[0].hash.clone(),
        );
        // non-adjacent selection is rejected
        assert!(engine
            .squash_commits(&[c1.clone(), c3.clone()], "no")
            .is_err());
        // adjacent c2+c3 squash into one
        engine.squash_commits(&[c2, c3], "c2 + c3 merged").unwrap();
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2);
        assert!(log.iter().any(|c| c.summary == "c2 + c3 merged"));
        assert!(log.iter().any(|c| c.summary == "c1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drop_middle_and_tip_commit() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        three_commits(&engine, &dir);
        // drop the middle commit (independent files -> no conflict)
        let c2 = engine.log(10).unwrap()[1].hash.clone();
        engine.drop_commit(&c2).unwrap();
        let log = engine.log(10).unwrap();
        assert_eq!(log.len(), 2);
        assert!(!log.iter().any(|c| c.summary == "c2"));
        assert!(log.iter().any(|c| c.summary == "c1"));
        assert!(log.iter().any(|c| c.summary == "c3"));
        // drop the tip
        let tip = engine.log(10).unwrap()[0].hash.clone();
        engine.drop_commit(&tip).unwrap();
        assert_eq!(engine.log(10).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fetch_then_rebase_onto_remote() {
        let run = |dir: &Path, a: &[&str]| {
            let o = Command::new("git")
                .current_dir(dir)
                .args(a)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {a:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        // upstream repo: a default branch + a develop branch with one commit
        let up = init_repo();
        std::fs::write(up.join("base.txt"), "base\n").unwrap();
        run(&up, &["add", "-A"]);
        run(&up, &["commit", "-q", "-m", "base"]);
        let def = String::from_utf8(
            Command::new("git")
                .current_dir(&up)
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        run(&up, &["checkout", "-q", "-b", "develop"]);
        std::fs::write(up.join("dev1.txt"), "d1\n").unwrap();
        run(&up, &["add", "-A"]);
        run(&up, &["commit", "-q", "-m", "dev1"]);
        run(&up, &["checkout", "-q", &def]);

        // clone it (origin/develop is captured at clone time = base + dev1)
        let work =
            std::env::temp_dir().join(format!("mygit-clone-{}-{}", std::process::id(), next_id()));
        let _ = std::fs::remove_dir_all(&work);
        let o = Command::new("git")
            .args(["clone", "-q", up.to_str().unwrap(), work.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "clone: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        run(&work, &["config", "user.email", "t@t"]);
        run(&work, &["config", "user.name", "t"]);

        // work: a feature commit on the default branch
        std::fs::write(work.join("feat.txt"), "f\n").unwrap();
        run(&work, &["add", "-A"]);
        run(&work, &["commit", "-q", "-m", "feat"]);

        // upstream develop advances -> the clone's origin/develop is now stale
        run(&up, &["checkout", "-q", "develop"]);
        std::fs::write(up.join("dev2.txt"), "d2\n").unwrap();
        run(&up, &["add", "-A"]);
        run(&up, &["commit", "-q", "-m", "dev2"]);

        // fetch + rebase must pull dev2 and replay feat on top of it
        let engine = GixEngine::discover(&work).unwrap();
        engine.fetch_and_rebase_onto("origin/develop").unwrap();
        let log = engine.log(20).unwrap();
        assert!(log.iter().any(|c| c.summary == "feat"));
        assert!(log.iter().any(|c| c.summary == "dev1"));
        assert!(
            log.iter().any(|c| c.summary == "dev2"),
            "fetch pulled the newest develop commit before rebasing"
        );
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&up);
    }

    #[test]
    fn rebase_onto_autostashes_dirty_worktree() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        let git = |a: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&dir)
                .args(a)
                .output()
                .unwrap()
                .status
                .success());
        };
        std::fs::write(dir.join("a.txt"), "base\n").unwrap();
        engine
            .commit(&["a.txt".to_string()], "base", false)
            .unwrap();
        let main = engine.branch_state().unwrap().current_branch.unwrap();
        // develop gets its own commit
        git(&["checkout", "-q", "-b", "develop"]);
        std::fs::write(dir.join("d.txt"), "dev\n").unwrap();
        engine.commit(&["d.txt".to_string()], "dev", false).unwrap();
        // main gets a divergent commit
        git(&["checkout", "-q", &main]);
        std::fs::write(dir.join("m.txt"), "main\n").unwrap();
        engine
            .commit(&["m.txt".to_string()], "main", false)
            .unwrap();
        // a tracked change we don't want to commit (dirty tree)
        std::fs::write(dir.join("a.txt"), "base\nWIP\n").unwrap();
        // rebase onto develop must succeed despite the dirty tree (autostash)
        engine.rebase_onto("develop").unwrap();
        let log = engine.log(10).unwrap();
        assert!(log.iter().any(|c| c.summary == "dev"));
        assert!(log.iter().any(|c| c.summary == "main"));
        // and the WIP change is restored to the working tree afterwards
        let content = std::fs::read_to_string(dir.join("a.txt")).unwrap();
        assert!(
            content.contains("WIP"),
            "autostash restored the dirty change"
        );
        assert!(engine.status().unwrap().iter().any(|f| f.path == "a.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cherry_pick_conflict_is_drivable() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        let git = |args: &[&str]| {
            let o = Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "base", false)
            .unwrap();
        let main = engine.branch_state().unwrap().current_branch.unwrap();
        // divergent branch changing the same line
        git(&["checkout", "-q", "-b", "other"]);
        std::fs::write(dir.join("f.txt"), "other\n").unwrap();
        let other_c = engine
            .commit(&["f.txt".to_string()], "on-other", false)
            .unwrap();
        git(&["checkout", "-q", &main]);
        std::fs::write(dir.join("f.txt"), "main\n").unwrap();
        engine
            .commit(&["f.txt".to_string()], "on-main", false)
            .unwrap();
        // cherry-pick the divergent commit -> conflict, left in progress
        assert!(engine.cherry_pick(&other_c).is_err());
        assert_eq!(engine.op_in_progress(), Some("cherry-pick"));
        assert!(engine.conflicts().unwrap().iter().any(|f| f == "f.txt"));
        // abort restores a clean tree at on-main
        engine.op_abort().unwrap();
        assert!(engine.op_in_progress().is_none());
        assert!(engine.status().unwrap().is_empty());
        assert_eq!(engine.log(10).unwrap()[0].summary, "on-main");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stash_roundtrip() {
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "1").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        std::fs::write(dir.join("a.txt"), "2").unwrap();
        engine.stash_push("wip").unwrap();
        assert!(
            engine.status().unwrap().is_empty(),
            "stash cleared the tree"
        );
        let list = engine.stash_list().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].1.contains("wip"), "named stash: {}", list[0].1);
        engine.stash_pop(&list[0].0).unwrap();
        assert!(engine.status().unwrap().iter().any(|f| f.path == "a.txt"));
        assert!(engine.stash_list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_moves_head_and_startup_pipeline_persists() {
        use crate::changelists::{store_path, ChangelistStore};
        let dir = init_repo();
        let engine = GixEngine::discover(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"one").unwrap();
        engine.commit(&["a.txt".to_string()], "c1", false).unwrap();
        std::fs::write(dir.join("b.txt"), b"two").unwrap();
        engine.commit(&["b.txt".to_string()], "c2", false).unwrap();
        assert_eq!(engine.log(10).unwrap().len(), 2);
        // mixed reset back one commit -> b.txt becomes an uncommitted change
        engine.reset("HEAD~1", ResetMode::Mixed).unwrap();
        assert_eq!(engine.log(10).unwrap().len(), 1);
        // b.txt is now untracked (the reset dropped c2); it shows in status but
        // lives in the derived, non-persisted Unversioned list.
        assert!(engine.status().unwrap().iter().any(|f| f.path == "b.txt"));

        // A tracked modification persists through the startup pipeline (AC#2).
        std::fs::write(dir.join("a.txt"), b"one-modified").unwrap();
        let sp = store_path(engine.repo_root());
        let mut store = ChangelistStore::load(&sp).unwrap();
        store.sync(&engine.status().unwrap());
        store.persist(&sp).unwrap();
        let reloaded = ChangelistStore::load(&sp).unwrap();
        assert!(
            reloaded
                .changelists
                .iter()
                .any(|c| c.files.iter().any(|f| f == "a.txt")),
            "modified tracked file persists to Default"
        );
        assert!(
            !reloaded
                .changelists
                .iter()
                .any(|c| c.files.iter().any(|f| f == "b.txt")),
            "untracked file is not persisted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
