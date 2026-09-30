//! "Ignore" from the Changes menu: which rule goes into the root `.gitignore` for an
//! untracked path, writing it, and taking it back out for Undo.
//!
//! The pattern is always computed **here**, from the path and the kind the user
//! picked — the menu shows what [`choices`] returns and sends back only the kind, so
//! the client never hands in a pattern to be written verbatim.
//!
//! ## The rules
//!
//! - **This file** — `/<path>`. The leading `/` anchors the rule at the root: a
//!   pattern without a slash in the middle (every file at the root) matches at *any*
//!   depth, so `a.txt` would also hide `sub/a.txt`; with one in the middle it is
//!   anchored already, and the `/` is written for all paths so the rule reads the same
//!   way whatever the depth.
//! - **All `*.<ext>` files** — `*.<ext>`, deliberately unanchored: "every file with
//!   this extension" is meant repository-wide. A name whose only dot starts it
//!   (`.env`) or ends it (`archive.`) has no extension, and the choice is not offered.
//! - **The folder** — `/<folder>/`: anchored like a file, the trailing `/` limits it
//!   to a directory. For a file at the root there is no folder to offer (ignoring the
//!   root would ignore everything); an untracked folder entry (`dir/`, how `git
//!   status` collapses one) offers only itself.
//!
//! Every path component is written **literally**: `\`, `*`, `?` and `[` are
//! backslash-escaped (they are wildcards), trailing spaces are escaped (git drops
//! unescaped ones), and a component starting with `#` or `!` is escaped too. Behind
//! the leading `/` those two could not start the line anyway — the escape keeps the
//! rule literal even if someone later deletes the anchor by hand. A path with a line
//! break cannot be written as one gitignore line and is refused.
//!
//! ## Writing, and taking it back
//!
//! The rule goes on a new last line, in the file's own line ending (CRLF when the file
//! has any `\r\n`, else LF; a new file gets LF); a last line without a terminator gets
//! one first. A rule already present (line for line, `\r` aside) is not added again —
//! that is a refusal, not a silent no-op: the path is then still visible for a reason
//! a second copy would not change (a later `!` rule, a nested `.gitignore`).
//!
//! The write is raw bytes through `cli::replace_file` (temp file + rename), never a
//! text round trip — the rest of the file stays byte for byte. A `.gitignore` that is
//! a symlink or a directory is refused: the rename would replace a link with a file.
//!
//! [`IgnoreEdit`] records what was appended and the FNV-1a fingerprints of the file on
//! both sides; Undo cuts the suffix off (or deletes the file this action created) and
//! Redo appends it again — each only over the exact bytes it expects. No content is
//! stored beyond the appended rule.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::cli::{fnv1a, literal, replace_file};
use super::exec;
use crate::error::{Error, Result};
use crate::model::{IgnoreChoice, IgnoreKind};

/// The file the rules go to, relative to the worktree root.
pub const IGNORE_FILE: &str = ".gitignore";

/// One rule written by [`apply`]: enough to take it back out and put it in again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoreEdit {
    /// The rule itself, for the undo label.
    pub pattern: String,
    /// `.gitignore` did not exist before: Undo deletes it.
    pub created: bool,
    /// Fingerprint of the file before (`""` when it was created).
    pub before: String,
    /// Fingerprint of the file with the rule.
    pub after: String,
    /// The bytes appended — the rule, its line ending, and a terminator put in front
    /// when the last line had none.
    pub suffix: String,
}

/// One path component, literal in gitignore.
fn escape_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len() + 2);
    if segment.starts_with('#') || segment.starts_with('!') {
        out.push('\\');
    }
    for c in segment.chars() {
        if matches!(c, '\\' | '*' | '?' | '[') {
            out.push('\\');
        }
        out.push(c);
    }
    let body = out.trim_end_matches(' ');
    let spaces = out.len() - body.len();
    format!("{body}{}", "\\ ".repeat(spaces))
}

fn escape_path(path: &str) -> String {
    path.split('/')
        .map(escape_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// Refuse what cannot be a repository-relative path written on one line.
fn check_path(path: &str) -> Result<()> {
    let bad = || Error::Rule(format!("{path:?} cannot be written as a .gitignore rule"));
    if path.is_empty() || path.contains(['\0', '\n', '\r']) || path.starts_with('/') {
        return Err(bad());
    }
    let body = path.strip_suffix('/').unwrap_or(path);
    if body
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(bad());
    }
    Ok(())
}

/// The rules the menu offers for one untracked path, most specific first.
pub fn choices(path: &str) -> Result<Vec<IgnoreChoice>> {
    check_path(path)?;
    if let Some(folder) = path.strip_suffix('/') {
        return Ok(vec![IgnoreChoice {
            kind: IgnoreKind::Folder,
            pattern: format!("/{}/", escape_path(folder)),
        }]);
    }
    let mut out = vec![IgnoreChoice {
        kind: IgnoreKind::File,
        pattern: format!("/{}", escape_path(path)),
    }];
    let (folder, name) = match path.rsplit_once('/') {
        Some((f, n)) => (Some(f), n),
        None => (None, path),
    };
    if let Some(dot) = name.rfind('.') {
        if dot > 0 && dot + 1 < name.len() {
            out.push(IgnoreChoice {
                kind: IgnoreKind::Extension,
                pattern: format!("*.{}", escape_segment(&name[dot + 1..])),
            });
        }
    }
    if let Some(folder) = folder {
        out.push(IgnoreChoice {
            kind: IgnoreKind::Folder,
            pattern: format!("/{}/", escape_path(folder)),
        });
    }
    Ok(out)
}

/// `content` with `pattern` as a new last line, and the bytes that were appended;
/// `None` when the rule is there already.
fn append_rule(content: &[u8], pattern: &str) -> Option<Vec<u8>> {
    let present = content
        .split(|b| *b == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == pattern.as_bytes());
    if present {
        return None;
    }
    let eol: &[u8] = if content.windows(2).any(|w| w == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let mut suffix = Vec::new();
    if !content.is_empty() && !content.ends_with(b"\n") {
        suffix.extend_from_slice(eol);
    }
    suffix.extend_from_slice(pattern.as_bytes());
    suffix.extend_from_slice(eol);
    Some(suffix)
}

/// What the rule for (`path`, `kind`) will change, read from the file as it is now.
/// Nothing is written. A rule already present is refused with the reason, and so is
/// a tracked path: `.gitignore` does not apply to what git already tracks, and the
/// rule would change nothing the user could see.
pub fn plan(repo: &Path, path: &str, kind: IgnoreKind) -> Result<IgnoreEdit> {
    let pattern = choices(path)?
        .into_iter()
        .find(|c| c.kind == kind)
        .map(|c| c.pattern)
        .ok_or_else(|| Error::Rule(format!("{path} cannot be ignored that way")))?;
    let tracked = exec::git(repo, &["ls-files", "-z", "--", &literal(path)])
        .run()?
        .checked()?;
    if !tracked.is_empty() {
        return Err(Error::Rule(format!(
            "{path} is tracked; .gitignore only hides untracked files"
        )));
    }
    let current = read(repo)?;
    let bytes = current.as_deref().unwrap_or_default();
    let suffix = append_rule(bytes, &pattern).ok_or_else(|| {
        Error::Rule(format!(
            "{IGNORE_FILE} already has the rule {pattern}; a later rule or a .gitignore further down un-ignores {path}"
        ))
    })?;
    let mut after = bytes.to_vec();
    after.extend_from_slice(&suffix);
    Ok(IgnoreEdit {
        pattern,
        created: current.is_none(),
        before: current.as_deref().map(fnv1a).unwrap_or_default(),
        after: fnv1a(&after),
        suffix: String::from_utf8(suffix).map_err(|_| Error::Parse("rule is not UTF-8".into()))?,
    })
}

/// Undo: back to the file before the rule — only from the exact bytes the rule left.
pub fn revert(repo: &Path, edit: &IgnoreEdit) -> Result<()> {
    let changed = || {
        Error::Rule(format!(
            "{IGNORE_FILE} changed since the rule {} was added; nothing was reversed",
            edit.pattern
        ))
    };
    let current = read(repo)?.ok_or_else(changed)?;
    if fnv1a(&current) != edit.after || !current.ends_with(edit.suffix.as_bytes()) {
        return Err(changed());
    }
    let target = target(repo);
    if edit.created {
        std::fs::remove_file(&target).map_err(|e| Error::Io(format!("{IGNORE_FILE}: {e}")))
    } else {
        let before = &current[..current.len() - edit.suffix.len()];
        replace_file(&target, before).map_err(|e| Error::Io(format!("{IGNORE_FILE}: {e}")))
    }
}

/// Write the rule [`plan`] computed — only over the file the plan read. Redo is the
/// same call: the file must be back to exactly what it was before the rule.
pub fn apply(repo: &Path, edit: &IgnoreEdit) -> Result<()> {
    let current = read(repo)?;
    let fresh = match &current {
        None => edit.created,
        Some(bytes) => !edit.created && fnv1a(bytes) == edit.before,
    };
    if !fresh {
        return Err(Error::Rule(format!(
            "{IGNORE_FILE} changed since it was read; the rule {} was not added",
            edit.pattern
        )));
    }
    let mut bytes = current.unwrap_or_default();
    bytes.extend_from_slice(edit.suffix.as_bytes());
    replace_file(&target(repo), &bytes).map_err(|e| Error::Io(format!("{IGNORE_FILE}: {e}")))
}

fn target(repo: &Path) -> PathBuf {
    repo.join(IGNORE_FILE)
}

/// The file's bytes, `None` when it does not exist. A symlink or a directory in its
/// place is refused: a write by rename would replace the link with a plain file.
fn read(repo: &Path) -> Result<Option<Vec<u8>>> {
    let path = target(repo);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_file() => Err(Error::Rule(format!(
            "{IGNORE_FILE} is not a regular file (a link or a folder); add the rule by hand"
        ))),
        Ok(_) => std::fs::read(&path)
            .map(Some)
            .map_err(|e| Error::Io(format!("{IGNORE_FILE}: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(format!("{IGNORE_FILE}: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("spawn git")
    }

    /// `git check-ignore` says the path is ignored (by any rule).
    fn ignored(dir: &Path, path: &str) -> bool {
        git(dir, &["check-ignore", "-q", "--", path])
            .status
            .success()
    }

    /// The path is listed by `git status` (untracked, not ignored).
    fn listed(dir: &Path, path: &str) -> bool {
        let out = git(
            dir,
            &["status", "--porcelain", "-z", "--untracked-files=all"],
        );
        String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .any(|r| r.get(3..) == Some(path))
    }

    fn touch(dir: &Path, path: &str) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x\n").unwrap();
    }

    fn ignore(dir: &Path, path: &str, kind: IgnoreKind) -> IgnoreEdit {
        let edit = plan(dir, path, kind).unwrap();
        apply(dir, &edit).unwrap();
        edit
    }

    fn patterns(path: &str) -> Vec<(IgnoreKind, String)> {
        choices(path)
            .unwrap()
            .into_iter()
            .map(|c| (c.kind, c.pattern))
            .collect()
    }

    #[test]
    fn choices_follow_the_path() {
        use IgnoreKind::*;
        assert_eq!(
            patterns("src/a.log"),
            vec![
                (File, "/src/a.log".into()),
                (Extension, "*.log".into()),
                (Folder, "/src/".into())
            ]
        );
        assert_eq!(
            patterns("a.tar.gz"),
            vec![(File, "/a.tar.gz".into()), (Extension, "*.gz".into())],
            "no folder at the root"
        );
        assert_eq!(
            patterns(".env"),
            vec![(File, "/.env".into())],
            "a leading dot is a name, not an extension"
        );
        assert_eq!(patterns("archive."), vec![(File, "/archive.".into())]);
        assert_eq!(
            patterns("build/out/"),
            vec![(Folder, "/build/out/".into())],
            "a folder entry offers itself only"
        );
        for bad in ["", "/abs", "a/../b", "./a", "a//b", "a\nb", "a\rb"] {
            assert!(choices(bad).is_err(), "{bad:?} refused");
        }
    }

    #[test]
    fn each_rule_hides_what_it_names_and_nothing_else() {
        let dir = scratch_repo();
        let p = dir.path();
        for f in [
            "a.log",
            "sub/a.log",
            "sub/b.txt",
            "deep/x/y.txt",
            "deep/x/z.log",
            "b.log",
        ] {
            touch(p, f);
        }

        ignore(p, "a.log", IgnoreKind::File);
        assert!(
            ignored(p, "a.log") && !listed(p, "a.log"),
            "the file is hidden"
        );
        assert!(
            !ignored(p, "sub/a.log") && listed(p, "sub/a.log"),
            "the leading / keeps a namesake in a subfolder visible"
        );

        ignore(p, "sub/b.txt", IgnoreKind::Folder);
        assert!(
            ignored(p, "sub/b.txt") && ignored(p, "sub/a.log"),
            "the folder takes all it holds"
        );
        assert!(!ignored(p, "deep/x/y.txt"));

        ignore(p, "b.log", IgnoreKind::Extension);
        assert!(ignored(p, "b.log"), "the extension rule");
        assert!(
            ignored(p, "deep/x/z.log") && !listed(p, "deep/x/z.log"),
            "*.log is unanchored: it reaches every depth"
        );
        assert!(!ignored(p, "deep/x/y.txt"), "and only that extension");

        ignore(p, "deep/", IgnoreKind::Folder);
        assert!(
            ignored(p, "deep/x/y.txt") && !listed(p, "deep/x/y.txt"),
            "an untracked folder entry"
        );

        let text = std::fs::read_to_string(p.join(IGNORE_FILE)).unwrap();
        assert_eq!(text, "/a.log\n/sub/\n*.log\n/deep/\n");
    }

    #[test]
    fn special_characters_are_written_literally() {
        let dir = scratch_repo();
        let p = dir.path();
        // each special name next to a plain namesake a wildcard reading would also hit
        let cases = [
            ("#x", "/\\#x", "ax"),
            ("!x", "/\\!x", "bx"),
            ("sp ", "/sp\\ ", "sp"),
            ("[d]*?.txt", "/\\[d]\\*\\?.txt", "d-long.txt"),
            ("back\\slash", "/back\\\\slash", "backslash"),
        ];
        for (name, pattern, other) in cases {
            touch(p, name);
            touch(p, other);
            let edit = ignore(p, name, IgnoreKind::File);
            assert_eq!(edit.pattern, pattern, "{name:?}");
            assert!(
                ignored(p, name) && !listed(p, name),
                "{name:?} is hidden by {pattern}"
            );
            assert!(!ignored(p, other), "{other:?} is not caught by {pattern}");
        }
        assert_eq!(
            patterns("a.t[x]")[1].1,
            "*.t\\[x]",
            "the extension is escaped too"
        );
    }

    #[test]
    fn crlf_is_kept_and_the_rule_still_works() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join(IGNORE_FILE), b"*.tmp\r\nbuild/").unwrap();
        touch(p, "a.bin");
        let edit = ignore(p, "a.bin", IgnoreKind::File);
        assert_eq!(
            std::fs::read(p.join(IGNORE_FILE)).unwrap(),
            b"*.tmp\r\nbuild/\r\n/a.bin\r\n"
        );
        assert_eq!(
            edit.suffix, "\r\n/a.bin\r\n",
            "the missing terminator goes first"
        );
        assert!(ignored(p, "a.bin") && !listed(p, "a.bin"));

        revert(p, &edit).unwrap();
        assert_eq!(
            std::fs::read(p.join(IGNORE_FILE)).unwrap(),
            b"*.tmp\r\nbuild/",
            "Undo gives back the bytes"
        );
    }

    #[test]
    fn a_rule_already_there_is_not_added_twice() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join(IGNORE_FILE), "/a.log\r\n!/a.log\r\n").unwrap();
        touch(p, "a.log");
        let e = plan(p, "a.log", IgnoreKind::File).unwrap_err();
        assert!(matches!(e, Error::Rule(_)), "{e:?}");
        assert_eq!(
            std::fs::read_to_string(p.join(IGNORE_FILE)).unwrap(),
            "/a.log\r\n!/a.log\r\n"
        );
    }

    #[test]
    fn undo_deletes_a_created_file_and_keeps_an_empty_one() {
        let dir = scratch_repo();
        let p = dir.path();
        touch(p, "a.log");
        let edit = ignore(p, "a.log", IgnoreKind::File);
        assert!(edit.created);
        revert(p, &edit).unwrap();
        assert!(
            !p.join(IGNORE_FILE).exists(),
            "created by the action, gone with its Undo"
        );
        apply(p, &edit).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join(IGNORE_FILE)).unwrap(),
            "/a.log\n"
        );

        std::fs::write(p.join(IGNORE_FILE), "").unwrap();
        let edit = ignore(p, "a.log", IgnoreKind::File);
        assert!(!edit.created, "an empty file is a file");
        revert(p, &edit).unwrap();
        assert_eq!(
            std::fs::read(p.join(IGNORE_FILE)).unwrap(),
            b"",
            "and stays one"
        );
    }

    #[test]
    fn a_file_changed_since_is_left_alone() {
        let dir = scratch_repo();
        let p = dir.path();
        touch(p, "a.log");
        std::fs::write(p.join(IGNORE_FILE), "x\n").unwrap();
        let edit = ignore(p, "a.log", IgnoreKind::File);
        std::fs::write(p.join(IGNORE_FILE), "x\n/a.log\nmine\n").unwrap();
        assert!(matches!(revert(p, &edit), Err(Error::Rule(_))));
        assert_eq!(
            std::fs::read_to_string(p.join(IGNORE_FILE)).unwrap(),
            "x\n/a.log\nmine\n",
            "Undo wrote nothing"
        );

        // between the plan and the write
        let edit = plan(p, "b.log", IgnoreKind::File).unwrap();
        std::fs::write(p.join(IGNORE_FILE), "other\n").unwrap();
        assert!(matches!(apply(p, &edit), Err(Error::Rule(_))));
        assert_eq!(
            std::fs::read_to_string(p.join(IGNORE_FILE)).unwrap(),
            "other\n"
        );
    }

    #[test]
    fn a_tracked_path_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        assert!(matches!(
            plan(p, "a.txt", IgnoreKind::File),
            Err(Error::Rule(_))
        ));
        assert!(!p.join(IGNORE_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_gitignore_is_refused() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("rules"), "").unwrap();
        std::os::unix::fs::symlink("rules", p.join(IGNORE_FILE)).unwrap();
        touch(p, "a.log");
        assert!(matches!(
            plan(p, "a.log", IgnoreKind::File),
            Err(Error::Rule(_))
        ));
    }
}
