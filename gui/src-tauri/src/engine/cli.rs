use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::GitEngine;
use crate::error::{Error, Result};
use crate::model::{
    BranchInfo, DiffLine, FileDiff, FileState, FileStatus, Hunk, RepoSnapshot,
};

/// git backend implemented by shelling out to the system `git`.
pub struct CliEngine {
    repo: PathBuf,
}

impl CliEngine {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self { repo: repo.into() }
    }

    /// Resolve the repository top-level for an arbitrary path inside a working tree.
    pub fn resolve_root(path: &Path) -> Result<PathBuf> {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"])
            .output()?;
        if !out.status.success() {
            return Err(Error::Git {
                command: "rev-parse --show-toplevel".into(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(PathBuf::from(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    /// Run `git -C <repo> <args>` capturing raw stdout bytes. On failure produce
    /// `Error::Git` carrying the command and its stderr verbatim.
    fn git_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()?;
        if !out.status.success() {
            return Err(Error::Git {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(out.stdout)
    }

    /// Run git capturing stdout as UTF-8 text (git errors still carry stderr).
    fn git(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git_bytes(args)?).to_string())
    }

    /// Ignored paths for the "Show Ignored" view. git collapses ignored
    /// directories to a single entry (trailing slash), so this stays small even
    /// with a fat `node_modules`/`target`. Parsed from porcelain v1 `!!` records;
    /// kept separate from `snapshot()` so ignored paths never reach the changelist
    /// store via `sync()`.
    pub fn ignored(&self) -> Result<Vec<String>> {
        let out = self.git_bytes(&["status", "--porcelain", "-z", "--ignored"])?;
        let mut v = Vec::new();
        for tok in out.split(|&c| c == 0) {
            if tok.len() > 3 && &tok[0..3] == b"!! " {
                v.push(String::from_utf8_lossy(&tok[3..]).to_string());
            }
        }
        Ok(v)
    }

    /// Restore changed files to their HEAD content (discarding local edits). A file
    /// that exists in HEAD is checked out from it; an added/new file (absent from
    /// HEAD) is unstaged and removed from disk. Callers confirm on the UI first —
    /// this is destructive.
    pub fn rollback(&self, paths: &[String]) -> Result<()> {
        for p in paths {
            let in_head = self.git(&["cat-file", "-e", &format!("HEAD:{p}")]).is_ok();
            if in_head {
                self.git(&["checkout", "HEAD", "--", p])?;
            } else {
                let _ = self.git(&["rm", "-f", "--", p]); // unstage if it was `git add`ed
                let _ = std::fs::remove_file(self.repo.join(p));
            }
        }
        Ok(())
    }

    /// Run git feeding `input` on stdin (used by `git apply`).
    fn git_stdin(&self, args: &[&str], input: &[u8]) -> Result<()> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        {
            let mut si = child
                .stdin
                .take()
                .ok_or_else(|| Error::Io("no stdin".into()))?;
            si.write_all(input).map_err(|e| Error::Io(e.to_string()))?;
        } // drop stdin ⇒ EOF
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(Error::Git {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(())
    }

    /// Run git, returning stdout and ignoring a non-zero exit (for `diff --no-index`,
    /// which exits 1 precisely when there is a difference to show).
    fn git_allow_fail(&self, args: &[&str]) -> String {
        Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    /// Resolve paths **inside the git directory** by asking git, never by joining
    /// `.git` onto the worktree root: in a linked worktree and in a submodule `.git`
    /// is a file, and the real markers live under `.git/worktrees/<name>/`. One
    /// `rev-parse` answers for all names at once. A path git returns relative is
    /// relative to the worktree root it was run in.
    pub(crate) fn git_paths(&self, names: &[&str]) -> Result<Vec<PathBuf>> {
        let mut args = vec!["rev-parse"];
        for n in names {
            args.push("--git-path");
            args.push(n);
        }
        let out = self.git(&args)?;
        let paths: Vec<PathBuf> = out
            .lines()
            .map(|l| {
                let p = PathBuf::from(l.trim());
                if p.is_absolute() {
                    p
                } else {
                    self.repo.join(p)
                }
            })
            .collect();
        if paths.len() != names.len() {
            return Err(Error::Parse(format!(
                "rev-parse --git-path returned {} paths for {} names",
                paths.len(),
                names.len()
            )));
        }
        Ok(paths)
    }

    /// Whether git has the path in the index (i.e. it is not an untracked file).
    fn is_tracked(&self, path: &str) -> bool {
        !self
            .git_allow_fail(&["ls-files", "--", path])
            .trim()
            .is_empty()
    }

    /// Diff a file against a base: `worktree` (unstaged), `index` (staged) or `head`.
    ///
    /// `whitespace` is one of `none` (do not ignore — the historical behaviour),
    /// `trailing` (`--ignore-space-at-eol`) or `all` (`-w`). Any other value is
    /// treated as `none`, so a caller that has not been taught about the mode yet
    /// keeps seeing every difference rather than silently hiding some.
    pub fn diff_file(&self, path: &str, against: &str, whitespace: &str) -> Result<FileDiff> {
        let ws = whitespace_args(whitespace)?;
        let raw = match against {
            "index" => {
                let mut a = vec!["diff", "--cached"];
                a.extend_from_slice(&ws);
                a.extend_from_slice(&["--", path]);
                self.git(&a)?
            }
            "head" => {
                let mut a = vec!["diff", "HEAD"];
                a.extend_from_slice(&ws);
                a.extend_from_slice(&["--", path]);
                self.git(&a)?
            }
            _ => {
                let mut a = vec!["diff"];
                a.extend_from_slice(&ws);
                a.extend_from_slice(&["--", path]);
                let d = self.git(&a)?;
                // "Empty diff ⇒ untracked file" is exactly right while nothing is
                // ignored, and that is the behaviour `none` must keep. Only when a
                // whitespace mode is active can an empty diff also mean "the change
                // is whitespace-only" — there, and only there, ask git whether it
                // knows the path, so a whitespace-only change is not re-rendered as
                // an all-add diff of the whole file.
                let empty_means_untracked = ws.is_empty() || !self.is_tracked(path);
                if d.trim().is_empty() && empty_means_untracked {
                    // untracked/new file: synthesize an all-add diff (view only)
                    let mut a = vec!["diff", "--no-index"];
                    a.extend_from_slice(&ws);
                    a.extend_from_slice(&["--", "/dev/null", path]);
                    self.git_allow_fail(&a)
                } else {
                    d
                }
            }
        };
        Ok(parse_diff(path, &raw))
    }

    /// Apply a single-hunk patch to the index (`cached`) or worktree, forward or
    /// reversed. This is the whole mechanism behind hunk-level stage (cached,
    /// forward), unstage (cached, reverse) and revert (worktree, reverse) — the index
    /// is touched only by the exact hunk, never by `git add -A`/`git add <dir>`
    /// (which would over-stage other lists; cf. commit staging discipline, Правка
    /// `ad8c42e`).
    pub fn apply_patch(&self, patch: &str, cached: bool, reverse: bool) -> Result<()> {
        let mut args = vec!["apply", "--whitespace=nowarn"];
        if cached {
            args.push("--cached");
        }
        if reverse {
            args.push("-R");
        }
        self.git_stdin(&args, patch.as_bytes())
    }

    /// Stage EXACTLY these paths. Existing files are `git add`ed; worktree deletions
    /// are staged via `git rm`. Never `git add -A` or `git add <dir>` — both would
    /// sweep in other changelists' files or miss deletions (Правка `ad8c42e`; the
    /// `-A`/`<dir>` ban is about *unscoped* staging, exactly what this avoids).
    pub fn stage_paths(&self, paths: &[String]) -> Result<()> {
        let (deleted, existing): (Vec<&String>, Vec<&String>) =
            paths.iter().partition(|p| !self.repo.join(p).exists());
        if !existing.is_empty() {
            let mut args = vec!["add", "--"];
            args.extend(existing.iter().map(|s| s.as_str()));
            self.git(&args)?;
        }
        if !deleted.is_empty() {
            let mut args = vec!["rm", "-q", "--"];
            args.extend(deleted.iter().map(|s| s.as_str()));
            self.git(&args)?;
        }
        Ok(())
    }

    /// Commit exactly the given paths: stage only them, then commit the index. Other
    /// changelists' files, being unstaged, stay out of the commit (AC#3).
    pub fn commit_paths(&self, paths: &[String], message: &str, amend: bool) -> Result<()> {
        if message.trim().is_empty() {
            return Err(Error::Rule("commit message cannot be empty".into()));
        }
        self.stage_paths(paths)?;
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
        }
        self.git(&args)?;
        Ok(())
    }

    // ── branches & remotes (task_06) ────────────────────────────────────────

    fn current_branch(&self) -> Result<String> {
        Ok(self.git(&["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string())
    }

    pub fn branches(&self) -> Result<Vec<BranchInfo>> {
        let out = self.git(&[
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)",
            "refs/heads",
            "refs/remotes",
        ])?;
        let mut v = Vec::new();
        for line in out.lines() {
            let f: Vec<&str> = line.split('\u{0}').collect();
            if f.len() < 3 {
                continue;
            }
            let (full, short, head) = (f[0], f[1], f[2]);
            let is_remote = full.starts_with("refs/remotes/");
            if is_remote && short.ends_with("/HEAD") {
                continue; // skip the origin/HEAD symref
            }
            v.push(BranchInfo {
                name: short.to_string(),
                is_remote,
                is_current: head == "*",
                upstream: f.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string()),
            });
        }
        Ok(v)
    }

    /// Create a branch from HEAD (or `from`) and switch to it.
    pub fn create_branch(&self, name: &str, from: Option<&str>) -> Result<()> {
        let mut args = vec!["checkout", "-b", name];
        if let Some(f) = from {
            args.push(f);
        }
        self.git(&args)?;
        Ok(())
    }

    /// Switch branch. `stash` first shelves tracked+untracked changes so a dirty
    /// tree does not block the switch (the UI offers stash / switch-anyway / cancel).
    pub fn checkout(&self, name: &str, stash: bool) -> Result<()> {
        if stash {
            self.git(&[
                "stash",
                "push",
                "-u",
                "-m",
                &format!("mygit: switching to {name}"),
            ])?;
        }
        self.git(&["checkout", name])?;
        Ok(())
    }

    /// Push. `upstream` sets `-u` for a branch with no upstream; `force` uses
    /// `--force-with-lease` (only ever called behind explicit confirmation).
    pub fn push(&self, mode: &str) -> Result<()> {
        match mode {
            "upstream" => {
                let br = self.current_branch()?;
                self.git(&["push", "-u", "origin", &br])?;
            }
            "force" => {
                self.git(&["push", "--force-with-lease"])?;
            }
            _ => {
                self.git(&["push"])?;
            }
        }
        Ok(())
    }

    pub fn fetch(&self) -> Result<()> {
        self.git(&["fetch", "--prune"])?;
        Ok(())
    }

    pub fn pull(&self) -> Result<()> {
        self.git(&["pull"])?;
        Ok(())
    }
}

fn parse_hunk_header(h: &str) -> (u32, u32) {
    // "@@ -a,b +c,d @@ section"
    let (mut old_no, mut new_no) = (1u32, 1u32);
    for tok in h.split_whitespace() {
        if let Some(r) = tok.strip_prefix('-') {
            old_no = r.split(',').next().unwrap_or("1").parse().unwrap_or(1);
        } else if let Some(r) = tok.strip_prefix('+') {
            new_no = r.split(',').next().unwrap_or("1").parse().unwrap_or(1);
        }
    }
    (old_no, new_no)
}

/// Parse `git diff` output for a single file into hunks, keeping each hunk's exact
/// applicable patch text (file header + hunk) so stage/revert is byte-exact.
fn parse_diff(path: &str, raw: &str) -> FileDiff {
    if raw.contains("Binary files ") || raw.contains("GIT binary patch") {
        return FileDiff {
            path: path.into(),
            binary: true,
            hunks: Vec::new(),
        };
    }
    let lines: Vec<&str> = raw.split('\n').collect();
    let Some(first) = lines.iter().position(|l| l.starts_with("@@")) else {
        return FileDiff {
            path: path.into(),
            binary: false,
            hunks: Vec::new(),
        };
    };
    let header = lines[..first].join("\n");

    let mut hunks = Vec::new();
    let mut i = first;
    while i < lines.len() {
        if !lines[i].starts_with("@@") {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        while j < lines.len() && !lines[j].starts_with("@@") {
            j += 1;
        }
        let block = &lines[start..j];

        let (mut old_no, mut new_no) = parse_hunk_header(block[0]);
        let mut dls = Vec::new();
        for &l in &block[1..] {
            if l.is_empty() || l.starts_with('\\') {
                continue; // trailing artifact / "\ No newline at end of file"
            }
            let origin = l.as_bytes()[0] as char;
            let content = l[1..].to_string();
            match origin {
                '+' => {
                    dls.push(DiffLine {
                        origin: "+".into(),
                        content,
                        old_no: None,
                        new_no: Some(new_no),
                    });
                    new_no += 1;
                }
                '-' => {
                    dls.push(DiffLine {
                        origin: "-".into(),
                        content,
                        old_no: Some(old_no),
                        new_no: None,
                    });
                    old_no += 1;
                }
                _ => {
                    dls.push(DiffLine {
                        origin: " ".into(),
                        content,
                        old_no: Some(old_no),
                        new_no: Some(new_no),
                    });
                    old_no += 1;
                    new_no += 1;
                }
            }
        }

        let mut patch = String::with_capacity(header.len() + 64);
        patch.push_str(&header);
        patch.push('\n');
        patch.push_str(&block.join("\n"));
        if !patch.ends_with('\n') {
            patch.push('\n');
        }
        hunks.push(Hunk {
            header: block[0].to_string(),
            lines: dls,
            patch,
        });
        i = j;
    }

    FileDiff {
        path: path.into(),
        binary: false,
        hunks,
    }
}

/// Build a `FileStatus` from a porcelain-v2 `<XY>` field. `X` is the index (staged)
/// status, `Y` the worktree (unstaged) status; `.` means unchanged on that side.
fn make_status(xy: &str, path: String, old_path: Option<String>, renamed: bool) -> FileStatus {
    let b = xy.as_bytes();
    let x = *b.first().unwrap_or(&b'.') as char;
    let y = *b.get(1).unwrap_or(&b'.') as char;
    let status = if renamed || x == 'R' || y == 'R' {
        FileState::Renamed
    } else if x == 'A' || y == 'A' {
        FileState::Added
    } else if x == 'D' || y == 'D' {
        FileState::Deleted
    } else {
        FileState::Modified
    };
    FileStatus {
        path,
        status,
        old_path,
        staged: x != '.',
        unstaged: y != '.',
    }
}

/// git flags for a whitespace mode: `none` | `trailing` | `all`.
///
/// A closed dictionary crossing the Tauri boundary as a string is checked, not
/// folded into a default: a typo that silently means "none" shows a diff the user
/// did not ask for and reports nothing.
pub fn whitespace_args(mode: &str) -> Result<Vec<&'static str>> {
    match mode {
        "none" => Ok(vec![]),
        "trailing" => Ok(vec!["--ignore-space-at-eol"]),
        "all" => Ok(vec!["--ignore-all-space"]),
        other => Err(Error::Rule(format!(
            "unknown whitespace mode: {other} (expected none, trailing or all)"
        ))),
    }
}

impl GitEngine for CliEngine {
    fn snapshot(&self) -> Result<RepoSnapshot> {
        // porcelain=v2 gives per-side staging + rename detail; --branch adds the
        // branch/ahead/behind headers; -z makes paths NUL-safe (spaces, unicode).
        let out = self.git_bytes(&["status", "--porcelain=v2", "--branch", "-z"])?;

        let mut branch = String::from("(unknown)");
        let mut upstream = None;
        let (mut ahead, mut behind) = (0u32, 0u32);
        let mut detached = false;
        let mut files = Vec::new();

        // Records are NUL-terminated. A rename record ('2') is followed by an extra
        // NUL-delimited token holding its source path — so we index and can look ahead.
        let tokens: Vec<&[u8]> = out.split(|&c| c == 0).collect();
        let mut i = 0;
        while i < tokens.len() {
            let tok = tokens[i];
            if tok.is_empty() {
                i += 1;
                continue;
            }
            let s = String::from_utf8_lossy(tok);
            match s.as_bytes()[0] as char {
                '#' => {
                    let rest = &s[2..];
                    if let Some(v) = rest.strip_prefix("branch.head ") {
                        if v == "(detached)" {
                            detached = true;
                        } else {
                            branch = v.to_string();
                        }
                    } else if let Some(v) = rest.strip_prefix("branch.upstream ") {
                        upstream = Some(v.to_string());
                    } else if let Some(v) = rest.strip_prefix("branch.ab ") {
                        for part in v.split_whitespace() {
                            if let Some(n) = part.strip_prefix('+') {
                                ahead = n.parse().unwrap_or(0);
                            } else if let Some(n) = part.strip_prefix('-') {
                                behind = n.parse().unwrap_or(0);
                            }
                        }
                    }
                    i += 1;
                }
                '1' => {
                    // "1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>"
                    let f: Vec<&str> = s.splitn(9, ' ').collect();
                    if f.len() == 9 {
                        files.push(make_status(f[1], f[8].to_string(), None, false));
                    }
                    i += 1;
                }
                '2' => {
                    // "2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <Xscore> <path>" + next token = source path
                    let f: Vec<&str> = s.splitn(10, ' ').collect();
                    let orig = tokens
                        .get(i + 1)
                        .map(|t| String::from_utf8_lossy(t).to_string());
                    if f.len() == 10 {
                        files.push(make_status(f[1], f[9].to_string(), orig, true));
                    }
                    i += 2; // consume the source-path token
                }
                'u' => {
                    // unmerged: "u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>"
                    let f: Vec<&str> = s.splitn(11, ' ').collect();
                    if f.len() == 11 {
                        files.push(FileStatus {
                            path: f[10].to_string(),
                            status: FileState::Conflicted,
                            old_path: None,
                            staged: false,
                            unstaged: true,
                        });
                    }
                    i += 1;
                }
                '?' => {
                    files.push(FileStatus {
                        path: s[2..].to_string(),
                        status: FileState::Untracked,
                        old_path: None,
                        staged: false,
                        unstaged: true,
                    });
                    i += 1;
                }
                _ => i += 1,
            }
        }

        Ok(RepoSnapshot {
            branch,
            upstream,
            ahead,
            behind,
            detached,
            files,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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

    /// A scratch repo with one commit and deterministic identity/branch.
    pub(crate) fn scratch_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-b", "main"]);
        run(p, &["config", "user.email", "t@example.com"]);
        run(p, &["config", "user.name", "Test"]);
        run(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "one\n").unwrap();
        run(p, &["add", "a.txt"]);
        run(p, &["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn snapshot_reports_branch_and_file_states() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "two\n").unwrap(); // modify tracked
        std::fs::write(p.join("b.txt"), "new\n").unwrap(); // add untracked

        let snap = CliEngine::new(p).snapshot().unwrap();
        assert_eq!(snap.branch, "main");
        assert!(!snap.detached);

        let a = snap.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(a.status, FileState::Modified);
        assert!(a.unstaged);

        let b = snap.files.iter().find(|f| f.path == "b.txt").unwrap();
        assert_eq!(b.status, FileState::Untracked);
    }

    #[test]
    fn rollback_restores_modified_and_removes_added() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "changed\n").unwrap(); // modify tracked
        std::fs::write(p.join("added.txt"), "x\n").unwrap();
        run(p, &["add", "added.txt"]); // staged-new (in index, not HEAD)

        let eng = CliEngine::new(p);
        eng.rollback(&["a.txt".to_string(), "added.txt".to_string()])
            .unwrap();

        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "one\n");
        assert!(!p.join("added.txt").exists());
        assert!(eng.snapshot().unwrap().files.is_empty(), "tree is clean again");
    }

    #[test]
    fn hunk_stage_and_revert_are_independent() {
        let dir = scratch_repo();
        let p = dir.path();
        let base: String = (1..=10).map(|n| format!("line{n}\n")).collect();
        std::fs::write(p.join("f.txt"), &base).unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["-c", "commit.gpgsign=false", "commit", "-m", "f"]);

        // change line 1 and line 10 → two well-separated hunks
        let mut lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
        lines[0] = "CHANGED1".into();
        lines[9] = "CHANGED10".into();
        std::fs::write(p.join("f.txt"), lines.join("\n") + "\n").unwrap();

        let eng = CliEngine::new(p);
        let diff = eng.diff_file("f.txt", "worktree", "none").unwrap();
        assert_eq!(diff.hunks.len(), 2, "two separated hunks");

        // stage only the first hunk
        eng.apply_patch(&diff.hunks[0].patch, true, false).unwrap();
        assert!(eng
            .git(&["diff", "--cached", "--name-only"])
            .unwrap()
            .contains("f.txt"));
        let remaining = eng.diff_file("f.txt", "worktree", "none").unwrap();
        assert_eq!(remaining.hunks.len(), 1, "one hunk left unstaged");

        // revert the remaining (line 10) hunk in the worktree
        eng.apply_patch(&remaining.hunks[0].patch, false, true).unwrap();
        let content = std::fs::read_to_string(p.join("f.txt")).unwrap();
        assert!(content.contains("CHANGED1"), "staged change stays in worktree");
        assert!(content.contains("line10"), "line 10 restored");
        assert!(!content.contains("CHANGED10"), "line 10 change reverted");
    }

    fn head_files(p: &Path) -> String {
        String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(p)
                .args(["show", "--name-status", "--format=", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .to_string()
    }

    #[test]
    fn commit_isolates_to_given_paths() {
        // AC#3: committing one list excludes another's files.
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "A\n").unwrap(); // "Default"
        std::fs::write(p.join("b.txt"), "B\n").unwrap(); // "Not for commit"

        let eng = CliEngine::new(p);
        eng.commit_paths(&["a.txt".to_string()], "commit a", false)
            .unwrap();

        assert!(head_files(p).contains("a.txt"), "a.txt is committed");
        assert!(!head_files(p).contains("b.txt"), "b.txt stays out of the commit");
        let snap = eng.snapshot().unwrap();
        assert!(
            snap.files.iter().any(|f| f.path == "b.txt"),
            "b.txt is still a pending change"
        );
        assert!(!snap.files.iter().any(|f| f.path == "a.txt"));
    }

    #[test]
    fn commit_records_a_deletion() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::remove_file(p.join("a.txt")).unwrap();
        let eng = CliEngine::new(p);
        eng.commit_paths(&["a.txt".to_string()], "remove a", false)
            .unwrap();
        assert!(head_files(p).contains("D\ta.txt"), "deletion committed");
    }

    #[test]
    fn amend_folds_into_head_without_new_commit() {
        let dir = scratch_repo();
        let p = dir.path();
        let count = |p: &Path| {
            String::from_utf8_lossy(
                &Command::new("git")
                    .arg("-C")
                    .arg(p)
                    .args(["rev-list", "--count", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .trim()
            .to_string()
        };
        let before = count(p);
        std::fs::write(p.join("a.txt"), "amended\n").unwrap();
        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "init amended", true)
            .unwrap();
        assert_eq!(count(p), before, "amend does not add a commit");
        assert!(head_files(p).contains("a.txt"));
    }

    #[test]
    fn empty_message_is_rejected() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "x\n").unwrap();
        assert!(CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "   ", false)
            .is_err());
    }

    #[test]
    fn push_sets_upstream_and_fetch_sees_remote_advance() {
        // AC#5/#6 against a local bare repo used as origin — exercises the real push/
        // fetch/ahead-behind code paths without a network.
        let bare = tempfile::tempdir().unwrap();
        run(bare.path(), &["init", "--bare", "-b", "main"]);
        let bare_path = bare.path().to_str().unwrap();

        let work = scratch_repo();
        let wp = work.path();
        run(wp, &["remote", "add", "origin", bare_path]);

        let eng = CliEngine::new(wp);
        eng.push("upstream").unwrap();

        let snap = eng.snapshot().unwrap();
        assert_eq!(snap.upstream.as_deref(), Some("origin/main"));
        assert_eq!((snap.ahead, snap.behind), (0, 0));

        // advance origin from a second clone
        let w2 = tempfile::tempdir().unwrap();
        run(w2.path(), &["clone", bare_path, "c"]);
        let c = w2.path().join("c");
        run(&c, &["config", "user.email", "t@e"]);
        run(&c, &["config", "user.name", "T"]);
        run(&c, &["config", "commit.gpgsign", "false"]);
        std::fs::write(c.join("r.txt"), "remote\n").unwrap();
        run(&c, &["add", "r.txt"]);
        run(&c, &["commit", "-m", "remote commit"]);
        run(&c, &["push", "origin", "main"]);

        eng.fetch().unwrap();
        assert_eq!(eng.snapshot().unwrap().behind, 1, "fetch reflects remote advance");
    }

    #[test]
    fn branches_lists_local_with_current_marked() {
        let dir = scratch_repo();
        let eng = CliEngine::new(dir.path());
        eng.create_branch("feature/x", None).unwrap();
        let bs = eng.branches().unwrap();
        assert!(bs.iter().any(|b| b.name == "feature/x" && b.is_current && !b.is_remote));
        assert!(bs.iter().any(|b| b.name == "main" && !b.is_current));
    }

    #[test]
    fn launch_path_groups_and_persists_store() {
        // The exact sequence build_state runs on window open, on a real repo (real
        // .git dir): snapshot → load → sync → save → build_views.
        use crate::changelists;
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "changed\n").unwrap(); // modified tracked
        std::fs::write(p.join("new.txt"), "n\n").unwrap(); // untracked

        let snap = CliEngine::new(p).snapshot().unwrap();
        let mut store = changelists::load(p).unwrap();
        if changelists::sync(&mut store, &snap) {
            changelists::save(p, &store).unwrap();
        }
        let views = changelists::build_views(&store, &snap);

        let def = views.iter().find(|v| v.is_default).unwrap();
        assert!(def.files.iter().any(|f| f.path == "a.txt"), "modified file in Default");
        assert!(
            views
                .iter()
                .any(|v| v.is_unversioned && v.files.iter().any(|f| f.path == "new.txt")),
            "untracked file in synthetic Unversioned"
        );
        assert!(
            p.join(".git").join("changelists.json").exists(),
            "store persisted into real .git/"
        );
    }
    /// R34i: three whitespace modes. `none` must keep the historical behaviour —
    /// a whitespace-only change is still a difference.
    #[test]
    fn diff_file_whitespace_modes() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("f.txt"), "alpha\nbeta\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["-c", "commit.gpgsign=false", "commit", "-m", "f"]);
        // indent one line and add a trailing space on the other: whitespace only
        std::fs::write(p.join("f.txt"), "    alpha\nbeta   \n").unwrap();

        let eng = CliEngine::new(p);
        assert_eq!(
            eng.diff_file("f.txt", "worktree", "none").unwrap().hunks.len(),
            1,
            "do-not-ignore shows the whitespace-only change"
        );
        assert!(
            eng.diff_file("f.txt", "worktree", "all").unwrap().hunks.is_empty(),
            "ignore-all-whitespace hides it"
        );
        let trailing = eng.diff_file("f.txt", "worktree", "trailing").unwrap();
        assert_eq!(
            trailing.hunks.len(),
            1,
            "ignore-trailing still shows the leading indent"
        );
        assert!(
            trailing.hunks[0]
                .lines
                .iter()
                .filter(|l| l.origin != " ")
                .all(|l| !l.content.contains("beta")),
            "the trailing-space-only line is context, not a difference"
        );
    }

    /// DoD: `none` gives the prior result. A tracked file with everything staged had
    /// an empty worktree diff even before whitespace modes existed, and the fallback
    /// synthesized an all-add diff for it — that stays.
    #[test]
    fn diff_file_none_mode_keeps_prior_empty_diff_fallback() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("f.txt"), "alpha\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["-c", "commit.gpgsign=false", "commit", "-m", "f"]);
        std::fs::write(p.join("f.txt"), "alpha\nbeta\n").unwrap();
        run(p, &["add", "f.txt"]);

        let eng = CliEngine::new(p);
        let d = eng.diff_file("f.txt", "worktree", "none").unwrap();
        assert_eq!(d.hunks.len(), 1, "prior behaviour: synthesized all-add diff");
        assert!(d.hunks[0].lines.iter().all(|l| l.origin == "+"));
        assert_eq!(
            eng.diff_file("f.txt", "index", "none").unwrap().hunks.len(),
            1,
            "the staged change is visible against the index"
        );
    }

    /// A closed dictionary is checked at the boundary, not folded into a default.
    #[test]
    fn diff_file_rejects_unknown_whitespace_mode() {
        let dir = scratch_repo();
        let err = CliEngine::new(dir.path())
            .diff_file("a.txt", "worktree", "ignore-everything")
            .unwrap_err();
        match err {
            Error::Rule(m) => assert!(m.contains("ignore-everything"), "{m}"),
            other => panic!("expected a rule error, got {other:?}"),
        }
    }

    /// An untracked file still gets the synthesized all-add diff.
    #[test]
    fn diff_file_untracked_file_is_all_add() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("new.txt"), "alpha\nbeta\n").unwrap();

        let diff = CliEngine::new(p).diff_file("new.txt", "worktree", "none").unwrap();
        assert_eq!(diff.hunks.len(), 1, "untracked file shows as one all-add hunk");
        assert!(diff.hunks[0].lines.iter().all(|l| l.origin == "+"));
    }

}
