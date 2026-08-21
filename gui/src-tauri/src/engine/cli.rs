use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::GitEngine;
use crate::error::{Error, Result};
use crate::model::{
    BranchInfo, DiffLine, EditBlock, Eol, FileDiff, FileState, FileStatus, Hunk, RefKind, RefLabel,
    RepoSnapshot, TextFile,
};

/// Largest working-tree file offered for in-place editing: 2 MiB. Craft, not a
/// format limit — a textarea in the webview stops keeping up above it, and every
/// automatic save ships the whole text across the Tauri boundary.
pub const EDIT_SIZE_CEILING: u64 = 2 * 1024 * 1024;

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// FNV-1a over raw bytes, rendered as sixteen hex digits. The crate's only
/// implementation: `engine::log` fingerprints a filter's argument list with it and
/// this module fingerprints a file's bytes, and a second copy of the loop would be a
/// second chance to get the constants wrong.
///
/// A "did it change" probe, not a security boundary — a cryptographic hash would cost
/// a crate for the same answer.
pub(crate) fn fnv1a(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// git backend implemented by shelling out to the system `git`.
pub struct CliEngine {
    repo: PathBuf,
}

impl CliEngine {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self { repo: repo.into() }
    }

    /// Resolve the repository top-level for an arbitrary path inside a working tree.
    ///
    /// A pre-flight `metadata` call runs first, so the two cases that are not about
    /// git are not reported as git failures: a path that is gone (a remembered repo
    /// whose folder was deleted or lives on an unmounted volume), and a path macOS
    /// refuses to let this application read at all — on a first launch, or on every
    /// launch of a build with no stable signing identity, folders like Documents and
    /// Desktop are behind a TCC prompt, and a denial arrives as `PermissionDenied`.
    /// `git rev-parse --show-toplevel failed: not a git repository` is a true
    /// sentence about the wrong thing in both cases.
    ///
    /// This wraps failures git never saw; git's own stderr is still passed through
    /// verbatim below (докблок `error.rs`).
    pub fn resolve_root(path: &Path) -> Result<PathBuf> {
        if let Err(e) = std::fs::metadata(path) {
            let shown = path.display();
            return Err(match e.kind() {
                std::io::ErrorKind::NotFound => {
                    Error::Rule(format!("no such folder: {shown}"))
                }
                std::io::ErrorKind::PermissionDenied => Error::Rule(format!(
                    "{shown} cannot be read: macOS has not granted this application access to that folder"
                )),
                _ => Error::Io(format!("{shown}: {e}")),
            });
        }
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

    /// Resolve a client-supplied path against the repository root, refusing anything
    /// that leaves it — lexically **and** after following symlinks.
    ///
    /// The lexical pass alone would pass a symlink that lives inside the repository and
    /// points outside it. The real-path pass alone cannot answer for a file that is
    /// *missing*, which this feature has to report rather than fail on — so a missing
    /// tail is resolved through its deepest existing ancestor. Both sides are
    /// canonicalised before comparing: on macOS a temporary directory lives at
    /// `/var/...` whose real path is `/private/var/...`, and comparing the two spellings
    /// would reject perfectly ordinary paths.
    ///
    /// What comes back is the **resolved** path when the target exists. That is what
    /// makes a write go *through* a symlink rather than over it: `rename` does not
    /// follow one, so renaming onto the link's own path would replace the link with a
    /// plain file — the very change of type the outward-pointing link is refused for.
    fn worktree_path(&self, rel: &str) -> Result<PathBuf> {
        use std::path::Component;
        let outside = || Error::Rule(format!("{rel} is not a path inside the repository"));
        if rel.is_empty() {
            return Err(Error::Rule("no file path given".into()));
        }
        let candidate = Path::new(rel);
        for c in candidate.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                _ => return Err(outside()),
            }
        }
        let joined = self.repo.join(candidate);
        let root = std::fs::canonicalize(&self.repo)
            .map_err(|e| Error::Io(format!("{}: {e}", self.repo.display())))?;

        // The target itself: resolved, so a link is followed to what it really names.
        if let Ok(real) = std::fs::canonicalize(&joined) {
            return if real.starts_with(&root) {
                Ok(real)
            } else {
                Err(outside())
            };
        }
        // Not there yet — a component that does not exist cannot be a symlink, so the
        // deepest existing ancestor answers the question. The path is returned
        // unresolved: there is nothing to resolve it to.
        let mut probe = joined.as_path();
        loop {
            probe = match probe.parent() {
                Some(parent) if parent != probe => parent,
                _ => return Err(outside()),
            };
            if let Ok(real) = std::fs::canonicalize(probe) {
                return if real.starts_with(&root) {
                    Ok(joined)
                } else {
                    Err(outside())
                };
            }
        }
    }

    /// Read a working-tree file for in-place editing.
    ///
    /// Everything that makes the file unfit for editing comes back as a `blocked` key
    /// with `text: None` — not as an error: the reason has to be shown *before* the
    /// user reaches for the control, and a project rule says an inactive control must
    /// carry its reason. Only a path that is not the repository's business is an error.
    ///
    /// `text` is the whole file, its line endings normalised to `\n` for the webview
    /// and its trailing newline — or absence of one — left exactly as it lies on disk.
    /// It is the single truth about the bytes: `write_text_file` converts the endings
    /// back and writes what it is given, adding and removing nothing. `final_newline`
    /// travels alongside as information for the UI, not as an instruction to the write.
    pub fn read_text_file(&self, rel: &str) -> Result<TextFile> {
        let path = self.worktree_path(rel)?;
        let blocked = |b: EditBlock| TextFile {
            text: None,
            digest: String::new(),
            eol: Eol::Lf,
            final_newline: true,
            blocked: Some(b),
        };

        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            // A directory is as un-editable as an absent file, and for the same
            // reason: there is no text there.
            Ok(_) => return Ok(blocked(EditBlock::Missing)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(blocked(EditBlock::Missing))
            }
            Err(e) => return Err(Error::Io(format!("{rel}: {e}"))),
        };
        // Judged from the metadata, before reading: slurping a gigabyte only to
        // announce it is too big is the failure this ceiling exists to avoid.
        if meta.len() > EDIT_SIZE_CEILING {
            return Ok(blocked(EditBlock::TooLarge));
        }

        let bytes = std::fs::read(&path).map_err(|e| Error::Io(format!("{rel}: {e}")))?;
        // A NUL byte is what git itself calls binary, and no editor should hand it to
        // a textarea — valid UTF-8 or not.
        if bytes.contains(&0) {
            return Ok(blocked(EditBlock::Binary));
        }
        // Validated over the slice: a copy of a two-megabyte buffer is made only once
        // the file is known to be text, and never for a file that is not.
        let text = match std::str::from_utf8(&bytes) {
            Ok(t) => t,
            Err(_) => return Ok(blocked(EditBlock::Binary)),
        };

        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        let eol = match (crlf, lf) {
            // No CRLF at all: LF, and a file with no line endings whatsoever lands
            // here too — a one-line `VERSION` is an ordinary editable file.
            (0, _) => Eol::Lf,
            (c, l) if c == l => Eol::Crlf,
            // Rewriting a mixed file would normalise every line at once and show up
            // as a whole-file diff, so it is refused rather than silently repaired.
            _ => return Ok(blocked(EditBlock::MixedEol)),
        };
        // A bare `\r` (classic Mac, or a stray one inside a CRLF file) is mixed too:
        // it would not survive the round trip.
        if text.bytes().filter(|&b| b == b'\r').count() != crlf {
            return Ok(blocked(EditBlock::MixedEol));
        }

        Ok(TextFile {
            digest: fnv1a(&bytes),
            final_newline: text.ends_with('\n'),
            text: Some(text.replace("\r\n", "\n")),
            eol,
            blocked: None,
        })
    }

    /// Write an edited working-tree file back, and report the fingerprint of what now
    /// lies on disk.
    ///
    /// **Freshness is judged first**, before every other refusal. A write that is both
    /// stale and out of bounds is still, first of all, a file someone else changed:
    /// reporting the other reason would leave the outside change unannounced and the
    /// client without its "reread or overwrite" choice, which only `kind: "stale"`
    /// triggers.
    ///
    /// `expect` is not optional and has one reserved value: **the empty string means
    /// "there should be no file here"**. That is how the overwrite branch recreates a
    /// file deleted underneath the editor — a reread of a missing file reports
    /// `digest: ""`, and handing it back asks for exactly that state. It is unambiguous:
    /// an existing file never fingerprints to the empty string, not even an empty one.
    /// Every other value must match the bytes on disk, or the write is `Error::Stale`.
    ///
    /// Line endings in `text` are **normalised on the way in** — `\r\n`, a lone `\r` and
    /// `\n` alike all become line breaks, and every break leaves as `eol`. A lone `\r`
    /// arrives from a paste and, written through, would make the next read classify the
    /// file as `mixed-eol`: the application would have locked the user out of a file it
    /// wrote itself. Whatever this method writes, `read_text_file` can open again.
    /// Apart from the endings the text is written verbatim: no terminator is appended
    /// and none is removed.
    ///
    /// The write goes through a uniquely named temp file next to the target plus a
    /// rename, the way `changelists.json` and `graft-ui.json` are written — an
    /// interrupted write must not leave the file half-written. The target's permissions
    /// are carried over: a rename would otherwise hand a 755 script the temp file's mode
    /// and turn "one line changed" into a mode change in git.
    pub fn write_text_file(&self, rel: &str, text: &str, eol: Eol, expect: &str) -> Result<String> {
        let path = self.worktree_path(rel)?;

        match std::fs::read(&path) {
            Ok(current) => {
                if expect.is_empty() {
                    return Err(Error::Stale(format!(
                        "{rel} is on disk again; it was expected to be absent"
                    )));
                }
                if fnv1a(&current) != expect {
                    return Err(Error::Stale(format!(
                        "{rel} changed on disk since it was read"
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !expect.is_empty() {
                    return Err(Error::Stale(format!("{rel} was deleted on disk")));
                }
                // Absent, and absence is what the caller expected: recreated below.
            }
            Err(e) => return Err(Error::Io(format!("{rel}: {e}"))),
        }

        let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
        let bytes = match eol {
            Eol::Lf => normalised.into_bytes(),
            Eol::Crlf => normalised.replace('\n', "\r\n").into_bytes(),
        };
        // The ceiling guards the way out as well as the way in: a paste could otherwise
        // grow a file past the size at which `read_text_file` will open it again, and
        // the user would be locked out of the file the application itself wrote.
        if bytes.len() as u64 > EDIT_SIZE_CEILING {
            return Err(Error::Rule(format!(
                "{rel} would be larger than the {} MiB editing ceiling",
                EDIT_SIZE_CEILING / (1024 * 1024)
            )));
        }

        let dir = path
            .parent()
            .ok_or_else(|| Error::Io(format!("{rel} has no parent directory")))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::Io(format!("{rel} has no file name")))?;
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.graft.tmp.{}.{n}", std::process::id()));
        std::fs::write(&tmp, &bytes).map_err(|e| Error::Io(format!("{rel}: {e}")))?;
        if let Ok(meta) = std::fs::metadata(&path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::Io(format!("{rel}: {e}")));
        }
        Ok(fnv1a(&bytes))
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
    /// `trailing` (`--ignore-space-at-eol`) or `all` (`--ignore-all-space`). Any
    /// other value is rejected with `Error::Rule`: a mode folded into a default
    /// would show a diff nobody asked for and report nothing.
    ///
    /// `context` is how many unchanged lines to keep around each change; `None`
    /// leaves the command line without `-U` and reproduces the historical patch
    /// exactly (see [`context_arg`]).
    pub fn diff_file(
        &self,
        path: &str,
        against: &str,
        whitespace: &str,
        context: Option<u32>,
    ) -> Result<FileDiff> {
        let ws = whitespace_args(whitespace)?;
        let ctx = context_arg(context);
        let ctx: Vec<&str> = ctx.iter().map(String::as_str).collect();
        let raw = match against {
            "index" => {
                let mut a = vec!["diff", "--cached"];
                a.extend_from_slice(&ws);
                a.extend_from_slice(&ctx);
                a.extend_from_slice(&["--", path]);
                self.git(&a)?
            }
            "head" => {
                let mut a = vec!["diff", "HEAD"];
                a.extend_from_slice(&ws);
                a.extend_from_slice(&ctx);
                a.extend_from_slice(&["--", path]);
                self.git(&a)?
            }
            _ => {
                let mut a = vec!["diff"];
                a.extend_from_slice(&ws);
                a.extend_from_slice(&ctx);
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
                    a.extend_from_slice(&ctx);
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

    /// Push.
    ///
    /// `upstream` sets `-u` for a branch with no upstream, `force` is
    /// `--force-with-lease`, `force-hard` is a bare `--force`. Both forcing
    /// modes are only ever reached after the plain push was refused and the
    /// reader picked one of them by name.
    ///
    /// There is no catch-all arm: an unrecognised mode used to fall through to a
    /// plain push, so a typo in the caller looked like a working button that
    /// quietly did the *safe* thing — and the two forcing modes differ precisely
    /// in what they are allowed to destroy.
    pub fn push(&self, mode: &str) -> Result<()> {
        match mode {
            "upstream" => {
                let br = self.current_branch()?;
                self.git(&["push", "-u", "origin", &br])?;
            }
            "force" => {
                self.git(&["push", "--force-with-lease"])?;
            }
            "force-hard" => {
                self.git(&["push", "--force"])?;
            }
            "normal" => {
                self.git(&["push"])?;
            }
            other => {
                return Err(Error::Rule(format!("unknown push mode: {other}")));
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
///
/// Visible to the whole engine: a commit's diff has the same shape as a worktree
/// diff, and a second parser would be a second set of edge cases (binary files,
/// renames, "\ No newline") drifting away from this one.
pub(crate) fn parse_diff(path: &str, raw: &str) -> FileDiff {
    if raw.contains("Binary files ") || raw.contains("GIT binary patch") {
        return FileDiff {
            path: path.into(),
            binary: true,
            ..FileDiff::default()
        };
    }
    let lines: Vec<&str> = raw.split('\n').collect();
    let Some(first) = lines.iter().position(|l| l.starts_with("@@")) else {
        return FileDiff {
            path: path.into(),
            binary: false,
            ..FileDiff::default()
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
        ..FileDiff::default()
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

/// Parse the `%D` decoration of a commit — "HEAD -> main, origin/main, tag: v1" —
/// into typed labels.
///
/// Lives here, next to the other parsers of git output, because the log rows and the
/// commit card decorate the same commits and two copies of this parse drift apart:
/// the first thing lost is the distinction between a remote branch and a local one
/// whose name merely contains a slash. `remotes` is the repo's remote list — the
/// only way to tell `origin/main` from a local `origin/main`-shaped branch.
pub(crate) fn parse_refs(deco: &str, remotes: &[String]) -> Vec<RefLabel> {
    let mut out = Vec::new();
    for raw in deco.split(", ") {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let (name, kind) = if let Some(tag) = t.strip_prefix("tag: ") {
            (tag.trim(), RefKind::Tag)
        } else if let Some(branch) = t.strip_prefix("HEAD -> ") {
            // the branch HEAD currently points at
            (branch.trim(), RefKind::Head)
        } else if t == "HEAD" {
            // detached: HEAD decorates the commit on its own
            (t, RefKind::Head)
        } else if remotes.iter().any(|r| t.starts_with(&format!("{r}/"))) {
            (t, RefKind::Remote)
        } else {
            (t, RefKind::Local)
        };
        out.push(RefLabel {
            name: name.to_string(),
            kind,
        });
    }
    out
}

/// `user.email` as the repository resolves it (local, global or system), or
/// `None` when git has none configured.
///
/// A missing value is not an error: a repository without an identity is a
/// repository whose reader simply has no "my commits" to emphasise, and failing
/// the whole state read over it would be worse than saying nothing.
///
/// Read once per repository and kept for the session: every mutation rebuilds
/// `RepoState`, so an uncached read would spawn a `git config` process on each
/// stage, commit and checkout to learn a value that does not change while the
/// application is open. Keyed by path rather than memoised once, so switching
/// repositories still gets that repository's own identity.
pub fn user_email(repo: &Path) -> Option<String> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, Option<String>>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(repo) {
            return hit.clone();
        }
    }
    let value = read_user_email(repo);
    if let Ok(mut map) = cache.lock() {
        map.insert(repo.to_path_buf(), value.clone());
    }
    value
}

fn read_user_email(repo: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["config", "--get", "user.email"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// git flags for a whitespace mode: `none` | `trailing` | `all`.
///
/// A closed dictionary crossing the Tauri boundary as a string is checked, not
/// folded into a default: a typo that silently means "none" shows a diff the user
/// did not ask for and reports nothing.

/// `-U<n>` for a requested amount of context around each change, or nothing.
///
/// `None` is not "zero" and not "three": it means the caller did not ask, and the
/// command line then carries no `-U` at all — byte for byte the command this
/// project has always run, so the historical output is reproduced rather than
/// re-derived from git's current default (R46i, D04).
pub fn context_arg(context: Option<u32>) -> Option<String> {
    context.map(|n| format!("-U{n}"))
}

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

    // ---- read_text_file / write_text_file (prd_03 task 01) ----

    /// The read/write round trip must be byte-identical when nothing was changed.
    /// This is the invariant "two saves in a row" and the staleness check both rest
    /// on: if writing back an untouched file changed a single byte, the digest the
    /// write returns would describe a file the user never asked for.
    #[test]
    fn reading_and_writing_back_leaves_the_bytes_alone() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\ntwo\nthree\n").unwrap();
        let eng = CliEngine::new(p);

        let f = eng.read_text_file("t.txt").unwrap();
        assert_eq!(f.blocked, None);
        assert_eq!(f.text.as_deref(), Some("one\ntwo\nthree\n"));
        assert_eq!(f.eol, Eol::Lf);
        assert!(f.final_newline);

        let d = eng
            .write_text_file("t.txt", f.text.as_deref().unwrap(), f.eol, &f.digest)
            .unwrap();
        assert_eq!(
            std::fs::read(p.join("t.txt")).unwrap(),
            b"one\ntwo\nthree\n".to_vec()
        );
        assert_eq!(d, f.digest, "unchanged bytes must fingerprint the same");
    }

    #[test]
    fn an_edit_reaches_the_file() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\ntwo\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        eng.write_text_file("t.txt", "one\nTWO\n", f.eol, &f.digest)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("t.txt")).unwrap(),
            "one\nTWO\n"
        );
    }

    /// Scenario "Two saves in a row": the second save uses the digest the first
    /// returned, and succeeds — the file on disk is the application's own write.
    #[test]
    fn two_saves_in_a_row_succeed() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        let d1 = eng
            .write_text_file("t.txt", "two\n", f.eol, &f.digest)
            .unwrap();
        let d2 = eng
            .write_text_file("t.txt", "three\n", f.eol, &d1)
            .expect("second save must not be refused as stale");
        assert_ne!(d1, d2);
        assert_eq!(std::fs::read_to_string(p.join("t.txt")).unwrap(), "three\n");
    }

    /// Scenario "File changed by another program". The refusal is asserted through
    /// the *serialization* — that is the seam the client branches on; matching the
    /// prose would break on the first rewording or translation.
    #[test]
    fn a_stale_digest_is_refused_and_the_file_is_untouched() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        std::fs::write(p.join("t.txt"), "changed by someone else\n").unwrap();

        let err = eng
            .write_text_file("t.txt", "mine\n", f.eol, &f.digest)
            .expect_err("a write over an outside change must be refused");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "stale");
        assert_eq!(
            std::fs::read_to_string(p.join("t.txt")).unwrap(),
            "changed by someone else\n",
            "a refused write must not touch the file"
        );
    }

    /// Scenario "A file with CRLF endings": editing one line leaves every other
    /// ending as it was.
    #[test]
    fn crlf_endings_survive_an_edit() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("w.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("w.txt").unwrap();
        assert_eq!(f.eol, Eol::Crlf);
        assert_eq!(f.text.as_deref(), Some("one\ntwo\nthree\n"));
        assert!(f.final_newline);

        eng.write_text_file("w.txt", "one\nTWO\nthree\n", f.eol, &f.digest)
            .unwrap();
        assert_eq!(
            std::fs::read(p.join("w.txt")).unwrap(),
            b"one\r\nTWO\r\nthree\r\n".to_vec()
        );
    }

    #[test]
    fn a_file_without_a_final_newline_keeps_none() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\ntwo").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        assert!(!f.final_newline);
        assert_eq!(f.text.as_deref(), Some("one\ntwo"));
        eng.write_text_file("t.txt", "one\nTWO", f.eol, &f.digest)
            .unwrap();
        assert_eq!(
            std::fs::read(p.join("t.txt")).unwrap(),
            b"one\nTWO".to_vec()
        );
    }

    /// Scenario "Mixed line endings".
    #[test]
    fn mixed_line_endings_block_editing() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("m.txt"), "one\r\ntwo\nthree\r\n").unwrap();
        let f = CliEngine::new(p).read_text_file("m.txt").unwrap();
        assert_eq!(f.blocked, Some(EditBlock::MixedEol));
        assert_eq!(f.text, None);
    }

    /// Scenario "Binary file".
    #[test]
    fn a_non_utf8_file_blocks_editing() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.bin"), [0x00, 0xff, 0xfe, b'a']).unwrap();
        let f = CliEngine::new(p).read_text_file("b.bin").unwrap();
        assert_eq!(f.blocked, Some(EditBlock::Binary));
        assert_eq!(f.text, None);
    }

    /// Scenario "File above the size ceiling".
    #[test]
    fn a_file_above_the_ceiling_blocks_editing() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(
            p.join("big.txt"),
            vec![b'x'; EDIT_SIZE_CEILING as usize + 1],
        )
        .unwrap();
        let f = CliEngine::new(p).read_text_file("big.txt").unwrap();
        assert_eq!(f.blocked, Some(EditBlock::TooLarge));
        assert_eq!(f.text, None);
    }

    /// Scenario "File no longer on disk".
    #[test]
    fn a_missing_file_blocks_editing() {
        let dir = scratch_repo();
        let f = CliEngine::new(dir.path())
            .read_text_file("gone.txt")
            .unwrap();
        assert_eq!(f.blocked, Some(EditBlock::Missing));
        assert_eq!(f.text, None);
    }

    /// Project rule: the command takes a path from the client, so `..`, an absolute
    /// path and a path that leaves the repository root are all refused — on read and
    /// on write alike.
    #[test]
    fn a_path_outside_the_repository_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        let eng = CliEngine::new(p);
        for bad in ["../outside.txt", "sub/../../outside.txt", "/etc/hosts"] {
            let e = eng
                .read_text_file(bad)
                .err()
                .unwrap_or_else(|| panic!("read of {bad} must be refused"));
            assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "rule");
            let e = eng
                .write_text_file(bad, "x\n", Eol::Lf, "")
                .err()
                .unwrap_or_else(|| panic!("write of {bad} must be refused"));
            assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "rule");
        }
    }

    /// A write leaves no leftovers next to the file: the temp name is unique per
    /// call and always renamed away.
    #[test]
    fn a_write_leaves_no_temp_file_behind() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("sub/t.txt").unwrap();
        eng.write_text_file("sub/t.txt", "two\n", f.eol, &f.digest)
            .unwrap();
        let names: Vec<_> = std::fs::read_dir(p.join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["t.txt".to_string()]);
    }

    /// `text` is the whole truth about the bytes: whatever tail it carries is written
    /// verbatim, and nothing is appended or trimmed on the way. Both directions are
    /// checked here — a tail grown by a blank line, and a tail that is not there.
    #[test]
    fn the_tail_of_the_text_is_written_verbatim() {
        let dir = scratch_repo();
        let p = dir.path();
        let eng = CliEngine::new(p);
        for (on_disk, typed) in [
            ("one\ntwo", "one\ntwo\n\n"),
            ("one\ntwo\n", "one\ntwo"),
            ("one\ntwo\n", "one\ntwo\n\n\n"),
        ] {
            std::fs::write(p.join("t.txt"), on_disk).unwrap();
            let f = eng.read_text_file("t.txt").unwrap();
            assert_eq!(
                f.text.as_deref(),
                Some(on_disk),
                "read gives back the tail too"
            );
            let d = eng
                .write_text_file("t.txt", typed, f.eol, &f.digest)
                .unwrap();
            assert_eq!(
                std::fs::read_to_string(p.join("t.txt")).unwrap(),
                typed,
                "the file holds exactly the text it was handed"
            );
            let again = eng.read_text_file("t.txt").unwrap();
            assert_eq!(
                again.text.as_deref(),
                Some(typed),
                "a reread agrees with the write"
            );
            assert_eq!(again.digest, d);
        }
    }

    /// A rename hands the target the temp file's mode unless it is carried over, and
    /// a 755 script silently losing its exec bit turns "one line changed" into a mode
    /// change in git.
    #[cfg(unix)]
    #[test]
    fn a_write_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_repo();
        let p = dir.path();
        let f = p.join("run.sh");
        std::fs::write(&f, "echo one\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        let eng = CliEngine::new(p);
        let t = eng.read_text_file("run.sh").unwrap();
        eng.write_text_file("run.sh", "echo two\n", t.eol, &t.digest)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    /// A symlink living inside the repository but pointing outside it passes every
    /// lexical check. Reading it would show a file the repository does not contain, and
    /// writing it would additionally replace the link with a plain file, because
    /// `rename` does not follow one.
    #[cfg(unix)]
    #[test]
    fn a_symlink_that_leaves_the_repository_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("outside.txt");
        std::fs::write(&outside, "not ours\n").unwrap();
        std::os::unix::fs::symlink(&outside, p.join("link.txt")).unwrap();
        let eng = CliEngine::new(p);

        let e = eng
            .read_text_file("link.txt")
            .expect_err("read must be refused");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "rule");
        let e = eng
            .write_text_file("link.txt", "mine\n", Eol::Lf, "")
            .expect_err("write must be refused");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "rule");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "not ours\n");
        assert!(
            std::fs::symlink_metadata(p.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself must survive a refused write"
        );
    }

    /// A deletion is an outside change like any other, so it has to arrive at the same
    /// seam: the client offers "reread or overwrite" on `kind: "stale"` alone.
    #[test]
    fn a_file_deleted_under_us_is_stale_not_io() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        std::fs::remove_file(p.join("t.txt")).unwrap();
        let e = eng
            .write_text_file("t.txt", "mine\n", f.eol, &f.digest)
            .expect_err("writing over a deleted file must be refused");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "stale");
    }

    /// The ceiling guards the way out too: growing a file past it would leave a file the
    /// reader refuses to reopen, written by the application itself.
    #[test]
    fn a_write_above_the_ceiling_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        let huge = "x".repeat(EDIT_SIZE_CEILING as usize + 1);
        let e = eng
            .write_text_file("t.txt", &huge, f.eol, &f.digest)
            .expect_err("a write past the ceiling must be refused");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "rule");
        assert_eq!(std::fs::read_to_string(p.join("t.txt")).unwrap(), "one\n");
    }

    /// The two dimensions crossed: CRLF endings and no terminator on the last line.
    #[test]
    fn crlf_without_a_final_newline_survives() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("w.txt"), "one\r\ntwo").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("w.txt").unwrap();
        assert_eq!(f.eol, Eol::Crlf);
        assert!(!f.final_newline);
        assert_eq!(f.text.as_deref(), Some("one\ntwo"));
        eng.write_text_file("w.txt", "one\nTWO", f.eol, &f.digest)
            .unwrap();
        assert_eq!(
            std::fs::read(p.join("w.txt")).unwrap(),
            b"one\r\nTWO".to_vec()
        );
    }

    /// A file with no line endings at all — a one-line `VERSION` — has nothing to be
    /// inconsistent about and must not be mistaken for mixed endings.
    #[test]
    fn a_file_with_no_line_endings_is_editable() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("VERSION"), "1.2.3").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("VERSION").unwrap();
        assert_eq!(f.blocked, None);
        assert_eq!(f.eol, Eol::Lf);
        assert!(!f.final_newline);
        eng.write_text_file("VERSION", "1.2.4", f.eol, &f.digest)
            .unwrap();
        assert_eq!(std::fs::read(p.join("VERSION")).unwrap(), b"1.2.4".to_vec());
    }

    /// The overwrite branch after an outside deletion: a reread reports `missing` with
    /// an empty digest, and handing that digest back means "there should be no file
    /// here" — which is what recreates it with the typed text.
    #[test]
    fn an_empty_expect_recreates_a_deleted_file() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        std::fs::remove_file(p.join("t.txt")).unwrap();

        let after = eng.read_text_file("t.txt").unwrap();
        assert_eq!(after.blocked, Some(EditBlock::Missing));
        assert_eq!(after.digest, "");

        let d = eng
            .write_text_file("t.txt", "typed\n", f.eol, &after.digest)
            .expect("overwriting a deleted file must recreate it");
        assert_eq!(std::fs::read_to_string(p.join("t.txt")).unwrap(), "typed\n");
        assert_eq!(eng.read_text_file("t.txt").unwrap().digest, d);
    }

    /// The empty digest is a claim about the file's absence, not a way to skip the
    /// probe: a file that came back is an outside change like any other.
    #[test]
    fn an_empty_expect_is_refused_when_the_file_exists() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "someone else\n").unwrap();
        let e = CliEngine::new(p)
            .write_text_file("t.txt", "mine\n", Eol::Lf, "")
            .expect_err("a present file must not be overwritten by an absence claim");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "stale");
        assert_eq!(
            std::fs::read_to_string(p.join("t.txt")).unwrap(),
            "someone else\n"
        );
    }

    /// A write that is both stale and out of bounds is first of all a file someone else
    /// changed: only `kind: "stale"` gives the client its "reread or overwrite" choice,
    /// so reporting the other reason would leave the outside change unannounced.
    #[test]
    fn staleness_is_judged_before_the_ceiling() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        std::fs::write(p.join("t.txt"), "changed by someone else\n").unwrap();

        let huge = "x".repeat(EDIT_SIZE_CEILING as usize + 1);
        let e = eng
            .write_text_file("t.txt", &huge, f.eol, &f.digest)
            .expect_err("both refusals apply");
        assert_eq!(
            serde_json::to_value(&e).unwrap()["kind"],
            "stale",
            "the outside change must be the reason the client hears"
        );
    }

    /// A symlink whose target is inside the repository is written *through*: `rename`
    /// does not follow one, so renaming onto the link's own path would swap the link for
    /// a plain file — a change of type in git, from an edit of one line.
    #[cfg(unix)]
    #[test]
    fn a_write_goes_through_a_symlink_inside_the_repository() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir_all(p.join("real")).unwrap();
        std::fs::write(p.join("real/t.txt"), "one\n").unwrap();
        std::os::unix::fs::symlink(p.join("real/t.txt"), p.join("link.txt")).unwrap();
        let eng = CliEngine::new(p);

        let f = eng.read_text_file("link.txt").unwrap();
        assert_eq!(f.blocked, None);
        eng.write_text_file("link.txt", "two\n", f.eol, &f.digest)
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(p.join("real/t.txt")).unwrap(),
            "two\n",
            "the target holds the new text"
        );
        assert!(
            std::fs::symlink_metadata(p.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must still be a link"
        );
    }

    /// The invariant: a file the application just wrote is a file the application can
    /// open again. A lone `\r` arrives from a paste, and written through it would make
    /// the next read call the file `mixed-eol`.
    #[test]
    fn a_pasted_lone_carriage_return_does_not_lock_the_file() {
        let dir = scratch_repo();
        let p = dir.path();
        for (eol, on_disk, expect_bytes) in [
            (Eol::Lf, "one\n", b"one\ntwo\nthree\n".to_vec()),
            (Eol::Crlf, "one\r\n", b"one\r\ntwo\r\nthree\r\n".to_vec()),
        ] {
            std::fs::write(p.join("t.txt"), on_disk).unwrap();
            let eng = CliEngine::new(p);
            let f = eng.read_text_file("t.txt").unwrap();
            assert_eq!(f.eol, eol);
            // What a paste from a classic-Mac source looks like.
            eng.write_text_file("t.txt", "one\rtwo\r\nthree\n", f.eol, &f.digest)
                .unwrap();
            assert_eq!(std::fs::read(p.join("t.txt")).unwrap(), expect_bytes);

            let again = eng.read_text_file("t.txt").unwrap();
            assert_eq!(
                again.blocked, None,
                "the application must reopen its own write"
            );
            assert_eq!(again.text.as_deref(), Some("one\ntwo\nthree\n"));
        }
    }

    #[test]
    fn ref_labels_carry_their_kind() {
        let remotes = vec!["origin".to_string()];
        let refs = parse_refs("HEAD -> main, origin/main, tag: v1, later, feature/main", &remotes);
        let kind = |name: &str| {
            refs.iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no ref {name} in {refs:?}"))
                .kind
        };
        assert_eq!(refs.len(), 5, "every decoration becomes one label: {refs:?}");
        assert_eq!(kind("main"), RefKind::Head, "HEAD -> main is the current branch head");
        assert_eq!(kind("origin/main"), RefKind::Remote);
        assert_eq!(kind("v1"), RefKind::Tag);
        assert_eq!(kind("later"), RefKind::Local);
        assert_eq!(kind("feature/main"), RefKind::Local, "a slash alone does not make a remote");

        // detached HEAD decorates on its own, and an empty decoration is no labels
        assert_eq!(parse_refs("HEAD", &remotes)[0].kind, RefKind::Head);
        assert!(parse_refs("", &remotes).is_empty());
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
        let diff = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        assert_eq!(diff.hunks.len(), 2, "two separated hunks");

        // stage only the first hunk
        eng.apply_patch(&diff.hunks[0].patch, true, false).unwrap();
        assert!(eng
            .git(&["diff", "--cached", "--name-only"])
            .unwrap()
            .contains("f.txt"));
        let remaining = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
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

    /// A path that is not there is not a git failure: the pre-flight names the
    /// folder instead of reporting `rev-parse --show-toplevel` over it. The same
    /// door catches a folder macOS refuses to let the application read.
    #[test]
    fn resolving_a_missing_folder_says_so_instead_of_blaming_git() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("not-here");
        match CliEngine::resolve_root(&gone) {
            Err(Error::Rule(m)) => assert!(m.contains("no such folder"), "{m}"),
            other => panic!("expected a domain refusal: {other:?}"),
        }
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
            eng.diff_file("f.txt", "worktree", "none", None).unwrap().hunks.len(),
            1,
            "do-not-ignore shows the whitespace-only change"
        );
        assert!(
            eng.diff_file("f.txt", "worktree", "all", None).unwrap().hunks.is_empty(),
            "ignore-all-whitespace hides it"
        );
        let trailing = eng.diff_file("f.txt", "worktree", "trailing", None).unwrap();
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
        let d = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        assert_eq!(d.hunks.len(), 1, "prior behaviour: synthesized all-add diff");
        assert!(d.hunks[0].lines.iter().all(|l| l.origin == "+"));
        assert_eq!(
            eng.diff_file("f.txt", "index", "none", None).unwrap().hunks.len(),
            1,
            "the staged change is visible against the index"
        );
    }

    /// A closed dictionary is checked at the boundary, not folded into a default.
    #[test]
    fn diff_file_rejects_unknown_whitespace_mode() {
        let dir = scratch_repo();
        let err = CliEngine::new(dir.path())
            .diff_file("a.txt", "worktree", "ignore-everything", None)
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

        let diff = CliEngine::new(p).diff_file("new.txt", "worktree", "none", None).unwrap();
        assert_eq!(diff.hunks.len(), 1, "untracked file shows as one all-add hunk");
        assert!(diff.hunks[0].lines.iter().all(|l| l.origin == "+"));
    }

    /// `user.email` is read once per repository: every mutation rebuilds
    /// `RepoState`, and a `git config` process per stage is a cost paid for a
    /// value that cannot change while the application is open. The identity is
    /// rewritten between the two reads — the second answer is the first one.
    #[test]
    fn user_email_is_read_once_per_repo() {
        let dir = scratch_repo();
        let p = dir.path();
        assert_eq!(user_email(p).as_deref(), Some("t@example.com"));

        run(p, &["config", "user.email", "other@example.com"]);
        assert_eq!(
            user_email(p).as_deref(),
            Some("t@example.com"),
            "the cached value is kept for the session"
        );

        // Another repository is another identity, not the cached one.
        let other = tempfile::tempdir().unwrap();
        run(other.path(), &["init", "-b", "main"]);
        run(other.path(), &["config", "user.email", "second@example.com"]);
        assert_eq!(user_email(other.path()).as_deref(), Some("second@example.com"));
    }
}
