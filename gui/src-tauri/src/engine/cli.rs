use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::commit::parse_name_status;
use super::exec;
use super::patch::{self, Kind, Pick};
use super::GitEngine;
use crate::error::{Error, Result};
use crate::model::{
    BranchInfo, CommitFileEntry, DiffLine, EditBlock, Eol, FileDiff, FileState, FileStatus, Hunk,
    HunkPick, LinePick, RefKind, RefLabel, RepoSnapshot, TextFile,
};

/// Largest working-tree file offered for in-place editing: 2 MiB. Craft, not a
/// format limit — a textarea in the webview stops keeping up above it, and every
/// automatic save ships the whole text across the Tauri boundary.
pub const EDIT_SIZE_CEILING: u64 = 2 * 1024 * 1024;

pub(crate) static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A throwaway index file inside the git directory, removed (with its lock) on drop.
/// The crate's only one: the discard backups build their trees in it, and a list
/// commit builds its whole commit in it.
///
/// It must not exist when git first opens it: a missing index is an empty one, a
/// zero-byte file is a corrupt one. The path comes from `rev-parse --git-path`, so a
/// linked worktree gets its own.
pub(crate) struct TempIndex(PathBuf);

impl TempIndex {
    pub(crate) fn new(repo: &Path, prefix: &str) -> Result<Self> {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!("{prefix}-{}-{n}-{nanos}.index", std::process::id());
        let path = CliEngine::new(repo)
            .git_paths(&[name.as_str()])?
            .pop()
            .ok_or_else(|| Error::Parse("rev-parse --git-path returned nothing".into()))?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The value for `GIT_INDEX_FILE`.
    pub(crate) fn env_value(&self) -> String {
        self.0.to_string_lossy().to_string()
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

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

/// A file path as a git pathspec that matches that one file and nothing else.
///
/// A bare path after `--` is still a pattern: `app/[id]/page.tsx` also matches
/// `app/i/page.tsx`, so a rollback of the first reverts the second, and `rm -f` of a
/// new `x[ab].txt` deletes a tracked `xa.txt` from disk. Every client-supplied path
/// that reaches a pathspec slot goes through here. Not for `diff --no-index`, which
/// takes plain filesystem paths, and not for `git blame`, which reads its path
/// literally already and would look for a file named `:(literal)…`.
pub(crate) fn literal(path: &str) -> String {
    format!(":(literal){path}")
}

/// Refuse a new branch name git would not accept, **before** anything is changed.
///
/// Asked of `git check-ref-format --branch` rather than re-implemented: git's rules
/// are the ones that matter, and they move between versions. Three things on top:
///
/// * A leading `-` is refused before git is run at all — the name would be read as
///   an option by the very command that checks it.
/// * `--branch` *expands* `@{-1}` to the previously checked-out branch and exits 0,
///   so an echo that differs from the input is a refusal too: the name would create
///   or rename to something other than what was typed.
/// * A bare `@` passes the check and makes a branch that shadows `HEAD`'s shorthand.
///
/// Any non-zero exit is a refusal (`Error::Rule`): `--branch` dies with 128 on an
/// invalid name and reads no repository state but the reflog for `@{-N}`, so there
/// is no second failure mode for it to be confused with.
pub(crate) fn check_branch_name(repo: &Path, name: &str) -> Result<()> {
    let refuse =
        |why: &str| Err(Error::Rule(format!("\"{name}\" is not a valid branch name{why}")));
    if name.is_empty() {
        return Err(Error::Rule("branch name is empty".into()));
    }
    if name.starts_with('-') {
        return refuse(": it cannot start with \"-\"");
    }
    if name == "@" {
        return refuse(": \"@\" is git's shorthand for HEAD");
    }
    let out = exec::git(repo, &["check-ref-format", "--branch", name]).run()?;
    if !out.success() {
        return refuse("");
    }
    if out.stdout_text().trim_end_matches(['\n', '\r']) != name {
        return refuse(": git reads it as a reference to another branch");
    }
    Ok(())
}

/// Refuse a new tag name git would not accept, before anything is changed — the
/// counterpart of [`check_branch_name`], checked as the full ref `refs/tags/<name>`.
///
/// Here the exit code *is* tri-state: 1 is "invalid", anything else non-zero is a
/// question git failed to answer and stays an `Error::Git` (докблок `error.rs`).
pub(crate) fn check_tag_name(repo: &Path, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::Rule("tag name is empty".into()));
    }
    if name.starts_with('-') {
        return Err(Error::Rule(format!(
            "\"{name}\" is not a valid tag name: it cannot start with \"-\""
        )));
    }
    let full = format!("refs/tags/{name}");
    let out = exec::git(repo, &["check-ref-format", full.as_str()]).run()?;
    match out.code {
        Some(0) => Ok(()),
        Some(1) => Err(Error::Rule(format!("\"{name}\" is not a valid tag name"))),
        _ => Err(out.fail_stderr()),
    }
}

/// Take back the directories a write made for itself, after that write failed.
///
/// **Only empty ones, from the deepest up, stopping at the first that will not go.**
/// The right this has is over what this write created and nothing else: another process
/// may have put a file into the new directory between the `create_dir_all` and the
/// refusal, and removing the subtree would take that file with it. A directory that is
/// no longer there is skipped rather than ending the walk — a partly made chain still
/// has its shallower links to undo.
///
/// Best-effort: the write is already being refused, and a failure to tidy up must not
/// replace the reason the caller is waiting for.
fn undo(deepest: &Path, created: &Option<PathBuf>) {
    let Some(top) = created else { return };
    let mut cur = deepest;
    loop {
        if cur.exists() && std::fs::remove_dir(cur).is_err() {
            return;
        }
        if cur == top.as_path() {
            return;
        }
        cur = match cur.parent() {
            Some(p) => p,
            None => return,
        };
    }
}

/// Remove a file or link; one already gone is fine, any other failure is reported.
fn remove_existing(abs: &Path, rel: &str) -> Result<()> {
    match std::fs::remove_file(abs) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(Error::Io(format!("{rel}: {e}")))
        }
        _ => Ok(()),
    }
}

/// Remove every folder under `dir` (and `dir` itself) left empty, deepest first.
/// A folder that still holds something — an ignored file — stays; links are not
/// followed. Returns whether `dir` was removed.
fn prune_empty_dirs(dir: &Path, rel: &str) -> Result<bool> {
    let io = |e: std::io::Error| Error::Io(format!("{rel}: {e}"));
    let mut empty = true;
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let is_dir = entry.file_type().map_err(io)?.is_dir();
        if !(is_dir && prune_empty_dirs(&entry.path(), rel)?) {
            empty = false;
        }
    }
    if empty {
        std::fs::remove_dir(dir).map_err(io)?;
    }
    Ok(empty)
}

/// git backend implemented by shelling out to the system `git`.
pub struct CliEngine {
    repo: PathBuf,
}

/// Both streams and the exit code of an arbitrary `git` invocation, captured
/// regardless of success — see [`CliEngine::exec_raw`].
pub struct RawOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// Journal entry id of this run.
    pub journal: u64,
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
        let out = exec::git(path, &["rev-parse", "--show-toplevel"]).run()?;
        if !out.success() {
            return Err(out.fail_stderr());
        }
        Ok(PathBuf::from(out.stdout_text().trim().to_string()))
    }

    /// Run `git -C <repo> <args>` capturing raw stdout bytes. On failure produce
    /// `Error::Git` carrying the command and its stderr verbatim.
    fn git_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        exec::git(&self.repo, args).run()?.checked()
    }

    /// Run git capturing stdout as UTF-8 text (git errors still carry stderr).
    fn git(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git_bytes(args)?).to_string())
    }

    /// [`Self::git`] for a command that talks to a remote — `exec::Git::network`:
    /// no prompt, a stalled connection gives up.
    fn git_net(&self, args: &[&str]) -> Result<String> {
        let out = exec::git(&self.repo, args).network().run()?.checked()?;
        Ok(String::from_utf8_lossy(&out).to_string())
    }

    /// Run `git -C <repo> <args>` for the git console panel — an arbitrary
    /// command the user typed, at their own privilege level; this is not a
    /// sandbox, and no attempt is made to restrict what git is asked to do.
    ///
    /// Unlike `git`/`git_bytes`, a non-zero exit is **not** an `Error::Git`: it is
    /// output the user typed the command to see (`git status` on a bad pathspec,
    /// `git branch -D` on an unmerged branch), not a failure of this application.
    /// Only a failure to spawn `git` itself becomes an `Err`.
    ///
    /// stdin is `/dev/null` and the environment tells git never to open an
    /// interactive prompt or editor — `commit` with no `-m`, `rebase -i`,
    /// `tag -a`, and a credential prompt on `push`/`pull` would otherwise spawn
    /// something that waits forever for input this process never supplies,
    /// hanging the whole application on the first such command.
    pub fn exec_raw(&self, args: &[String]) -> Result<RawOutput> {
        // `network()`: any typed command may be a fetch or a push — no prompt, the
        // ssh and http stall limits of every other remote call.
        let out = exec::git(&self.repo, args)
            .network()
            .env(&[("GIT_EDITOR", "false"), ("GIT_SEQUENCE_EDITOR", "false")])
            .run()?;
        Ok(RawOutput {
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            exit_code: out.code.unwrap_or(-1),
            journal: out.journal,
        })
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
    ///
    /// An untracked folder arrives as one entry, `dir/` — `git status` collapses it that
    /// way — and is removed file by file: the untracked files under it (the same list
    /// the discard backup takes, [`Self::untracked_under`]), then every folder left
    /// empty, from the bottom up. Ignored files stay, and so do the folders holding them.
    pub fn rollback(&self, paths: &[String]) -> Result<()> {
        for p in paths {
            let in_head = self.git(&["cat-file", "-e", &format!("HEAD:{p}")]).is_ok();
            let spec = literal(p);
            if in_head {
                self.git(&["checkout", "HEAD", "--", &spec])?;
                continue;
            }
            // Unstage it if it was `git add`ed. Fails, harmlessly, for a path git does
            // not have in the index — an untracked file or folder.
            let _ = self.git(&["rm", "-f", "--", &spec]);
            let abs = self.repo.join(p);
            match std::fs::symlink_metadata(&abs) {
                Ok(meta) if meta.is_dir() => self.remove_untracked_dir(p, &abs)?,
                Ok(_) => remove_existing(&abs, p)?,
                // `git rm -f` already took it off the disk.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::Io(format!("{p}: {e}"))),
            }
        }
        Ok(())
    }

    /// Untracked files under the folder `rel`, as `git status` would list them: ignored
    /// ones left out. Paths relative to the root.
    pub(crate) fn untracked_under(&self, rel: &str) -> Result<Vec<String>> {
        let dir = format!("{}/", rel.trim_end_matches('/'));
        let out = self.git_bytes(&[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            &literal(&dir),
        ])?;
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect())
    }

    fn remove_untracked_dir(&self, rel: &str, abs: &Path) -> Result<()> {
        for f in self.untracked_under(rel)? {
            remove_existing(&self.repo.join(&f), &f)?;
        }
        prune_empty_dirs(abs, rel)?;
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
    pub(crate) fn worktree_path(&self, rel: &str) -> Result<PathBuf> {
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

    /// Resolve a client-supplied path for an operation on the **entry itself** — the
    /// discard backup (`engine::discard`) reads and writes a symlink as a link, never
    /// through it.
    ///
    /// The escape rule is [`Self::worktree_path`]'s, applied to the parent directory:
    /// `linkdir/file`, where `linkdir` points outside the repository, is refused, because
    /// reading or writing that path would touch a file outside. The last component is
    /// not followed: a committed link to `/usr/bin/tool` is backed up as its target
    /// text and restored as a link, which leaves the outside alone — refusing it would
    /// only make that link impossible to roll back. What comes back is the lexical path.
    pub(crate) fn worktree_entry(&self, rel: &str) -> Result<PathBuf> {
        use std::path::Component;
        let candidate = Path::new(rel);
        if !matches!(candidate.components().next_back(), Some(Component::Normal(_))) {
            return Err(Error::Rule(format!("{rel} is not a path inside the repository")));
        }
        if let Some(parent) = candidate.parent().filter(|p| !p.as_os_str().is_empty()) {
            let parent = parent
                .to_str()
                .ok_or_else(|| Error::Rule(format!("{rel} is not a path inside the repository")))?;
            self.worktree_path(parent)?;
        }
        Ok(self.repo.join(candidate))
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
    ///
    /// **A NUL byte is dropped rather than refused**, for the same invariant: written
    /// through, it would make the next read call the file `binary` and lock the user
    /// out of what the application itself wrote. Dropping is chosen over an `Error::Rule`
    /// because a NUL is invisible in a textarea — it can only have arrived in a paste,
    /// the user cannot see it to delete it, and a refusal would leave them unable to
    /// save at all. Apart from the endings and that byte the text is written verbatim:
    /// no terminator is appended and none is removed.
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

        let normalised = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\0', "");
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
        // The overwrite branch recreates a file that was deleted outside the editor,
        // and `git rm` of the last file in a folder takes the folder too. Created only
        // here, after the freshness and ceiling verdicts, so an Io failure of the mkdir
        // cannot preempt a `stale` or a `rule` (order of refusals, prd_03_interfaces).
        // Staying inside the repository is already settled: `worktree_path` rejected
        // every `..`, and for a path that does not exist yet it canonicalised the
        // deepest existing ancestor and required it under the root.
        //
        // Whatever this makes, a failure below unmakes: an operation that did not
        // happen must leave nothing new in the working tree. Git does not track empty
        // directories, so no `git status` would ever report the leftover — which is
        // precisely why it would stay there.
        let mut created: Option<PathBuf> = None;
        if !dir.exists() {
            // The shallowest directory that is about to appear: removing that one
            // removes the whole chain, and only the chain.
            let mut top = dir;
            while let Some(parent) = top.parent() {
                if parent.exists() {
                    break;
                }
                top = parent;
            }
            created = Some(top.to_path_buf());
            if let Err(e) = std::fs::create_dir_all(dir) {
                // It may have made part of the chain before failing.
                undo(dir, &created);
                return Err(Error::Io(format!("{rel}: {e}")));
            }
        }
        if let Err(e) = replace_file(&path, &bytes) {
            undo(dir, &created);
            return Err(Error::Io(format!("{rel}: {e}")));
        }
        Ok(fnv1a(&bytes))
    }

    /// Run git feeding `input` on stdin (used by `git apply`).
    fn git_stdin(&self, args: &[&str], input: &[u8]) -> Result<()> {
        exec::git(&self.repo, args).input(input).run()?.checked()?;
        Ok(())
    }

    /// Run git, returning stdout and ignoring a non-zero exit (for `diff --no-index`,
    /// which exits 1 precisely when there is a difference to show).
    fn git_allow_fail(&self, args: &[&str]) -> String {
        exec::git(&self.repo, args)
            .run()
            .map(|o| o.stdout_text())
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
            .git_allow_fail(&["ls-files", "--", &literal(path)])
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
        let raw = self.raw_diff(path, against, whitespace, context)?;
        let mut d = parse_diff(path, &String::from_utf8_lossy(&raw));
        d.digest = fnv1a(&raw);
        Ok(d)
    }

    /// The bytes git prints for [`CliEngine::diff_file`] — what the panel draws and
    /// what a line action rebuilds its patch from, so the two are one call.
    ///
    /// Pinned against configuration that would make the text unappliable or change
    /// its spelling: `--no-ext-diff` / `--no-textconv` (an external or converted diff
    /// is for reading, `git apply` refuses it), `--no-color`, and the `a/` / `b/`
    /// prefixes (`diff.noprefix` and `diff.mnemonicPrefix` would move the path under
    /// `git apply -p1`).
    fn raw_diff(
        &self,
        path: &str,
        against: &str,
        whitespace: &str,
        context: Option<u32>,
    ) -> Result<Vec<u8>> {
        let ws = whitespace_args(whitespace)?;
        let ctx = context_arg(context);
        let ctx: Vec<&str> = ctx.iter().map(String::as_str).collect();
        let spec = literal(path);
        let pinned = [
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ];
        let mut a: Vec<&str> = match against {
            "index" => vec!["diff", "--cached"],
            "head" => vec!["diff", "HEAD"],
            _ => vec!["diff"],
        };
        a.extend_from_slice(&pinned);
        a.extend_from_slice(&ws);
        a.extend_from_slice(&ctx);
        a.extend_from_slice(&["--", &spec]);
        let d = self.git_bytes(&a)?;
        if against == "index" || against == "head" {
            return Ok(d);
        }
        // An empty diff has two readings: the file is untracked (git compares
        // nothing), or it is tracked and has nothing unstaged — staged whole, or
        // only whitespace changed under an ignoring mode. Only the first is drawn
        // as an all-add diff, so git is asked which one it is. Reading "empty" as
        // "untracked" drew a fully staged file as new in the Unstaged view, and a
        // revert there removed lines that were never a change. An intent-to-add
        // file is in the index and has a diff of its own; it never gets here.
        if !d.iter().all(u8::is_ascii_whitespace) || self.is_tracked(path) {
            return Ok(d);
        }
        // Untracked: an all-add diff against nothing. `--no-index` exits 1 exactly
        // when there is a difference, so the exit code says nothing here.
        let mut a = vec!["diff", "--no-index"];
        a.extend_from_slice(&pinned);
        a.extend_from_slice(&ws);
        a.extend_from_slice(&ctx);
        a.extend_from_slice(&["--", "/dev/null", path]);
        Ok(exec::git(&self.repo, &a)
            .run()
            .map(|o| o.stdout)
            .unwrap_or_default())
    }

    /// The patch that applies the chosen lines of this file's diff (`engine::patch`).
    ///
    /// `against` is the diff the reader was shown — `worktree` for stage and revert,
    /// `index` for unstage — read again here with the same `context`, never with a
    /// whitespace mode: a diff with whitespace ignored does not apply. `digest` is the
    /// fingerprint that diff was drawn with; a different one is `Error::Stale`, because
    /// hunk and line indexes point into the drawn diff and would name other lines in
    /// this one.
    pub fn selection_patch(
        &self,
        path: &str,
        against: &str,
        picks: &[HunkPick],
        digest: &str,
        context: Option<u32>,
        reverse: bool,
    ) -> Result<Vec<u8>> {
        if context == Some(0) {
            return Err(Error::Rule(
                "a diff without context lines cannot be applied line by line".into(),
            ));
        }
        let raw = self.raw_diff(path, against, "none", context)?;
        if raw.iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Rule(format!(
                "{path} has no change here to choose from"
            )));
        }
        if fnv1a(&raw) != digest {
            return Err(Error::Stale(format!(
                "{path} changed since its diff was shown; look at it again and choose anew"
            )));
        }
        let picks: Vec<(usize, Pick)> = picks
            .iter()
            .map(|p| {
                let pick = match &p.lines {
                    LinePick::All(_) => Pick::All,
                    LinePick::Lines(v) => Pick::Lines(v.clone()),
                };
                (p.hunk, pick)
            })
            .collect();
        patch::build(&raw, &picks, reverse)?
            .ok_or_else(|| Error::Rule("no added or removed line is chosen".into()))
    }

    /// Apply a patch built by [`CliEngine::selection_patch`] to the index (`cached`)
    /// or worktree, forward or reversed. This is the whole mechanism behind line and
    /// hunk stage (cached, forward), unstage (cached, reverse) and revert (worktree,
    /// reverse) — the index is touched only by the chosen lines, never by `git add -A`/`git add <dir>`
    /// (which would over-stage other lists; cf. commit staging discipline, Правка
    /// `ad8c42e`). A hunk staged here is what [`CliEngine::commit_paths`] later
    /// commits for that file — the index wins.
    pub fn apply_patch(&self, patch: &[u8], cached: bool, reverse: bool) -> Result<()> {
        let mut args = vec!["apply", "--whitespace=nowarn"];
        if cached {
            args.push("--cached");
        }
        if reverse {
            args.push("-R");
        }
        self.git_stdin(&args, patch)
    }

    /// Stage EXACTLY these paths. Existing files are `git add`ed; worktree deletions
    /// are staged via `git rm`. Never `git add -A` or `git add <dir>` — both would
    /// sweep in other changelists' files or miss deletions (Правка `ad8c42e`; the
    /// `-A`/`<dir>` ban is about *unscoped* staging, exactly what this avoids).
    ///
    /// Whole files: a partly staged file loses its staged/unstaged split here. Only
    /// the mid-operation fallback of [`CliEngine::commit_paths`] still commits
    /// through it.
    pub fn stage_paths(&self, paths: &[String]) -> Result<()> {
        let (deleted, existing): (Vec<&String>, Vec<&String>) =
            paths.iter().partition(|p| !self.repo.join(p).exists());
        if !existing.is_empty() {
            let specs: Vec<String> = existing.iter().map(|p| literal(p)).collect();
            let mut args = vec!["add", "--"];
            args.extend(specs.iter().map(String::as_str));
            self.git(&args)?;
        }
        if !deleted.is_empty() {
            let specs: Vec<String> = deleted.iter().map(|p| literal(p)).collect();
            let mut args = vec!["rm", "-q", "--"];
            args.extend(specs.iter().map(String::as_str));
            self.git(&args)?;
        }
        Ok(())
    }

    /// Commit exactly the given paths, and nothing the user staged for anything else.
    ///
    /// **The index wins.** A path of the set with something staged goes into the
    /// commit exactly as staged — a hunk staged with `hunk_stage` is the whole change
    /// of that file in this commit, the unstaged rest stays in the working tree. A
    /// path with nothing staged goes in whole from the working tree (deletion, new
    /// file, mode, symlink — as `git add` records them). A path outside the set never
    /// goes in, staged or not, and stays staged.
    ///
    /// The commit is made by `git commit` over a throwaway index ([`TempIndex`],
    /// `GIT_INDEX_FILE`), so hooks, `commit.gpgsign`, identity, `--amend` and the
    /// message cleanup are git's own, as before; hooks see the throwaway index. It
    /// starts as HEAD — for `--amend` too: the amended commit keeps everything the
    /// old HEAD changed, and git gives it the old HEAD's parents — and only the set's
    /// paths are laid over it. The throwaway index starts as a *copy* of the user's
    /// and is then reset to HEAD: `read-tree --reset` keeps the stat data of every
    /// entry that matches, and without it `git commit`'s refresh would re-read every
    /// file of the working tree. An unborn branch starts empty.
    ///
    /// Only after the commit landed is the user's index touched, and only at the
    /// set's paths: they are reset to the new HEAD, which leaves the other lists'
    /// staged changes where they were. A refused commit (hook, empty) leaves the
    /// user's index byte for byte as it was.
    ///
    /// A staged rename (`git mv`) is one row in the snapshot, the new path; its
    /// source's removal goes along, or the commit would hold a copy and the deletion
    /// would stay staged. The pair is the snapshot's own: where status shows the two
    /// sides as two rows, they are two changes and each goes with its own list.
    ///
    /// **While `MERGE_HEAD`, `CHERRY_PICK_HEAD` or `REVERT_HEAD` exists** the old
    /// behaviour stays: stage the set whole and commit the whole index. `git commit`
    /// reads those markers from the git directory whatever the index is — a merge
    /// commit whose tree lacks the merged files outside the set would claim a merge
    /// it does not contain, and a pick's commit would take the picked commit's
    /// message and author for part of its change. Every other stop (a rebase on
    /// `edit` / `break` / `exec`, a rebase conflict without those markers) makes an
    /// ordinary commit, and goes the throwaway-index way like any other.
    pub fn commit_paths(&self, paths: &[String], message: &str, amend: bool) -> Result<()> {
        if message.trim().is_empty() {
            return Err(Error::Rule("commit message cannot be empty".into()));
        }
        let markers = self.git_paths(&["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"])?;
        if markers.iter().any(|m| m.exists()) {
            self.stage_paths(paths)?;
            return self.git_commit(message, amend, &[]);
        }

        let head = exec::git(&self.repo, &["rev-parse", "--verify", "--quiet", "HEAD"]).run()?;
        let born = match head.code {
            Some(0) => true,
            Some(1) => false,
            _ => return Err(head.fail_stderr()),
        };

        let index = TempIndex::new(&self.repo, "graft-commit")?;
        let index_path = index.env_value();
        let env = [("GIT_INDEX_FILE", index_path.as_str())];
        if born {
            let real = self
                .git_paths(&["index"])?
                .pop()
                .ok_or_else(|| Error::Parse("rev-parse --git-path returned nothing".into()))?;
            match std::fs::copy(&real, index.path()) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::Io(format!("copying the index: {e}"))),
            }
            exec::git(&self.repo, &["read-tree", "--reset", "HEAD"])
                .env(&env)
                .run()?
                .checked()?;
        }

        // What the set has staged, file by file. `--no-renames`: scoped to the set, a
        // rename's other side is outside the pathspec anyway. An unmerged path is no
        // staged version — it goes in from the working tree, as `git add` would.
        let specs: Vec<String> = paths.iter().map(|p| literal(p)).collect();
        let mut args = vec![
            "diff",
            "--cached",
            "--no-renames",
            "--name-status",
            "-z",
            "--",
        ];
        args.extend(specs.iter().map(String::as_str));
        let staged: Vec<CommitFileEntry> = parse_name_status(&self.git(&args)?)?
            .into_iter()
            .filter(|e| e.status != FileState::Conflicted)
            .collect();
        let mut from_index: Vec<String> = staged.iter().map(|e| e.path.clone()).collect();
        let mut in_index: HashSet<String> = from_index.iter().cloned().collect();
        let mut sources: Vec<String> = Vec::new();
        if born && staged.iter().any(|e| e.status == FileState::Added) {
            // The pairs the snapshot shows, not a rename detection of our own: with
            // `status.renames` / `diff.renames` off, `git mv` is two rows that may sit
            // in two lists, and the source's deletion is then another list's change.
            for f in self.snapshot()?.files {
                if let Some(old) = f.old_path {
                    if in_index.contains(&f.path) && in_index.insert(old.clone()) {
                        sources.push(old);
                    }
                }
            }
            from_index.extend(sources.iter().cloned());
        }

        // Nothing staged: the working-tree version, laid first so that a staged file
        // under a folder of the set (`dir/`) is then overridden by its index entry.
        let worktree: Vec<&String> = paths.iter().filter(|p| !in_index.contains(*p)).collect();
        if !worktree.is_empty() {
            let specs: Vec<String> = worktree.iter().map(|p| literal(p)).collect();
            let mut args = vec!["add", "--"];
            args.extend(specs.iter().map(String::as_str));
            exec::git(&self.repo, &args).env(&env).run()?.checked()?;
        }

        if !from_index.is_empty() {
            let specs: Vec<String> = from_index.iter().map(|p| literal(p)).collect();
            let mut args = vec!["ls-files", "--stage", "-z", "--"];
            args.extend(specs.iter().map(String::as_str));
            let listed = self.git_bytes(&args)?;
            let mut records = Vec::new();
            let mut present: HashSet<String> = HashSet::new();
            for rec in listed.split(|&b| b == 0).filter(|r| !r.is_empty()) {
                let (meta, path) = rec
                    .iter()
                    .position(|&b| b == b'\t')
                    .map(|i| (&rec[..i], &rec[i + 1..]))
                    .ok_or_else(|| {
                        Error::Parse(format!(
                            "ls-files --stage record without a path: {:?}",
                            String::from_utf8_lossy(rec)
                        ))
                    })?;
                let path = String::from_utf8_lossy(path).to_string();
                if !in_index.contains(&path) || !meta.ends_with(b" 0") {
                    continue;
                }
                records.extend_from_slice(rec);
                records.push(0);
                present.insert(path);
            }
            if !records.is_empty() {
                exec::git(&self.repo, &["update-index", "-z", "--index-info"])
                    .env(&env)
                    .input(&records)
                    .run()?
                    .checked()?;
            }
            // Staged as gone: `update-index` takes plain paths, not pathspecs.
            let mut gone = Vec::new();
            for p in from_index.iter().filter(|p| !present.contains(*p)) {
                gone.extend_from_slice(p.as_bytes());
                gone.push(0);
            }
            if !gone.is_empty() {
                exec::git(
                    &self.repo,
                    &["update-index", "-z", "--force-remove", "--stdin"],
                )
                .env(&env)
                .input(&gone)
                .run()?
                .checked()?;
            }
        }

        self.git_commit(message, amend, &env)?;

        // The set's paths in the user's index now match the new HEAD; every other
        // entry, other lists' staged changes included, is left as it was.
        let mut specs: Vec<String> = paths.iter().map(|p| literal(p)).collect();
        specs.extend(sources.iter().map(|p| literal(p)));
        let mut args = vec!["reset", "-q", "HEAD", "--"];
        args.extend(specs.iter().map(String::as_str));
        let out = exec::git(&self.repo, &args).run()?;
        if !out.success() {
            return Err(out.fail_with(format!(
                "the commit was made, but the index of its files was not brought up to it: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// `git commit -m <message> [--amend]`, with `env` (the throwaway index, or none).
    ///
    /// A refusal carries **both** streams: "nothing to commit" and the status around
    /// it are printed on stdout, and stderr alone left the banner without a reason.
    fn git_commit(&self, message: &str, amend: bool, env: &[(&str, &str)]) -> Result<()> {
        let mut args = vec!["commit", "-m", message];
        if amend {
            args.push("--amend");
        }
        exec::git(&self.repo, &args)
            .env(env)
            .run()?
            .checked_both()?;
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

    /// Create a branch from HEAD (or `from`) and switch to it. The name is checked
    /// first ([`check_branch_name`]); `--end-of-options` keeps a `from` that starts
    /// with `-` a revision, and the trailing `--` keeps it from being read as a path.
    pub fn create_branch(&self, name: &str, from: Option<&str>) -> Result<()> {
        check_branch_name(&self.repo, name)?;
        let mut args = vec!["checkout", "-b", name];
        if let Some(f) = from {
            args.push("--end-of-options");
            args.push(f);
        }
        args.push("--");
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
        // `--end-of-options`: a branch named `-x` exists (update-ref makes one) and
        // is not an option; `--`: a branch that is also a file name stays a branch.
        self.git(&["checkout", "--end-of-options", name, "--"])?;
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
                self.git_net(&["push", "-u", "--end-of-options", "origin", &br])?;
            }
            "force" => {
                self.git_net(&["push", "--force-with-lease"])?;
            }
            "force-hard" => {
                self.git_net(&["push", "--force"])?;
            }
            "normal" => {
                self.git_net(&["push"])?;
            }
            other => {
                return Err(Error::Rule(format!("unknown push mode: {other}")));
            }
        }
        Ok(())
    }

    pub fn fetch(&self) -> Result<()> {
        self.git_net(&["fetch", "--prune"])?;
        Ok(())
    }

    pub fn pull(&self) -> Result<()> {
        self.git_net(&["pull"])?;
        Ok(())
    }
}

/// Parse `git diff` output for a single file into hunks for display.
///
/// The hunks and their line indexes are `engine::patch::hunks`' own — the numbering
/// a line selection is sent back in, so the panel and the patch builder can never
/// count the lines of a hunk differently.
///
/// Visible to the whole engine: a commit's diff has the same shape as a worktree
/// diff, and a second parser would be a second set of edge cases (binary files,
/// renames, "\ No newline") drifting away from this one.
pub(crate) fn parse_diff(path: &str, raw: &str) -> FileDiff {
    // The marker is a header line git writes, not text anywhere in the diff: a
    // changed line reading "Binary files a/x and b/x differ" is a line.
    let binary = raw
        .lines()
        .take_while(|l| !l.starts_with("@@"))
        .any(|l| l.starts_with("Binary files ") || l == "GIT binary patch");
    if binary {
        return FileDiff {
            path: path.into(),
            binary: true,
            ..FileDiff::default()
        };
    }
    let hunks = patch::hunks(raw.as_bytes())
        .into_iter()
        .map(|h| {
            let (mut old_no, mut new_no) = (h.old_start, h.new_start);
            let lines = h
                .lines
                .into_iter()
                .map(|l| {
                    let content = String::from_utf8_lossy(&l.text).to_string();
                    let (origin, old, new) = match l.kind {
                        Kind::Add => ("+", None, Some(new_no)),
                        Kind::Del => ("-", Some(old_no), None),
                        Kind::Context => (" ", Some(old_no), Some(new_no)),
                    };
                    old_no += u32::from(old.is_some());
                    new_no += u32::from(new.is_some());
                    DiffLine {
                        origin: origin.into(),
                        content,
                        old_no: old,
                        new_no: new,
                    }
                })
                .collect();
            Hunk {
                header: String::from_utf8_lossy(&h.header).to_string(),
                lines,
            }
        })
        .collect();
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

/// Write `bytes` to `path` as a whole: a uniquely named temp file next to it, then a
/// rename, so an interrupted write never leaves the file half-written. The target's
/// permissions are carried over — a rename would otherwise hand a 755 script the temp
/// file's mode and turn "one line changed" into a mode change in git. The directory
/// must exist. Used by `write_text_file` and by `engine::ignore` for `.gitignore`.
pub(crate) fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent directory"))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.graft.tmp.{}.{n}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
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
    let out = exec::git(repo, &["config", "--get", "user.email"]).run().ok()?;
    if !out.success() {
        return None;
    }
    let v = out.stdout_text().trim().to_string();
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
    use std::process::Command;

    /// [`run`] for the other modules' tests.
    pub(crate) fn run_git(dir: &Path, args: &[&str]) {
        run(dir, args)
    }

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

    /// A write that fails leaves the working tree as it found it: the directories it
    /// had to make for the recreated file go back too (prd_03 task 03).
    #[test]
    fn a_failed_write_takes_its_new_directories_back() {
        let dir = scratch_repo();
        let p = dir.path();
        // A name at the length limit: the temp file next to it is longer still, so the
        // write fails *after* the directories were made and before any rename.
        let rel = format!("sub/deep/{}.txt", "x".repeat(250));
        let e = CliEngine::new(p)
            .write_text_file(&rel, "text\n", Eol::Lf, "")
            .expect_err("the write cannot succeed with that name");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "io");
        assert!(
            !p.join("sub").exists(),
            "a refused write leaves nothing behind it"
        );
    }

    /// A NUL byte cannot survive a round trip — `read_text_file` calls a file holding
    /// one binary — so the write drops it and the file stays editable (prd_03 task 03).
    #[test]
    fn a_nul_byte_does_not_lock_the_file_out_of_editing() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("t.txt"), "one\n").unwrap();
        let eng = CliEngine::new(p);
        let f = eng.read_text_file("t.txt").unwrap();
        let d = eng
            .write_text_file("t.txt", "one\ntw\0o\n", f.eol, &f.digest)
            .unwrap();

        let again = eng.read_text_file("t.txt").unwrap();
        assert_eq!(
            again.blocked, None,
            "what the application wrote it can reopen"
        );
        assert_eq!(again.text.as_deref(), Some("one\ntwo\n"));
        assert_eq!(again.digest, d, "the digest describes the bytes on disk");
    }

    /// `git rm` of the last file in a folder takes the folder with it. Recreating the
    /// file through the overwrite branch has to put the folder back (prd_03 task 03).
    #[test]
    fn recreating_a_file_remakes_its_missing_directory() {
        let dir = scratch_repo();
        let p = dir.path();
        let eng = CliEngine::new(p);
        assert!(!p.join("sub").exists());

        let gone = eng.read_text_file("sub/deep/t.txt").unwrap();
        assert_eq!(gone.blocked, Some(EditBlock::Missing));
        assert_eq!(
            gone.digest, "",
            "an absent file fingerprints to the sentinel"
        );

        eng.write_text_file("sub/deep/t.txt", "back\n", Eol::Lf, "")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("sub/deep/t.txt")).unwrap(),
            "back\n"
        );
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

    /// `git status` without `-uall` reports an untracked folder as one entry, `d/`, and
    /// that entry is a row of the Unversioned list with a checkbox and "Revert to HEAD" —
    /// so the rollback receives it. It used to return Ok and leave every file in place:
    /// `rm -f` of a path not in the index fails, `remove_file` refuses a folder, and both
    /// errors were swallowed.
    #[test]
    fn rolling_back_an_untracked_folder_removes_its_files() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join(".gitignore"), "*.log\n").unwrap();
        run(p, &["add", ".gitignore"]);
        run(p, &["commit", "-m", "ignore logs"]);
        std::fs::create_dir_all(p.join("d/e/empty")).unwrap();
        std::fs::write(p.join("d/one.txt"), "1\n").unwrap();
        std::fs::write(p.join("d/e/two.txt"), "2\n").unwrap();
        std::fs::create_dir_all(p.join("k/sub")).unwrap();
        std::fs::write(p.join("k/sub/new.txt"), "n\n").unwrap();
        std::fs::write(p.join("k/keep.log"), "ignored\n").unwrap();

        let eng = CliEngine::new(p);
        let untracked: Vec<String> = eng
            .snapshot()
            .unwrap()
            .files
            .into_iter()
            .filter(|f| f.status == FileState::Untracked)
            .map(|f| f.path)
            .collect();
        assert_eq!(untracked, vec!["d/".to_string(), "k/".to_string()], "what the UI is handed");

        eng.rollback(&untracked).unwrap();
        assert!(!p.join("d").exists(), "files and the emptied folders are gone");
        assert_eq!(
            std::fs::read_to_string(p.join("k/keep.log")).unwrap(),
            "ignored\n",
            "an ignored file is not the rollback's to delete"
        );
        assert!(!p.join("k/sub").exists(), "its emptied subfolder is");
        assert!(eng.snapshot().unwrap().files.is_empty());
    }

    /// A name with glob characters (`app/[id]/page.tsx` in Next.js) names that file
    /// only. As a bare pathspec `x[ab].txt` also matches `xa.txt`: the rollback
    /// reverted the neighbour's edit, and `rm -f` of a new file deleted it from disk.
    #[test]
    fn a_glob_looking_name_touches_that_file_only() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("xa.txt"), "one\n").unwrap();
        std::fs::write(p.join("x[ab].txt"), "one\n").unwrap();
        run(p, &["add", "xa.txt", ":(literal)x[ab].txt"]);
        run(p, &["commit", "-m", "both"]);
        let eng = CliEngine::new(p);

        std::fs::write(p.join("xa.txt"), "mine\n").unwrap();
        std::fs::write(p.join("x[ab].txt"), "two\n").unwrap();
        eng.rollback(&["x[ab].txt".to_string()]).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("x[ab].txt")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(p.join("xa.txt")).unwrap(),
            "mine\n",
            "neighbour kept"
        );

        std::fs::write(p.join("y[ab].txt"), "new\n").unwrap();
        std::fs::write(p.join("ya.txt"), "tracked\n").unwrap();
        run(p, &["add", "ya.txt"]);
        run(p, &["commit", "-m", "ya"]);
        run(p, &["add", ":(literal)y[ab].txt"]);
        eng.rollback(&["y[ab].txt".to_string()]).unwrap();
        assert!(!p.join("y[ab].txt").exists());
        assert!(p.join("ya.txt").exists(), "a tracked neighbour is not deleted");

        std::fs::write(p.join("x[ab].txt"), "two\n").unwrap();
        let d = eng.diff_file("x[ab].txt", "worktree", "none", None).unwrap();
        let shown: Vec<&str> = d
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .map(|l| l.content.as_str())
            .collect();
        assert!(shown.iter().any(|c| c.contains("two")), "{shown:?}");
        assert!(
            !shown.iter().any(|c| c.contains("mine")),
            "the neighbour's hunk is not this file's: {shown:?}"
        );

        eng.stage_paths(&["x[ab].txt".to_string()]).unwrap();
        let staged = eng.git(&["diff", "--cached", "--name-only"]).unwrap();
        assert_eq!(staged.trim(), "x[ab].txt", "xa.txt stays out of the commit");
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
        let patch = eng
            .selection_patch("f.txt", "worktree", &[all(0)], &diff.digest, None, false)
            .unwrap();
        eng.apply_patch(&patch, true, false).unwrap();
        assert!(eng
            .git(&["diff", "--cached", "--name-only"])
            .unwrap()
            .contains("f.txt"));
        let remaining = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        assert_eq!(remaining.hunks.len(), 1, "one hunk left unstaged");

        // revert the remaining (line 10) hunk in the worktree
        let patch = eng
            .selection_patch(
                "f.txt",
                "worktree",
                &[all(0)],
                &remaining.digest,
                None,
                true,
            )
            .unwrap();
        eng.apply_patch(&patch, false, true).unwrap();
        let content = std::fs::read_to_string(p.join("f.txt")).unwrap();
        assert!(content.contains("CHANGED1"), "staged change stays in worktree");
        assert!(content.contains("line10"), "line 10 restored");
        assert!(!content.contains("CHANGED10"), "line 10 change reverted");
    }

    // ---- line selections, checked by git itself ----

    fn all(hunk: usize) -> HunkPick {
        HunkPick {
            hunk,
            lines: LinePick::All(crate::model::AllLines::All),
        }
    }

    fn some(hunk: usize, lines: &[usize]) -> HunkPick {
        HunkPick {
            hunk,
            lines: LinePick::Lines(lines.to_vec()),
        }
    }

    /// `git apply [--cached] [-R] --check` on the patch: git's own verdict that it
    /// applies, before it is applied.
    fn git_accepts(p: &Path, patch: &[u8], cached: bool, reverse: bool) {
        use std::io::Write;
        let mut args = vec!["apply", "--check"];
        if cached {
            args.push("--cached");
        }
        if reverse {
            args.push("-R");
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(patch).unwrap();
        let o = child.wait_with_output().unwrap();
        assert!(
            o.status.success(),
            "git apply --check refused:\n{}\n{}",
            String::from_utf8_lossy(patch),
            String::from_utf8_lossy(&o.stderr)
        );
    }

    /// Stage `picks` of the working-tree diff as the panel would: the digest of the
    /// diff it was shown, the patch checked by git, then applied.
    fn stage_picks(p: &Path, path: &str, picks: &[HunkPick]) {
        let eng = CliEngine::new(p);
        let d = eng.diff_file(path, "worktree", "none", None).unwrap();
        let patch = eng
            .selection_patch(path, "worktree", picks, &d.digest, None, false)
            .unwrap();
        git_accepts(p, &patch, true, false);
        eng.apply_patch(&patch, true, false).unwrap();
    }

    fn unstage_picks(p: &Path, path: &str, picks: &[HunkPick]) {
        let eng = CliEngine::new(p);
        let d = eng.diff_file(path, "index", "none", None).unwrap();
        let patch = eng
            .selection_patch(path, "index", picks, &d.digest, None, true)
            .unwrap();
        git_accepts(p, &patch, true, true);
        eng.apply_patch(&patch, true, true).unwrap();
    }

    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line{i}")).collect()
    }

    fn text(lines: &[String]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// A tracked file staged whole has nothing unstaged: its working-tree diff is
    /// empty, not the whole file drawn as new — and there is nothing to revert in it.
    #[test]
    fn a_fully_staged_file_has_no_unstaged_diff_and_nothing_to_revert() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        run(p, &["add", "a.txt"]);
        let eng = CliEngine::new(p);

        let d = eng.diff_file("a.txt", "worktree", "none", None).unwrap();
        assert!(d.hunks.is_empty(), "no unstaged change: {:?}", d.hunks);
        for ws in ["trailing", "all"] {
            let w = eng.diff_file("a.txt", "worktree", ws, None).unwrap();
            assert!(w.hunks.is_empty(), "{ws}: {:?}", w.hunks);
        }

        let r = eng.selection_patch("a.txt", "worktree", &[all(0)], &d.digest, None, true);
        assert!(matches!(r, Err(Error::Rule(_))), "{r:?}");
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "two\n");

        // An untracked file still gets its all-add diff.
        std::fs::write(p.join("u.txt"), "u\n").unwrap();
        let u = eng.diff_file("u.txt", "worktree", "none", None).unwrap();
        assert_eq!(u.hunks.len(), 1);
    }

    /// A text file whose lines say "Binary files ..." is still a text diff: the
    /// marker counts only where git writes it, in the file header.
    #[test]
    fn a_line_that_reads_like_the_binary_marker_is_just_a_line() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "Binary files a/x and b/x differ\n").unwrap();
        let d = CliEngine::new(p)
            .diff_file("a.txt", "worktree", "none", None)
            .unwrap();
        assert!(!d.binary);
        assert_eq!(d.hunks.len(), 1);

        std::fs::write(p.join("b.bin"), [0u8, 1, 2, 0, 255]).unwrap();
        run(p, &["add", "b.bin"]);
        run(p, &["commit", "-q", "-m", "bin"]);
        std::fs::write(p.join("b.bin"), [0u8, 9, 9, 0, 255]).unwrap();
        let b = CliEngine::new(p)
            .diff_file("b.bin", "worktree", "none", None)
            .unwrap();
        assert!(b.binary, "a real binary diff is still binary");
    }

    /// Two hunks; the first grows by one line, so the second one's written side
    /// sits where the earlier choices put it, not where git printed it.
    fn two_hunk_file(p: &Path) -> Vec<String> {
        let base = numbered(20);
        std::fs::write(p.join("f.txt"), text(&base)).unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["commit", "-q", "-m", "f"]);
        let mut edited = base.clone();
        edited.splice(1..2, ["A".to_string(), "B".to_string()]);
        edited[15] = "X".into(); // was line15
        std::fs::write(p.join("f.txt"), text(&edited)).unwrap();
        base
    }

    #[test]
    fn staging_chosen_lines_across_two_hunks_puts_exactly_them_in_the_index() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = two_hunk_file(p);
        let d = CliEngine::new(p)
            .diff_file("f.txt", "worktree", "none", None)
            .unwrap();
        assert_eq!(d.hunks.len(), 2);
        let origins: Vec<&str> = d.hunks[0].lines.iter().map(|l| l.origin.as_str()).collect();
        assert_eq!(origins, [" ", "-", "+", "+", " ", " ", " "]);

        // `-line2` and `+A` of the first hunk (not `+B`), all of the second.
        stage_picks(p, "f.txt", &[some(0, &[1, 2]), all(1)]);

        let mut want = base.clone();
        want[1] = "A".into();
        want[14] = "X".into();
        assert_eq!(blob(p, ":f.txt"), text(&want));
    }

    #[test]
    fn unstaging_chosen_lines_leaves_the_rest_staged() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = two_hunk_file(p);
        run(p, &["add", "f.txt"]);

        // Unstage `+A` only and the whole second hunk: `line2` stays removed, `B`
        // stays added, line 15 is back.
        unstage_picks(p, "f.txt", &[some(0, &[2]), all(1)]);

        let mut want = base.clone();
        want[1] = "B".into();
        assert_eq!(blob(p, ":f.txt"), text(&want));
    }

    #[test]
    fn a_last_line_without_newline_is_staged_and_unstaged_line_by_line() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("e.txt"), "a\nb").unwrap();
        run(p, &["add", "e.txt"]);
        run(p, &["commit", "-q", "-m", "e"]);
        std::fs::write(p.join("e.txt"), "a\nb\nc").unwrap();
        // " a", "-b" (no newline), "+b", "+c" (no newline)
        let d = CliEngine::new(p)
            .diff_file("e.txt", "worktree", "none", None)
            .unwrap();
        let origins: Vec<&str> = d.hunks[0].lines.iter().map(|l| l.origin.as_str()).collect();
        assert_eq!(origins, [" ", "-", "+", "+"]);

        // `+c` alone: b needs its newline for c to follow it.
        stage_picks(p, "e.txt", &[some(0, &[3])]);
        assert_eq!(blob(p, ":e.txt"), "a\nb\nc");

        // Back out `+c` again, from the index side.
        unstage_picks(p, "e.txt", &[some(0, &[3])]);
        assert_eq!(blob(p, ":e.txt"), "a\nb\n");
    }

    #[test]
    fn a_new_file_is_staged_in_part_from_intent_to_add_and_from_untracked() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("n.txt"), "l1\nl2\nl3\n").unwrap();
        run(p, &["add", "-N", "n.txt"]);
        stage_picks(p, "n.txt", &[some(0, &[1])]);
        assert_eq!(blob(p, ":n.txt"), "l2\n");

        std::fs::write(p.join("u.txt"), "u1\nu2\nu3\n").unwrap();
        stage_picks(p, "u.txt", &[some(0, &[0, 2])]);
        assert_eq!(blob(p, ":u.txt"), "u1\nu3\n");
    }

    #[test]
    fn a_staged_new_file_is_unstaged_in_part_or_whole() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("n.txt"), "l1\nl2\nl3\n").unwrap();
        run(p, &["add", "n.txt"]);

        unstage_picks(p, "n.txt", &[some(0, &[1])]);
        assert_eq!(blob(p, ":n.txt"), "l1\nl3\n");

        unstage_picks(p, "n.txt", &[all(0)]);
        assert_eq!(out(p, &["ls-files", "--", "n.txt"]), "", "out of the index");
    }

    #[test]
    fn a_deletion_staged_in_part_keeps_the_file_in_the_index() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("g.txt"), "l1\nl2\nl3\n").unwrap();
        run(p, &["add", "g.txt"]);
        run(p, &["commit", "-q", "-m", "g"]);
        std::fs::remove_file(p.join("g.txt")).unwrap();

        stage_picks(p, "g.txt", &[some(0, &[0])]);
        assert_eq!(blob(p, ":g.txt"), "l2\nl3\n");

        stage_picks(p, "g.txt", &[all(0)]);
        assert_eq!(
            out(p, &["ls-files", "--", "g.txt"]),
            "",
            "the deletion is staged"
        );
    }

    /// Unstaging part of a staged deletion puts those lines back as the file in the
    /// index: the reverse of a deletion patch that keeps `deleted file mode`.
    #[test]
    fn a_staged_deletion_is_unstaged_in_part() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("g.txt"), "l1\nl2\nl3\n").unwrap();
        run(p, &["add", "g.txt"]);
        run(p, &["commit", "-q", "-m", "g"]);
        run(p, &["rm", "-q", "g.txt"]);

        unstage_picks(p, "g.txt", &[some(0, &[1])]);
        assert_eq!(blob(p, ":g.txt"), "l2\n");
    }

    #[test]
    fn a_path_with_a_space_and_brackets_is_staged_and_its_sibling_is_not() {
        let dir = scratch_repo();
        let p = dir.path();
        let odd = "sp ace [x].txt";
        let sibling = "sp ace x.txt"; // what `[x]` would match as a glob
        for f in [odd, sibling] {
            std::fs::write(p.join(f), "one\ntwo\n").unwrap();
            run(p, &["add", f]);
        }
        run(p, &["commit", "-q", "-m", "odd"]);
        for f in [odd, sibling] {
            std::fs::write(p.join(f), "one\nTWO\nthree\n").unwrap();
        }

        // " one", "-two", "+TWO", "+three": take `-two` and `+TWO`.
        stage_picks(p, odd, &[some(0, &[1, 2])]);
        assert_eq!(blob(p, &format!(":{odd}")), "one\nTWO\n");
        assert_eq!(blob(p, &format!(":{sibling}")), "one\ntwo\n");
    }

    #[test]
    fn a_choice_made_on_a_diff_that_has_since_changed_is_stale() {
        let dir = scratch_repo();
        let p = dir.path();
        two_hunk_file(p);
        let eng = CliEngine::new(p);
        let shown = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        let index = out(p, &["ls-files", "-s"]);
        std::fs::write(p.join("f.txt"), "rewritten\n").unwrap();

        let r = eng.selection_patch("f.txt", "worktree", &[all(0)], &shown.digest, None, false);
        assert!(matches!(r, Err(Error::Stale(_))), "{r:?}");
        assert_eq!(out(p, &["ls-files", "-s"]), index);
    }

    #[test]
    fn a_choice_of_context_lines_only_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        two_hunk_file(p);
        let eng = CliEngine::new(p);
        let d = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        let r = eng.selection_patch(
            "f.txt",
            "worktree",
            &[some(0, &[0])],
            &d.digest,
            None,
            false,
        );
        assert!(matches!(r, Err(Error::Rule(_))), "{r:?}");
    }

    /// A conflicted file is still drawn (its combined `diff --cc`), and choosing
    /// lines in it is refused with the reason rather than a parse failure.
    #[test]
    fn a_conflicted_file_is_drawn_and_its_lines_are_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "-q", "-b", "side"]);
        std::fs::write(p.join("a.txt"), "side\n").unwrap();
        run(p, &["commit", "-q", "-am", "side"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("a.txt"), "main\n").unwrap();
        run(p, &["commit", "-q", "-am", "main"]);
        let merge = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["merge", "side"])
            .output()
            .unwrap();
        assert!(!merge.status.success(), "the merge must conflict");

        let eng = CliEngine::new(p);
        let d = eng.diff_file("a.txt", "worktree", "none", None).unwrap();
        assert_eq!(d.hunks.len(), 1, "the conflict is drawn");
        assert!(d.hunks[0]
            .lines
            .iter()
            .any(|l| l.content.contains("<<<<<<<")));
        let r = eng.selection_patch("a.txt", "worktree", &[all(0)], &d.digest, None, false);
        assert!(matches!(r, Err(Error::Rule(_))), "{r:?}");
    }

    /// A widened diff is a different diff: the digest and the hunks are the ones
    /// drawn at that context, and the patch is built at it too.
    #[test]
    fn a_widened_diff_is_staged_at_the_context_it_was_drawn_with() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = two_hunk_file(p);
        let eng = CliEngine::new(p);
        let wide = eng
            .diff_file("f.txt", "worktree", "none", Some(20))
            .unwrap();
        assert_eq!(wide.hunks.len(), 1, "the two hunks merge");
        let narrow = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        assert!(matches!(
            eng.selection_patch(
                "f.txt",
                "worktree",
                &[all(0)],
                &narrow.digest,
                Some(20),
                false
            ),
            Err(Error::Stale(_))
        ));
        let patch = eng
            .selection_patch(
                "f.txt",
                "worktree",
                &[all(0)],
                &wide.digest,
                Some(20),
                false,
            )
            .unwrap();
        git_accepts(p, &patch, true, false);
        eng.apply_patch(&patch, true, false).unwrap();
        let mut want = base;
        want.splice(1..2, ["A".to_string(), "B".to_string()]);
        want[15] = "X".into();
        assert_eq!(blob(p, ":f.txt"), text(&want));
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

    /// The message the co-author picker builds (`coAuthorRules.ts` puts the block
    /// after a blank line) reaches the commit as trailers git itself reads — through
    /// `commit -m`'s whitespace cleanup, and again on an amend of that commit.
    #[test]
    fn co_author_trailers_survive_commit_and_amend() {
        let dir = scratch_repo();
        let p = dir.path();
        let trailers = |p: &Path| out(p, &["log", "-1", "--format=%(trailers:key=Co-authored-by,valueonly)"]);
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "feat: x\n\nBody.\n\nCo-authored-by: Other One <o@example.com>", false)
            .unwrap();
        assert_eq!(trailers(p).trim(), "Other One <o@example.com>");

        std::fs::write(p.join("a.txt"), "three\n").unwrap();
        CliEngine::new(p)
            .commit_paths(
                &["a.txt".to_string()],
                "feat: x\n\nBody.\n\nCo-authored-by: Other One <o@example.com>\nCo-authored-by: Third <t@example.com>",
                true,
            )
            .unwrap();
        assert_eq!(trailers(p).trim(), "Other One <o@example.com>\nThird <t@example.com>");
    }

    // ---- commit_paths: the index wins, other lists stay out ----

    /// `git -C dir <args>` stdout as text; panics on failure.
    fn out(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            o.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    /// `git show <spec>` — `HEAD:x` for the commit, `:x` for the index.
    fn blob(dir: &Path, spec: &str) -> String {
        out(dir, &["show", spec])
    }

    fn staged_names(dir: &Path) -> String {
        out(dir, &["diff", "--cached", "--name-only"])
    }

    fn unstaged_names(dir: &Path) -> String {
        out(dir, &["diff", "--name-only"])
    }

    /// A file of another list, staged whole, is neither committed with this list nor
    /// knocked out of the index by it.
    #[test]
    fn commit_leaves_another_lists_staged_file_out_and_staged() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b0\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-m", "b"]);
        std::fs::write(p.join("a.txt"), "A\n").unwrap();
        std::fs::write(p.join("b.txt"), "B staged\n").unwrap();
        run(p, &["add", "b.txt"]);

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "commit a", false)
            .unwrap();

        assert_eq!(blob(p, "HEAD:a.txt"), "A\n");
        assert_eq!(
            blob(p, "HEAD:b.txt"),
            "b0\n",
            "b.txt must stay out of the commit"
        );
        assert_eq!(
            staged_names(p),
            "b.txt\n",
            "b.txt is still staged, a.txt is not"
        );
        assert_eq!(blob(p, ":b.txt"), "B staged\n");
        assert_eq!(unstaged_names(p), "");
    }

    /// A partly staged file of the list commits exactly its staged part; the rest
    /// stays in the working tree, unstaged.
    #[test]
    fn commit_takes_only_the_staged_part_of_a_partly_staged_file() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "one\nstaged\n").unwrap();
        run(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "one\nstaged\nnot staged\n").unwrap();

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "commit a", false)
            .unwrap();

        assert_eq!(blob(p, "HEAD:a.txt"), "one\nstaged\n");
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).unwrap(),
            "one\nstaged\nnot staged\n"
        );
        assert_eq!(
            staged_names(p),
            "",
            "the index of a.txt matches the new HEAD"
        );
        assert_eq!(unstaged_names(p), "a.txt\n", "the rest is left unstaged");
    }

    /// A file with nothing staged goes in whole, from the working tree — and the
    /// index of the committed path ends up at the new HEAD.
    #[test]
    fn commit_takes_an_unstaged_file_whole() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "commit a", false)
            .unwrap();

        assert_eq!(blob(p, "HEAD:a.txt"), "one\ntwo\n");
        assert_eq!(staged_names(p), "");
        assert_eq!(unstaged_names(p), "");
    }

    /// A deletion, a new untracked file and a staged new file of the list, together;
    /// another list's staged deletion stays staged and out.
    #[test]
    fn commit_carries_deletions_and_new_files_of_the_list_only() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("keep.txt"), "keep\n").unwrap();
        run(p, &["add", "keep.txt"]);
        run(p, &["commit", "-m", "keep"]);
        std::fs::remove_file(p.join("a.txt")).unwrap();
        std::fs::write(p.join("new.txt"), "new\n").unwrap();
        std::fs::write(p.join("added.txt"), "added\n").unwrap();
        run(p, &["add", "added.txt"]);
        run(p, &["rm", "-q", "keep.txt"]); // another list's staged deletion

        let set = ["a.txt", "new.txt", "added.txt"].map(String::from);
        CliEngine::new(p)
            .commit_paths(&set, "mixed", false)
            .unwrap();

        let files = head_files(p);
        assert!(files.contains("D\ta.txt"), "{files}");
        assert!(files.contains("A\tnew.txt"), "{files}");
        assert!(files.contains("A\tadded.txt"), "{files}");
        assert!(!files.contains("keep.txt"), "{files}");
        assert_eq!(blob(p, "HEAD:keep.txt"), "keep\n");
        assert_eq!(
            staged_names(p),
            "keep.txt\n",
            "the other list's deletion stays staged"
        );
    }

    /// A new symlink whose target does not exist is a file to commit, not a deletion:
    /// `Path::exists` follows the link and would call it gone.
    #[cfg(unix)]
    #[test]
    fn commit_takes_a_dangling_symlink_as_a_link() {
        let dir = scratch_repo();
        let p = dir.path();
        std::os::unix::fs::symlink("nowhere", p.join("link")).unwrap();

        CliEngine::new(p)
            .commit_paths(&["link".to_string()], "link", false)
            .unwrap();

        let tree = out(p, &["ls-tree", "HEAD", "--", "link"]);
        assert!(tree.starts_with("120000 "), "{tree}");
        assert_eq!(blob(p, "HEAD:link"), "nowhere");
    }

    /// Amend folds the list into HEAD: same parent, HEAD's own other changes kept,
    /// another list's staged file still out and still staged.
    #[test]
    fn amend_keeps_heads_changes_and_leaves_other_lists_out() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b0\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-m", "b"]);
        std::fs::write(p.join("c.txt"), "c\n").unwrap();
        run(p, &["add", "c.txt"]);
        run(p, &["commit", "-m", "c"]);
        let parent = out(p, &["rev-parse", "HEAD~1"]);
        std::fs::write(p.join("a.txt"), "amended\n").unwrap();
        std::fs::write(p.join("b.txt"), "B staged\n").unwrap();
        run(p, &["add", "b.txt"]);

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "c and a", true)
            .unwrap();

        assert_eq!(out(p, &["rev-parse", "HEAD~1"]), parent, "same parent");
        assert_eq!(out(p, &["log", "-1", "--format=%s"]), "c and a\n");
        assert_eq!(blob(p, "HEAD:c.txt"), "c\n", "HEAD's own change survives");
        assert_eq!(blob(p, "HEAD:a.txt"), "amended\n");
        assert_eq!(blob(p, "HEAD:b.txt"), "b0\n");
        assert_eq!(staged_names(p), "b.txt\n");
    }

    /// A pre-commit hook that refuses still stops the commit, and the user's index —
    /// a partly staged file of the list and another list's staged file — is as it was.
    #[cfg(unix)]
    #[test]
    fn refused_by_a_hook_the_commit_leaves_the_index_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b0\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-m", "b"]);
        std::fs::write(p.join("a.txt"), "one\nstaged\n").unwrap();
        run(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "one\nstaged\nnot staged\n").unwrap();
        std::fs::write(p.join("b.txt"), "B staged\n").unwrap();
        run(p, &["add", "b.txt"]);
        let hooks = PathBuf::from(out(p, &["rev-parse", "--git-path", "hooks"]).trim());
        let hooks = if hooks.is_absolute() {
            hooks
        } else {
            p.join(hooks)
        };
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho no >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let head = out(p, &["rev-parse", "HEAD"]);
        let index = out(p, &["ls-files", "-s"]);

        let r = CliEngine::new(p).commit_paths(&["a.txt".to_string()], "nope", false);

        assert!(r.is_err(), "the hook refused");
        assert_eq!(out(p, &["rev-parse", "HEAD"]), head, "no commit was made");
        assert_eq!(out(p, &["ls-files", "-s"]), index, "the index is untouched");
        assert_eq!(blob(p, ":a.txt"), "one\nstaged\n");
    }

    /// The first commit of an unborn branch: only the list goes in, another list's
    /// staged new file stays staged.
    #[test]
    fn first_commit_on_an_unborn_branch_takes_the_list_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-b", "main"]);
        run(p, &["config", "user.email", "t@example.com"]);
        run(p, &["config", "user.name", "Test"]);
        run(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "a\n").unwrap();
        std::fs::write(p.join("b.txt"), "b\n").unwrap();
        run(p, &["add", "b.txt"]);

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "first", false)
            .unwrap();

        assert_eq!(out(p, &["ls-tree", "--name-only", "HEAD"]), "a.txt\n");
        assert_eq!(staged_names(p), "b.txt\n");
        assert_eq!(unstaged_names(p), "");
    }

    /// A staged `git mv` shows as one row, the new path; committing that path takes
    /// the source's removal along, or the commit would be a copy and the deletion
    /// would stay staged.
    #[test]
    fn commit_of_a_staged_rename_takes_its_source_along() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["mv", "a.txt", "r.txt"]);

        CliEngine::new(p)
            .commit_paths(&["r.txt".to_string()], "rename", false)
            .unwrap();

        assert_eq!(out(p, &["ls-tree", "--name-only", "HEAD"]), "r.txt\n");
        assert_eq!(staged_names(p), "");
    }

    /// With rename detection off, status shows a `git mv` as two rows — a deletion
    /// and a new file that may sit in two lists. The new path's list takes only the
    /// new file; the deletion stays staged for its own list.
    #[test]
    fn with_status_renames_off_the_source_stays_with_its_own_list() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["config", "status.renames", "false"]);
        run(p, &["mv", "a.txt", "r.txt"]);

        CliEngine::new(p)
            .commit_paths(&["r.txt".to_string()], "new side only", false)
            .unwrap();

        assert_eq!(
            out(p, &["ls-tree", "--name-only", "HEAD"]),
            "a.txt\nr.txt\n"
        );
        assert_eq!(staged_names(p), "a.txt\n", "the deletion stays staged");
    }

    /// `git status` reports an untracked folder as one entry, `d/`; the list commits
    /// its files, and another list's staged file stays out.
    #[test]
    fn commit_of_an_untracked_folder_entry_takes_its_files() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir_all(p.join("d/e")).unwrap();
        std::fs::write(p.join("d/x.txt"), "x\n").unwrap();
        std::fs::write(p.join("d/e/y.txt"), "y\n").unwrap();
        std::fs::write(p.join("a.txt"), "staged\n").unwrap();
        run(p, &["add", "a.txt"]);

        CliEngine::new(p)
            .commit_paths(&["d/".to_string()], "folder", false)
            .unwrap();

        assert_eq!(
            out(p, &["ls-tree", "-r", "--name-only", "HEAD"]),
            "a.txt\nd/e/y.txt\nd/x.txt\n"
        );
        assert_eq!(blob(p, "HEAD:a.txt"), "one\n");
        assert_eq!(staged_names(p), "a.txt\n");
    }

    /// A conflict with no operation in progress (a `stash pop` that collided): the
    /// resolved file goes in from the working tree and its index entry is resolved.
    #[test]
    fn commit_of_an_unmerged_path_takes_the_resolution() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "stashed\n").unwrap();
        run(p, &["stash", "push", "-q"]);
        std::fs::write(p.join("a.txt"), "committed\n").unwrap();
        run(p, &["commit", "-q", "-am", "moved on"]);
        let pop = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["stash", "pop", "-q"])
            .output()
            .unwrap();
        assert!(!pop.status.success(), "the pop must conflict");
        assert!(out(p, &["ls-files", "-u"]).contains("a.txt"));
        std::fs::write(p.join("a.txt"), "resolved\n").unwrap();

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "resolve", false)
            .unwrap();

        assert_eq!(blob(p, "HEAD:a.txt"), "resolved\n");
        assert_eq!(out(p, &["ls-files", "-u"]), "", "no longer unmerged");
        assert_eq!(staged_names(p), "");
    }

    /// A rebase stopped on `edit` has no `MERGE_HEAD` / `CHERRY_PICK_HEAD` /
    /// `REVERT_HEAD`: `git commit` there is an ordinary commit, and the list commit
    /// keeps the other list's staged file out, as anywhere else.
    #[test]
    fn during_a_rebase_stop_the_list_commit_leaves_other_lists_out() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b0\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-m", "b"]);
        let stop = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rebase", "-i", "HEAD~1"])
            .env("GIT_SEQUENCE_EDITOR", "sed -i.bak s/^pick/edit/")
            .output()
            .unwrap();
        assert!(
            stop.status.success(),
            "{}",
            String::from_utf8_lossy(&stop.stderr)
        );
        let rebase_dir = p.join(out(p, &["rev-parse", "--git-path", "rebase-merge"]).trim());
        assert!(rebase_dir.exists(), "the rebase is stopped");
        std::fs::write(p.join("a.txt"), "A\n").unwrap();
        std::fs::write(p.join("b.txt"), "B staged\n").unwrap();
        run(p, &["add", "b.txt"]);

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "during edit", false)
            .unwrap();

        assert_eq!(blob(p, "HEAD:a.txt"), "A\n");
        assert_eq!(blob(p, "HEAD:b.txt"), "b0\n", "b.txt stays out");
        assert_eq!(staged_names(p), "b.txt\n", "b.txt is still staged");
        assert!(rebase_dir.exists(), "the rebase is still stopped");
    }

    /// "Nothing to commit" is printed on stdout; the error carries both streams, so
    /// the banner has a reason to show.
    #[test]
    fn nothing_to_commit_says_why() {
        let dir = scratch_repo();
        let p = dir.path();
        match CliEngine::new(p).commit_paths(&["a.txt".to_string()], "empty", false) {
            Err(Error::Git { stderr, .. }) => {
                assert!(stderr.contains("nothing to commit"), "{stderr:?}")
            }
            other => panic!("expected a git refusal, got {other:?}"),
        }
    }

    /// Mid-merge the commit concludes the merge, and a merge commit must hold the
    /// whole merge result — so the list commit falls back to committing the index.
    #[test]
    fn during_a_merge_the_commit_takes_the_whole_index() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "-q", "-b", "side"]);
        std::fs::write(p.join("x.txt"), "x\n").unwrap();
        run(p, &["add", "x.txt"]);
        run(p, &["commit", "-m", "x"]);
        run(p, &["checkout", "-q", "main"]);
        run(p, &["merge", "--no-commit", "--no-ff", "side"]);
        std::fs::write(p.join("a.txt"), "A\n").unwrap();

        CliEngine::new(p)
            .commit_paths(&["a.txt".to_string()], "merge side", false)
            .unwrap();

        assert_eq!(
            out(p, &["rev-list", "--parents", "-1", "HEAD"])
                .split(' ')
                .count(),
            3
        );
        assert_eq!(
            blob(p, "HEAD:x.txt"),
            "x\n",
            "the merged file is in the merge commit"
        );
        assert_eq!(blob(p, "HEAD:a.txt"), "A\n");
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

    /// A tracked file with everything staged has an empty worktree diff in every
    /// whitespace mode, `none` included. This test used to pin the opposite — the
    /// all-add fallback for it, kept as "the prior result" — and that was the bug:
    /// the Unstaged view drew a staged file as new, and a revert there removed real
    /// lines. The staged change is where it belongs, against the index.
    #[test]
    fn diff_file_of_a_fully_staged_file_is_empty_against_the_worktree() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("f.txt"), "alpha\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["-c", "commit.gpgsign=false", "commit", "-m", "f"]);
        std::fs::write(p.join("f.txt"), "alpha\nbeta\n").unwrap();
        run(p, &["add", "f.txt"]);

        let eng = CliEngine::new(p);
        let d = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        assert!(d.hunks.is_empty(), "nothing unstaged: {:?}", d.hunks);
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

    // ---- exec_raw (git console panel) ----

    #[test]
    fn exec_raw_returns_stdout_on_success() {
        let dir = scratch_repo();
        let out = CliEngine::new(dir.path())
            .exec_raw(&["log".into(), "--oneline".into()])
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("init"), "{}", out.stdout);
        assert!(out.stderr.is_empty());
    }

    /// A non-zero exit is data for the user who typed the command, not an
    /// application error — the call itself still returns `Ok`.
    #[test]
    fn exec_raw_reports_a_nonzero_exit_instead_of_erroring() {
        let dir = scratch_repo();
        let out = CliEngine::new(dir.path())
            .exec_raw(&["not-a-real-git-subcommand".into()])
            .unwrap();
        assert_ne!(out.exit_code, 0);
        assert!(!out.stderr.is_empty());
    }

    /// `commit` with no `-m` normally opens `$EDITOR` and waits. `exec_raw` must
    /// return instead of hanging the whole application on the first such command.
    #[test]
    fn exec_raw_does_not_hang_on_a_command_that_would_open_an_editor() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "two\n").unwrap();
        run(p, &["add", "b.txt"]);
        let out = CliEngine::new(p).exec_raw(&["commit".into()]).unwrap();
        assert_ne!(out.exit_code, 0, "GIT_EDITOR=false must abort the commit");
    }

    fn branch_list(p: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["for-each-ref", "--format=%(refname)", "refs/heads"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// An invalid name is a domain refusal stated before any mutation — not a
    /// `git checkout` failure after one, and never an option smuggled in by `-`.
    #[test]
    fn create_branch_refuses_an_invalid_name_before_changing_anything() {
        let dir = scratch_repo();
        let p = dir.path();
        let eng = CliEngine::new(p);
        let before = branch_list(p);
        for name in ["a..b", "has space", "-x", "--orphan", "x.lock", "feature/", "@", ""] {
            match eng.create_branch(name, None) {
                Err(Error::Rule(m)) => assert!(m.contains("branch name"), "{name}: {m}"),
                other => panic!("{name:?}: expected a domain refusal, got {other:?}"),
            }
        }
        assert_eq!(branch_list(p), before, "no branch was created");
        assert_eq!(eng.current_branch().unwrap(), "main", "HEAD did not move");
    }

    /// `check-ref-format --branch` expands `@{-1}` to the previous branch and exits
    /// 0; taking its word would create a branch under a name nobody typed.
    #[test]
    fn create_branch_refuses_a_name_git_would_expand() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "-q", "-b", "side"]);
        run(p, &["checkout", "-q", "main"]);
        match CliEngine::new(p).create_branch("@{-1}", None) {
            Err(Error::Rule(m)) => assert!(m.contains("another branch"), "{m}"),
            other => panic!("expected a domain refusal, got {other:?}"),
        }
    }

    #[test]
    fn create_branch_accepts_a_valid_name_with_and_without_a_start_point() {
        let dir = scratch_repo();
        let p = dir.path();
        let eng = CliEngine::new(p);
        eng.create_branch("feat/one", None).unwrap();
        assert_eq!(eng.current_branch().unwrap(), "feat/one");
        eng.create_branch("two", Some("main")).unwrap();
        assert_eq!(eng.current_branch().unwrap(), "two");
    }

    fn dash_out(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `git branch -x` refuses the name, `update-ref` does not — so a branch
    /// starting with `-` can exist, and switching to it must neither read it as an
    /// option nor, when a file has the same name, as a path to restore.
    #[test]
    fn checkout_takes_a_dash_branch_as_a_branch_not_an_option_or_a_path() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("-x"), "one\n").unwrap();
        run(p, &["add", "--", "-x"]);
        run(p, &["commit", "-m", "file named -x"]);
        run(p, &["update-ref", "refs/heads/-x", "HEAD"]);
        // An edit a path checkout would throw away.
        std::fs::write(p.join("-x"), "edited\n").unwrap();

        let eng = CliEngine::new(p);
        eng.checkout("-x", false).unwrap();
        assert_eq!(dash_out(p, &["symbolic-ref", "HEAD"]), "refs/heads/-x");
        assert_eq!(std::fs::read_to_string(p.join("-x")).unwrap(), "edited\n");

        eng.create_branch("from-dash", Some("-x")).unwrap();
        assert_eq!(dash_out(p, &["symbolic-ref", "HEAD"]), "refs/heads/from-dash");
        assert_eq!(
            dash_out(p, &["rev-parse", "HEAD"]),
            dash_out(p, &["rev-parse", "refs/heads/-x"])
        );
    }
}
