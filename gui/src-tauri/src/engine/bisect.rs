//! `git bisect` — the search for the commit that brought a bug in.
//!
//! The state is read from the files git itself keeps for a bisect, never from the
//! sentences `git bisect` prints: `BISECT_START` (the branch — or, on a detached
//! HEAD, the commit — `git bisect reset` returns to; its presence is the test git
//! itself uses), `BISECT_TERMS` (the words for "bad" and "good", line one and line
//! two), `BISECT_LOG` (the replay script: every answer, and the answer of the search
//! itself) and `refs/bisect/*` (the marks git's algorithm works from). Every path is
//! resolved through `rev-parse --git-path`, as for the other operation markers.
//!
//! ## `BISECT_LOG` is parsed strictly
//!
//! A command line is `git bisect start <quoted args>` or `git bisect <term> <oid>`
//! with `<term>` one of the repository's two words or `skip`; anything else is an
//! `Error::Parse`, not a skipped line — a log read short would draw a search with a
//! mark missing, indistinguishable from the real one. The comments git writes are
//! read where they carry data: `# <term>: [<oid>] <subject>` (the only oid record of
//! the ends given to `git bisect start <bad> <good>`, whose command line keeps the
//! revisions as typed), `# first <bad> commit: [<oid>] <subject>` (the answer) and
//! `# possible first <bad> commit: [<oid>] …` (only skipped commits were left).
//! Other comments (`# status: …`, `# only skipped commits left to test`) are
//! comments: `git bisect replay` ignores them, and so does this parser.
//!
//! A bisect whose log does not parse is still a bisect: [`state`] reports it with
//! the parse error in `problem` and no marks, so the strip can say what is wrong
//! and "Finish" (`git bisect reset`, which needs only `BISECT_START`) still works.
//! Failing the whole `RepoState` over it would leave the user with no way out but a
//! terminal.
//!
//! ## Revisions
//!
//! `git bisect` accepts no `--end-of-options` (it answers "unrecognized option").
//! Every revision from the client is therefore resolved first by `rev-parse
//! --verify --end-of-options <rev>^{commit}`, and only the full hex object id — which
//! can never read as an option — reaches `git bisect`. `start` closes its revisions
//! with `--`: what follows would be pathspecs.
//!
//! `git bisect run` is not offered: it executes an arbitrary command, and this
//! application runs nothing but git.

use std::path::Path;

use crate::engine::cli::CliEngine;
use crate::engine::exec;
use crate::error::{Error, Result};
use crate::model::BisectState;

/// git's own words when a bisect was started without `--term-*`.
pub const DEFAULT_BAD: &str = "bad";
pub const DEFAULT_GOOD: &str = "good";

/// An answer given about one commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Bad,
    Good,
    Skip,
}

/// What `BISECT_LOG` says, in the order it says it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Log {
    /// Every answer, oldest first — the `# <term>: [<oid>]` records.
    pub marks: Vec<(String, Mark)>,
    /// `(oid, subject)` of the first bad commit, when git named it and no later
    /// answer reopened the search.
    pub first_bad: Option<(String, String)>,
    /// "The first bad commit could be any of": only skipped commits were left.
    pub candidates: Vec<String>,
}

fn is_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A term reaches argv as the subcommand word of `git bisect <term>`, so it is
/// checked even though it comes from git's own `BISECT_TERMS`: a repository is user
/// data. git refuses terms that are not valid ref name components; a leading dash
/// or whitespace would make the argument read as something else.
fn check_term(term: &str) -> Result<()> {
    let ok = !term.is_empty()
        && !term.starts_with('-')
        && !term.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(Error::Parse(format!(
            "BISECT_TERMS: unusable term {term:?}"
        )))
    }
}

/// `# <word>: [<oid>] <subject>` → `(word, oid, subject)`; `None` for a comment of
/// another shape. An oid that is not one is an error: the shape promised one.
fn record(comment: &str, line_no: usize) -> Result<Option<(&str, String, String)>> {
    let Some((word, rest)) = comment.split_once(": [") else {
        return Ok(None);
    };
    let Some((oid, subject)) = rest.split_once(']') else {
        return Err(Error::Parse(format!(
            "BISECT_LOG line {line_no}: unterminated commit record {comment:?}"
        )));
    };
    if !is_oid(oid) {
        return Err(Error::Parse(format!(
            "BISECT_LOG line {line_no}: {oid:?} is not an object id"
        )));
    }
    Ok(Some((
        word,
        oid.to_ascii_lowercase(),
        subject.trim().to_string(),
    )))
}

/// Parse `BISECT_LOG` under the repository's terms (see the module docs for the
/// grammar). Strict: a line that is neither a comment nor a bisect command this
/// parser knows, or a command whose argument is not what git writes, is an
/// `Error::Parse` naming the line.
pub(crate) fn parse_log(text: &str, bad: &str, good: &str) -> Result<Log> {
    let mut log = Log::default();
    let word_mark = |w: &str| {
        if w == bad {
            Some(Mark::Bad)
        } else if w == good {
            Some(Mark::Good)
        } else if w == "skip" {
            Some(Mark::Skip)
        } else {
            None
        }
    };
    let first_prefix = format!("first {bad} commit");
    let possible_prefix = format!("possible first {bad} commit");
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            let comment = comment.trim_start();
            let Some((word, oid, subject)) = record(comment, n)? else {
                continue; // `# status: …` and friends: comments, as for replay
            };
            if word == first_prefix {
                log.first_bad = Some((oid, subject));
            } else if word == possible_prefix {
                log.candidates.push(oid);
            } else if let Some(mark) = word_mark(word) {
                // A new answer reopens a search that had ended.
                log.first_bad = None;
                log.candidates.clear();
                log.marks.push((oid, mark));
            } else {
                return Err(Error::Parse(format!(
                    "BISECT_LOG line {n}: {word:?} is none of the terms {bad:?}, {good:?}, \"skip\""
                )));
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("git bisect ") else {
            return Err(Error::Parse(format!(
                "BISECT_LOG line {n}: not a bisect command: {line:?}"
            )));
        };
        let (verb, args) = rest.split_once(' ').unwrap_or((rest, ""));
        if verb == "start" {
            // Arguments are shell-quoted by git (`'HEAD' '--term-new=x'`).
            let args = args.trim();
            if !args.is_empty() && !(args.starts_with('\'') && args.ends_with('\'')) {
                return Err(Error::Parse(format!(
                    "BISECT_LOG line {n}: unquoted start arguments {args:?}"
                )));
            }
        } else if word_mark(verb).is_some() {
            let oid = args.trim();
            if !is_oid(oid) {
                return Err(Error::Parse(format!(
                    "BISECT_LOG line {n}: \"git bisect {verb}\" without an object id: {args:?}"
                )));
            }
        } else {
            return Err(Error::Parse(format!(
                "BISECT_LOG line {n}: unknown bisect command {verb:?}"
            )));
        }
        log.first_bad = None;
        log.candidates.clear();
    }
    Ok(log)
}

/// Where git keeps the files of a bisect for this worktree.
struct Files {
    start: std::path::PathBuf,
    terms: std::path::PathBuf,
    log: std::path::PathBuf,
    head: std::path::PathBuf,
}

fn files(repo: &Path) -> Result<Files> {
    let mut p = CliEngine::new(repo).git_paths(&[
        "BISECT_START",
        "BISECT_TERMS",
        "BISECT_LOG",
        "BISECT_HEAD",
    ])?;
    let head = p.pop().unwrap_or_default();
    let log = p.pop().unwrap_or_default();
    let terms = p.pop().unwrap_or_default();
    let start = p.pop().unwrap_or_default();
    Ok(Files {
        start,
        terms,
        log,
        head,
    })
}

/// Is a bisect in progress? `BISECT_START` is the file git itself tests for.
pub fn active(repo: &Path) -> Result<bool> {
    Ok(files(repo)?.start.exists())
}

fn read_file(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(format!("{}: {e}", path.display()))),
    }
}

/// The repository's two words: `(bad, good)`. No file means git's defaults (a
/// bisect started by an old git); a file of another shape is an error.
fn terms(f: &Files) -> Result<(String, String)> {
    let Some(text) = read_file(&f.terms)? else {
        return Ok((DEFAULT_BAD.into(), DEFAULT_GOOD.into()));
    };
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let (Some(bad), Some(good), None) = (lines.next(), lines.next(), lines.next()) else {
        return Err(Error::Parse(format!(
            "BISECT_TERMS: expected two lines, got {text:?}"
        )));
    };
    check_term(bad)?;
    check_term(good)?;
    if bad == good || bad == "skip" || good == "skip" {
        return Err(Error::Parse(format!(
            "BISECT_TERMS: unusable terms {bad:?} / {good:?}"
        )));
    }
    Ok((bad.to_string(), good.to_string()))
}

/// `(bad, good, skip)` from `refs/bisect/*`. A ref under it that is none of git's
/// three shapes is an error: it would be a mark this reading cannot place.
fn marks_from_refs(
    repo: &Path,
    bad: &str,
    good: &str,
) -> Result<(Option<String>, Vec<String>, Vec<String>)> {
    let out = exec::git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)",
            "refs/bisect/",
        ],
    )
    .run()?
    .checked()?;
    let text = String::from_utf8_lossy(&out);
    let (mut b, mut g, mut s) = (None, Vec::new(), Vec::new());
    let good_prefix = format!("{good}-");
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (Some(name), Some(oid)) = (line.split('\0').next(), line.split('\0').nth(1)) else {
            return Err(Error::Parse(format!("bisect ref record {line:?}")));
        };
        let leaf = name.strip_prefix("refs/bisect/").unwrap_or(name);
        if !is_oid(oid) {
            return Err(Error::Parse(format!("bisect ref {name}: {oid:?}")));
        }
        let oid = oid.to_ascii_lowercase();
        if leaf == bad {
            b = Some(oid);
        } else if leaf.starts_with(&good_prefix) {
            g.push(oid);
        } else if leaf.starts_with("skip-") {
            s.push(oid);
        } else {
            return Err(Error::Parse(format!(
                "unknown bisect ref {name} (terms {bad:?} / {good:?})"
            )));
        }
    }
    Ok((b, g, s))
}

/// `(hash, subject)` of a revision; `None` when it does not resolve (an unborn
/// HEAD).
fn commit_line(repo: &Path, rev: &str) -> Result<Option<(String, String)>> {
    let out = exec::git(
        repo,
        &[
            "log",
            "-1",
            "--format=%H%x00%s",
            "--end-of-options",
            rev,
            "--",
        ],
    )
    .run()?;
    if !out.success() {
        return Ok(None);
    }
    let text = out.stdout_text();
    let text = text.trim_end_matches('\n');
    let Some((hash, subject)) = text.split_once('\0') else {
        return Err(Error::Parse(format!("log record of {rev}: {text:?}")));
    };
    Ok(Some((hash.to_string(), subject.to_string())))
}

/// `(bisect_nr, bisect_steps)` for the range `bad` minus `good` — the numbers git
/// prints as "N revisions left to test after this (roughly M steps)". Skipped
/// commits are not counted out, exactly as in git's own sentence. An empty range
/// (`rev-list` exits 1 and prints nothing) has no estimate.
fn estimate(repo: &Path, bad: &str, good: &[String]) -> Result<(Option<u32>, Option<u32>)> {
    let mut args = vec![
        "rev-list".to_string(),
        "--bisect-vars".to_string(),
        "--end-of-options".to_string(),
        bad.to_string(),
    ];
    args.extend(good.iter().map(|g| format!("^{g}")));
    let out = exec::git(repo, &args).run()?;
    if !out.success() {
        if out.code == Some(1) && out.stdout.is_empty() {
            return Ok((None, None));
        }
        return Err(out.fail_stderr());
    }
    let text = out.stdout_text();
    let var = |name: &str| -> Result<u32> {
        let v = text
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix(name)
                    .and_then(|r| r.strip_prefix('='))
            })
            .ok_or_else(|| {
                Error::Parse(format!("rev-list --bisect-vars: no {name} in {text:?}"))
            })?;
        v.trim_matches('\'')
            .parse()
            .map_err(|_| Error::Parse(format!("rev-list --bisect-vars: {name}={v}")))
    };
    Ok((Some(var("bisect_nr")?), Some(var("bisect_steps")?)))
}

/// The bisect as it stands, **strictly**: a log or a ref this cannot read is an
/// error. `Ok(None)` when no bisect is in progress.
pub fn read(repo: &Path) -> Result<Option<BisectState>> {
    let f = files(repo)?;
    let Some(start) = read_file(&f.start)? else {
        return Ok(None);
    };
    let mut s = base(repo, &f, &start)?;
    fill(repo, &f, &mut s)?;
    Ok(Some(s))
}

/// What `detect_state` reports: [`read`], with a parse error folded into
/// `problem` instead of failing the state (see the module docs) — the terms, the
/// way back and the commit under test are still told, the marks are not. A git
/// that fails to run is still an error.
pub fn state(repo: &Path) -> Result<Option<BisectState>> {
    match read(repo) {
        Err(Error::Parse(m)) => {
            let f = files(repo)?;
            let start = read_file(&f.start)?.unwrap_or_default();
            let mut s = base(repo, &f, &start)?;
            if let Ok((bad, good)) = terms(&f) {
                s.term_bad = bad;
                s.term_good = good;
            }
            s.problem = Some(m);
            Ok(Some(s))
        }
        other => other,
    }
}

/// What can be read without the log and the refs: where the bisect returns to and
/// the commit under test. Terms are filled in by [`fill`]; the defaults stand in.
fn base(repo: &Path, f: &Files, start: &str) -> Result<BisectState> {
    let start = start.trim();
    let (start_branch, start_commit) = if is_oid(start) {
        (None, Some(start.to_ascii_lowercase()))
    } else if start.is_empty() {
        (None, None)
    } else {
        (Some(start.to_string()), None)
    };
    let rev = if f.head.exists() {
        "BISECT_HEAD"
    } else {
        "HEAD"
    };
    let current = commit_line(repo, rev)?;
    Ok(BisectState {
        term_bad: DEFAULT_BAD.into(),
        term_good: DEFAULT_GOOD.into(),
        start_branch,
        start_commit,
        current: current.as_ref().map(|c| c.0.clone()),
        current_subject: current.map(|c| c.1),
        ..BisectState::default()
    })
}

fn fill(repo: &Path, f: &Files, s: &mut BisectState) -> Result<()> {
    let (bad, good) = terms(f)?;
    s.term_bad = bad.clone();
    s.term_good = good.clone();
    let text = read_file(&f.log)?.unwrap_or_default();
    let log = parse_log(&text, &bad, &good)?;
    let (b, g, k) = marks_from_refs(repo, &bad, &good)?;
    s.bad = b;
    s.good = g;
    s.skip = k;
    s.candidates = log.candidates;
    if let Some((oid, subject)) = log.first_bad {
        s.first_bad = Some(oid);
        s.first_bad_subject = Some(subject);
    }
    if let (Some(bad), false, None) = (&s.bad, s.good.is_empty(), &s.first_bad) {
        let (nr, steps) = estimate(repo, bad, &s.good)?;
        s.remaining = nr;
        s.steps = steps;
    }
    Ok(())
}

/// Full object id of a commit-ish from the client. `--end-of-options`: a
/// revision named `-x` is a revision, not a flag.
fn resolve(repo: &Path, rev: &str) -> Result<String> {
    let out = exec::git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .run()?
    .checked_both()?;
    let oid = out.trim().to_string();
    if !is_oid(&oid) {
        return Err(Error::Parse(format!("rev-parse {rev}: {oid:?}")));
    }
    Ok(oid)
}

/// Start a bisect: `bad` is the commit known to have the bug (HEAD when `None`),
/// `good` the ones known not to. With no good commit git waits for one; with both
/// ends it checks out the first commit to test at once.
///
/// Refused while any operation — a bisect included — is in progress.
pub fn start(repo: &Path, bad: Option<&str>, good: &[String]) -> Result<()> {
    let kind = crate::engine::ops::detect_kind(repo)?;
    if kind != crate::model::OperationKind::None {
        return Err(Error::Rule(
            "an operation is in progress; finish or abort it before starting a bisect".into(),
        ));
    }
    let bad = resolve(repo, bad.unwrap_or("HEAD"))?;
    let mut args = vec!["bisect".to_string(), "start".to_string(), bad];
    for g in good {
        args.push(resolve(repo, g)?);
    }
    args.push("--".into());
    exec::git(repo, &args).run()?.checked_both()?;
    Ok(())
}

/// "<oid> is the first <term> commit" — git's announcement on stdout.
fn announced_first(stdout: &str, bad: &str) -> Option<String> {
    let tail = format!(" is the first {bad} commit");
    stdout.lines().find_map(|l| {
        let oid = l.trim().strip_suffix(&tail)?;
        is_oid(oid).then(|| oid.to_ascii_lowercase())
    })
}

/// Answer for a commit: `mark` is `bad`, `good` or `skip` — the roles, which
/// are spelled with the repository's own terms on the way to git. `hash` `None`
/// marks the commit under test.
///
/// When git names the first bad commit, the log must record it too — the answer
/// is read back from there by every later state read, so a mismatch is an error
/// rather than an answer that vanishes on the next refresh. When only skipped
/// commits are left git exits non-zero ("We cannot bisect more!"); that is an
/// answer as well (`candidates`), not a failure.
pub fn mark(repo: &Path, mark: &str, hash: Option<&str>) -> Result<()> {
    let kind = crate::engine::ops::detect_kind(repo)?;
    if kind != crate::model::OperationKind::Bisect {
        return Err(Error::Rule(if active(repo)? {
            "another operation is in progress inside the bisect; finish it first".into()
        } else {
            "no bisect in progress".into()
        }));
    }
    let f = files(repo)?;
    let (bad, good) = terms(&f)?;
    let word = match mark {
        "bad" => bad.clone(),
        "good" => good.clone(),
        "skip" => "skip".to_string(),
        other => {
            return Err(Error::Rule(format!(
                "unknown bisect mark: {other} (expected bad, good or skip)"
            )))
        }
    };
    let log_text = || -> Result<String> { Ok(read_file(&f.log)?.unwrap_or_default()) };
    let read_log = || -> Result<Log> { parse_log(&log_text()?, &bad, &good) };
    // A log this cannot read is refused before git adds to it: the answer the
    // mark may produce would be written where no later state read finds it.
    let before = log_text()?;
    parse_log(&before, &bad, &good)?;
    let mut args = vec!["bisect".to_string(), word];
    if let Some(h) = hash {
        args.push(resolve(repo, h)?);
    }
    let out = exec::git(repo, &args).run()?;
    if !out.success() {
        // "Only skipped commits left" is exit 2 *and* a log that grew by this
        // answer. Candidates alone prove nothing: they may be left over from an
        // earlier answer, and a refusal would then read as a success.
        if out.code == Some(2) {
            let after = log_text()?;
            if after != before && !parse_log(&after, &bad, &good)?.candidates.is_empty() {
                return Ok(());
            }
        }
        return Err(out.fail_both());
    }
    if let Some(oid) = announced_first(&out.stdout_text(), &bad) {
        let recorded = read_log()?.first_bad.map(|f| f.0);
        if recorded.as_deref() != Some(oid.as_str()) {
            return Err(Error::Parse(format!(
                "git named {oid} the first {bad} commit, but BISECT_LOG records {recorded:?}"
            )));
        }
    }
    Ok(())
}

/// End the bisect: `git bisect reset` returns to the branch (or commit) it started
/// from and removes `refs/bisect/*`. Needs nothing but `BISECT_START`, so it works
/// on a bisect whose log does not parse.
pub fn reset(repo: &Path) -> Result<()> {
    if !active(repo)? {
        return Err(Error::Rule("no bisect in progress".into()));
    }
    exec::git(repo, &["bisect", "reset"])
        .run()?
        .checked_both()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::engine::ops::{self, detect_state};
    use crate::model::OperationKind;
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

    /// Eight commits c1..c8 on `main`, `a.txt` holding the number. Returns the repo
    /// and the hashes, oldest first. The "bug" appears in c6: `a.txt >= 6`.
    fn repo_with_history() -> (tempfile::TempDir, Vec<String>) {
        let dir = scratch_repo();
        let p = dir.path();
        let mut hashes = Vec::new();
        for i in 1..=8 {
            std::fs::write(p.join("a.txt"), format!("{i}\n")).unwrap();
            git(p, &["commit", "-qam", &format!("c{i}")]);
            hashes.push(git(p, &["rev-parse", "HEAD"]));
        }
        (dir, hashes)
    }

    fn has_bug(p: &Path) -> bool {
        let n: u32 = std::fs::read_to_string(p.join("a.txt"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        n >= 6
    }

    fn bisect(p: &Path) -> BisectState {
        let s = detect_state(p).unwrap();
        s.bisect.expect("a bisect is reported")
    }

    /// Bisect from Graft to the end: the first bad commit is found, and the answer
    /// travels in the state — hash and subject — together with the marks.
    #[test]
    fn a_bisect_from_graft_finds_the_first_bad_commit() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        start(p, Some(&h[7]), &[h[0].clone()]).unwrap();

        let s = detect_state(p).unwrap();
        assert_eq!(s.kind, OperationKind::Bisect);
        let b = s.bisect.unwrap();
        assert_eq!(b.start_branch.as_deref(), Some("main"));
        assert_eq!(b.bad.as_deref(), Some(h[7].as_str()));
        assert_eq!(b.good, vec![h[0].clone()]);
        assert!(
            b.current.is_some() && b.current != b.bad,
            "a commit is checked out to test"
        );
        assert_eq!(b.remaining, Some(3), "git: 3 revisions left after this");
        assert_eq!(b.steps, Some(2), "roughly 2 steps");
        assert_eq!(b.first_bad, None);

        for _ in 0..10 {
            let b = bisect(p);
            if b.first_bad.is_some() {
                break;
            }
            mark(p, if has_bug(p) { "bad" } else { "good" }, None).unwrap();
        }
        let b = bisect(p);
        assert_eq!(
            b.first_bad.as_deref(),
            Some(h[5].as_str()),
            "c6 brought the bug in"
        );
        assert_eq!(b.first_bad_subject.as_deref(), Some("c6"));
        assert_eq!(b.remaining, None, "nothing left to estimate");
        assert!(
            b.good.len() >= 2,
            "every good answer is listed: {:?}",
            b.good
        );
    }

    /// A bisect started in a terminal, before Graft looked: recognised, with its
    /// state — here the bare `git bisect start` that knows neither end yet.
    #[test]
    fn a_bisect_started_outside_is_recognised() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        git(p, &["bisect", "start"]);
        let s = detect_state(p).unwrap();
        assert_eq!(s.kind, OperationKind::Bisect);
        let b = s.bisect.unwrap();
        assert_eq!(
            (b.bad.clone(), b.good.len()),
            (None, 0),
            "both ends unknown"
        );
        assert_eq!(
            b.current.as_deref(),
            Some(h[7].as_str()),
            "HEAD is under test"
        );
        assert_eq!(b.remaining, None);
        assert_eq!(b.problem, None);

        git(p, &["bisect", "bad"]);
        git(p, &["bisect", "good", &h[2]]);
        let b = bisect(p);
        assert_eq!(b.bad.as_deref(), Some(h[7].as_str()));
        assert_eq!(b.good, vec![h[2].clone()]);
        assert!(b.remaining.is_some() && b.steps.is_some());
    }

    /// `--term-new` / `--term-old`: the state speaks the repository's words, and a
    /// mark by role reaches git spelled with them (`git bisect good` would be
    /// refused under these terms).
    #[test]
    fn custom_terms_are_read_and_used() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        git(
            p,
            &[
                "bisect",
                "start",
                "--term-new=broken",
                "--term-old=fixed",
                &h[7],
                &h[0],
            ],
        );
        let b = bisect(p);
        assert_eq!(
            (b.term_bad.as_str(), b.term_good.as_str()),
            ("broken", "fixed")
        );
        assert_eq!(b.bad.as_deref(), Some(h[7].as_str()));
        assert_eq!(b.good, vec![h[0].clone()]);

        for _ in 0..10 {
            if bisect(p).first_bad.is_some() {
                break;
            }
            mark(p, if has_bug(p) { "bad" } else { "good" }, None).unwrap();
        }
        assert_eq!(bisect(p).first_bad.as_deref(), Some(h[5].as_str()));
        let log = std::fs::read_to_string(
            CliEngine::new(p)
                .git_paths(&["BISECT_LOG"])
                .unwrap()
                .remove(0),
        )
        .unwrap();
        assert!(
            log.contains("git bisect broken ") || log.contains("git bisect fixed "),
            "{log}"
        );
        assert!(!log.contains("git bisect good "), "{log}");
    }

    /// Skip: the commit is listed as skipped and another is checked out; skipping
    /// everything ends with candidates, not with an error.
    #[test]
    fn skip_is_listed_and_skipping_everything_leaves_candidates() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        start(p, Some(&h[7]), &[h[0].clone()]).unwrap();
        let first = bisect(p).current.unwrap();
        mark(p, "skip", None).unwrap();
        let b = bisect(p);
        assert_eq!(b.skip, vec![first.clone()]);
        assert_ne!(
            b.current.as_deref(),
            Some(first.as_str()),
            "another commit to test"
        );

        for _ in 0..10 {
            if !bisect(p).candidates.is_empty() {
                break;
            }
            mark(p, "skip", None).unwrap();
        }
        let b = bisect(p);
        assert!(b.candidates.contains(&h[7]), "{:?}", b.candidates);
        assert!(b.candidates.len() >= 6, "{:?}", b.candidates);
        assert_eq!(b.first_bad, None);

        // Candidates left in the log from that answer must not turn a later
        // refusal into a silent success: a locked ref makes git refuse the mark
        // before it writes anything.
        let lock = CliEngine::new(p)
            .git_paths(&[&format!("refs/bisect/good-{}.lock", h[3])])
            .unwrap()
            .remove(0);
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, b"").unwrap();
        match mark(p, "good", Some(&h[3])) {
            Err(Error::Git { .. }) => {}
            other => panic!("git refused; that must reach the caller: {other:?}"),
        }
        assert!(!bisect(p).good.contains(&h[3]), "nothing was marked");
    }

    /// Marking a named commit, not the one checked out.
    #[test]
    fn a_named_commit_can_be_marked() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        start(p, None, &[]).unwrap();
        assert_eq!(
            bisect(p).bad.as_deref(),
            Some(h[7].as_str()),
            "bad defaults to HEAD"
        );
        mark(p, "good", Some(&h[4])).unwrap();
        let b = bisect(p);
        assert_eq!(b.good, vec![h[4].clone()]);
        assert_eq!(b.remaining, Some(1));
        match mark(p, "worse", None) {
            Err(Error::Rule(m)) => assert!(m.contains("worse"), "{m}"),
            other => panic!("expected a refusal: {other:?}"),
        }
    }

    /// Reset returns to the branch the bisect started from; `op_abort` is the
    /// same thing for a bisect.
    #[test]
    fn reset_returns_to_the_starting_branch() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        git(p, &["checkout", "-qb", "feature"]);
        start(p, Some(&h[7]), &[h[0].clone()]).unwrap();
        assert_eq!(
            git(p, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "HEAD",
            "detached"
        );
        assert_eq!(bisect(p).start_branch.as_deref(), Some("feature"));

        reset(p).unwrap();
        assert_eq!(git(p, &["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
        assert_eq!(detect_state(p).unwrap().kind, OperationKind::None);
        assert!(detect_state(p).unwrap().bisect.is_none());
        assert_eq!(
            git(p, &["for-each-ref", "refs/bisect/"]),
            "",
            "marks removed"
        );

        start(p, Some(&h[7]), &[h[0].clone()]).unwrap();
        ops::op_abort(p, None).unwrap();
        assert_eq!(git(p, &["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
        match reset(p) {
            Err(Error::Rule(m)) => assert!(m.contains("no bisect"), "{m}"),
            other => panic!("expected a refusal: {other:?}"),
        }
    }

    /// The drivers of the operation strip: continue has nothing to do and says so,
    /// skip is `git bisect skip`. An operation stopped inside the bisect is the
    /// `kind` — the one to finish first — and the search is still reported.
    #[test]
    fn drivers_and_an_operation_inside_the_bisect() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        start(p, Some(&h[7]), &[h[0].clone()]).unwrap();
        match ops::op_continue(p, None) {
            Err(Error::Rule(m)) => assert!(m.contains("nothing to continue"), "{m}"),
            other => panic!("expected a refusal: {other:?}"),
        }
        let tested = bisect(p).current.unwrap();
        ops::op_skip(p, None).unwrap();
        assert_eq!(bisect(p).skip, vec![tested]);

        let merge_head = CliEngine::new(p)
            .git_paths(&["MERGE_HEAD"])
            .unwrap()
            .remove(0);
        std::fs::write(&merge_head, format!("{}\n", h[3])).unwrap();
        let s = detect_state(p).unwrap();
        assert_eq!(s.kind, OperationKind::Merge);
        assert!(s.bisect.is_some(), "the search is not hidden by the merge");
        match mark(p, "good", None) {
            Err(Error::Rule(m)) => assert!(m.contains("inside the bisect"), "{m}"),
            other => panic!("marking under a stopped merge is refused: {other:?}"),
        }
        std::fs::remove_file(&merge_head).unwrap();
    }

    /// A bisect started on a detached HEAD keeps the commit to return to.
    #[test]
    fn a_detached_start_names_the_commit_to_return_to() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        git(p, &["checkout", "-q", "--detach", &h[6]]);
        start(p, None, &[h[0].clone()]).unwrap();
        let b = bisect(p);
        assert_eq!(b.start_branch, None);
        assert_eq!(b.start_commit.as_deref(), Some(h[6].as_str()));
    }

    /// A log that does not parse is an error from the strict reader, a visible
    /// `problem` in the state — never silence — and the bisect can still be ended.
    #[test]
    fn a_broken_log_is_an_error_not_silence() {
        let (dir, h) = repo_with_history();
        let p = dir.path();
        start(p, Some(&h[7]), &[h[0].clone()]).unwrap();
        let log = CliEngine::new(p)
            .git_paths(&["BISECT_LOG"])
            .unwrap()
            .remove(0);
        let mut text = std::fs::read_to_string(&log).unwrap();
        text.push_str("git bisect frobnicate deadbeef\n");
        std::fs::write(&log, text).unwrap();

        match read(p) {
            Err(Error::Parse(m)) => assert!(m.contains("frobnicate"), "{m}"),
            other => panic!("expected a parse error: {other:?}"),
        }
        let s = detect_state(p).unwrap();
        assert_eq!(s.kind, OperationKind::Bisect);
        let b = s.bisect.unwrap();
        assert!(
            b.problem.as_deref().unwrap_or("").contains("frobnicate"),
            "{b:?}"
        );
        assert!(
            b.good.is_empty() && b.bad.is_none(),
            "no marks from a log that lies"
        );
        match mark(p, "good", None) {
            Err(Error::Parse(_)) => {}
            other => panic!("marking under a broken log is refused: {other:?}"),
        }

        ops::op_abort(p, None).unwrap();
        assert_eq!(git(p, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
        assert!(detect_state(p).unwrap().bisect.is_none());
    }

    #[test]
    fn parse_log_is_strict_and_reads_the_answer() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let ok = format!(
            "# bad: [{a}] c8\n# good: [{b}] c1\ngit bisect start 'HEAD' 'HEAD~7'\n\
             # status: waiting\n# bad: [{b}] c6\ngit bisect bad {b}\n\
             # first bad commit: [{b}] c6 subject: with colon\n"
        );
        let log = parse_log(&ok, "bad", "good").unwrap();
        assert_eq!(log.marks.len(), 3);
        assert_eq!(
            log.first_bad,
            Some((b.clone(), "c6 subject: with colon".into()))
        );

        // a later answer reopens the search
        let reopened = format!("{ok}# good: [{a}] x\ngit bisect good {a}\n");
        assert_eq!(parse_log(&reopened, "bad", "good").unwrap().first_bad, None);

        for broken in [
            format!("{ok}git bisect good\n"),
            format!("{ok}git bisect good HEAD~1\n"),
            format!("{ok}git bisect visualize\n"),
            format!("{ok}rm -rf /\n"),
            format!("{ok}# bad: [nothex] x\n"),
            format!("{ok}# weird: [{a}] x\n"),
            "git bisect start HEAD\n".to_string(),
        ] {
            match parse_log(&broken, "bad", "good") {
                Err(Error::Parse(_)) => {}
                other => panic!("{broken:?} must not parse: {other:?}"),
            }
        }
        // the repository's own terms: `good` is not one of them
        let custom = format!("git bisect start '--term-new=new'\ngit bisect good {a}\n");
        assert!(parse_log(&custom, "new", "old").is_err());
        assert!(parse_log(&custom.replace("good", "old"), "new", "old").is_ok());
    }
}
