//! Conflicted files: what each side holds, and the three ways out of a conflict —
//! a resolved text, a whole side, or the deletion one side asked for (R05e).
//!
//! The markers themselves are **not** parsed here. The editor re-parses the result
//! on every keystroke, so the parser lives on the client, in the import-free
//! `components/conflicts/conflictRules.ts` under the harness; this module reads what
//! the index and the working tree hold and writes the verdict back.
//!
//! Seams worth naming:
//!
//! * **The index is the source of the sides**, one `ls-files -u` per question: stage
//!   1 is the base, 2 is ours, 3 is theirs, and a missing stage is a side on which
//!   the file does not exist. The conflict's kind is read off the same set of
//!   stages ([`kind_of`]), which is exactly how git picks the two letters
//!   `git status` prints — so the kind needs no second command and cannot disagree
//!   with the sides.
//! * **Blobs are read by object id**, never as `:<n>:<path>`: that is revision
//!   syntax, where `literal()` has no place, and `git show` would run textconv.
//! * **"Resolved" is `git add`** — the index entry collapses to stage 0. `add -A`
//!   because the resolution may be the absence of the file: a resolved text that
//!   is gone from the tree is staged as the deletion it is.
//! * **The resolution text goes through `write_text_file`**, freshness and all: a
//!   conflict file edited in a terminal while the editor is open is `Error::Stale`,
//!   never silently overwritten.
//! * **`check-attr` takes pathnames, not pathspecs** — like `git blame` and
//!   `diff --no-index`, it is the other exception to `literal()`: wrapped, it would
//!   look up the attributes of a file literally named `:(literal)…`.

use std::path::Path;

use crate::engine::cli::{literal, CliEngine, EDIT_SIZE_CEILING};
use crate::engine::exec;
use crate::error::{Error, Result};
use crate::model::{ConflictEntry, ConflictFile, ConflictKind, ConflictSide, EditBlock, Eol};

/// git's marker length when `conflict-marker-size` is not set.
pub const DEFAULT_MARKER_SIZE: u32 = 7;

/// `git -C <repo> <args>`, stdout on success, `Error::Git` with stderr otherwise.
fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    exec::git(repo, args).run()?.checked()
}

/// One unmerged index entry of a path.
#[derive(Debug, Clone)]
struct Stage {
    mode: String,
    oid: String,
    stage: u8,
    path: String,
}

/// `ls-files -u -z`: `<mode> SP <oid> SP <stage> TAB <path> NUL`. A record that does
/// not read that way is an error, not a skipped line — a shorter list would pass for
/// "that file is no longer conflicted".
fn parse_unmerged(raw: &[u8]) -> Result<Vec<Stage>> {
    let mut out = Vec::new();
    for rec in raw.split(|&b| b == 0) {
        if rec.is_empty() {
            continue;
        }
        let rec = String::from_utf8_lossy(rec);
        let bad = || Error::Parse(format!("unexpected ls-files -u record: {rec:?}"));
        let (head, path) = rec.split_once('\t').ok_or_else(bad)?;
        let mut f = head.split(' ');
        let (Some(mode), Some(oid), Some(stage), None) = (f.next(), f.next(), f.next(), f.next())
        else {
            return Err(bad());
        };
        let stage: u8 = stage.parse().map_err(|_| bad())?;
        if !(1..=3).contains(&stage) || path.is_empty() {
            return Err(bad());
        }
        out.push(Stage {
            mode: mode.to_string(),
            oid: oid.to_string(),
            stage,
            path: path.to_string(),
        });
    }
    Ok(out)
}

/// The kind of a conflict from the set of stages present — git's own table
/// (`wt-status.c`): 1 DD, 2 AU, 3 UD, 4 UA, 5 DU, 6 AA, 7 UU, bit 0 base, bit 1
/// ours, bit 2 theirs.
pub(crate) fn kind_of(has_base: bool, has_ours: bool, has_theirs: bool) -> Option<ConflictKind> {
    let mask = has_base as u8 | (has_ours as u8) << 1 | (has_theirs as u8) << 2;
    Some(match mask {
        1 => ConflictKind::BothDeleted,
        2 => ConflictKind::AddedByUs,
        3 => ConflictKind::DeletedByThem,
        4 => ConflictKind::AddedByThem,
        5 => ConflictKind::DeletedByUs,
        6 => ConflictKind::BothAdded,
        7 => ConflictKind::BothModified,
        _ => return None,
    })
}

/// Every unmerged path of the repository with its kind, in index order.
pub fn list(repo: &Path) -> Result<Vec<ConflictEntry>> {
    let stages = parse_unmerged(&git(repo, &["ls-files", "-u", "-z"])?)?;
    let mut out: Vec<ConflictEntry> = Vec::new();
    let mut i = 0;
    while i < stages.len() {
        let path = &stages[i].path;
        let mut present = [false; 3];
        while i < stages.len() && &stages[i].path == path {
            present[(stages[i].stage - 1) as usize] = true;
            i += 1;
        }
        let kind = kind_of(present[0], present[1], present[2])
            .ok_or_else(|| Error::Parse(format!("no stages for {path}")))?;
        out.push(ConflictEntry {
            path: path.clone(),
            kind,
        });
    }
    Ok(out)
}

/// The unmerged entries of exactly this path. A literal pathspec of a directory
/// matches everything under it, so the entries are filtered by equality as well.
fn stages_of(repo: &Path, path: &str) -> Result<Vec<Stage>> {
    let raw = git(repo, &["ls-files", "-u", "-z", "--", &literal(path)])?;
    Ok(parse_unmerged(&raw)?
        .into_iter()
        .filter(|s| s.path == path)
        .collect())
}

fn not_conflicted(path: &str) -> Error {
    Error::Rule(format!("{path} has no conflict to resolve"))
}

/// The `conflict-marker-size` attribute of `path`, or git's 7. `check-attr` reads
/// pathnames, not pathspecs — see the module docblock. `-z` answers
/// `<path> NUL <attr> NUL <value> NUL`.
fn marker_size(repo: &Path, path: &str) -> Result<u32> {
    let raw = git(
        repo,
        &["check-attr", "-z", "conflict-marker-size", "--", path],
    )?;
    let text = String::from_utf8_lossy(&raw);
    let value = text.split('\0').nth(2).unwrap_or("");
    // `unspecified`, `set`, `unset` and anything unparsable are git's default too.
    Ok(value
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MARKER_SIZE))
}

/// A side's text, judged the way `read_text_file` judges a file: over the ceiling,
/// a NUL byte or invalid UTF-8 is not offered to a textarea.
fn side_from_bytes(mode: &str, bytes: &[u8]) -> ConflictSide {
    let blocked = |b| ConflictSide {
        text: None,
        blocked: Some(b),
        mode: mode.to_string(),
    };
    if bytes.len() as u64 > EDIT_SIZE_CEILING {
        return blocked(EditBlock::TooLarge);
    }
    if bytes.contains(&0) {
        return blocked(EditBlock::Binary);
    }
    match std::str::from_utf8(bytes) {
        Ok(t) => ConflictSide {
            text: Some(t.replace("\r\n", "\n")),
            blocked: None,
            mode: mode.to_string(),
        },
        Err(_) => blocked(EditBlock::Binary),
    }
}

/// Symlinks and submodules: resolved whole, never line by line.
fn special_mode(mode: &str) -> bool {
    mode == "120000" || mode == "160000"
}

fn read_side(repo: &Path, st: &Stage) -> Result<ConflictSide> {
    if special_mode(&st.mode) {
        return Ok(ConflictSide {
            text: None,
            blocked: None,
            mode: st.mode.clone(),
        });
    }
    // Size first: a conflicted video is not read into memory to be refused.
    let size = git(repo, &["cat-file", "-s", &st.oid])?;
    let size: u64 = String::from_utf8_lossy(&size)
        .trim()
        .parse()
        .map_err(|_| Error::Parse(format!("cat-file -s {}: not a size", st.oid)))?;
    if size > EDIT_SIZE_CEILING {
        return Ok(ConflictSide {
            text: None,
            blocked: Some(EditBlock::TooLarge),
            mode: st.mode.clone(),
        });
    }
    let bytes = git(repo, &["cat-file", "blob", &st.oid])?;
    Ok(side_from_bytes(&st.mode, &bytes))
}

/// Everything the editor shows about one conflicted path: the three sides from the
/// index, the working file with the markers, the marker size. A path with no
/// unmerged entries is `Error::Rule` — there is nothing to resolve.
pub fn read(repo: &Path, path: &str) -> Result<ConflictFile> {
    let stages = stages_of(repo, path)?;
    if stages.is_empty() {
        return Err(not_conflicted(path));
    }
    let find = |n: u8| stages.iter().find(|s| s.stage == n);
    let kind = kind_of(find(1).is_some(), find(2).is_some(), find(3).is_some())
        .ok_or_else(|| not_conflicted(path))?;
    let side =
        |n: u8| -> Result<Option<ConflictSide>> { find(n).map(|s| read_side(repo, s)).transpose() };
    let (base, ours, theirs) = (side(1)?, side(2)?, side(3)?);
    let worktree = CliEngine::new(repo).read_text_file(path)?;

    let text_side = |s: &Option<ConflictSide>| !matches!(s, Some(ConflictSide { text: None, .. }));
    // Line-level work needs markers in a text file, and markers are only ever
    // written when both sides are there: a delete/modify conflict leaves one
    // side's version in the tree, whole.
    let whole_only = worktree.blocked.is_some()
        || !(text_side(&base) && text_side(&ours) && text_side(&theirs))
        || ours.is_none()
        || theirs.is_none();

    Ok(ConflictFile {
        path: path.to_string(),
        kind,
        base,
        ours,
        theirs,
        worktree,
        marker_size: marker_size(repo, path)?,
        whole_only,
    })
}

/// Mark the conflict of `path` resolved, optionally writing the resolution first.
///
/// * `text: Some` — written with [`CliEngine::write_text_file`] against `expect`
///   (required then), so a file changed on disk since the editor read it is
///   `Error::Stale` and nothing is staged.
/// * `text: None, expect: Some` — the file is taken as it lies, provided it is still
///   what the editor last saw (`""`: absent).
/// * both `None` — as it lies, no questions: the user resolved it elsewhere.
///
/// "Is it conflicted at all" is asked first: that is a question about the index,
/// and a write to a file with nothing to resolve is not one to offer "overwrite" for.
pub fn resolve(
    repo: &Path,
    path: &str,
    text: Option<&str>,
    eol: Eol,
    expect: Option<&str>,
) -> Result<()> {
    if stages_of(repo, path)?.is_empty() {
        return Err(not_conflicted(path));
    }
    let eng = CliEngine::new(repo);
    match (text, expect) {
        (Some(text), Some(expect)) => {
            eng.write_text_file(path, text, eol, expect)?;
        }
        (Some(_), None) => {
            return Err(Error::Rule(
                "a resolution text needs the digest of the file it replaces".into(),
            ))
        }
        (None, Some(expect)) => {
            let current = match std::fs::read(eng.worktree_path(path)?) {
                Ok(b) => Some(crate::engine::cli::fnv1a(&b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(Error::Io(format!("{path}: {e}"))),
            };
            let fresh = match current {
                Some(d) => d == expect,
                None => expect.is_empty(),
            };
            if !fresh {
                return Err(Error::Stale(format!(
                    "{path} changed on disk since it was read"
                )));
            }
        }
        (None, None) => {}
    }
    git(repo, &["add", "-A", "--", &literal(path)])?;
    Ok(())
}

/// Resolve `path` by taking one side whole: `side` is `ours` or `theirs`.
///
/// A side on which the file exists is checked out (`checkout --ours|--theirs`) and
/// staged. A side on which it does not — `ours` of a `DeletedByUs`, `theirs` of a
/// `DeletedByThem` — is the deletion, and `git rm` is how git records it:
/// `checkout --ours` would only fail with "does not have our version". Binary files,
/// symlinks and submodules are resolved exactly this way; there is no other.
pub fn take(repo: &Path, path: &str, side: &str) -> Result<()> {
    let (flag, stage) = match side {
        "ours" => ("--ours", 2u8),
        "theirs" => ("--theirs", 3u8),
        other => {
            return Err(Error::Rule(format!(
                "unknown side: {other} (expected ours or theirs)"
            )))
        }
    };
    let stages = stages_of(repo, path)?;
    if stages.is_empty() {
        return Err(not_conflicted(path));
    }
    let spec = literal(path);
    if stages.iter().any(|s| s.stage == stage) {
        git(repo, &["checkout", flag, "--", &spec])?;
        git(repo, &["add", "--", &spec])?;
    } else {
        git(repo, &["rm", "-q", "--", &spec])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::engine::ops;
    use crate::model::OperationKind;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) -> String {
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

    /// A command that is *expected* to fail (a conflicting merge).
    fn run_failing(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            !out.status.success(),
            "git {args:?} was expected to conflict"
        );
    }

    fn unmerged(p: &Path) -> String {
        run(p, &["ls-files", "-u"])
    }

    /// `main` and `feat` both rewrite line 2 of `f.txt`; `main` is checked out and
    /// `feat` merged into it, which stops with a content conflict.
    fn merge_conflict(style: Option<&str>) -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        if let Some(s) = style {
            run(p, &["config", "merge.conflictStyle", s]);
        }
        std::fs::write(p.join("f.txt"), "a\nb\nc\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        std::fs::write(p.join("f.txt"), "a\nTHEIRS\nc\n").unwrap();
        run(p, &["commit", "-qam", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("f.txt"), "a\nOURS\nc\n").unwrap();
        run(p, &["commit", "-qam", "ours"]);
        run_failing(p, &["merge", "feat"]);
        dir
    }

    #[test]
    fn kind_follows_gits_stage_table_and_status_letters() {
        // Checked against the XY `status --porcelain=v2` prints, below.
        assert_eq!(kind_of(true, true, true), Some(ConflictKind::BothModified));
        assert_eq!(kind_of(false, true, true), Some(ConflictKind::BothAdded));
        assert_eq!(kind_of(true, false, true), Some(ConflictKind::DeletedByUs));
        assert_eq!(
            kind_of(true, true, false),
            Some(ConflictKind::DeletedByThem)
        );
        assert_eq!(kind_of(false, true, false), Some(ConflictKind::AddedByUs));
        assert_eq!(kind_of(false, false, true), Some(ConflictKind::AddedByThem));
        assert_eq!(kind_of(true, false, false), Some(ConflictKind::BothDeleted));
        assert_eq!(kind_of(false, false, false), None);

        // UU, DU and AA from one real merge, compared with git's own letters.
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("m.txt"), "base\n").unwrap();
        std::fs::write(p.join("d.txt"), "base\n").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        std::fs::write(p.join("m.txt"), "theirs\n").unwrap();
        std::fs::write(p.join("d.txt"), "theirs\n").unwrap();
        std::fs::write(p.join("n.txt"), "theirs\n").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-qm", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("m.txt"), "ours\n").unwrap();
        std::fs::write(p.join("n.txt"), "ours\n").unwrap();
        run(p, &["rm", "-q", "d.txt"]);
        run(p, &["add", "."]);
        run(p, &["commit", "-qm", "ours"]);
        run_failing(p, &["merge", "feat"]);

        let status = run(p, &["status", "--porcelain=v2", "-z"]);
        let letters = |path: &str| {
            status
                .split('\0')
                .find(|r| r.starts_with("u ") && r.ends_with(&format!(" {path}")))
                .map(|r| r[2..4].to_string())
                .unwrap()
        };
        let got = list(p).unwrap();
        let kind = |path: &str| got.iter().find(|e| e.path == path).unwrap().kind;
        assert_eq!(letters("m.txt"), "UU");
        assert_eq!(kind("m.txt"), ConflictKind::BothModified);
        assert_eq!(letters("d.txt"), "DU");
        assert_eq!(kind("d.txt"), ConflictKind::DeletedByUs);
        assert_eq!(letters("n.txt"), "AA");
        assert_eq!(kind("n.txt"), ConflictKind::BothAdded);
    }

    /// The whole road of a content conflict: three sides read, the resolution
    /// written and staged, the path no longer unmerged, and the merge continues.
    #[test]
    fn a_merge_conflict_is_read_resolved_and_continued() {
        let dir = merge_conflict(Some("diff3"));
        let p = dir.path();

        let c = read(p, "f.txt").unwrap();
        assert_eq!(c.kind, ConflictKind::BothModified);
        assert_eq!(c.base.unwrap().text.as_deref(), Some("a\nb\nc\n"));
        assert_eq!(c.ours.unwrap().text.as_deref(), Some("a\nOURS\nc\n"));
        assert_eq!(c.theirs.unwrap().text.as_deref(), Some("a\nTHEIRS\nc\n"));
        assert_eq!(c.marker_size, 7);
        assert!(!c.whole_only);
        let merged = c.worktree.text.unwrap();
        assert!(merged.contains("<<<<<<< HEAD\nOURS\n||||||| "), "{merged}");
        assert!(
            merged.contains("=======\nTHEIRS\n>>>>>>> feat\n"),
            "{merged}"
        );

        let op = ops::detect_state(p).unwrap();
        assert_eq!(op.kind, OperationKind::Merge);
        assert_eq!(
            op.conflicted,
            vec![ConflictEntry {
                path: "f.txt".into(),
                kind: ConflictKind::BothModified
            }]
        );

        resolve(
            p,
            "f.txt",
            Some("a\nOURS\nTHEIRS\nc\n"),
            Eol::Lf,
            Some(&c.worktree.digest),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("f.txt")).unwrap(),
            "a\nOURS\nTHEIRS\nc\n"
        );
        assert_eq!(unmerged(p), "", "resolved in the index");
        let op = ops::detect_state(p).unwrap();
        assert!(op.conflicted.is_empty());

        ops::op_continue(p, None).unwrap();
        assert_eq!(ops::detect_state(p).unwrap().kind, OperationKind::None);
        assert_eq!(run(p, &["show", "HEAD:f.txt"]), "a\nOURS\nTHEIRS\nc");
        assert_eq!(run(p, &["rev-list", "--count", "--merges", "HEAD"]), "1");
    }

    #[test]
    fn a_resolution_over_a_file_changed_on_disk_is_stale() {
        let dir = merge_conflict(None);
        let p = dir.path();
        let c = read(p, "f.txt").unwrap();
        std::fs::write(p.join("f.txt"), "someone else\n").unwrap();

        let err = resolve(
            p,
            "f.txt",
            Some("mine\n"),
            Eol::Lf,
            Some(&c.worktree.digest),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Stale(_)), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(p.join("f.txt")).unwrap(),
            "someone else\n"
        );
        assert_ne!(unmerged(p), "", "nothing was staged");

        // "As it lies" is fresh-checked too.
        let err = resolve(p, "f.txt", None, Eol::Lf, Some(&c.worktree.digest)).unwrap_err();
        assert!(matches!(err, Error::Stale(_)), "{err:?}");
        assert_ne!(unmerged(p), "");
    }

    #[test]
    fn a_path_without_a_conflict_is_refused() {
        let dir = merge_conflict(None);
        let p = dir.path();
        assert!(matches!(read(p, "a.txt"), Err(Error::Rule(_))));
        assert!(matches!(
            resolve(p, "a.txt", None, Eol::Lf, None),
            Err(Error::Rule(_))
        ));
        assert!(matches!(take(p, "a.txt", "ours"), Err(Error::Rule(_))));
        assert!(matches!(take(p, "f.txt", "base"), Err(Error::Rule(_))));
    }

    /// zdiff3 writes the same markers as diff3; the markers carry the base.
    #[test]
    fn zdiff3_markers_carry_the_base() {
        let dir = merge_conflict(Some("zdiff3"));
        let c = read(dir.path(), "f.txt").unwrap();
        let merged = c.worktree.text.unwrap();
        assert!(merged.contains("||||||| "), "{merged}");
        assert!(merged.contains("\nb\n======="), "{merged}");
    }

    /// `main` deletes `f.txt`, `feat` changes it: a delete/modify conflict (DU when
    /// `feat` is merged into `main`). No markers — one side is the deletion.
    fn delete_modify() -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("f.txt"), "a\nb\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        std::fs::write(p.join("f.txt"), "a\nB\n").unwrap();
        run(p, &["commit", "-qam", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        run(p, &["rm", "-q", "f.txt"]);
        run(p, &["commit", "-qm", "ours deletes"]);
        run_failing(p, &["merge", "feat"]);
        dir
    }

    #[test]
    fn delete_modify_is_resolved_whole_by_deletion_or_by_the_change() {
        let dir = delete_modify();
        let p = dir.path();
        let c = read(p, "f.txt").unwrap();
        assert_eq!(c.kind, ConflictKind::DeletedByUs);
        assert!(c.ours.is_none(), "deleted on our side");
        assert_eq!(c.theirs.unwrap().text.as_deref(), Some("a\nB\n"));
        assert!(c.whole_only);

        // Taking ours is taking the deletion.
        take(p, "f.txt", "ours").unwrap();
        assert!(!p.join("f.txt").exists());
        assert_eq!(unmerged(p), "");
        ops::op_continue(p, None).unwrap();
        assert!(run(p, &["ls-tree", "--name-only", "HEAD"])
            .lines()
            .all(|l| l != "f.txt"));

        // The other way: taking theirs keeps the change.
        let dir = delete_modify();
        let p = dir.path();
        take(p, "f.txt", "theirs").unwrap();
        assert_eq!(std::fs::read_to_string(p.join("f.txt")).unwrap(), "a\nB\n");
        assert_eq!(unmerged(p), "");
        ops::op_continue(p, None).unwrap();
        assert_eq!(run(p, &["show", "HEAD:f.txt"]), "a\nB");
    }

    /// The deletion is also reachable as a plain "resolved as it lies" once the file
    /// is gone from the tree: `add -A` stages the absence.
    #[test]
    fn resolving_an_absent_file_stages_the_deletion() {
        let dir = delete_modify();
        let p = dir.path();
        std::fs::remove_file(p.join("f.txt")).unwrap();
        resolve(p, "f.txt", None, Eol::Lf, Some("")).unwrap();
        assert_eq!(unmerged(p), "");
        assert_eq!(run(p, &["status", "--porcelain", "--", "f.txt"]), "");
    }

    #[test]
    fn a_binary_conflict_is_offered_whole_only() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("b.bin"), b"\0base\0").unwrap();
        run(p, &["add", "b.bin"]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        std::fs::write(p.join("b.bin"), b"\0theirs\0").unwrap();
        run(p, &["commit", "-qam", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("b.bin"), b"\0ours\0").unwrap();
        run(p, &["commit", "-qam", "ours"]);
        run_failing(p, &["merge", "feat"]);

        let c = read(p, "b.bin").unwrap();
        assert_eq!(c.kind, ConflictKind::BothModified);
        assert!(c.whole_only);
        assert_eq!(c.ours.as_ref().unwrap().blocked, Some(EditBlock::Binary));
        assert_eq!(c.theirs.as_ref().unwrap().text, None);
        assert_eq!(c.worktree.blocked, Some(EditBlock::Binary));

        take(p, "b.bin", "theirs").unwrap();
        assert_eq!(
            std::fs::read(p.join("b.bin")).unwrap(),
            b"\0theirs\0".to_vec()
        );
        assert_eq!(unmerged(p), "");
    }

    /// `x[ab].txt` as a pathspec also matches `xa.txt`: both are conflicted here, and
    /// resolving or taking the first must leave the second exactly as it was.
    #[test]
    fn a_bracketed_path_touches_only_itself() {
        let dir = scratch_repo();
        let p = dir.path();
        for f in ["x[ab].txt", "xa.txt"] {
            std::fs::write(p.join(f), "base\n").unwrap();
        }
        run(p, &["add", "."]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        for f in ["x[ab].txt", "xa.txt"] {
            std::fs::write(p.join(f), "theirs\n").unwrap();
        }
        run(p, &["commit", "-qam", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        for f in ["x[ab].txt", "xa.txt"] {
            std::fs::write(p.join(f), "ours\n").unwrap();
        }
        run(p, &["commit", "-qam", "ours"]);
        run_failing(p, &["merge", "feat"]);
        let xa_before = std::fs::read(p.join("xa.txt")).unwrap();

        let c = read(p, "x[ab].txt").unwrap();
        assert_eq!(c.ours.unwrap().text.as_deref(), Some("ours\n"));
        take(p, "x[ab].txt", "theirs").unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("x[ab].txt")).unwrap(),
            "theirs\n"
        );
        assert_eq!(
            std::fs::read(p.join("xa.txt")).unwrap(),
            xa_before,
            "xa.txt untouched"
        );
        let left = list(p).unwrap();
        assert_eq!(left.len(), 1, "{left:?}");
        assert_eq!(left[0].path, "xa.txt");

        // And the text road.
        let dir2 = scratch_repo();
        let p2 = dir2.path();
        for f in ["x[ab].txt", "xa.txt"] {
            std::fs::write(p2.join(f), "base\n").unwrap();
        }
        run(p2, &["add", "."]);
        run(p2, &["commit", "-qm", "base"]);
        run(p2, &["checkout", "-qb", "feat"]);
        for f in ["x[ab].txt", "xa.txt"] {
            std::fs::write(p2.join(f), "theirs\n").unwrap();
        }
        run(p2, &["commit", "-qam", "theirs"]);
        run(p2, &["checkout", "-q", "main"]);
        for f in ["x[ab].txt", "xa.txt"] {
            std::fs::write(p2.join(f), "ours\n").unwrap();
        }
        run(p2, &["commit", "-qam", "ours"]);
        run_failing(p2, &["merge", "feat"]);
        let c = read(p2, "x[ab].txt").unwrap();
        resolve(
            p2,
            "x[ab].txt",
            Some("done\n"),
            Eol::Lf,
            Some(&c.worktree.digest),
        )
        .unwrap();
        let left = list(p2).unwrap();
        assert_eq!(left.len(), 1, "{left:?}");
        assert_eq!(left[0].path, "xa.txt");
    }

    #[test]
    fn the_marker_size_attribute_is_reported() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join(".gitattributes"), "*.txt conflict-marker-size=12\n").unwrap();
        run(p, &["add", ".gitattributes"]);
        run(p, &["commit", "-qm", "attrs"]);
        std::fs::write(p.join("f.txt"), "a\nb\nc\n").unwrap();
        run(p, &["add", "f.txt"]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        std::fs::write(p.join("f.txt"), "a\nT\nc\n").unwrap();
        run(p, &["commit", "-qam", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("f.txt"), "a\nO\nc\n").unwrap();
        run(p, &["commit", "-qam", "ours"]);
        run_failing(p, &["merge", "feat"]);

        let c = read(p, "f.txt").unwrap();
        assert_eq!(c.marker_size, 12);
        assert!(c
            .worktree
            .text
            .unwrap()
            .contains(&format!("{} HEAD\n", "<".repeat(12))));
    }

    /// A CRLF file conflicts with CRLF markers; its text reaches the editor in `\n`
    /// and the resolution goes back in CRLF.
    #[test]
    fn a_crlf_conflict_round_trips_its_line_endings() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["config", "core.autocrlf", "false"]);
        std::fs::write(p.join("w.txt"), "a\r\nb\r\nc\r\n").unwrap();
        run(p, &["add", "w.txt"]);
        run(p, &["commit", "-qm", "base"]);
        run(p, &["checkout", "-qb", "feat"]);
        std::fs::write(p.join("w.txt"), "a\r\nT\r\nc\r\n").unwrap();
        run(p, &["commit", "-qam", "theirs"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("w.txt"), "a\r\nO\r\nc\r\n").unwrap();
        run(p, &["commit", "-qam", "ours"]);
        run_failing(p, &["merge", "feat"]);

        let c = read(p, "w.txt").unwrap();
        assert_eq!(
            c.worktree.blocked, None,
            "git writes CRLF markers into a CRLF file"
        );
        assert_eq!(c.worktree.eol, Eol::Crlf);
        assert_eq!(c.ours.unwrap().text.as_deref(), Some("a\nO\nc\n"));
        resolve(
            p,
            "w.txt",
            Some("a\nO\nT\nc\n"),
            c.worktree.eol,
            Some(&c.worktree.digest),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(p.join("w.txt")).unwrap(),
            b"a\r\nO\r\nT\r\nc\r\n".to_vec()
        );
    }
}
