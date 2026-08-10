use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::GitEngine;
use crate::error::{Error, Result};
use crate::model::{DiffLine, FileDiff, FileState, FileStatus, Hunk, RepoSnapshot};

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

    /// Diff a file against a base: `worktree` (unstaged), `index` (staged) or `head`.
    pub fn diff_file(&self, path: &str, against: &str) -> Result<FileDiff> {
        let raw = match against {
            "index" => self.git(&["diff", "--cached", "--", path])?,
            "head" => self.git(&["diff", "HEAD", "--", path])?,
            _ => {
                let d = self.git(&["diff", "--", path])?;
                if d.trim().is_empty() {
                    // untracked/new file: synthesize an all-add diff (view only)
                    self.git_allow_fail(&["diff", "--no-index", "--", "/dev/null", path])
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
            return Err(Error::Rule("сообщение коммита не может быть пустым".into()));
        }
        self.stage_paths(paths)?;
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
        }
        self.git(&args)?;
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
mod tests {
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
        let diff = eng.diff_file("f.txt", "worktree").unwrap();
        assert_eq!(diff.hunks.len(), 2, "two separated hunks");

        // stage only the first hunk
        eng.apply_patch(&diff.hunks[0].patch, true, false).unwrap();
        assert!(eng
            .git(&["diff", "--cached", "--name-only"])
            .unwrap()
            .contains("f.txt"));
        let remaining = eng.diff_file("f.txt", "worktree").unwrap();
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
}
