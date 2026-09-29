//! Blame of one file: which commit last changed each line (prd_02 R05b).
//!
//! `git blame --line-porcelain --no-textconv <hash> -- <path>` for a committed
//! version, the same without `<hash>` for the working tree. Every line comes with
//! the commit it is attributed to; the commits are listed once ([`BlameOrigin`]) and
//! the lines point at them by index. From a line, "blame before this change"
//! ([`before`]) opens the version its commit started from and says where the line
//! lands there.
//!
//! Decisions forced by git, each probed on git 2.54:
//!
//! * **No `--end-of-options` here.** `git blame --end-of-options <rev> -- <path>`
//!   fails with `bad revision '<path>'`: blame parses its revision arguments on its
//!   own. The revision is resolved first ([`crate::engine::file_history::resolve`],
//!   which does use `--end-of-options`) and blame is handed the full hash, which
//!   cannot be read as an option. `cat-file` and `diff` around it take the marker.
//! * **The path is not `:(literal)`.** Blame reads the path after `--` as a path
//!   already; `:(literal)x` would look for a file of that name (see
//!   [`crate::engine::cli::literal`]). The pathspecs of `ls-files` and of the
//!   working-tree `diff` below are ordinary pathspecs and go through `literal()`.
//! * **Paths in the porcelain are C-quoted.** Blame has no `-z`: `filename` and
//!   `previous` carry a path quoted the way `core.quotePath` quotes it — octal
//!   bytes for anything non-ASCII, `\"` and `\\` escaped. [`unquote`] undoes it; a
//!   Cyrillic name otherwise arrives as `"\320\266.txt"` and "blame before" asks
//!   for a file that does not exist.
//! * **`boundary` is not only a shallow edge.** Without `--root` git marks a root
//!   commit as a boundary too, and a shallow clone's grafted commit looks like a
//!   root either way — the two cannot be told apart from the output, so `boundary`
//!   is reported as "the earliest version reachable here".
//! * **The working tree is blamed by git, not by us.** Uncommitted lines come back
//!   under the all-zero hash (`Not Committed Yet`) with `previous` pointing at
//!   `HEAD`; a staged rename is followed. An untracked file is refused by git
//!   ("no such path in HEAD"), so it is recognised beforehand and answered with
//!   [`BlameBlock::Untracked`] rather than by matching that prose.
//!
//! Unfit files are an answer, not an error — [`Blame::blocked`], like
//! `TextFile::blocked`: binary, too large, missing, untracked. They are judged from
//! the blob (or the file) **before** blame runs, so a gigabyte is not blamed only to
//! be refused, and blame's own failure stays an honest `Error::Git`.

use std::collections::HashMap;
use std::path::Path;

use crate::engine::cli::{literal, CliEngine};
use crate::engine::exec;
use crate::engine::file_history::resolve;
use crate::engine::log::short;
use crate::engine::patch::{self, HunkText, Kind};
use crate::error::{Error, Result};
use crate::model::{Blame, BlameBefore, BlameBlock, BlameLine, BlameOrigin, BlamePrevious};

/// Bytes a blamed file may have. `--line-porcelain` repeats a dozen header lines
/// for every line of the file, so the output is many times the file; this keeps
/// one blame to tens of megabytes of git output at worst.
pub const BLAME_SIZE_CEILING: u64 = 4 * 1024 * 1024;

/// Lines a blamed file may have. The overlay virtualises its rows, but every line
/// crosses the Tauri boundary as an object, and blame's own cost grows with them.
pub const BLAME_LINE_CEILING: usize = 50_000;

/// The all-zero hash of a working-tree blame — SHA-1 or SHA-256 length.
fn is_uncommitted(hash: &str) -> bool {
    !hash.is_empty() && hash.bytes().all(|b| b == b'0')
}

fn is_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The commit `rev` names, or a domain error naming `rev`.
fn commit_of(repo: &Path, rev: &str, what: &str) -> Result<String> {
    resolve(repo, rev)?.ok_or_else(|| {
        Error::Rule(format!(
            "{what}: {rev:?} names no commit in this repository"
        ))
    })
}

/// Unfit content: a NUL byte, or more lines than the ceiling.
fn judge(bytes: &[u8]) -> Option<BlameBlock> {
    if bytes.contains(&0) {
        return Some(BlameBlock::Binary);
    }
    let mut lines = bytes.iter().filter(|&&b| b == b'\n').count();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        lines += 1;
    }
    (lines > BLAME_LINE_CEILING).then_some(BlameBlock::TooLarge)
}

/// Why the blob at `<hash>:<path>` cannot be blamed, if it cannot. The type first:
/// `cat-file -s` answers for a directory too (with the tree's size).
fn check_blob(repo: &Path, hash: &str, path: &str) -> Result<Option<BlameBlock>> {
    let spec = format!("{hash}:{path}");
    let kind = exec::git(repo, &["cat-file", "-t", "--end-of-options", &spec]).run()?;
    // The commit is resolved already, so a failure here is the path not being in it.
    if !kind.success() || kind.stdout_text().trim() != "blob" {
        return Ok(Some(BlameBlock::Missing));
    }
    let size = exec::git(repo, &["cat-file", "-s", "--end-of-options", &spec])
        .run()?
        .checked()?;
    let size: u64 = String::from_utf8_lossy(&size)
        .trim()
        .parse()
        .map_err(|_| Error::Parse(format!("cat-file -s {spec}: not a size")))?;
    if size > BLAME_SIZE_CEILING {
        return Ok(Some(BlameBlock::TooLarge));
    }
    let bytes = exec::git(repo, &["cat-file", "blob", "--end-of-options", &spec])
        .run()?
        .checked()?;
    Ok(judge(&bytes))
}

/// Why the working-tree file at `path` cannot be blamed, if it cannot.
///
/// The last component is not followed (`worktree_entry`): a tracked symlink is
/// blamed as git stores it, its target text, and its target is none of our business.
fn check_worktree(repo: &Path, path: &str) -> Result<Option<BlameBlock>> {
    let full = CliEngine::new(repo).worktree_entry(path)?;
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(BlameBlock::Missing)),
        Err(e) => return Err(Error::Io(format!("{path}: {e}"))),
    };
    if meta.is_dir() {
        return Ok(Some(BlameBlock::Missing));
    }
    let spec = literal(path);
    let known = exec::git(repo, &["ls-files", "-z", "--cached", "--", &spec])
        .run()?
        .checked()?;
    if known.is_empty() {
        return Ok(Some(BlameBlock::Untracked));
    }
    if meta.file_type().is_symlink() {
        return Ok(None);
    }
    if meta.len() > BLAME_SIZE_CEILING {
        return Ok(Some(BlameBlock::TooLarge));
    }
    let bytes = std::fs::read(&full).map_err(|e| Error::Io(format!("{path}: {e}")))?;
    Ok(judge(&bytes))
}

/// Undo git's C-quoting of a path: `"…"` with `\n \t \" \\` and octal bytes.
/// An unquoted value is returned as it is.
pub(crate) fn unquote(raw: &[u8]) -> String {
    let body = match raw {
        [b'"', inner @ .., b'"'] => inner,
        _ => return String::from_utf8_lossy(raw).into_owned(),
    };
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let b = body[i];
        i += 1;
        if b != b'\\' || i >= body.len() {
            out.push(b);
            continue;
        }
        let e = body[i];
        i += 1;
        match e {
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            b'0'..=b'7' => {
                let mut v = u32::from(e - b'0');
                for _ in 0..2 {
                    match body.get(i) {
                        Some(&d @ b'0'..=b'7') => {
                            v = v * 8 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push((v & 0xff) as u8);
            }
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Header fields of one line's group, as far as they were given.
#[derive(Default)]
struct Meta {
    author: String,
    mail: String,
    time: i64,
    summary: String,
    filename: Option<String>,
    previous: Option<BlamePrevious>,
    boundary: bool,
}

/// Parse `git blame --line-porcelain` output.
///
/// Each line of the file is a group: `<hash> <orig> <final>[ <count>]`, header
/// lines `key value`, then the line itself after a TAB. The text is everything
/// after that first TAB — a line that starts with a TAB keeps it. Unknown keys are
/// skipped (git adds them now and then). A group without its text line, a header
/// that does not parse, or final numbers that do not run 1, 2, 3… is an error: a
/// shorter blame would read as a shorter file.
///
/// Commits are listed once per `(hash, path)`: without copy detection a commit
/// names one path, but nothing in the format promises it.
pub(crate) fn parse(raw: &[u8]) -> Result<(Vec<BlameLine>, Vec<BlameOrigin>)> {
    let mut lines: Vec<BlameLine> = Vec::new();
    let mut origins: Vec<BlameOrigin> = Vec::new();
    let mut index: HashMap<(String, String), u32> = HashMap::new();

    let mut rows = raw.split(|&b| b == b'\n').peekable();
    while let Some(head) = rows.next() {
        if head.is_empty() && rows.peek().is_none() {
            break; // the tail after the last newline
        }
        let head = String::from_utf8_lossy(head);
        let f: Vec<&str> = head.split(' ').collect();
        let bad = || Error::Parse(format!("blame header {head:?} does not parse"));
        if !(3..=4).contains(&f.len()) || !is_hash(f[0]) {
            return Err(bad());
        }
        let hash = f[0].to_ascii_lowercase();
        let orig: u32 = f[1].parse().map_err(|_| bad())?;
        let fin: u32 = f[2].parse().map_err(|_| bad())?;
        if f.len() == 4 {
            f[3].parse::<u32>().map_err(|_| bad())?;
        }

        let mut meta = Meta::default();
        let mut text: Option<&[u8]> = None;
        for row in rows.by_ref() {
            if let Some(rest) = row.strip_prefix(b"\t") {
                text = Some(rest);
                break;
            }
            let (key, value) = match row.iter().position(|&b| b == b' ') {
                Some(i) => (&row[..i], &row[i + 1..]),
                None => (row, &b""[..]),
            };
            let lossy = || String::from_utf8_lossy(value).into_owned();
            match key {
                b"author" => meta.author = lossy(),
                b"author-mail" => {
                    meta.mail = lossy().trim_start_matches('<').trim_end_matches('>').into()
                }
                b"author-time" => meta.time = lossy().trim().parse().unwrap_or(0),
                b"summary" => meta.summary = lossy(),
                b"boundary" => meta.boundary = true,
                b"filename" => meta.filename = Some(unquote(value)),
                b"previous" => {
                    // `<hash> <path>`: the path may hold spaces, so split once.
                    let at = value.iter().position(|&b| b == b' ');
                    let (h, p) = match at {
                        Some(i) => (String::from_utf8_lossy(&value[..i]), &value[i + 1..]),
                        None => return Err(Error::Parse(format!("blame of {hash}: bad previous"))),
                    };
                    if !is_hash(&h) || p.is_empty() {
                        return Err(Error::Parse(format!("blame of {hash}: bad previous")));
                    }
                    meta.previous = Some(BlamePrevious {
                        hash: h.to_ascii_lowercase(),
                        path: unquote(p),
                    });
                }
                _ => {}
            }
        }
        let text =
            text.ok_or_else(|| Error::Parse(format!("blame of line {fin}: no text line")))?;
        if fin as usize != lines.len() + 1 {
            return Err(Error::Parse(format!(
                "blame line {fin} arrived where line {} was expected",
                lines.len() + 1
            )));
        }
        let path = meta
            .filename
            .take()
            .ok_or_else(|| Error::Parse(format!("blame of line {fin}: no filename")))?;
        let key = (hash.clone(), path.clone());
        let at = match index.get(&key) {
            Some(&i) => i,
            None => {
                let i = origins.len() as u32;
                origins.push(BlameOrigin {
                    short_hash: short(&hash),
                    uncommitted: is_uncommitted(&hash),
                    hash,
                    parents: Vec::new(),
                    author: meta.author,
                    author_email: meta.mail,
                    author_at: meta.time,
                    summary: meta.summary,
                    path,
                    previous: meta.previous,
                    boundary: meta.boundary,
                });
                index.insert(key, i);
                i
            }
        };
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        lines.push(BlameLine {
            line: fin,
            orig_line: orig,
            text: String::from_utf8_lossy(text).into_owned(),
            origin: at,
        });
    }
    Ok((lines, origins))
}

/// Fill `parents` of every committed origin with one `rev-list`.
fn attach_parents(repo: &Path, origins: &mut [BlameOrigin]) -> Result<()> {
    let mut input = String::new();
    for o in origins.iter().filter(|o| !o.uncommitted) {
        input.push_str(&o.hash);
        input.push('\n');
    }
    if input.is_empty() {
        return Ok(());
    }
    let out = exec::git(
        repo,
        &["rev-list", "--no-walk=unsorted", "--parents", "--stdin"],
    )
    .input(input.as_bytes())
    .run()?
    .checked()?;
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    for row in String::from_utf8_lossy(&out).lines() {
        let mut f = row.split_whitespace();
        if let Some(h) = f.next() {
            parents.insert(h.to_string(), f.map(str::to_string).collect());
        }
    }
    for o in origins.iter_mut().filter(|o| !o.uncommitted) {
        o.parents = parents.remove(&o.hash).unwrap_or_default();
    }
    Ok(())
}

/// Blame `path` at `rev` — any revision git resolves to a commit — or, with `None`,
/// the file in the working tree, uncommitted lines included.
pub fn file(repo: &Path, rev: Option<&str>, path: &str) -> Result<Blame> {
    if path.is_empty() {
        return Err(Error::Rule("blame needs a file path".into()));
    }
    let rev = rev.map(str::trim).filter(|r| !r.is_empty());
    let hash = rev.map(|r| commit_of(repo, r, "blame")).transpose()?;
    let answer = |blocked: Option<BlameBlock>| Blame {
        path: path.to_string(),
        rev: hash.clone(),
        lines: Vec::new(),
        origins: Vec::new(),
        blocked,
    };
    let blocked = match &hash {
        Some(h) => check_blob(repo, h, path)?,
        None => check_worktree(repo, path)?,
    };
    if blocked.is_some() {
        return Ok(answer(blocked));
    }

    let mut args = vec!["blame", "--line-porcelain", "--no-textconv"];
    if let Some(h) = &hash {
        args.push(h);
    }
    args.extend_from_slice(&["--", path]);
    let raw = exec::git(repo, &args).run()?.checked()?;
    let (lines, mut origins) = parse(&raw)?;
    // The working tree may have changed between the check and the blame.
    if lines.len() > BLAME_LINE_CEILING {
        return Ok(answer(Some(BlameBlock::TooLarge)));
    }
    if lines.iter().any(|l| l.text.contains('\0')) {
        return Ok(answer(Some(BlameBlock::Binary)));
    }
    attach_parents(repo, &mut origins)?;
    Ok(Blame {
        lines,
        origins,
        ..answer(None)
    })
}

/// Where one line of a new version lands in the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Landing {
    pub from: u32,
    pub to: u32,
    /// The line itself is there, only moved; otherwise `from..=to` is the block
    /// the change replaced, or the line it was inserted after.
    pub exact: bool,
}

/// Map line `new_line` of the new side of a one-file diff back to the old side.
///
/// Outside every hunk the line moved by the lines added and removed above it and
/// is there exactly. Inside a hunk a context line is there exactly too; an added
/// line was not there at all, and lands on the lines its block replaced — the
/// deletions since the last context line — or, for a pure insertion, on the old
/// line it was inserted after. The blamed line is almost always such an added
/// line: blame attributes it to the commit that introduced it.
///
/// Zero counts follow the unified-diff convention: `@@ -5,0 +6,2 @@` inserts after
/// old line 5, `@@ -3 +2,0 @@` deletes old line 3 after new line 2.
pub(crate) fn map_line_back(hunks: &[HunkText], new_line: u32) -> Landing {
    let exact = |n: i64| {
        let n = n.max(1) as u32;
        Landing {
            from: n,
            to: n,
            exact: true,
        }
    };
    // old minus new, for lines below the hunks walked so far
    let mut delta: i64 = 0;
    for h in hunks {
        // First line past the hunk on each side, and the first new line it holds.
        let old_start = if h.old_count == 0 {
            h.old_start + 1
        } else {
            h.old_start
        };
        let new_start = if h.new_count == 0 {
            h.new_start + 1
        } else {
            h.new_start
        };
        if new_line < new_start {
            return exact(i64::from(new_line) + delta);
        }
        if new_line < new_start + h.new_count {
            let (mut old_no, mut new_no) = (old_start, new_start);
            let mut block: Option<(u32, u32)> = None;
            for l in &h.lines {
                match l.kind {
                    Kind::Del => {
                        block = Some((block.map_or(old_no, |b| b.0), old_no));
                        old_no += 1;
                    }
                    Kind::Add => {
                        if new_no == new_line {
                            let (from, to) = block.unwrap_or_else(|| {
                                let at = old_no.saturating_sub(1).max(1);
                                (at, at)
                            });
                            return Landing {
                                from,
                                to,
                                exact: false,
                            };
                        }
                        new_no += 1;
                    }
                    Kind::Context => {
                        if new_no == new_line {
                            return exact(i64::from(old_no));
                        }
                        block = None;
                        old_no += 1;
                        new_no += 1;
                    }
                }
            }
        }
        delta = i64::from(old_start + h.old_count) - i64::from(new_start + h.new_count);
    }
    exact(i64::from(new_line) + delta)
}

/// "Blame before this change" for a line attributed to `hash` at `path`, line
/// `line` there (`BlameLine.orig_line`), whose origin names `prev_hash` /
/// `prev_path` as its `previous`.
///
/// Blames `prev_path` at `prev_hash` and maps the line back through the diff
/// between the two versions of the file — blob against blob, so a rename between
/// them is one pair and never an addition plus a deletion. An uncommitted line
/// (`hash` all zeros) is mapped through the diff of `prev_hash` against the working
/// tree. The landing is clamped to the length of the older version.
pub fn before(
    repo: &Path,
    hash: &str,
    path: &str,
    line: u32,
    prev_hash: &str,
    prev_path: &str,
) -> Result<BlameBefore> {
    if path.is_empty() || prev_path.is_empty() {
        return Err(Error::Rule(
            "blame before this change needs a file path".into(),
        ));
    }
    let prev = commit_of(repo, prev_hash, "blame before this change")?;
    let base = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "-U0",
    ];
    let raw = if is_uncommitted(hash) {
        let (old, new) = (literal(prev_path), literal(path));
        let mut a = base.to_vec();
        a.extend_from_slice(&["-M", "--end-of-options", &prev, "--", &old]);
        if prev_path != path {
            a.push(&new);
        }
        exec::git(repo, &a).run()?.checked()?
    } else {
        let at = commit_of(repo, hash, "blame before this change")?;
        let (old, new) = (format!("{prev}:{prev_path}"), format!("{at}:{path}"));
        let mut a = base.to_vec();
        a.extend_from_slice(&["--end-of-options", &old, &new]);
        exec::git(repo, &a).run()?.checked()?
    };
    // Two file sections (a working-tree rename git did not pair) would interleave
    // the hunks of two files; the line then stays where it is, marked approximate.
    let sections = raw
        .split(|&b| b == b'\n')
        .filter(|l| l.starts_with(b"diff --git "))
        .count();
    let mut landing = if sections <= 1 {
        map_line_back(&patch::hunks(&raw), line.max(1))
    } else {
        Landing {
            from: line.max(1),
            to: line.max(1),
            exact: false,
        }
    };

    let blame = file(repo, Some(&prev), prev_path)?;
    let n = blame.lines.len() as u32;
    if n > 0 {
        landing.from = landing.from.min(n);
        landing.to = landing.to.clamp(landing.from, n);
    }
    Ok(BlameBefore {
        blame,
        from: landing.from,
        to: landing.to,
        exact: landing.exact,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit(dir: &Path, file: &str, body: &str, msg: &str) {
        if let Some(parent) = Path::new(file).parent() {
            std::fs::create_dir_all(dir.join(parent)).unwrap();
        }
        std::fs::write(dir.join(file), body).unwrap();
        run(dir, &["add", "--", file]);
        run(dir, &["commit", "-q", "-m", msg]);
    }

    fn rev(dir: &Path, r: &str) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", r])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn origin<'a>(b: &'a Blame, line: u32) -> &'a BlameOrigin {
        let l = &b.lines[line as usize - 1];
        &b.origins[l.origin as usize]
    }

    fn texts(b: &Blame) -> Vec<&str> {
        b.lines.iter().map(|l| l.text.as_str()).collect()
    }

    fn summaries(b: &Blame) -> Vec<&str> {
        (1..=b.lines.len() as u32)
            .map(|n| origin(b, n).summary.as_str())
            .collect()
    }

    /// `x [1].txt`: created, edited, renamed to `y [1].txt`, edited again. A
    /// bracket and a space in both names: blame takes the path as a path, not as
    /// a pattern.
    fn renamed_repo() -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "x [1].txt", "a\nb\nc\n", "create");
        commit(p, "x [1].txt", "a\nB\nc\nd\n", "edit");
        run(p, &["mv", "x [1].txt", "y [1].txt"]);
        run(p, &["commit", "-q", "-m", "rename"]);
        commit(p, "y [1].txt", "a\nB\nc\nd\ne\n", "edit again");
        // the look-alike a glob would match: never blamed in its place
        commit(p, "x 1.txt", "noise\n", "noise");
        dir
    }

    #[test]
    fn a_file_is_blamed_across_its_rename_line_by_line() {
        let dir = renamed_repo();
        let p = dir.path();
        let b = file(p, Some("HEAD"), "y [1].txt").unwrap();
        assert_eq!(b.blocked, None);
        assert_eq!(
            b.rev.as_deref(),
            Some(rev(p, "HEAD").as_str()),
            "pinned to a hash"
        );
        assert_eq!(texts(&b), vec!["a", "B", "c", "d", "e"]);
        assert_eq!(
            summaries(&b),
            vec!["create", "edit", "create", "edit", "edit again"]
        );
        assert_eq!(
            b.lines.iter().map(|l| l.line).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );

        // commits are listed once, however many lines point to them
        assert_eq!(b.origins.len(), 3);
        let create = origin(&b, 1);
        assert_eq!(
            create.path, "x [1].txt",
            "the name the file had in that commit"
        );
        assert!(!create.boundary, "`create` sits on top of `init`");
        assert_eq!(create.previous, None, "the file was created there");
        assert_eq!(create.parents, vec![rev(p, "HEAD~5")]);
        assert_eq!(create.author, "Test");
        assert_eq!(create.author_email, "t@example.com");
        assert!(create.author_at > 0);
        assert_eq!(create.short_hash, create.hash[..7]);

        // `previous` crosses the rename: the edit's parent had the old name
        let edit = origin(&b, 2);
        assert!(!edit.boundary);
        let prev = edit
            .previous
            .clone()
            .expect("the edit has a version before it");
        assert_eq!(prev.path, "x [1].txt");
        assert_eq!(prev.hash, create.hash);
        assert_eq!(
            edit.parents,
            vec![create.hash.clone()],
            "parents are attached"
        );
        let again = origin(&b, 5);
        assert_eq!(again.path, "y [1].txt");
        assert_eq!(again.previous.as_ref().unwrap().path, "y [1].txt");
        assert_eq!(again.previous.as_ref().unwrap().hash, rev(p, "HEAD~2"));
        // line 4 was line 4 in its commit too; orig_line is the number there
        assert_eq!(b.lines[3].orig_line, 4);

        // the root commit is a boundary: nothing before it
        let root = file(p, Some("HEAD"), "a.txt").unwrap();
        let init = origin(&root, 1);
        assert_eq!(init.summary, "init");
        assert!(init.boundary);
        assert_eq!((init.previous.clone(), init.parents.len()), (None, 0));
    }

    #[test]
    fn blame_before_steps_through_the_rename_and_lands_on_the_replaced_line() {
        let dir = renamed_repo();
        let p = dir.path();
        let b = file(p, Some("HEAD"), "y [1].txt").unwrap();
        // line 2 "B" replaced "b" in `edit`
        let l = &b.lines[1];
        let o = origin(&b, 2);
        let prev = o.previous.clone().unwrap();
        let back = before(p, &o.hash, &o.path, l.orig_line, &prev.hash, &prev.path).unwrap();
        assert_eq!(back.blame.path, "x [1].txt");
        assert_eq!(texts(&back.blame), vec!["a", "b", "c"]);
        assert_eq!(
            (back.from, back.to, back.exact),
            (2, 2, false),
            "the line it replaced"
        );

        // line 5 "e" was appended in `edit again`, after the rename: the version
        // before is `y [1].txt` at `rename`, and the landing is the line it followed
        let l = &b.lines[4];
        let o = origin(&b, 5);
        let prev = o.previous.clone().unwrap();
        let back = before(p, &o.hash, &o.path, l.orig_line, &prev.hash, &prev.path).unwrap();
        assert_eq!(back.blame.path, "y [1].txt");
        assert_eq!(texts(&back.blame), vec!["a", "B", "c", "d"]);
        assert_eq!((back.from, back.to, back.exact), (4, 4, false));

        // a revision that names nothing is refused with its name
        match before(p, &o.hash, &o.path, 1, "no-such-rev", "y [1].txt") {
            Err(Error::Rule(m)) => assert!(m.contains("no-such-rev"), "{m}"),
            other => panic!("expected a rule error, got {:?}", other.map(|x| x.from)),
        }
    }

    #[test]
    fn a_quoted_path_in_the_porcelain_is_unquoted() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "ж \"q\".txt", "one\ntwo\n", "create");
        run(p, &["mv", "ж \"q\".txt", "дом.txt"]);
        std::fs::write(p.join("дом.txt"), "one\nTWO\n").unwrap();
        run(p, &["commit", "-q", "-am", "rename and edit"]);
        let b = file(p, Some("HEAD"), "дом.txt").unwrap();
        assert_eq!(origin(&b, 1).path, "ж \"q\".txt");
        let o = origin(&b, 2);
        assert_eq!(o.path, "дом.txt");
        let prev = o.previous.clone().unwrap();
        assert_eq!(prev.path, "ж \"q\".txt");
        // and the unquoted name is one git finds
        let back = before(p, &o.hash, &o.path, 2, &prev.hash, &prev.path).unwrap();
        assert_eq!(texts(&back.blame), vec!["one", "two"]);

        assert_eq!(unquote(br#""\320\266 \"q\".txt""#), "ж \"q\".txt");
        assert_eq!(unquote(br#""a\\b\tc""#), "a\\b\tc");
        assert_eq!(unquote(b"plain name.txt"), "plain name.txt");
    }

    #[test]
    fn a_leading_dash_and_a_leading_tab_are_just_text() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "-n.txt", "\tindented\n-x\n\n", "dash");
        let b = file(p, Some("HEAD"), "-n.txt").unwrap();
        assert_eq!(texts(&b), vec!["\tindented", "-x", ""]);
    }

    #[test]
    fn an_unfit_file_is_an_answer_not_an_error() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("bin.dat"), b"ab\0cd\n").unwrap();
        std::fs::create_dir_all(p.join("dir")).unwrap();
        std::fs::write(p.join("dir/f.txt"), "x\n").unwrap();
        std::fs::write(
            p.join("big.txt"),
            vec![b'x'; BLAME_SIZE_CEILING as usize + 1],
        )
        .unwrap();
        std::fs::write(p.join("long.txt"), "x\n".repeat(BLAME_LINE_CEILING + 1)).unwrap();
        std::fs::write(p.join("empty.txt"), "").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-q", "-m", "files"]);

        let at = |path: &str| file(p, Some("HEAD"), path).unwrap();
        assert_eq!(at("bin.dat").blocked, Some(BlameBlock::Binary));
        assert_eq!(at("big.txt").blocked, Some(BlameBlock::TooLarge));
        assert_eq!(at("long.txt").blocked, Some(BlameBlock::TooLarge));
        assert_eq!(at("nope.txt").blocked, Some(BlameBlock::Missing));
        assert_eq!(
            at("dir").blocked,
            Some(BlameBlock::Missing),
            "a directory is no file"
        );
        let empty = at("empty.txt");
        assert_eq!((empty.blocked, empty.lines.len()), (None, 0));
        assert!(at("bin.dat").lines.is_empty());

        // the working tree is judged the same way, from disk
        let wt = |path: &str| file(p, None, path).unwrap();
        assert_eq!(wt("bin.dat").blocked, Some(BlameBlock::Binary));
        assert_eq!(wt("big.txt").blocked, Some(BlameBlock::TooLarge));
        assert_eq!(wt("gone.txt").blocked, Some(BlameBlock::Missing));
        std::fs::write(p.join("fresh.txt"), "new\n").unwrap();
        assert_eq!(wt("fresh.txt").blocked, Some(BlameBlock::Untracked));

        // one line under the ceiling is blamed
        std::fs::write(p.join("long.txt"), "x\n".repeat(BLAME_LINE_CEILING)).unwrap();
        assert_eq!(wt("long.txt").lines.len(), BLAME_LINE_CEILING);

        match file(p, Some("no-such-branch"), "a.txt") {
            Err(Error::Rule(m)) => assert!(m.contains("no-such-branch"), "{m}"),
            other => panic!(
                "expected a rule error, got {:?}",
                other.map(|b| b.lines.len())
            ),
        }
        assert!(matches!(file(p, None, ""), Err(Error::Rule(_))));
        assert!(matches!(
            file(p, None, "../outside.txt"),
            Err(Error::Rule(_))
        ));
    }

    #[test]
    fn the_working_tree_marks_uncommitted_lines_and_steps_back_into_head() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "f.txt", "one\ntwo\nthree\n", "base");
        std::fs::write(p.join("f.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        let b = file(p, None, "f.txt").unwrap();
        assert_eq!(b.rev, None);
        assert_eq!(texts(&b), vec!["one", "TWO", "three", "four"]);
        let flags: Vec<bool> = (1..=4).map(|n| origin(&b, n).uncommitted).collect();
        assert_eq!(flags, vec![false, true, false, true]);
        let o = origin(&b, 2);
        assert!(o.parents.is_empty());
        let prev = o
            .previous
            .clone()
            .expect("an uncommitted line steps back into HEAD");
        assert_eq!(prev.hash, rev(p, "HEAD"));
        let back = before(
            p,
            &o.hash,
            &o.path,
            b.lines[1].orig_line,
            &prev.hash,
            &prev.path,
        )
        .unwrap();
        assert_eq!(texts(&back.blame), vec!["one", "two", "three"]);
        assert_eq!((back.from, back.to, back.exact), (2, 2, false));
        // appended past the end: lands on the last line, clamped to the file
        let back = before(p, &o.hash, &o.path, 4, &prev.hash, &prev.path).unwrap();
        assert_eq!((back.from, back.to), (3, 3));

        // a staged rename is followed by git
        run(p, &["mv", "f.txt", "g.txt"]);
        let b = file(p, None, "g.txt").unwrap();
        assert_eq!(origin(&b, 1).path, "f.txt");
    }

    /// In the working tree `ls-files` and the diff of "blame before" take a
    /// pathspec: an untracked `x[ab].txt` next to a tracked `xa.txt` is untracked,
    /// not a file the glob found tracked (which blame would then fail on).
    #[test]
    fn a_bracketed_working_tree_path_is_taken_literally() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "xa.txt", "tracked\n", "look-alike");
        std::fs::write(p.join("x[ab].txt"), "untracked\n").unwrap();
        assert_eq!(
            file(p, None, "x[ab].txt").unwrap().blocked,
            Some(BlameBlock::Untracked)
        );

        // tracked, edited: the uncommitted line steps back through a diff of this
        // file only, not of `xa.txt` too
        commit(p, "x[ab].txt", "one\ntwo\n", "bracketed");
        std::fs::write(p.join("xa.txt"), "noise\nnoise\nnoise\n").unwrap();
        std::fs::write(p.join("x[ab].txt"), "one\nTWO\n").unwrap();
        let b = file(p, None, "x[ab].txt").unwrap();
        let o = origin(&b, 2);
        assert!(o.uncommitted);
        let prev = o.previous.clone().unwrap();
        let back = before(p, &o.hash, &o.path, 2, &prev.hash, &prev.path).unwrap();
        assert_eq!(texts(&back.blame), vec!["one", "two"]);
        assert_eq!((back.from, back.to, back.exact), (2, 2, false));
    }

    #[test]
    fn a_blame_that_does_not_parse_is_an_error() {
        let h = "a".repeat(40);
        let group = |fin: u32| {
            format!(
                "{h} {fin} {fin} 1\nauthor A\nauthor-mail <a@e>\nauthor-time 1700000000\nsummary s\nfilename f.txt\n\tline {fin}\n"
            )
        };
        let good = format!("{}{}", group(1), group(2));
        let (lines, origins) = parse(good.as_bytes()).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(origins.len(), 1, "one commit, one entry");
        assert_eq!(origins[0].author_email, "a@e");
        assert_eq!(lines[1].text, "line 2");

        // the text line missing
        let cut = format!("{}{h} 2 2\nfilename f.txt\n", group(1));
        assert!(matches!(parse(cut.as_bytes()), Err(Error::Parse(_))));
        // a line skipped
        let gap = format!("{}{}", group(1), group(3));
        assert!(matches!(parse(gap.as_bytes()), Err(Error::Parse(_))));
        // a header that is no header
        assert!(matches!(
            parse(b"nonsense 1 1\n\tx\n"),
            Err(Error::Parse(_))
        ));
        // no filename
        let bare = format!("{h} 1 1 1\nsummary s\n\tx\n");
        assert!(matches!(parse(bare.as_bytes()), Err(Error::Parse(_))));
        // empty output is an empty file
        assert_eq!(parse(b"").unwrap().0.len(), 0);
        // an unknown key is skipped, a CR is not part of the line
        let extra = format!("{h} 1 1 1\nfuture-key v\nfilename f.txt\n\tx\r\n");
        assert_eq!(parse(extra.as_bytes()).unwrap().0[0].text, "x");
    }

    // ---- map_line_back ----

    fn hunks_of(diff: &str) -> Vec<HunkText> {
        patch::hunks(diff.as_bytes())
    }

    fn land(diff: &str, line: u32) -> (u32, u32, bool) {
        let l = map_line_back(&hunks_of(diff), line);
        (l.from, l.to, l.exact)
    }

    #[test]
    fn a_line_maps_back_through_the_hunks_of_its_commit() {
        // old a b c d e f / new a X Y d e f g — lines 2..3 replace b c, g appended
        let replace = "@@ -2,2 +2,2 @@\n-b\n-c\n+X\n+Y\n@@ -6,0 +7 @@\n+g\n";
        assert_eq!(land(replace, 1), (1, 1, true), "above every hunk");
        assert_eq!(
            land(replace, 2),
            (2, 3, false),
            "a replaced run lands on what it replaced"
        );
        assert_eq!(land(replace, 3), (2, 3, false));
        assert_eq!(
            land(replace, 4),
            (4, 4, true),
            "between hunks, shifted by nothing"
        );
        assert_eq!(
            land(replace, 7),
            (6, 6, false),
            "appended: the line it follows"
        );

        // pure insertion after old line 5
        let insert = "@@ -5,0 +6,2 @@\n+n1\n+n2\n";
        assert_eq!(land(insert, 6), (5, 5, false));
        assert_eq!(land(insert, 7), (5, 5, false));
        assert_eq!(
            land(insert, 8),
            (6, 6, true),
            "below: shifted by the two added"
        );
        assert_eq!(land(insert, 5), (5, 5, true));

        // insertion at the very top of an empty file clamps to 1
        assert_eq!(land("@@ -0,0 +1 @@\n+first\n", 1), (1, 1, false));
        assert_eq!(land("@@ -0,0 +1,2 @@\n+a\n+b\n", 2), (1, 1, false));

        // a pure deletion: the lines below move up
        let delete = "@@ -3,2 +2,0 @@\n-c\n-d\n";
        assert_eq!(land(delete, 2), (2, 2, true));
        assert_eq!(land(delete, 3), (5, 5, true), "net delta of two");

        // a hunk with context: a context line is there exactly
        let ctx = "@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n d\n";
        assert_eq!(land(ctx, 1), (1, 1, true));
        assert_eq!(land(ctx, 2), (2, 2, false));
        assert_eq!(land(ctx, 3), (3, 3, true));
        assert_eq!(land(ctx, 9), (9, 9, true));

        // no hunks at all (a pure rename): every line is where it was
        assert_eq!(land("", 4), (4, 4, true));
    }
}
