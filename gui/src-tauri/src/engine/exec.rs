//! The one place a `git` process is started, and the journal of every one started.
//!
//! **Production code spawns git only through [`git`] here.** Every later feature adds
//! git calls; with one spawn point the journal, credential masking and any future
//! policy (timeouts, environment) apply to all of them at once instead of to the
//! wrappers someone remembered to update. The old per-module wrappers
//! (`branches::git`, `ops::git`, `CliEngine::git_bytes`, …) stay as thin callers of
//! this module, because what counts as a failure **differs per site** and is kept
//! there byte for byte: stderr alone or both streams in `Error::Git.stderr`, a
//! tri-state exit code (`show-ref`, `merge-base --is-ancestor`), a swallowed spawn
//! failure (`git_allow_fail`, `blob_size`). This module decides nothing of that — it
//! runs the process, records it, and hands back the raw [`Output`].
//!
//! ## Origin
//!
//! Each journal entry says whether a person asked for it (`user`) or the application
//! read state for itself (`background`). It is declared, never guessed from the
//! subcommand: a `git status` is a background snapshot after a commit and a user
//! action in the console. The Tauri layer wraps the engine call of a mutating command
//! in [`as_user`]; everything else is background by default. The scope is a closure
//! over a thread-local — command bodies are synchronous, and a closure makes an
//! `.await` inside the scope a compile error rather than a scope leaking onto another
//! task polled on the same worker thread. `build_state` runs *outside* the closure,
//! so the snapshot after a mutation is background. Consequence worth knowing: a
//! mutation's own preflight reads (`show-ref`, `check-ref-format`, `detect_state`)
//! are part of that action and are listed under "mine" with it.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::model::{JournalOrigin, JournalOutput, JournalSummary};

/// User actions the journal keeps for the whole process, across repositories.
///
/// Two rings, not one: background reads (log pages, diffs, snapshots) are many times
/// more frequent than anything a person does, and in a shared ring they would push
/// the user's own actions out within minutes.
pub const USER_CAP: usize = 1000;

/// Background reads the journal keeps, in their own ring.
pub const BACKGROUND_CAP: usize = 2000;

/// Bytes of each stream a user action — or a failed background read — keeps. One git
/// command can print megabytes, and the journal is for reading what happened, not for
/// replaying it. Callers always get the full output; only the journal's copy is cut.
pub const STREAM_CAP: usize = 256 * 1024;

/// Bytes of each stream a **successful** background read keeps. Those are the log
/// pages and diffs the panels already show; the journal needs their beginning to say
/// what ran, not a second copy. Worst case of the whole journal is then
/// `USER_CAP × 2 × STREAM_CAP` plus `BACKGROUND_CAP × 2 × QUIET_STREAM_CAP` (≈ 512 MB
/// + 64 MB), and only failures and the user's own commands can reach the first term.
pub const QUIET_STREAM_CAP: usize = 16 * 1024;

/// How much of each stream a run keeps: the full [`STREAM_CAP`] for a user action and
/// for any run that did not exit 0 (the output is then the reason), else
/// [`QUIET_STREAM_CAP`].
fn stream_cap(user: bool, code: Option<i32>) -> usize {
    if user || code != Some(0) {
        STREAM_CAP
    } else {
        QUIET_STREAM_CAP
    }
}

thread_local! {
    /// The user action the current thread is running, if any — see [`as_user`].
    static ACTION: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// Run `f` as a user action named `action` (the Tauri command's name): every git
/// process started inside is journaled with origin `user`.
///
/// It is also counted in [`OWN_ACTIONS`], process-wide: the thread-local above
/// answers "is *this* thread inside an action", which the git-dir watcher, living on
/// a thread of its own, can never ask.
pub fn as_user<T>(action: &'static str, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<&'static str>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTION.with(|a| a.set(self.0));
            OWN_ACTIONS.leave();
        }
    }
    OWN_ACTIONS.enter();
    let _restore = Restore(ACTION.with(|a| a.replace(Some(action))));
    f()
}

/// User actions running right now, process-wide, and when the last one ended.
///
/// The watcher (`crate::watch`) asks it whether a change in the git directory is
/// Graft's own doing: a commit made from the panel writes `HEAD` and `refs/` just as
/// one typed in a terminal does, and the panel re-reads after its own action anyway.
/// The grace after the end covers what the file-system events still have in flight
/// when the command returns.
pub struct Activity {
    running: AtomicUsize,
    ended: Mutex<Option<Instant>>,
}

impl Activity {
    pub const fn new() -> Self {
        Activity {
            running: AtomicUsize::new(0),
            ended: Mutex::new(None),
        }
    }

    pub fn enter(&self) {
        self.running.fetch_add(1, Ordering::SeqCst);
    }

    pub fn leave(&self) {
        // Stamp first, then decrement: a reader seeing zero also sees the stamp.
        if let Ok(mut e) = self.ended.lock() {
            *e = Some(Instant::now());
        }
        self.running.fetch_sub(1, Ordering::SeqCst);
    }

    /// Is an action running, or did one end less than `grace` ago?
    pub fn within(&self, grace: std::time::Duration) -> bool {
        if self.running.load(Ordering::SeqCst) > 0 {
            return true;
        }
        self.ended
            .lock()
            .ok()
            .and_then(|e| *e)
            .is_some_and(|at| at.elapsed() < grace)
    }
}

impl Default for Activity {
    fn default() -> Self {
        Self::new()
    }
}

/// Every [`as_user`] scope of the process.
pub static OWN_ACTIONS: Activity = Activity::new();

/// One git invocation being prepared. Built by [`git`], finished by [`Git::run`].
pub(crate) struct Git<'a> {
    dir: &'a Path,
    args: Vec<String>,
    env: &'a [(&'a str, &'a str)],
    env_remove: &'a [&'a str],
    input: Option<&'a [u8]>,
}

/// `git -C <dir> <args>`. stdin is closed unless [`Git::input`] supplies bytes —
/// the same as `Command::output()` did at every former call site.
pub(crate) fn git<'a, S: AsRef<str>>(dir: &'a Path, args: &[S]) -> Git<'a> {
    Git {
        dir,
        args: args.iter().map(|a| a.as_ref().to_string()).collect(),
        env: &[],
        env_remove: &[],
        input: None,
    }
}

impl<'a> Git<'a> {
    pub(crate) fn env(mut self, pairs: &'a [(&'a str, &'a str)]) -> Self {
        self.env = pairs;
        self
    }

    pub(crate) fn env_remove(mut self, names: &'a [&'a str]) -> Self {
        self.env_remove = names;
        self
    }

    /// Feed these bytes on stdin (`git apply`), then close it.
    pub(crate) fn input(mut self, bytes: &'a [u8]) -> Self {
        self.input = Some(bytes);
        self
    }

    /// Start the process, wait for it, journal it. A non-zero exit is **not** an
    /// error here — only a failure to spawn git (or to feed its stdin) is, as
    /// `Error::Io`, exactly as `?` on the `io::Error` made it before.
    pub(crate) fn run(self) -> Result<Output> {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let clock = Instant::now();
        let action = ACTION.with(|a| a.get());

        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(self.dir).args(&self.args);
        for (k, v) in self.env {
            cmd.env(k, v);
        }
        for k in self.env_remove {
            cmd.env_remove(k);
        }
        let result = match self.input {
            None => cmd.output(),
            Some(bytes) => feed(cmd, bytes),
        };

        // Clipping and masking up to half a megabyte happens before the lock: the
        // journal mutex is shared by every git run, reads running in parallel included.
        let record = |stdout: &[u8], stderr: &[u8], code: Option<i32>| {
            let cap = stream_cap(action.is_some(), code);
            let rec = Recorded {
                repo: self.dir.display().to_string(),
                argv: self
                    .args
                    .iter()
                    .map(|a| mask_credentials(a).into_owned())
                    .collect(),
                started_at,
                duration_ms: clock.elapsed().as_millis() as u64,
                exit_code: code,
                action,
                cap,
                stdout: clip(stdout, cap),
                stderr: clip(stderr, cap),
            };
            JOURNAL.lock().map_or(0, |mut j| j.push(rec))
        };

        match result {
            Ok(out) => {
                let code = out.status.code();
                let journal = record(&out.stdout, &out.stderr, code);
                Ok(Output {
                    command: self.args.join(" "),
                    stdout: out.stdout,
                    stderr: out.stderr,
                    code,
                    journal,
                })
            }
            Err(e) => {
                let text = format!("could not run git: {e}");
                record(b"", text.as_bytes(), None);
                Err(Error::Io(e.to_string()))
            }
        }
    }
}

/// Spawn with piped stdio, write `input`, close stdin, collect. Writes everything
/// before reading, as `CliEngine::git_stdin` always did.
fn feed(mut cmd: Command, input: &[u8]) -> std::io::Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let mut si = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("no stdin"))?;
        si.write_all(input)?;
    } // drop stdin ⇒ EOF
    child.wait_with_output()
}

/// What a finished git process produced, success or not.
pub(crate) struct Output {
    /// `args.join(" ")` — the `command` of an `Error::Git`, without `git` and `-C`.
    pub command: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `None` when the process was ended by a signal.
    pub code: Option<i32>,
    /// Id of this run's journal entry (0 if the journal lock was poisoned).
    pub journal: u64,
}

impl Output {
    /// Same answer as `ExitStatus::success()`: exited, with code 0.
    pub(crate) fn success(&self) -> bool {
        self.code == Some(0)
    }

    pub(crate) fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }

    /// `Error::Git` carrying `output` as its text, linked to this run's journal entry.
    pub(crate) fn fail_with(&self, output: String) -> Error {
        Error::Git {
            command: self.command.clone(),
            stderr: output,
            journal: (self.journal != 0).then_some(self.journal),
        }
    }

    /// `Error::Git` with stderr alone, trimmed — the cli / commit / log convention.
    pub(crate) fn fail_stderr(&self) -> Error {
        self.fail_with(String::from_utf8_lossy(&self.stderr).trim().to_string())
    }

    /// `Error::Git` with both streams — the branches / ops convention, see
    /// [`both_streams`].
    pub(crate) fn fail_both(&self) -> Error {
        self.fail_with(both_streams(&self.stdout, &self.stderr))
    }

    /// stdout on success, else [`Output::fail_stderr`].
    pub(crate) fn checked(self) -> Result<Vec<u8>> {
        if self.success() {
            Ok(self.stdout)
        } else {
            Err(self.fail_stderr())
        }
    }

    /// stdout as text on success, else [`Output::fail_both`].
    pub(crate) fn checked_both(self) -> Result<String> {
        if self.success() {
            Ok(self.stdout_text())
        } else {
            Err(self.fail_both())
        }
    }
}

/// Both output streams of a failed git command, stdout first.
///
/// `git merge`, `rebase`, `cherry-pick` and `revert` announce a conflict partly on
/// stdout ("CONFLICT (content): Merge conflict in a.txt") and partly on stderr, so
/// stderr alone would drop the very line naming the file. `Error::Git` has one
/// output field; both streams go into it, in the order git wrote them.
pub(crate) fn both_streams(stdout: &[u8], stderr: &[u8]) -> String {
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

// ── credentials ──────────────────────────────────────────────────────────────

/// Replace the userinfo of every `scheme://user:secret@host` URL in `s` with `***`.
///
/// git echoes remote URLs verbatim — in `remote -v`, in `fatal: unable to access
/// 'https://user:token@host/…'`, in the argv of a `push <url>` — and both the journal
/// and the error banner would otherwise put a token on screen. A user-only userinfo
/// is masked too: `https://ghp_xxx@github.com` is how a token is most often embedded.
///
/// The userinfo ends at the **last** `@` of the authority (a raw `@` inside a
/// password is common and invalid-but-accepted), and the authority ends at the first
/// `/ ? #`, whitespace or quote. Not touched: plain e-mail addresses and scp-style
/// `git@host:org/repo` (no `://`), and an `@` in the path (`https://host/@scope/pkg`).
pub fn mask_credentials(s: &str) -> Cow<'_, str> {
    if !s.contains("://") {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let auth_start = i + 3;
        let scheme_ok = rest[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        let after = &rest[auth_start..];
        let end = after
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | '"' | '\'' | '`' | '<' | '>') || c.is_whitespace()
            })
            .unwrap_or(after.len());
        out.push_str(&rest[..auth_start]);
        match after[..end].rfind('@') {
            Some(at) if scheme_ok && at > 0 => {
                out.push_str("***");
                rest = &after[at..];
            }
            _ => rest = after,
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

// ── journal ──────────────────────────────────────────────────────────────────

/// A stream as the journal keeps it: masked, at most `cap` bytes, cut on a character
/// boundary, and whether it was cut.
///
/// Masked before the cut, on a little more than the cap: a URL straddling the cut
/// would otherwise lose its `@host` and with it the only sign that it carries a
/// secret. A userinfo longer than the margin is not a case worth a full-output scan.
fn clip(bytes: &[u8], cap: usize) -> (String, bool) {
    const MARGIN: usize = 4096;
    let head = &bytes[..bytes.len().min(cap + MARGIN)];
    let text = mask_credentials(&String::from_utf8_lossy(head)).into_owned();
    if bytes.len() <= cap {
        return (text, false);
    }
    if text.len() <= cap {
        // Masking shortened the text below the cap; it is cut only if the margin was.
        return (text, bytes.len() > head.len());
    }
    let mut cut = cap;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (text[..cut].to_string(), true)
}

struct Recorded {
    repo: String,
    argv: Vec<String>,
    started_at: u64,
    duration_ms: u64,
    exit_code: Option<i32>,
    action: Option<&'static str>,
    /// The per-stream limit that applied, for the "truncated at" note.
    cap: usize,
    stdout: (String, bool),
    stderr: (String, bool),
}

struct Entry {
    id: u64,
    rec: Recorded,
}

/// Two rings of recent git runs — user actions and background reads — each oldest
/// first. Ids grow monotonically from 1 for the life of the process across both
/// rings, so a client can ask "everything after id N" of either or of their merge.
///
/// An id is handed out when the run is **recorded**, that is when it finished, so id
/// order is the time order of the journal; "All" is the merge of the two rings by
/// it. Merging by start time instead would slot a long command that began earlier
/// *behind* entries a client has already paged past.
pub(crate) struct Journal {
    user_cap: usize,
    background_cap: usize,
    next: u64,
    user: VecDeque<Entry>,
    background: VecDeque<Entry>,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal::new(USER_CAP, BACKGROUND_CAP));

impl Journal {
    pub(crate) const fn new(user_cap: usize, background_cap: usize) -> Self {
        Self {
            user_cap,
            background_cap,
            next: 1,
            user: VecDeque::new(),
            background: VecDeque::new(),
        }
    }

    fn push(&mut self, rec: Recorded) -> u64 {
        let id = self.next;
        self.next += 1;
        let (ring, cap) = if rec.action.is_some() {
            (&mut self.user, self.user_cap)
        } else {
            (&mut self.background, self.background_cap)
        };
        ring.push_back(Entry { id, rec });
        while ring.len() > cap {
            ring.pop_front();
        }
        id
    }

    fn list(&self, mine: bool, after: Option<u64>) -> Vec<JournalSummary> {
        let after = after.unwrap_or(0);
        let user = newer(&self.user, after);
        if mine {
            return user.into_iter().map(summary).collect();
        }
        let bg = newer(&self.background, after);
        let (mut u, mut b) = (user.into_iter().peekable(), bg.into_iter().peekable());
        let mut out = Vec::new();
        loop {
            let next = match (u.peek(), b.peek()) {
                (Some(x), Some(y)) if x.id < y.id => u.next(),
                (Some(_), Some(_)) => b.next(),
                (Some(_), None) => u.next(),
                (None, Some(_)) => b.next(),
                (None, None) => break,
            };
            out.extend(next.map(summary));
        }
        out
    }

    fn output(&self, id: u64) -> Option<JournalOutput> {
        // Ids are ascending in each ring; a binary search keeps the lookup cheap.
        let r = find(&self.user, id).or_else(|| find(&self.background, id))?;
        Some(JournalOutput {
            id,
            stdout: r.stdout.0.clone(),
            stderr: r.stderr.0.clone(),
            stdout_truncated: r.stdout.1,
            stderr_truncated: r.stderr.1,
            limit_bytes: r.cap as u64,
        })
    }
}

/// Entries of one ring newer than `after`. Ids ascend, so what the client already
/// has is skipped without scanning it.
fn newer(ring: &VecDeque<Entry>, after: u64) -> Vec<&Entry> {
    let from = ring.partition_point(|e| e.id <= after);
    ring.range(from..).collect()
}

fn find(ring: &VecDeque<Entry>, id: u64) -> Option<&Recorded> {
    ring.binary_search_by_key(&id, |e| e.id)
        .ok()
        .map(|i| &ring[i].rec)
}

fn summary(e: &Entry) -> JournalSummary {
    let r = &e.rec;
    JournalSummary {
        id: e.id,
        repo: r.repo.clone(),
        argv: r.argv.clone(),
        started_at: r.started_at,
        duration_ms: r.duration_ms,
        exit_code: r.exit_code,
        origin: if r.action.is_some() {
            JournalOrigin::User
        } else {
            JournalOrigin::Background
        },
        action: r.action.map(str::to_string),
    }
}

/// Journal entries with id greater than `after`, oldest first: the user ring alone
/// when `mine`, else both rings merged by id. Outputs are left out — see
/// [`journal_output`].
pub fn journal_list(mine: bool, after: Option<u64>) -> Vec<JournalSummary> {
    JOURNAL
        .lock()
        .map(|j| j.list(mine, after))
        .unwrap_or_default()
}

/// Both streams of one entry, or `None` once its ring has dropped it.
pub fn journal_output(id: u64) -> Option<JournalOutput> {
    JOURNAL.lock().ok().and_then(|j| j.output(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;

    fn rec(action: Option<&'static str>) -> Recorded {
        Recorded {
            cap: STREAM_CAP,
            repo: "/r".into(),
            argv: vec!["status".into()],
            started_at: 0,
            duration_ms: 0,
            exit_code: Some(0),
            action,
            stdout: ("out".into(), false),
            stderr: (String::new(), false),
        }
    }

    #[test]
    fn journal_is_a_ring_with_monotonic_ids() {
        let mut j = Journal::new(3, 3);
        for _ in 0..5 {
            j.push(rec(None));
        }
        let ids: Vec<u64> = j.list(false, None).iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 4, 5]);
        assert!(j.output(1).is_none(), "evicted entries are gone");
        assert_eq!(j.output(4).unwrap().stdout, "out");
        let after: Vec<u64> = j.list(false, Some(4)).iter().map(|e| e.id).collect();
        assert_eq!(after, vec![5]);
    }

    #[test]
    fn mine_keeps_only_user_actions() {
        let mut j = Journal::new(10, 10);
        j.push(rec(None));
        j.push(rec(Some("commit_list")));
        j.push(rec(None));
        let mine = j.list(true, None);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, 2);
        assert_eq!(mine[0].origin, JournalOrigin::User);
        assert_eq!(mine[0].action.as_deref(), Some("commit_list"));
        assert_eq!(j.list(false, None).len(), 3);
    }

    /// The point of two rings: a flood of background reads never pushes a user
    /// action out, and "All" is still one list in id order.
    #[test]
    fn background_does_not_evict_user_actions() {
        let mut j = Journal::new(2, 3);
        j.push(rec(Some("commit_list"))); // 1
        for _ in 0..10 {
            j.push(rec(None)); // 2..=11
        }
        j.push(rec(Some("push"))); // 12
        j.push(rec(None)); // 13
        let mine: Vec<u64> = j.list(true, None).iter().map(|e| e.id).collect();
        assert_eq!(mine, vec![1, 12], "the user ring kept both actions");
        let all: Vec<u64> = j.list(false, None).iter().map(|e| e.id).collect();
        assert_eq!(
            all,
            vec![1, 10, 11, 12, 13],
            "merged by id, background capped at 3"
        );
        let after: Vec<u64> = j.list(false, Some(11)).iter().map(|e| e.id).collect();
        assert_eq!(after, vec![12, 13]);
        assert!(j.output(1).is_some() && j.output(13).is_some());
        assert!(j.output(5).is_none(), "evicted from the background ring");

        j.push(rec(Some("fetch"))); // 14: the user ring is full, 1 goes
        let mine: Vec<u64> = j.list(true, None).iter().map(|e| e.id).collect();
        assert_eq!(mine, vec![12, 14]);
    }

    #[test]
    fn successful_background_reads_keep_less_output() {
        assert_eq!(stream_cap(true, Some(0)), STREAM_CAP);
        assert_eq!(stream_cap(false, Some(0)), QUIET_STREAM_CAP);
        assert_eq!(
            stream_cap(false, Some(128)),
            STREAM_CAP,
            "a failure keeps its reason"
        );
        assert_eq!(
            stream_cap(false, None),
            STREAM_CAP,
            "so does a run that never exited"
        );

        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("big.txt"), "x".repeat(40 * 1024)).unwrap();
        let bg = git(p, &["hash-object", "-w", "big.txt"]).run().unwrap();
        let blob = bg.stdout_text().trim().to_string();

        let quiet = git(p, &["cat-file", "-p", &blob]).run().unwrap();
        let o = journal_output(quiet.journal).unwrap();
        assert!(o.stdout_truncated);
        assert_eq!(o.stdout.len(), QUIET_STREAM_CAP);
        assert_eq!(o.limit_bytes, QUIET_STREAM_CAP as u64);
        assert_eq!(
            quiet.stdout.len(),
            40 * 1024,
            "the caller still gets everything"
        );

        let mine = as_user("test_action", || {
            git(p, &["cat-file", "-p", &blob]).run().unwrap()
        });
        let o = journal_output(mine.journal).unwrap();
        assert!(!o.stdout_truncated);
        assert_eq!(o.stdout.len(), 40 * 1024);
        assert_eq!(o.limit_bytes, STREAM_CAP as u64);
    }

    #[test]
    fn clip_cuts_on_a_char_boundary_and_says_so() {
        let (t, cut) = clip(b"short", STREAM_CAP);
        assert_eq!((t.as_str(), cut), ("short", false));

        // 'я' is two bytes; after a one-byte lead the cap lands inside one.
        let big = format!("a{}", "я".repeat(STREAM_CAP));
        let (t, cut) = clip(big.as_bytes(), STREAM_CAP);
        assert!(cut);
        assert_eq!(t.len(), STREAM_CAP - 1);
        assert!(
            t.chars().skip(1).all(|c| c == 'я'),
            "no half character at the cut"
        );

        let exact = "a".repeat(STREAM_CAP);
        assert!(
            !clip(exact.as_bytes(), STREAM_CAP).1,
            "exactly the cap is not truncated"
        );
    }

    #[test]
    fn clip_masks_before_cutting() {
        let mut s = "a".repeat(STREAM_CAP - 20);
        s.push_str(" https://user:secret-token@host/repo");
        let (t, cut) = clip(s.as_bytes(), STREAM_CAP);
        assert!(cut);
        assert!(
            !t.contains("secret"),
            "no part of the token survives the cut: {}",
            &t[t.len() - 40..]
        );
    }

    #[test]
    fn masks_userinfo_in_urls() {
        let cases = [
            (
                "https://user:token@host/org/repo.git",
                "https://***@host/org/repo.git",
            ),
            (
                "https://ghp_abc123@github.com/o/r",
                "https://***@github.com/o/r",
            ),
            (
                "fatal: unable to access 'https://u:p@h.example/r/': 403",
                "fatal: unable to access 'https://***@h.example/r/': 403",
            ),
            (
                "a https://u:p@one/x b http://v:q@two c",
                "a https://***@one/x b http://***@two c",
            ),
            ("ends with https://u:p@host", "ends with https://***@host"),
            (
                "raw at https://user:p@ss@host/r",
                "raw at https://***@host/r",
            ),
            ("ssh://git@host:22/r.git", "ssh://***@host:22/r.git"),
        ];
        for (input, want) in cases {
            assert_eq!(mask_credentials(input), want, "input: {input}");
        }
    }

    #[test]
    fn leaves_non_credentials_alone() {
        for s in [
            "Author: Jane <jane@example.com>",
            "git@github.com:org/repo.git",
            "https://host/@scope/pkg",
            "https://host/path?x=a@b",
            "see ://weird@thing",
            "plain text",
        ] {
            assert_eq!(mask_credentials(s), s);
        }
    }

    /// End to end through the process-wide journal. Tests share it across threads,
    /// so entries are found by this test's unique temporary directory, never by
    /// position.
    #[test]
    fn runs_are_journaled_with_their_origin() {
        let dir = scratch_repo();
        let p = dir.path();
        let repo = p.display().to_string();

        let bg = git(p, &["rev-parse", "HEAD"]).run().unwrap();
        assert!(bg.success());
        let failed = as_user("test_action", || {
            git(p, &["rev-parse", "--verify", "no-such-rev"])
                .run()
                .unwrap()
        });
        assert!(!failed.success());

        let all: Vec<_> = journal_list(false, None)
            .into_iter()
            .filter(|e| e.repo == repo)
            .collect();
        let b = all.iter().find(|e| e.id == bg.journal).unwrap();
        assert_eq!(b.origin, JournalOrigin::Background);
        assert_eq!(b.argv, vec!["rev-parse", "HEAD"]);
        assert_eq!(b.exit_code, Some(0));
        let f = all.iter().find(|e| e.id == failed.journal).unwrap();
        assert_eq!(f.origin, JournalOrigin::User);
        assert_eq!(f.action.as_deref(), Some("test_action"));
        assert_ne!(f.exit_code, Some(0));

        let mine: Vec<u64> = journal_list(true, None)
            .into_iter()
            .filter(|e| e.repo == repo)
            .map(|e| e.id)
            .collect();
        assert_eq!(
            mine,
            vec![failed.journal],
            "the scope ended with the closure"
        );

        // A URL in argv is masked at record time (offline: check-ref-format never
        // touches the network, and refuses this name).
        let url = git(p, &["check-ref-format", "https://u:secret@h/x"])
            .run()
            .unwrap();
        let e = journal_list(false, Some(url.journal - 1))
            .into_iter()
            .find(|e| e.id == url.journal)
            .unwrap();
        assert_eq!(e.argv, vec!["check-ref-format", "https://***@h/x"]);

        let out = journal_output(bg.journal).unwrap();
        assert_eq!(out.stdout, String::from_utf8_lossy(&bg.stdout));
        assert!(!journal_output(failed.journal).unwrap().stderr.is_empty());

        match failed.fail_stderr() {
            Error::Git { journal, .. } => assert_eq!(journal, Some(failed.journal)),
            other => panic!("expected Error::Git, got {other:?}"),
        }
    }

    #[test]
    fn input_reaches_stdin() {
        let dir = scratch_repo();
        let out = git(dir.path(), &["hash-object", "--stdin"])
            .input(b"one\n")
            .run()
            .unwrap();
        assert!(out.success());
        let want = git(dir.path(), &["rev-parse", "HEAD:a.txt"]).run().unwrap();
        assert_eq!(out.stdout_text().trim(), want.stdout_text().trim());
    }
}
