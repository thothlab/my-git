//! History of one file: every commit that touched it, renames followed (R05c).
//!
//! `git log --follow --name-status -z` from one pinned commit, one path. Each row
//! carries the path the file had **in that commit** (and the old name on the
//! commit that renamed it), so the per-commit diff — [`crate::engine::commit::file_diff`]
//! — is asked for under the right name. The row fields are the log's
//! ([`crate::engine::log`]), without the graph: a history narrowed to one file has
//! holes, and an edge drawn between two of its rows would be an invention.
//!
//! Three decisions, each forced by how `--follow` works:
//!
//! * **No `--skip`.** Under `--follow` the path limit is applied at output, not
//!   during the walk: `--skip=N` counts walked commits that are never shown, and a
//!   skipped commit is not diffed — so the rename switch it would have made is
//!   lost, and every row older than the rename silently disappears (probed on git
//!   2.54: `--skip=1..4` returned the same two rows, `--skip=6` nothing although
//!   two commits were left). `--max-count` counts shown rows, so a page is read as
//!   the first `skip + limit + 1` rows and cut here. Resuming from the boundary
//!   commit's parents instead would lose the other lines of a non-linear walk.
//! * **The history is pinned, so it cannot move.** The first page resolves `rev`
//!   to a commit hash and every later page walks from that hash. Commits are
//!   immutable, so the walk — and with it the offset — stays the same whatever
//!   happens to the branch meanwhile; that is the answer to the log's "stale
//!   cursor" question here: the cursor can go stale only if the pinned commit
//!   itself is gone (garbage-collected after a rewrite), and that is refused as a
//!   domain error with "reload". The list is a snapshot of the history at the
//!   moment it was opened.
//! * **Merges are not listed**, exactly as `git log --follow` without `-m` does:
//!   the log does not diff a merge, so the output-side path limit never sees it.
//!   Every change a merge brings in is listed on the side commit that made it;
//!   what is lost is a change made *in* the merge itself (a conflict resolution),
//!   same as in git. The log's Paths filter (plain `-- <path>`, history
//!   simplification) does list such merges — the two answer different questions
//!   and are not meant to agree.
//!
//! `--follow` takes exactly one pathspec; `:(literal)` is accepted and survives the
//! rename switch (the old name is followed literally too), so the path goes
//! through `literal()` like every other pathspec in the engine.

use std::path::Path;

use crate::engine::cli::{fnv1a, literal, parse_refs};
use crate::engine::commit::parse_name_status;
use crate::engine::exec;
use crate::engine::log::{remotes, short};
use crate::error::{Error, Result};
use crate::model::{FileHistoryCommit, FileHistoryCursor, FileHistoryPage};

/// First character of `FileHistoryCursor.anchor`: `@<pinned hash>:<path fingerprint>`.
const ANCHOR: char = '@';

/// Fields of one record. The separator **leads**: `--name-status` output comes
/// after the format, so a trailing `%x01` would hand one commit's status to the
/// next record. The subject is the last field; after its NUL comes the status.
const FORMAT: &str = "--format=%x01%H%x00%P%x00%an%x00%ae%x00%at%x00%D%x00%s";
const FIELDS: usize = 7;

/// The commit `rev` names, or `None` when it names nothing here (an unborn `HEAD`
/// included). A repository git cannot read explains itself on stderr — that is an
/// error, not "no such revision" (same rule as `log::commit_by_hash`).
pub(crate) fn resolve(repo: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    let out = exec::git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .run()?;
    if !out.success() {
        if String::from_utf8_lossy(&out.stderr).trim().is_empty() {
            return Ok(None);
        }
        return Err(out.fail_stderr());
    }
    let hash = out.stdout_text().trim().to_string();
    Ok((!hash.is_empty()).then_some(hash))
}

fn anchor(hash: &str, path: &str) -> String {
    format!("{ANCHOR}{hash}:{}", fnv1a(path.as_bytes()))
}

/// The pinned commit a cursor names, once it is checked against the path asked
/// for and against the repository as it is now.
fn pinned_of(repo: &Path, path: &str, cursor: &FileHistoryCursor) -> Result<String> {
    let body = cursor.anchor.strip_prefix(ANCHOR).unwrap_or_default();
    let (hash, fp) = body.split_once(':').unwrap_or_default();
    if hash.is_empty() || fp != fnv1a(path.as_bytes()) {
        return Err(Error::Rule(
            "file history cursor belongs to another file — reload the history from the first page"
                .into(),
        ));
    }
    match resolve(repo, hash)? {
        Some(full) if full == hash => Ok(full),
        _ => Err(Error::Rule(
            "the commit this file history was read from is gone — reload the history from the first page".into(),
        )),
    }
}

/// Parse `log --follow --name-status -z` output in [`FORMAT`]. A record that is
/// short of fields, or does not name exactly one file, is an error — a shorter
/// list would read as "the file was not touched there".
fn parse(out: &str) -> Result<Vec<(String, FileHistoryCommit)>> {
    let mut rows = Vec::new();
    for rec in out.split('\u{1}') {
        if rec.trim_matches(|c| c == '\0' || c == '\n').is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.splitn(FIELDS + 1, '\0').collect();
        if f.len() < FIELDS + 1 {
            return Err(Error::Parse(format!(
                "file history record has {} fields, expected {}",
                f.len(),
                FIELDS + 1
            )));
        }
        let hash = f[0].trim().to_string();
        let entries = parse_name_status(f[7].trim_start_matches('\n'))?;
        let [entry] = <[_; 1]>::try_from(entries).map_err(|e: Vec<_>| {
            Error::Parse(format!(
                "file history record {hash} names {} files, expected exactly one",
                e.len()
            ))
        })?;
        rows.push((
            f[5].to_string(),
            FileHistoryCommit {
                short_hash: short(&hash),
                hash,
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                author: f[2].to_string(),
                author_email: f[3].to_string(),
                author_at: f[4].trim().parse().unwrap_or(0),
                subject: f[6].to_string(),
                refs: Vec::new(),
                path: entry.path,
                old_path: entry.old_path,
                status: entry.status,
            },
        ));
    }
    Ok(rows)
}

/// One page of the history of `path`, newest first.
///
/// `rev` is where the history starts on the first page (`None` — `HEAD`); later
/// pages take it from the cursor. A path no commit touched is an empty page, not an
/// error — an untracked or freshly added file simply has no history yet. So is an
/// unborn `HEAD`. A `rev` that names nothing is a domain error.
pub fn page(
    repo: &Path,
    path: &str,
    rev: Option<&str>,
    cursor: Option<&FileHistoryCursor>,
    limit: u32,
) -> Result<FileHistoryPage> {
    let empty = || FileHistoryPage {
        commits: Vec::new(),
        next_cursor: None,
    };
    if path.is_empty() {
        return Err(Error::Rule("file history needs a file path".into()));
    }
    let (pinned, skip) = match cursor {
        Some(c) => (pinned_of(repo, path, c)?, c.skip),
        None => {
            let rev = rev.map(str::trim).filter(|r| !r.is_empty());
            match resolve(repo, rev.unwrap_or("HEAD"))? {
                Some(hash) => (hash, 0),
                None if rev.is_none() => return Ok(empty()),
                None => {
                    return Err(Error::Rule(format!(
                        "file history: {:?} names no commit in this repository",
                        rev.unwrap_or_default()
                    )))
                }
            }
        }
    };
    if limit == 0 {
        return Ok(empty());
    }

    // One row past the page: the only honest way to say whether there is more.
    let want = skip.saturating_add(limit).saturating_add(1);
    let spec = literal(path);
    let max_count = format!("--max-count={want}");
    let args = [
        "log",
        "--follow",
        // Asked for explicitly, like `commit::files`: with `diff.renames=false` the
        // rename commit would otherwise list as an addition and hide the old name.
        "-M",
        "--name-status",
        "-z",
        FORMAT,
        &max_count,
        "--end-of-options",
        &pinned,
        "--",
        &spec,
    ];
    let out = exec::git(repo, &args).run()?.checked()?;
    let rows = parse(&String::from_utf8_lossy(&out))?;

    let total = rows.len();
    let end = (skip as usize).saturating_add(limit as usize);
    let remotes = remotes(repo)?;
    let commits = rows
        .into_iter()
        .skip(skip as usize)
        .take(limit as usize)
        .map(|(decor, mut c)| {
            c.refs = parse_refs(&decor, &remotes);
            c
        })
        .collect::<Vec<_>>();
    let next_cursor = (total > end).then(|| FileHistoryCursor {
        skip: skip + commits.len() as u32,
        anchor: anchor(&pinned, path),
    });
    Ok(FileHistoryPage {
        commits,
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::engine::commit::file_diff;
    use crate::model::{FileState, LogFilter};
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
        run(dir, &["commit", "-m", msg]);
    }

    fn head(dir: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn all(p: &Path, path: &str, rev: Option<&str>) -> Vec<FileHistoryCommit> {
        page(p, path, rev, None, 100).unwrap().commits
    }

    fn subjects(rows: &[FileHistoryCommit]) -> Vec<&str> {
        rows.iter().map(|c| c.subject.as_str()).collect()
    }

    const BODY: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

    /// old.txt: created, edited, renamed (with an edit) to new.txt, edited again.
    fn renamed_repo() -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "old.txt", BODY, "create");
        commit(p, "old.txt", &BODY.replace("two", "TWO"), "edit old");
        run(p, &["mv", "old.txt", "new.txt"]);
        std::fs::write(
            p.join("new.txt"),
            BODY.replace("two", "TWO").replace("eight", "EIGHT"),
        )
        .unwrap();
        run(p, &["commit", "-am", "rename"]);
        commit(
            p,
            "new.txt",
            &BODY
                .replace("two", "TWO")
                .replace("eight", "EIGHT")
                .replace("five", "FIVE"),
            "edit new",
        );
        dir
    }

    #[test]
    fn a_rename_is_followed_and_every_row_names_the_file_as_it_was_there() {
        let dir = renamed_repo();
        let p = dir.path();
        let rows = all(p, "new.txt", None);
        assert_eq!(
            subjects(&rows),
            vec!["edit new", "rename", "edit old", "create"]
        );

        let at = |s: &str| rows.iter().find(|c| c.subject == s).unwrap();
        assert_eq!(
            (at("edit new").path.as_str(), at("edit new").status),
            ("new.txt", FileState::Modified)
        );
        assert_eq!(
            at("rename").path,
            "new.txt",
            "the rename commit names the new path"
        );
        assert_eq!(at("rename").old_path.as_deref(), Some("old.txt"));
        assert_eq!(at("rename").status, FileState::Renamed);
        assert_eq!(
            at("edit old").path,
            "old.txt",
            "older rows carry the old name"
        );
        assert_eq!(at("edit old").old_path, None);
        assert_eq!(
            (at("create").path.as_str(), at("create").status),
            ("old.txt", FileState::Added)
        );
        assert!(
            at("create").parents.len() == 1,
            "`create` sits on top of `init`"
        );
        assert_eq!(at("edit new").short_hash, at("edit new").hash[..7]);
        assert!(
            at("edit new").refs.iter().any(|r| r.name == "main"),
            "labels reach the row"
        );

        // the rename commit's diff is the edit, not the whole file as new …
        let c = at("rename");
        let d = file_diff(p, &c.hash, &c.path, c.old_path.as_deref(), "none", None).unwrap();
        let origins: String = d
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.origin.as_str()))
            .collect();
        assert!(
            origins.contains('-') && origins.contains(' '),
            "a rename with an edit, not an addition: {origins:?}"
        );
        assert_eq!(
            origins.matches('+').count(),
            1,
            "one line changed on the rename: {origins:?}"
        );

        // … and an older row diffs under the name the file had then
        let c = at("edit old");
        let d = file_diff(p, &c.hash, &c.path, None, "none", None).unwrap();
        assert!(
            !d.hunks.is_empty(),
            "the old path has its diff in that commit"
        );
    }

    #[test]
    fn a_history_started_at_an_older_commit_uses_the_name_there() {
        let dir = renamed_repo();
        let p = dir.path();
        let edit_old = all(p, "new.txt", None)
            .into_iter()
            .find(|c| c.subject == "edit old")
            .unwrap();
        let rows = all(p, "old.txt", Some(&edit_old.hash));
        assert_eq!(subjects(&rows), vec!["edit old", "create"]);
        // from HEAD the old name ends in the rename commit, seen from its side as
        // a deletion: `--follow` pairs a rename only towards the name it follows
        let rows = all(p, "old.txt", None);
        assert_eq!(subjects(&rows), vec!["rename", "edit old", "create"]);
        assert_eq!(rows[0].status, FileState::Deleted);
    }

    /// `x[ab].txt` is a name: `xa.txt`, touched alone before and after the rename,
    /// never appears — not under the new name and not after `--follow` switched to
    /// the old one.
    #[test]
    fn a_bracketed_name_is_followed_literally_across_the_rename() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "x[ab].txt", BODY, "create");
        commit(p, "xa.txt", "noise\n", "noise before");
        commit(p, "ya.txt", "noise\n", "noise y before");
        commit(p, "x[ab].txt", &BODY.replace("two", "TWO"), "edit");
        run(p, &["mv", "x[ab].txt", "y[ab].txt"]);
        run(p, &["commit", "-m", "rename"]);
        commit(p, "xa.txt", "noise 2\n", "noise after");
        commit(p, "ya.txt", "noise 2\n", "noise y after");
        let rows = all(p, "y[ab].txt", None);
        assert_eq!(subjects(&rows), vec!["rename", "edit", "create"]);
        assert_eq!(rows[2].path, "x[ab].txt");
    }

    #[test]
    fn a_file_no_commit_touched_has_an_empty_history() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("fresh.txt"), "new\n").unwrap();
        let pg = page(p, "fresh.txt", None, None, 50).unwrap();
        assert!(pg.commits.is_empty());
        assert!(pg.next_cursor.is_none());

        // an unborn HEAD is the same answer, not an error
        let empty = tempfile::tempdir().unwrap();
        run(empty.path(), &["init", "-b", "main"]);
        assert!(page(empty.path(), "a.txt", None, None, 50)
            .unwrap()
            .commits
            .is_empty());

        // a revision that names nothing is refused, with the reason
        match page(p, "a.txt", Some("no-such-branch"), None, 50) {
            Err(Error::Rule(m)) => assert!(m.contains("no-such-branch"), "{m}"),
            other => panic!(
                "expected a rule error, got {:?}",
                other.map(|x| x.commits.len())
            ),
        }
        assert!(matches!(page(p, "", None, None, 50), Err(Error::Rule(_))));
    }

    #[test]
    fn pages_join_into_the_whole_history_across_the_rename_and_stay_pinned() {
        let dir = renamed_repo();
        let p = dir.path();
        // noise on top: under `--follow` these are walked, never shown
        for n in 0..3 {
            commit(p, &format!("noise{n}.txt"), "n\n", &format!("noise {n}"));
        }
        let whole = all(p, "new.txt", None);
        assert_eq!(whole.len(), 4);

        // the first page ends right before the rename commit
        let first = page(p, "new.txt", None, None, 1).unwrap();
        assert_eq!(subjects(&first.commits), vec!["edit new"]);
        let c1 = first.next_cursor.clone().expect("more to come");
        assert!(c1.anchor.starts_with('@'));
        // a commit arriving meanwhile does not shift the pinned history
        commit(p, "new.txt", "moved on\n", "arrived later");
        let second = page(p, "new.txt", None, Some(&c1), 2).unwrap();
        assert_eq!(subjects(&second.commits), vec!["rename", "edit old"]);
        let c2 = second.next_cursor.clone().expect("one row is left");
        let third = page(p, "new.txt", None, Some(&c2), 2).unwrap();
        assert_eq!(subjects(&third.commits), vec!["create"]);
        assert!(
            third.next_cursor.is_none(),
            "the exact end promises nothing more"
        );

        let joined: Vec<String> = [first.commits, second.commits, third.commits]
            .concat()
            .into_iter()
            .map(|c| c.hash)
            .collect();
        assert_eq!(
            joined,
            whole.iter().map(|c| c.hash.clone()).collect::<Vec<_>>()
        );

        // a page that is exactly full promises nothing when nothing follows
        let exact = page(p, "old.txt", Some(&whole[2].hash), None, 2).unwrap();
        assert_eq!(exact.commits.len(), 2);
        assert!(exact.next_cursor.is_none());

        // the cursor of one file is refused for another
        match page(p, "a.txt", None, Some(&c1), 2) {
            Err(Error::Rule(m)) => assert!(m.contains("reload"), "{m}"),
            other => panic!(
                "expected a rule error, got {:?}",
                other.map(|x| x.commits.len())
            ),
        }
        // and a cursor naming a commit that is not there is refused too
        let gone = FileHistoryCursor {
            skip: 1,
            anchor: anchor(&"0".repeat(40), "new.txt"),
        };
        assert!(matches!(
            page(p, "new.txt", None, Some(&gone), 2),
            Err(Error::Rule(_))
        ));
    }

    /// main and side both edit f.txt; both are merged, one merge also changes the
    /// file itself. `--follow` without `-m` lists the side commits and no merge —
    /// the log's Paths filter lists the merges; the two are meant to differ.
    #[test]
    fn merges_are_not_listed_the_side_commits_are() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "f.txt", BODY, "base");
        run(p, &["checkout", "-q", "-b", "side"]);
        commit(p, "f.txt", &BODY.replace("one", "ONE"), "side edit");
        run(p, &["checkout", "-q", "main"]);
        commit(p, "f.txt", &BODY.replace("eight", "EIGHT"), "main edit");
        run(p, &["merge", "-q", "--no-ff", "-m", "clean merge", "side"]);
        run(p, &["checkout", "-q", "-b", "s2", "HEAD~1"]);
        commit(
            p,
            "f.txt",
            &BODY.replace("eight", "EIGHT").replace("five", "FIVE"),
            "s2 edit",
        );
        run(p, &["checkout", "-q", "main"]);
        run(p, &["merge", "-q", "--no-ff", "--no-commit", "s2"]);
        std::fs::write(
            p.join("f.txt"),
            BODY.replace("one", "ONE")
                .replace("eight", "EIGHT")
                .replace("five", "5!"),
        )
        .unwrap();
        run(p, &["commit", "-qam", "evil merge"]);

        let rows = all(p, "f.txt", None);
        let mut got = subjects(&rows);
        got.sort();
        assert_eq!(got, vec!["base", "main edit", "s2 edit", "side edit"]);
        assert!(rows.iter().all(|c| c.parents.len() == 1), "no merge row");

        let plain = crate::engine::log::page(
            p,
            &LogFilter {
                paths: vec!["f.txt".into()],
                ..Default::default()
            },
            None,
            50,
        )
        .unwrap();
        let plain: Vec<&str> = plain.commits.iter().map(|c| c.subject.as_str()).collect();
        assert!(
            plain.contains(&"evil merge") && plain.contains(&"clean merge"),
            "{plain:?}"
        );
    }

    #[test]
    fn a_record_that_does_not_parse_is_an_error() {
        let good =
            "\u{1}h\u{0}p\u{0}Ann\u{0}a@e\u{0}1700000000\u{0}\u{0}subject\u{0}\nM\u{0}f.txt\u{0}";
        assert_eq!(parse(good).unwrap().len(), 1);
        // a field short
        let short = "\u{1}h\u{0}p\u{0}Ann\u{0}1700000000\u{0}\u{0}subject\u{0}\nM\u{0}f.txt\u{0}";
        assert!(matches!(
            parse(&format!("{good}{short}")),
            Err(Error::Parse(_))
        ));
        // no file named — not "the file was not touched here"
        let none = "\u{1}h\u{0}p\u{0}Ann\u{0}a@e\u{0}1700000000\u{0}\u{0}subject\u{0}";
        assert!(matches!(parse(none), Err(Error::Parse(_))));
        // a status cut off before its path
        let cut =
            "\u{1}h\u{0}p\u{0}Ann\u{0}a@e\u{0}1700000000\u{0}\u{0}subject\u{0}\nR100\u{0}a\u{0}";
        assert!(matches!(parse(cut), Err(Error::Parse(_))));
    }

    #[test]
    fn a_head_that_moved_under_the_first_page_is_not_an_issue_for_rev() {
        let dir = renamed_repo();
        let p = dir.path();
        let at = head(p);
        commit(p, "new.txt", "later\n", "later");
        // an explicit revision is read from where it points, not from HEAD
        let rows = all(p, "new.txt", Some(&at));
        assert_eq!(rows[0].subject, "edit new");
    }
}
