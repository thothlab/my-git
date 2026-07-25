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
use std::path::{Path, PathBuf};

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
    /// Save the current HEAD as the undo point before a history rewrite.
    fn backup_head(&self) -> Result<()>;
    /// Whether an undo point exists.
    fn has_backup(&self) -> bool;
    /// Reset HEAD back to the saved undo point.
    fn restore_backup(&self) -> Result<()>;
    fn rebase_onto(&self, target: &str) -> Result<()>;
    fn rebase_continue(&self) -> Result<()>;
    fn rebase_skip(&self) -> Result<()>;
    fn rebase_abort(&self) -> Result<()>;
    fn conflicts(&self) -> Result<Vec<String>>;
    fn repo_root(&self) -> &Path;
}

/// Engine backed by `gix` discovery + `git` CLI operations.
pub struct GixEngine {
    #[allow(dead_code)]
    repo: gix::Repository,
    root: PathBuf,
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
        Ok(Self { repo, root })
    }

    /// Run `git` in the repo. `GIT_EDITOR`/`GIT_SEQUENCE_EDITOR` are neutralised
    /// (we supply messages ourselves) and terminal prompts are disabled so a
    /// missing credential fails fast instead of hanging the TUI.
    fn git(&self, args: &[&str]) -> Result<std::process::Output> {
        std::process::Command::new("git")
            .current_dir(&self.root)
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .output()
            .with_context(|| format!("running `git {}`", args.join(" ")))
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

    fn reword_commit(&self, hash: &str, message: &str) -> Result<()> {
        anyhow::ensure!(self.is_on_head(hash), "commit is not on the current branch");
        let target = self.git_check(&["rev-parse", hash])?.trim().to_string();
        let base = format!("{target}^");
        anyhow::ensure!(
            self.git(&["rev-parse", "--verify", "--quiet", &base])?
                .status
                .success(),
            "cannot reword the root commit here"
        );
        let list = self.git_check(&["rev-list", "--reverse", &format!("{base}..HEAD")])?;
        let mut todo = String::new();
        for h in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let action = if h == target { "reword" } else { "pick" };
            todo.push_str(&format!("{action} {h}\n"));
        }
        self.backup_head()?;
        self.run_rebase_todo(&base, &todo, Some(message))
    }

    fn squash_into_parent(&self, hash: &str, message: &str) -> Result<()> {
        anyhow::ensure!(self.is_on_head(hash), "commit is not on the current branch");
        let target = self.git_check(&["rev-parse", hash])?.trim().to_string();
        let parent = self
            .git_check(&["rev-parse", &format!("{target}^")])?
            .trim()
            .to_string();
        let base = format!("{parent}^");
        anyhow::ensure!(
            self.git(&["rev-parse", "--verify", "--quiet", &base])?
                .status
                .success(),
            "cannot squash into the root commit here"
        );
        let list = self.git_check(&["rev-list", "--reverse", &format!("{base}..HEAD")])?;
        let mut todo = String::new();
        for h in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let action = if h == target { "squash" } else { "pick" };
            todo.push_str(&format!("{action} {h}\n"));
        }
        self.backup_head()?;
        self.run_rebase_todo(&base, &todo, Some(message))
    }

    fn rebase_onto(&self, target: &str) -> Result<()> {
        self.git_check(&["rebase", target])?;
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
