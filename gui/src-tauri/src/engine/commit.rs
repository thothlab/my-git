//! One commit and comparison of two revisions.
//!
//! Hides `git show`, `git diff <a>..<b>` and `git branch --contains`.
//!
//! Everything here compares a commit **against its first parent**: that is what the
//! panel promises, and for a merge it is the only reading that stays a two-sided
//! diff. The answer carries `parents`, so the UI can say so out loud when there is
//! more than one. A root commit has no parent at all and is read with
//! `diff-tree --root`, which renders its whole tree as additions.

use std::path::Path;
use std::process::Command;

use crate::engine::cli::{parse_diff, parse_refs, whitespace_args};
use crate::error::{Error, Result};
use crate::model::{CommitDetails, CommitFileEntry, FileDiff, FileState};

/// How many containing branches travel to the UI. The card shows six and hides the
/// rest behind a control (prd_02 История 62); a commit on a busy repo can be
/// contained in hundreds, and shipping all of them buys nothing.
const BRANCH_LIMIT: usize = 64;

/// Run `git -C <repo> <args>`, keeping stderr verbatim on failure.
fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
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

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(repo, args)?).to_string())
}

/// The revision a commit is compared against: its first parent, or `None` when the
/// commit is a root commit.
fn first_parent(repo: &Path, hash: &str) -> Result<Option<String>> {
    Ok(parents_of(repo, hash)?.into_iter().next())
}

/// Parent hashes of a commit, oldest-listed first (`%P`).
fn parents_of(repo: &Path, hash: &str) -> Result<Vec<String>> {
    let out = git_text(repo, &["log", "-1", "--format=%P", hash, "--"])?;
    Ok(out.split_whitespace().map(str::to_string).collect())
}

/// Split `git … -z` output into records, dropping the trailing empty one.
fn nul_fields(raw: &str) -> Vec<&str> {
    raw.split('\0').filter(|s| !s.is_empty()).collect()
}

/// Statuses as `--name-status` spells them, with the similarity score stripped:
/// `A`, `D`, `M`, `T`, `R100`, `C75`, `U`.
///
/// An unknown letter is an error, not a `Modified` guess: git saying something this
/// code does not understand must reach the user, not be rendered as a plausible
/// wrong status.
fn file_state(code: &str) -> Result<FileState> {
    match code.chars().next() {
        Some('A') => Ok(FileState::Added),
        Some('D') => Ok(FileState::Deleted),
        Some('R') => Ok(FileState::Renamed),
        // A copy leaves the source in place, so for the tree it is simply a new
        // file. The source path is deliberately dropped: the model has no "copied"
        // state, and an entry that is `Added` yet carries an old path reads as a
        // move everywhere the old path is what tells them apart.
        Some('C') => Ok(FileState::Added),
        Some('U') => Ok(FileState::Conflicted),
        Some('M') | Some('T') => Ok(FileState::Modified),
        _ => Err(Error::Parse(format!(
            "commit: unknown --name-status code {code:?}"
        ))),
    }
}

/// Turn `--name-status -z` output into entries. Rename and copy records carry two
/// paths, everything else one.
///
/// A record that runs out of fields is an error rather than a stopping point: a
/// truncated stream would otherwise produce a shorter file list indistinguishable
/// from a complete one — the commit would silently appear to have touched fewer
/// files than it did.
fn parse_name_status(raw: &str) -> Result<Vec<CommitFileEntry>> {
    let toks = nul_fields(raw);
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let code = toks[i];
        let status = file_state(code)?;
        let two_paths = matches!(code.chars().next(), Some('R') | Some('C'));
        let width = if two_paths { 3 } else { 2 };
        if i + width > toks.len() {
            return Err(Error::Parse(format!(
                "commit: truncated --name-status record {code:?} ({} fields left)",
                toks.len() - i
            )));
        }
        let (path, old_path) = if two_paths {
            let old = (status == FileState::Renamed).then(|| toks[i + 1].to_string());
            (toks[i + 2].to_string(), old)
        } else {
            (toks[i + 1].to_string(), None)
        };
        out.push(CommitFileEntry {
            status,
            path,
            old_path,
        });
        i += width;
    }
    Ok(out)
}

/// Branches containing the commit, capped at [`BRANCH_LIMIT`].
///
/// Asked for full ref names on purpose. `git branch --contains` prints a pseudo-row
/// for a detached HEAD (`(HEAD detached at abc123)`) alongside the real branches,
/// and it is told apart by *not being a ref* — a branch may legitimately be named
/// with a leading parenthesis, so the first character decides nothing.
fn containing_branches(repo: &Path, hash: &str) -> Result<(Vec<String>, bool)> {
    let raw = git_text(
        repo,
        &["branch", "-a", "--contains", hash, "--format=%(refname)"],
    )?;
    let mut all: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter_map(|l| {
            l.strip_prefix("refs/heads/")
                .or_else(|| l.strip_prefix("refs/remotes/"))
                .map(str::to_string)
        })
        .collect();
    let truncated = all.len() > BRANCH_LIMIT;
    all.truncate(BRANCH_LIMIT);
    Ok((all, truncated))
}

/// Full commit card: author, committer, body, refs, containing branches.
pub fn details(repo: &Path, hash: &str) -> Result<CommitDetails> {
    // Body goes last: it is the only field allowed to contain anything, newlines
    // included, so nothing after it needs finding.
    const FMT: &str =
        "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%D%x00%s%x00%b";
    let raw = git_text(repo, &["log", "-1", FMT, hash, "--"])?;
    let f: Vec<&str> = raw.split('\0').collect();
    if f.len() < 11 {
        return Err(Error::Parse(format!(
            "commit: unexpected git log output for {hash} ({} fields)",
            f.len()
        )));
    }
    let remotes: Vec<String> = git_text(repo, &["remote"])?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let (branches, branches_truncated) = containing_branches(repo, hash)?;
    Ok(CommitDetails {
        hash: f[0].to_string(),
        parents: f[1].split_whitespace().map(str::to_string).collect(),
        author: f[2].to_string(),
        author_email: f[3].to_string(),
        author_at: f[4].trim().parse().unwrap_or(0),
        committer: f[5].to_string(),
        committer_email: f[6].to_string(),
        committer_at: f[7].trim().parse().unwrap_or(0),
        subject: f[9].to_string(),
        // git appends a newline after the body; the trailing one is formatting,
        // not content.
        body: f[10].trim_end_matches('\n').to_string(),
        refs: parse_refs(f[8], &remotes),
        branches,
        branches_truncated,
    })
}

/// Files touched by a commit.
pub fn files(repo: &Path, hash: &str) -> Result<Vec<CommitFileEntry>> {
    let raw = match first_parent(repo, hash)? {
        Some(base) => git_text(
            repo,
            &["diff", "--name-status", "-M", "-z", &base, hash, "--"],
        )?,
        // No parent: `git diff` has nothing to stand on, and `git show` prints
        // nothing for a root commit either. `diff-tree --root` renders the whole
        // tree as additions, which is what a root commit did.
        None => git_text(
            repo,
            &[
                "diff-tree",
                "-r",
                "-M",
                "-z",
                "--root",
                "--no-commit-id",
                "--name-status",
                hash,
                "--",
            ],
        )?,
    };
    parse_name_status(&raw)
}

/// Byte size of a blob at `<rev>:<path>`, or `None` when the file is absent there.
fn blob_size(repo: &Path, rev: &str, path: &str) -> Option<u64> {
    let spec = format!("{rev}:{path}");
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-s", &spec])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Byte size of the file as it lies on disk, for a comparison against the working tree.
fn worktree_size(repo: &Path, path: &str) -> Option<u64> {
    std::fs::metadata(repo.join(path)).ok().map(|m| m.len())
}

/// The path a file had **before** the change, when the change was a rename.
///
/// Rename detection has to be asked for identically on both calls of a pair: with
/// `-M` on the listing and without it on the diff, the panel says "renamed" on the
/// left and shows the whole file as an addition on the right — two contradictory
/// statements about one commit. git only recognises the rename when both paths are
/// in the pathspec, and the old path is already known from the listing.
fn rename_source(entries: &[CommitFileEntry], path: &str) -> Option<String> {
    entries
        .iter()
        .find(|e| e.path == path && e.status == FileState::Renamed)
        .and_then(|e| e.old_path.clone())
}

/// Diff of one file inside a commit, against the commit's first parent. `ws` is the
/// whitespace mode, same vocabulary as
/// [`crate::engine::cli::CliEngine::diff_file`]; an unknown value is rejected by
/// [`whitespace_args`], never folded into `none`.
///
/// The raw patch is parsed by the one diff parser in the project, so a commit diff
/// and a working-tree diff reach the panel in exactly the same shape.
pub fn file_diff(repo: &Path, hash: &str, path: &str, ws: &str) -> Result<FileDiff> {
    let wsa = whitespace_args(ws)?;
    let parents = parents_of(repo, hash)?;
    let mut d = match parents.first() {
        Some(base) => {
            let old = rename_source(&files(repo, hash)?, path);
            let mut a = vec!["diff", "-M"];
            a.extend_from_slice(&wsa);
            a.extend_from_slice(&[base.as_str(), hash, "--"]);
            if let Some(o) = old.as_deref() {
                a.push(o);
            }
            a.push(path);
            let mut d = parse_diff(path, &git_text(repo, &a)?);
            if d.binary {
                d.old_size = blob_size(repo, base, old.as_deref().unwrap_or(path));
                d.new_size = blob_size(repo, hash, path);
            }
            d
        }
        // Root commit: nothing to diff against, so read the tree itself — every
        // line of the file is an addition.
        None => {
            let mut a = vec!["diff-tree", "-p", "-r", "-M", "--root", "--no-commit-id"];
            a.extend_from_slice(&wsa);
            a.extend_from_slice(&[hash, "--", path]);
            let mut d = parse_diff(path, &git_text(repo, &a)?);
            if d.binary {
                d.new_size = blob_size(repo, hash, path);
            }
            d
        }
    };
    d.merge_first_parent = parents.len() > 1;
    Ok(d)
}

/// Files differing between two revisions.
///
/// An empty `to` means the working tree (История 77, «сравнить с рабочим деревом»):
/// `git diff <rev>` with one revision is exactly that comparison, so the same pair
/// of functions serves both cases and the UI only decides what to label the sides.
pub fn compare(repo: &Path, from: &str, to: &str) -> Result<Vec<CommitFileEntry>> {
    let mut args = vec!["diff", "--name-status", "-M", "-z", from];
    if !to.is_empty() {
        args.push(to);
    }
    args.push("--");
    parse_name_status(&git_text(repo, &args)?)
}

/// Diff of one file between two revisions. An empty `to` means the working tree,
/// exactly as in [`compare`].
pub fn compare_diff(repo: &Path, from: &str, to: &str, path: &str, ws: &str) -> Result<FileDiff> {
    let wsa = whitespace_args(ws)?;
    let old = rename_source(&compare(repo, from, to)?, path);
    let mut a = vec!["diff", "-M"];
    a.extend_from_slice(&wsa);
    a.push(from);
    if !to.is_empty() {
        a.push(to);
    }
    a.push("--");
    if let Some(o) = old.as_deref() {
        a.push(o);
    }
    a.push(path);
    let mut d = parse_diff(path, &git_text(repo, &a)?);
    if d.binary {
        d.old_size = blob_size(repo, from, old.as_deref().unwrap_or(path));
        d.new_size = if to.is_empty() {
            worktree_size(repo, path)
        } else {
            blob_size(repo, to, path)
        };
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::model::RefKind;
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
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn head(dir: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("spawn git");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn details_carry_author_committer_body_and_branches() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "two\n").unwrap();
        run(p, &["add", "a.txt"]);
        // author differs from the committer, and the message has a body
        run(
            p,
            &[
                "-c",
                "user.name=Committer",
                "-c",
                "user.email=c@example.com",
                "commit",
                "--author",
                "Author <a@example.com>",
                "-m",
                "subject line",
                "-m",
                "body first\nbody second",
            ],
        );
        run(p, &["branch", "later"]);
        let h = head(p);

        let d = details(p, &h).expect("details");
        assert_eq!(d.hash, h);
        assert_eq!(d.parents.len(), 1);
        assert_eq!(d.author, "Author");
        assert_eq!(d.author_email, "a@example.com");
        assert_eq!(d.committer, "Committer");
        assert_eq!(d.committer_email, "c@example.com");
        assert_eq!(d.subject, "subject line");
        assert_eq!(d.body, "body first\nbody second");
        assert!(d.author_at > 0 && d.committer_at > 0);
        let mut b = d.branches.clone();
        b.sort();
        assert_eq!(b, vec!["later".to_string(), "main".to_string()]);
        assert!(!d.branches_truncated);
        assert!(
            d.refs.iter().any(|r| r.name == "main"),
            "refs must carry the decorating branch: {:?}",
            d.refs
        );
    }

    /// A repo with a merge, a rename, a deletion and an empty commit.
    ///
    /// history: root(a.txt, d/b.txt) → main adds c.txt → merge of side (a.txt grew)
    fn merge_repo(p: &Path) -> (String, String) {
        run(p, &["checkout", "-b", "side"]);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        run(p, &["commit", "-am", "side"]);
        run(p, &["checkout", "main"]);
        std::fs::write(p.join("c.txt"), "c\n").unwrap();
        run(p, &["add", "c.txt"]);
        run(p, &["commit", "-m", "on main"]);
        let main_tip = head(p);
        run(p, &["merge", "--no-ff", "-m", "merged", "side"]);
        (main_tip, head(p))
    }

    #[test]
    fn files_of_a_plain_commit_and_of_a_deletion() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "b\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-m", "add b"]);
        let added = head(p);
        run(p, &["rm", "-q", "b.txt"]);
        run(p, &["commit", "-m", "drop b"]);
        let removed = head(p);

        let f = files(p, &added).expect("files");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "b.txt");
        assert_eq!(f[0].status, FileState::Added);
        assert!(f[0].old_path.is_none());

        let f = files(p, &removed).expect("files");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].status, FileState::Deleted);
    }

    #[test]
    fn rename_reports_both_paths() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["mv", "a.txt", "renamed.txt"]);
        run(p, &["commit", "-m", "rename"]);
        let f = files(p, &head(p)).expect("files");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].status, FileState::Renamed);
        assert_eq!(f[0].path, "renamed.txt");
        assert_eq!(f[0].old_path.as_deref(), Some("a.txt"));
    }

    #[test]
    fn merge_commit_is_read_against_its_first_parent() {
        let dir = scratch_repo();
        let p = dir.path();
        let (main_tip, merge) = merge_repo(p);

        let f = files(p, &merge).expect("files");
        // Against the first parent (main) only a.txt moved; against the second
        // (side) it would have been c.txt.
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].path, "a.txt");

        // and the answer says it is a merge, so the UI can label the comparison
        let d = details(p, &merge).expect("details");
        assert_eq!(d.parents.len(), 2);
        assert_eq!(d.parents[0], main_tip);
    }

    #[test]
    fn root_commit_lists_its_whole_tree_as_additions() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir(p.join("d")).unwrap();
        std::fs::write(p.join("d/b.txt"), "b\n").unwrap();
        run(p, &["add", "d/b.txt"]);
        run(p, &["commit", "-m", "second"]);
        let root = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(p)
                .args(["rev-list", "--max-parents=0", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        let f = files(p, &root).expect("root files");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "a.txt");
        assert_eq!(f[0].status, FileState::Added);
    }

    #[test]
    fn empty_commit_lists_nothing_and_is_not_an_error() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["commit", "--allow-empty", "-m", "nothing"]);
        let f = files(p, &head(p)).expect("empty commit must not fail");
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn compare_two_revisions_and_a_revision_with_the_working_tree() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = head(p);
        std::fs::write(p.join("c.txt"), "c\n").unwrap();
        run(p, &["add", "c.txt"]);
        run(p, &["commit", "-m", "add c"]);
        let tip = head(p);

        let f = compare(p, &base, &tip).expect("compare");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "c.txt");
        assert_eq!(f[0].status, FileState::Added);

        // the working tree adds one more change on top of the tip
        std::fs::write(p.join("a.txt"), "one\nchanged\n").unwrap();
        let f = compare(p, &base, "").expect("compare with the working tree");
        let mut paths: Vec<&str> = f.iter().map(|e| e.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a.txt", "c.txt"]);
    }

    /// Origins of every diff line of a file, in order: "+++", "---", " + " …
    fn origins(d: &FileDiff) -> String {
        d.hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .map(|l| l.origin.as_str())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn added_file_is_all_additions_and_deleted_file_all_deletions() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "one\ntwo\n").unwrap();
        run(p, &["add", "b.txt"]);
        run(p, &["commit", "-m", "add b"]);
        let added = head(p);
        run(p, &["rm", "-q", "b.txt"]);
        run(p, &["commit", "-m", "drop b"]);
        let removed = head(p);

        let d = file_diff(p, &added, "b.txt", "none").expect("added diff");
        assert!(!d.binary);
        assert_eq!(origins(&d), "++");
        assert_eq!(d.hunks[0].lines[1].content, "two");

        let d = file_diff(p, &removed, "b.txt", "none").expect("deleted diff");
        assert_eq!(origins(&d), "--");
    }

    #[test]
    fn merge_file_diff_is_against_the_first_parent() {
        let dir = scratch_repo();
        let p = dir.path();
        let (_main_tip, merge) = merge_repo(p);
        // a.txt is "one" on the first parent and "one\ntwo" on the merge, so the
        // first-parent diff adds exactly one line. Against the second parent the
        // file is identical and the diff would have been empty.
        let d = file_diff(p, &merge, "a.txt", "none").expect("merge diff");
        assert_eq!(origins(&d), " +");
        assert_eq!(d.hunks[0].lines[1].content, "two");
    }

    #[test]
    fn root_commit_file_diff_is_the_whole_file_as_additions() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "one\nmore\n").unwrap();
        run(p, &["commit", "-am", "second"]);
        let root = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(p)
                .args(["rev-list", "--max-parents=0", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();

        let d = file_diff(p, &root, "a.txt", "none").expect("root diff");
        assert_eq!(origins(&d), "+");
        assert_eq!(d.hunks[0].lines[0].content, "one");
    }

    #[test]
    fn binary_file_is_flagged_and_carries_no_hunks() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        run(p, &["add", "bin.dat"]);
        run(p, &["commit", "-m", "bin"]);
        let d = file_diff(p, &head(p), "bin.dat", "none").expect("binary diff");
        assert!(d.binary, "binary file must be flagged");
        assert!(d.hunks.is_empty());
    }

    #[test]
    fn whitespace_modes_change_what_the_commit_diff_shows() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("ws.txt"), "alpha\nbeta\n").unwrap();
        std::fs::write(p.join("eol.txt"), "gamma\n").unwrap();
        run(p, &["add", "ws.txt", "eol.txt"]);
        run(p, &["commit", "-m", "base"]);
        // indentation only
        std::fs::write(p.join("ws.txt"), "    alpha\nbeta\n").unwrap();
        // trailing whitespace only
        std::fs::write(p.join("eol.txt"), "gamma   \n").unwrap();
        run(p, &["commit", "-am", "reindent"]);
        let h = head(p);

        assert_eq!(origins(&file_diff(p, &h, "ws.txt", "none").unwrap()), "-+ ");
        assert!(
            file_diff(p, &h, "ws.txt", "all").unwrap().hunks.is_empty(),
            "ignoring all whitespace must leave no difference"
        );
        // leading indentation is not at the end of the line
        assert_eq!(
            origins(&file_diff(p, &h, "ws.txt", "trailing").unwrap()),
            "-+ "
        );

        assert_eq!(origins(&file_diff(p, &h, "eol.txt", "none").unwrap()), "-+");
        assert!(
            file_diff(p, &h, "eol.txt", "trailing")
                .unwrap()
                .hunks
                .is_empty(),
            "ignoring trailing whitespace must leave no difference"
        );

        let e = file_diff(p, &h, "ws.txt", "sideways").unwrap_err();
        assert!(
            format!("{e:?}").contains("whitespace"),
            "unknown mode must be rejected, got {e:?}"
        );
    }

    #[test]
    fn compare_diff_between_revisions_and_against_the_working_tree() {
        let dir = scratch_repo();
        let p = dir.path();
        let base = head(p);
        std::fs::write(p.join("a.txt"), "one\ntwo\n").unwrap();
        run(p, &["commit", "-am", "grow"]);
        let tip = head(p);

        let d = compare_diff(p, &base, &tip, "a.txt", "none").expect("compare_diff");
        assert_eq!(origins(&d), " +");

        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let d = compare_diff(p, &base, "", "a.txt", "none").expect("against the working tree");
        assert_eq!(origins(&d), " ++");
        assert_eq!(
            compare_diff(p, &base, "", "a.txt", "nope").is_err(),
            true,
            "unknown whitespace mode must be rejected here too"
        );
    }

    #[test]
    fn renamed_file_diffs_as_a_rename_not_as_a_whole_new_file() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        run(p, &["commit", "-am", "grow"]);
        let base = head(p);
        run(p, &["mv", "a.txt", "renamed.txt"]);
        std::fs::write(p.join("renamed.txt"), "one\ntwo\nfour\n").unwrap();
        run(p, &["commit", "-am", "rename and edit"]);
        let h = head(p);

        // the listing calls it a rename …
        let f = files(p, &h).expect("files");
        assert_eq!(f[0].status, FileState::Renamed);
        // … and the diff must agree: two kept lines and one replaced, not a whole
        // file of additions.
        let d = file_diff(p, &h, "renamed.txt", "none").expect("rename diff");
        assert_eq!(origins(&d), "  -+");

        // the same for a comparison of two revisions
        let d = compare_diff(p, &base, &h, "renamed.txt", "none").expect("compare rename");
        assert_eq!(origins(&d), "  -+");
    }

    #[test]
    fn merge_diff_says_it_is_against_the_first_parent() {
        let dir = scratch_repo();
        let p = dir.path();
        let (_main_tip, merge) = merge_repo(p);
        let d = file_diff(p, &merge, "a.txt", "none").expect("merge diff");
        assert!(
            d.merge_first_parent,
            "a merge diff must carry the fact that it is against the first parent"
        );
        // an ordinary commit on top of the merge must not claim it
        std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        run(p, &["commit", "-am", "plain"]);
        let plain = file_diff(p, &head(p), "a.txt", "none").expect("plain diff");
        assert!(!plain.merge_first_parent);
    }

    #[test]
    fn binary_file_carries_both_sizes() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
        run(p, &["add", "bin.dat"]);
        run(p, &["commit", "-m", "bin one"]);
        let added = head(p);
        std::fs::write(p.join("bin.dat"), [0u8, 1, 2, 3, 4, 5, 6]).unwrap();
        run(p, &["commit", "-am", "bin two"]);
        let grown = head(p);

        let d = file_diff(p, &added, "bin.dat", "none").expect("added binary");
        assert!(d.binary);
        assert_eq!(d.old_size, None, "the file did not exist before");
        assert_eq!(d.new_size, Some(4));

        let d = file_diff(p, &grown, "bin.dat", "none").expect("grown binary");
        assert_eq!((d.old_size, d.new_size), (Some(4), Some(7)));

        // against the working tree the new side is the file on disk
        std::fs::write(p.join("bin.dat"), [0u8; 9]).unwrap();
        let d = compare_diff(p, &added, "", "bin.dat", "none").expect("binary vs worktree");
        assert_eq!((d.old_size, d.new_size), (Some(4), Some(9)));

        // a text diff claims no sizes at all
        let d = file_diff(p, &head(p), "a.txt", "none").unwrap_or_else(|_| unreachable!());
        assert_eq!((d.old_size, d.new_size), (None, None));
    }

    #[test]
    fn details_carry_parsed_ref_labels() {
        // The classification itself is `engine::cli::parse_refs`' own test. What is
        // this module's to get wrong: handing the parser the `%D` field at all, and
        // handing it the repo's remote list — without the latter `origin/main` comes
        // back as a local branch.
        let dir = scratch_repo();
        let p = dir.path();
        let remote = tempfile::tempdir().unwrap();
        run(remote.path(), &["init", "--bare", "-b", "main"]);
        run(
            p,
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        run(p, &["push", "-q", "origin", "main"]);
        run(p, &["tag", "v1"]);

        let d = details(p, &head(p)).expect("details");
        let kind = |name: &str| {
            d.refs
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("no ref {name} in {:?}", d.refs))
                .kind
        };
        assert_eq!(kind("main"), RefKind::Head, "HEAD -> main");
        assert_eq!(kind("v1"), RefKind::Tag);
        assert_eq!(
            kind("origin/main"),
            RefKind::Remote,
            "remote list not passed"
        );
    }

    #[test]
    fn copy_does_not_pretend_to_be_a_move() {
        // `C` records a copy: the source file stays. Reported as an addition, and
        // without an old path — an addition carrying one reads as a rename.
        let e = parse_name_status("C75\0src.txt\0copy.txt\0").expect("copy record");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].status, FileState::Added);
        assert_eq!(e[0].path, "copy.txt");
        assert_eq!(e[0].old_path, None);
    }

    #[test]
    fn a_truncated_or_unknown_record_is_an_error_not_a_shorter_list() {
        // a rename record cut after the old path
        let e = parse_name_status("M\0a.txt\0R100\0old.txt\0");
        assert!(e.is_err(), "truncated record must not be swallowed: {e:?}");
        assert!(format!("{:?}", e.unwrap_err()).starts_with("Parse"));
        // a status letter this code does not know
        let e = parse_name_status("M\0a.txt\0X\0b.txt\0");
        assert!(e.is_err(), "unknown status must not be guessed: {e:?}");
    }

    #[test]
    fn detached_head_row_is_dropped_but_a_parenthesised_branch_survives() {
        let dir = scratch_repo();
        let p = dir.path();
        let h = head(p);
        run(p, &["branch", "(weird)"]);
        run(p, &["checkout", "-q", "--detach"]);

        let d = details(p, &h).expect("details on a detached HEAD");
        let mut b = d.branches.clone();
        b.sort();
        assert_eq!(b, vec!["(weird)".to_string(), "main".to_string()]);
    }
}
