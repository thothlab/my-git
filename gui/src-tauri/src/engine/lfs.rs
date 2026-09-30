//! Git LFS pointer files, recognised in a diff so the panel can say "a large file
//! stored in LFS, N MB → M MB" instead of drawing three lines of hashes.
//!
//! Nothing here needs `git-lfs` to be installed. The pointer is plain text in the
//! repository, the object store is a directory, and whether `git lfs` would find its
//! program is looked up the way git itself looks it up. `git-lfs` is only ever run
//! for [`pull`], and only when that lookup found it.
//!
//! ## Where the objects are
//!
//! `<common git dir>/lfs/objects/<oid[0..2]>/<oid[2..4]>/<oid>`, or under
//! `lfs.storage` when that is set (a relative value is taken inside the common git
//! dir, `objects/` below it). **Not** `git rev-parse --git-path lfs/objects`: `lfs`
//! is not on git's list of shared directories, so in a linked worktree that answers
//! `.git/worktrees/<name>/lfs/objects`, a directory git-lfs never writes, and every
//! object would read as missing.
//!
//! ## What "Download" does
//!
//! `git lfs pull --include=<path>` fetches the objects **of the checked-out files**
//! (the index, on Git 2.42+; `HEAD` before) that the `filter=lfs` attribute names,
//! and puts their content in the working tree. So the button is offered only when
//! it will fetch exactly the object the card shows: the pointer of the diff's new
//! side is the one in the index, and the path carries `filter=lfs`. A commit deep
//! in history shows its sizes and whether the object happens to be local, but no
//! button that would fetch something else.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::engine::cli::CliEngine;
use crate::engine::exec;
use crate::error::{Error, Result};
use crate::model::{FileDiff, LfsDiff, LfsDownload, LfsSide};

/// The version line of every pointer git-lfs writes.
pub const SPEC_V1: &str = "https://git-lfs.github.com/spec/v1";
/// The pre-release spelling; git-lfs still reads it.
const SPEC_HAWSER: &str = "https://hawser.github.com/spec/v1";

/// Pointer files are shorter than this (spec: "less than 1024 bytes in size").
pub const POINTER_LIMIT: usize = 1024;

/// A parsed pointer: which object, and how big the real file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    /// 64 lowercase hex digits of the sha256, without the `sha256:` prefix.
    pub oid: String,
    pub size: u64,
}

/// Parse a pointer file, strictly by the spec (git-lfs `docs/spec.md`). `None` for
/// anything else, which is then shown as the ordinary text it is.
///
/// - shorter than [`POINTER_LIMIT`] bytes, UTF-8;
/// - every line `key value\n`, the last one included, one space, no `\r`;
/// - `version` first, one of the two spec URLs, compared as a string;
/// - the other keys `[a-z0-9.-]`, in strictly ascending order (so no duplicates):
///   `ext-<digit>-<name>` extension lines, then `oid`, then `size` — the only keys
///   git-lfs's own parser accepts, which is narrower than the spec's "preserve
///   unknown keys";
/// - `oid sha256:<64 lowercase hex>`, `size` a canonical positive decimal (no sign,
///   no leading zero). The spec allows exactly one encoding of a pointer, and
///   git-lfs never writes one for size 0 — an empty file is its own pointer, and it
///   is not treated as one here: a new empty file is not "added to LFS".
pub fn parse_pointer(text: &str) -> Option<Pointer> {
    if text.is_empty() || text.len() >= POINTER_LIMIT || !text.ends_with('\n') {
        return None;
    }
    let body = &text[..text.len() - 1];
    let mut lines = body.split('\n');
    let version = lines.next()?.strip_prefix("version ")?;
    if version != SPEC_V1 && version != SPEC_HAWSER {
        return None;
    }
    let (mut oid, mut size) = (None, None);
    let mut previous: Option<&str> = None;
    for line in lines {
        let (key, value) = line.split_once(' ')?;
        if key.is_empty()
            || value.is_empty()
            || value.contains([' ', '\r'])
            || !key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        {
            return None;
        }
        if previous.is_some_and(|p| p >= key) {
            return None;
        }
        previous = Some(key);
        match key {
            "oid" => oid = Some(sha256(value)?),
            "size" => size = Some(canonical_size(value)?),
            _ if is_extension(key) => {
                sha256(value)?;
            }
            _ => return None,
        }
    }
    Some(Pointer {
        oid: oid?,
        size: size?,
    })
}

/// `sha256:<64 lowercase hex>` → the hex.
fn sha256(value: &str) -> Option<String> {
    let hex = value.strip_prefix("sha256:")?;
    (hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| hex.to_string())
}

fn canonical_size(value: &str) -> Option<u64> {
    if !value.bytes().all(|b| b.is_ascii_digit()) || value.starts_with('0') {
        return None;
    }
    value.parse().ok()
}

/// `ext-<one digit>-<name>`.
fn is_extension(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("ext-") else {
        return false;
    };
    let b = rest.as_bytes();
    b.len() > 2 && b[0].is_ascii_digit() && b[1] == b'-'
}

/// The pointers on each side of a one-file patch that changes an LFS pointer and
/// nothing else: `(None, Some)` added, `(Some, None)` deleted, both replaced.
/// `None` when the patch is anything else.
///
/// Each side is rebuilt from the patch itself — for the working tree that is the
/// only place its *cleaned* content exists (with git-lfs installed the file on
/// disk is the real content, and git diffs what the clean filter makes of it). A
/// side is accepted only when the patch proves it whole: exactly one hunk, the side
/// starting at line 1 (or absent, `-0,0`), every line the header counts present,
/// and a `\ No newline at end of file` marker — which no pointer has — taken into
/// account. At git's default context of three lines a pointer (three to five
/// lines) always arrives whole in one hunk.
///
/// The spec URL is looked for first, so the common case costs one substring scan.
pub fn pointer_sides(raw: &str) -> Option<(Option<Pointer>, Option<Pointer>)> {
    if !raw.contains(SPEC_V1) && !raw.contains(SPEC_HAWSER) {
        return None;
    }
    let mut header: Option<(u32, u32, u32, u32)> = None;
    let (mut old, mut new) = (Vec::<&str>::new(), Vec::<&str>::new());
    let (mut old_nl, mut new_nl) = (true, true);
    let mut last = b' ';
    for line in raw.split('\n') {
        if header.is_none() {
            if line.starts_with("@@ ") {
                header = Some(hunk_header(line)?);
            }
            continue;
        }
        let Some(&first) = line.as_bytes().first() else {
            // Only the split's tail: git prefixes even an empty context line.
            continue;
        };
        match first {
            b' ' => {
                old.push(&line[1..]);
                new.push(&line[1..]);
            }
            b'-' => old.push(&line[1..]),
            b'+' => new.push(&line[1..]),
            b'\\' => match last {
                b'-' => old_nl = false,
                b'+' => new_nl = false,
                _ => {
                    old_nl = false;
                    new_nl = false;
                }
            },
            // A second hunk (`@@`) or anything unexpected: not provably whole.
            _ => return None,
        }
        if first != b'\\' {
            last = first;
        }
    }
    let (os, oc, ns, nc) = header?;
    let side = |start: u32, count: u32, lines: &[&str], nl: bool| -> Option<Option<Pointer>> {
        if count as usize != lines.len() {
            return None;
        }
        if count == 0 {
            return Some(None);
        }
        if start != 1 {
            return None;
        }
        let mut text = lines.join("\n");
        if nl {
            text.push('\n');
        }
        parse_pointer(&text).map(Some)
    };
    let before = side(os, oc, &old, old_nl)?;
    let after = side(ns, nc, &new, new_nl)?;
    if before.is_none() && after.is_none() {
        return None;
    }
    Some((before, after))
}

/// `@@ -a[,b] +c[,d] @@…` → `(a, b, c, d)`, a missing count being 1.
fn hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, c)) => Some((s.parse().ok()?, c.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d))
}

/// Where this repository's LFS objects live (see the module header), or `None`
/// when git cannot say where the common git dir is.
pub(crate) fn object_dir(repo: &Path) -> Option<PathBuf> {
    // `config` is on git's shared list, so its parent is the common dir in a
    // linked worktree as well.
    let config = CliEngine::new(repo).git_paths(&["config"]).ok()?.pop()?;
    let common = config.parent()?.to_path_buf();
    // `--default ""`: an unset key answers 0 with nothing, not exit 1 — a "failed"
    // read in the journal would be noise next to real failures.
    let storage = exec::git(
        repo,
        &[
            "config",
            "--type=path",
            "--default",
            "",
            "--get",
            "lfs.storage",
        ],
    )
    .run()
    .ok()
    .filter(|o| o.success())
    .map(|o| o.stdout_text().trim().to_string())
    .unwrap_or_default();
    let base = if storage.is_empty() {
        common.join("lfs")
    } else {
        let s = PathBuf::from(storage);
        if s.is_absolute() {
            s
        } else {
            common.join(s)
        }
    };
    Some(base.join("objects"))
}

/// The object is in the local store, whole: a file of exactly the pointer's size
/// (a download interrupted halfway is not "downloaded").
pub(crate) fn has_object(objects: &Path, p: &Pointer) -> bool {
    std::fs::metadata(objects.join(&p.oid[0..2]).join(&p.oid[2..4]).join(&p.oid))
        .is_ok_and(|m| m.is_file() && m.len() == p.size)
}

/// Where `git lfs` would find its program: git's exec path first, then `PATH` —
/// git's own order for a `git-<name>` command. Looked up rather than run, so a
/// machine without git-lfs leaves no failed command in the journal for every LFS
/// diff it shows.
pub(crate) fn find_program(exec_path: Option<&Path>, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["git-lfs.exe", "git-lfs.cmd", "git-lfs.bat"]
    } else {
        &["git-lfs"]
    };
    let dirs = exec_path.map(Path::to_path_buf).into_iter().chain(
        path_var
            .map(|v| std::env::split_paths(v).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in names {
            let candidate = dir.join(name);
            if runnable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn runnable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn runnable(p: &Path) -> bool {
    p.is_file()
}

/// Is `git-lfs` there for this repository's git to run?
pub(crate) fn installed(repo: &Path) -> bool {
    let exec_path = exec::git(repo, &["--exec-path"])
        .run()
        .ok()
        .filter(|o| o.success())
        .map(|o| PathBuf::from(o.stdout_text().trim()));
    find_program(exec_path.as_deref(), std::env::var_os("PATH").as_deref()).is_some()
}

/// The path as a `--include` pattern, or the reason it cannot be one.
///
/// `--include` has no literal mode: it is a comma-separated list of gitignore-style
/// patterns (git-lfs `wildmatch`), whose ends are trimmed. A comma splits the path
/// in two; `* ? [ ]` are wildcards; `\` is an escape for some characters and a
/// Windows separator for the rest; a leading `!` or `#` means something in
/// gitignore. Rather than an escaping scheme that differs between git-lfs versions,
/// such a path is refused and the reader told to use the terminal. A path without
/// a slash matches the same name in subdirectories too — downloading a few more
/// files than asked is harmless, a pattern that matches nothing is not.
pub fn include_pattern(path: &str) -> Result<String> {
    let refused = path.is_empty()
        || path.contains([',', '*', '?', '[', ']', '\\'])
        || path.starts_with(['!', '#'])
        || path.chars().any(char::is_control)
        || path.trim() != path;
    if refused {
        return Err(Error::Rule(format!(
            "git lfs pull --include cannot name \"{path}\" literally (commas, wildcards, \
             backslashes and surrounding spaces are pattern syntax there); download it \
             from a terminal"
        )));
    }
    Ok(path.to_string())
}

/// Recognise an LFS pointer change in the diff `d` was parsed from and attach the
/// card's facts to it. Called by every producer of a `FileDiff` — the working tree,
/// a commit, a comparison — so the three can never disagree about what a pointer is.
pub(crate) fn attach(repo: &Path, d: &mut FileDiff, raw: &str) {
    attach_with(repo, d, raw, &|| installed(repo));
}

fn attach_with(repo: &Path, d: &mut FileDiff, raw: &str, installed: &dyn Fn() -> bool) {
    if d.binary {
        return;
    }
    let Some((before, after)) = pointer_sides(raw) else {
        return;
    };
    let objects = object_dir(repo);
    let side = |p: Pointer| LfsSide {
        downloaded: objects.as_deref().is_some_and(|o| has_object(o, &p)),
        oid: p.oid,
        size: p.size,
    };
    let old = before.map(&side);
    let new = after.map(&side);
    let download = download_state(repo, &d.path, new.as_ref(), installed);
    d.lfs = Some(LfsDiff { old, new, download });
}

/// Whether the card may offer "Download", and why not when it may not. Ordered
/// from the cheapest check to the ones that ask git.
fn download_state(
    repo: &Path,
    path: &str,
    new: Option<&LfsSide>,
    installed: &dyn Fn() -> bool,
) -> LfsDownload {
    match new {
        None => return LfsDownload::NotNeeded,
        Some(n) if n.downloaded => return LfsDownload::NotNeeded,
        Some(_) => {}
    }
    if !installed() {
        return LfsDownload::NoLfs;
    }
    if include_pattern(path).is_err() {
        return LfsDownload::UnsafePath;
    }
    let oid = &new.expect("checked above").oid;
    if !checked_out(repo, path, oid) {
        return LfsDownload::NotCheckedOut;
    }
    LfsDownload::Available
}

/// Would `git lfs pull --include=<path>` fetch object `oid`? The path carries
/// `filter=lfs` and its index entry (stage 0) is the pointer to exactly that oid.
fn checked_out(repo: &Path, path: &str, oid: &str) -> bool {
    let attr = exec::git(repo, &["check-attr", "-z", "filter", "--", path])
        .run()
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout_text())
        .unwrap_or_default();
    // `<path>\0filter\0<value>\0`
    if attr.split('\0').nth(2) != Some("lfs") {
        return false;
    }
    // `:0:<path>` — the explicit stage, so a path like `1:x` is not read as stage 1.
    let spec = format!(":0:{path}");
    exec::git(repo, &["cat-file", "blob", "--end-of-options", &spec])
        .run()
        .ok()
        .filter(|o| o.success() && o.stdout.len() < POINTER_LIMIT)
        .and_then(|o| parse_pointer(&o.stdout_text()))
        .is_some_and(|p| p.oid == oid)
}

/// Download the LFS content of one checked-out file: `git lfs pull --include`,
/// in network mode. Changes only that file in the working tree (pointer → content);
/// the index, `HEAD` and refs stay. Refused as a rule when the path cannot be a
/// literal pattern ([`include_pattern`]) or git-lfs is not there.
pub fn pull(repo: &Path, path: &str) -> Result<()> {
    let pattern = include_pattern(path)?;
    if !installed(repo) {
        return Err(Error::Rule(
            "git-lfs is not installed, so LFS content cannot be downloaded".into(),
        ));
    }
    let include = format!("--include={pattern}");
    exec::git(repo, &["lfs", "pull", &include])
        .network()
        .run()?
        .checked_both()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::{run_git, scratch_repo};
    use crate::engine::commit;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";
    const C: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn pointer(oid: &str, size: u64) -> String {
        format!("version {SPEC_V1}\noid sha256:{oid}\nsize {size}\n")
    }

    // ---- parse_pointer ----

    #[test]
    fn a_pointer_git_lfs_writes_parses() {
        assert_eq!(
            parse_pointer(&pointer(B, 12345)),
            Some(Pointer {
                oid: B.into(),
                size: 12345
            })
        );
        let hawser = format!("version {SPEC_HAWSER}\noid sha256:{B}\nsize 1\n");
        assert_eq!(parse_pointer(&hawser).map(|p| p.size), Some(1));
        let ext = format!("version {SPEC_V1}\next-0-foo sha256:{A}\noid sha256:{B}\nsize 7\n");
        assert_eq!(parse_pointer(&ext).map(|p| p.oid), Some(B.to_string()));
    }

    #[test]
    fn a_broken_pointer_is_text() {
        let broken = [
            // wrong or missing version
            format!("version https://git-lfs.github.com/spec/v2\noid sha256:{B}\nsize 1\n"),
            format!("oid sha256:{B}\nsize 1\n"),
            // oid: uppercase, short, other hash
            format!(
                "version {SPEC_V1}\noid sha256:{}\nsize 1\n",
                B.to_uppercase()
            ),
            format!("version {SPEC_V1}\noid sha256:{}\nsize 1\n", &B[1..]),
            format!("version {SPEC_V1}\noid sha1:{B}\nsize 1\n"),
            // missing a key
            format!("version {SPEC_V1}\noid sha256:{B}\n"),
            format!("version {SPEC_V1}\nsize 1\n"),
            // size: signed, leading zero, zero, letters, empty
            format!("version {SPEC_V1}\noid sha256:{B}\nsize +5\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize -1\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize 05\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize 0\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize 1k\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize \n"),
            String::new(),
        ];
        for text in &broken {
            assert_eq!(parse_pointer(text), None, "{text:?}");
        }
    }

    #[test]
    fn an_almost_pointer_is_text() {
        let almost = [
            // no final newline, CRLF, two spaces, trailing space
            pointer(B, 1).trim_end().to_string(),
            pointer(B, 1).replace('\n', "\r\n"),
            format!("version {SPEC_V1}\noid  sha256:{B}\nsize 1\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize 1 \n"),
            // unsorted, duplicated, unknown key, uppercase key, blank line
            format!("version {SPEC_V1}\nsize 1\noid sha256:{B}\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\noid sha256:{B}\nsize 1\n"),
            format!("version {SPEC_V1}\noid sha256:{B}\nsize 1\nzzz 1\n"),
            format!("version {SPEC_V1}\nOid sha256:{B}\nsize 1\n"),
            format!("version {SPEC_V1}\n\noid sha256:{B}\nsize 1\n"),
            // text before the pointer
            format!("hello\n{}", pointer(B, 1)),
            // 1024 bytes or more
            format!(
                "version {SPEC_V1}\next-0-{} sha256:{A}\noid sha256:{B}\nsize 1\n",
                "a".repeat(1000)
            ),
        ];
        for text in &almost {
            assert_eq!(parse_pointer(text), None, "{text:?}");
        }
    }

    // ---- pointer_sides ----

    fn patch(old: &str, new: &str, body: &str) -> String {
        format!("diff --git a/f b/f\nindex 1..2 100644\n--- {old}\n+++ {new}\n{body}")
    }

    #[test]
    fn sides_of_a_replaced_added_and_deleted_pointer() {
        let replaced = patch(
            "a/f",
            "b/f",
            &format!(
                "@@ -1,3 +1,3 @@\n version {SPEC_V1}\n-oid sha256:{A}\n-size 10\n+oid sha256:{B}\n+size 20\n"
            ),
        );
        let (b, a) = pointer_sides(&replaced).unwrap();
        assert_eq!(b.unwrap().oid, A);
        assert_eq!(a.unwrap().size, 20);

        let added = patch(
            "/dev/null",
            "b/f",
            &format!("@@ -0,0 +1,3 @@\n+version {SPEC_V1}\n+oid sha256:{B}\n+size 20\n"),
        );
        assert!(matches!(pointer_sides(&added), Some((None, Some(_)))));

        let deleted = patch(
            "a/f",
            "/dev/null",
            &format!("@@ -1,3 +0,0 @@\n-version {SPEC_V1}\n-oid sha256:{B}\n-size 20\n"),
        );
        assert!(matches!(pointer_sides(&deleted), Some((Some(_), None))));
    }

    #[test]
    fn a_patch_that_does_not_prove_a_whole_pointer_has_no_sides() {
        // No newline at the end of the new side: not a pointer.
        let no_eol = patch(
            "a/f",
            "b/f",
            &format!(
                "@@ -1,3 +1,3 @@\n version {SPEC_V1}\n-oid sha256:{A}\n-size 10\n+oid sha256:{B}\n+size 20\n\\ No newline at end of file\n"
            ),
        );
        assert_eq!(pointer_sides(&no_eol), None);
        // A hunk that does not start at line 1.
        let partial = patch(
            "a/f",
            "b/f",
            &format!("@@ -2,2 +2,2 @@\n-oid sha256:{A}\n-size 10\n+oid sha256:{B}\n+size 20\n"),
        );
        assert_eq!(pointer_sides(&partial), None);
        // Pointer replaced by real text.
        let to_text = patch(
            "a/f",
            "b/f",
            &format!("@@ -1,3 +1 @@\n-version {SPEC_V1}\n-oid sha256:{A}\n-size 10\n+hello\n"),
        );
        assert_eq!(pointer_sides(&to_text), None);
        // A document that quotes a pointer among other lines.
        let doc = patch(
            "a/f",
            "b/f",
            &format!("@@ -1,2 +1,3 @@\n # LFS\n+version {SPEC_V1}\n see above\n"),
        );
        assert_eq!(pointer_sides(&doc), None);
        // Two hunks.
        let two = format!("{replaced}@@ -9 +9 @@\n-x\n+y\n", replaced = patch(
            "a/f",
            "b/f",
            &format!("@@ -1,3 +1,3 @@\n version {SPEC_V1}\n-oid sha256:{A}\n-size 10\n+oid sha256:{B}\n+size 20\n"),
        ));
        assert_eq!(pointer_sides(&two), None);
        // A header that lies about the count.
        let short = patch(
            "a/f",
            "b/f",
            &format!("@@ -1,4 +1,3 @@\n version {SPEC_V1}\n-oid sha256:{A}\n-size 10\n+oid sha256:{B}\n+size 20\n"),
        );
        assert_eq!(pointer_sides(&short), None);
    }

    // ---- include_pattern ----

    #[test]
    fn only_a_path_the_include_list_reads_literally_is_passed() {
        for ok in [
            "big.bin",
            "assets/big file.psd",
            "a/b-c_d.e",
            "ünï/code.bin",
        ] {
            assert_eq!(include_pattern(ok).unwrap(), ok);
        }
        for bad in [
            "a,b.bin",
            "*.bin",
            "a?.bin",
            "a[1].bin",
            "a]b",
            "dir\\f.bin",
            "!neg",
            "#c",
            " lead",
            "trail ",
            "new\nline",
            "",
        ] {
            assert!(
                matches!(include_pattern(bad), Err(Error::Rule(_))),
                "{bad:?}"
            );
        }
    }

    // ---- find_program ----

    #[cfg(unix)]
    #[test]
    fn git_lfs_is_found_in_the_exec_path_or_on_path_and_only_if_runnable() {
        use std::os::unix::fs::PermissionsExt;
        let exec_dir = tempfile::tempdir().unwrap();
        let path_dir = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let tool = path_dir.path().join("git-lfs");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path_var = std::env::join_paths([empty.path(), path_dir.path()]).unwrap();
        // Present but not executable: git could not run it either.
        assert_eq!(find_program(Some(exec_dir.path()), Some(&path_var)), None);
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            find_program(Some(exec_dir.path()), Some(&path_var)),
            Some(tool.clone())
        );
        // The exec path wins over PATH.
        let first = exec_dir.path().join("git-lfs");
        std::fs::write(&first, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            find_program(Some(exec_dir.path()), Some(&path_var)),
            Some(first)
        );
        assert_eq!(find_program(None, None), None);
    }

    // ---- the card on real diffs ----

    /// A repository with `big.bin` committed as pointer A (size 10), then as
    /// pointer B (size 12); `.gitattributes` marks `*.bin` for LFS. No git-lfs is
    /// needed: a pointer is just the text of the file.
    fn lfs_repo() -> (tempfile::TempDir, String, String) {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join(".gitattributes"), "*.bin filter=lfs -text\n").unwrap();
        std::fs::write(p.join("big.bin"), pointer(A, 10)).unwrap();
        run_git(p, &["add", ".gitattributes", "big.bin"]);
        run_git(p, &["commit", "-q", "-m", "a"]);
        let first = head(p);
        std::fs::write(p.join("big.bin"), pointer(B, 12)).unwrap();
        run_git(p, &["commit", "-q", "-am", "b"]);
        let second = head(p);
        (dir, first, second)
    }

    fn head(p: &Path) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn store(p: &Path, oid: &str, bytes: usize) {
        let dir = p.join(".git/lfs/objects").join(&oid[0..2]).join(&oid[2..4]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(oid), vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn a_commit_a_comparison_and_the_working_tree_get_the_card() {
        let (dir, first, second) = lfs_repo();
        let p = dir.path();

        let d = commit::file_diff(p, &second, "big.bin", None, "none", None).unwrap();
        let lfs = d.lfs.expect("commit diff has a card");
        let (old, new) = (lfs.old.unwrap(), lfs.new.unwrap());
        assert_eq!((old.oid.as_str(), old.size, old.downloaded), (A, 10, false));
        assert_eq!((new.oid.as_str(), new.size, new.downloaded), (B, 12, false));
        // Whether git-lfs is on this machine decides between the two; neither says
        // "nothing to download".
        assert!(matches!(
            lfs.download,
            LfsDownload::NoLfs | LfsDownload::Available
        ));

        let root = commit::file_diff(p, &first, "big.bin", None, "none", None).unwrap();
        let lfs = root.lfs.expect("root commit adds the pointer");
        assert!(lfs.old.is_none());
        assert_eq!(lfs.new.unwrap().oid, A);

        let c = commit::compare_diff(p, &first, &second, "big.bin", "none", None).unwrap();
        assert_eq!(c.lfs.unwrap().new.unwrap().oid, B);

        std::fs::write(p.join("big.bin"), pointer(C, 5)).unwrap();
        let w = CliEngine::new(p)
            .diff_file("big.bin", "worktree", "none", None)
            .unwrap();
        let lfs = w.lfs.expect("worktree diff has a card");
        assert_eq!(lfs.old.unwrap().oid, B);
        assert_eq!(lfs.new.unwrap().oid, C);
        // Compared with the working tree, too.
        let c = commit::compare_diff(p, &second, "", "big.bin", "none", None).unwrap();
        assert_eq!(c.lfs.unwrap().new.unwrap().oid, C);
    }

    #[test]
    fn an_object_in_the_store_reads_as_downloaded_and_needs_no_button() {
        let (dir, _, second) = lfs_repo();
        let p = dir.path();
        // Wrong size first: an interrupted download is not a download.
        store(p, B, 11);
        let d = commit::file_diff(p, &second, "big.bin", None, "none", None).unwrap();
        assert!(!d.lfs.unwrap().new.unwrap().downloaded);
        store(p, B, 12);
        let d = commit::file_diff(p, &second, "big.bin", None, "none", None).unwrap();
        let lfs = d.lfs.unwrap();
        assert!(lfs.new.unwrap().downloaded);
        assert!(!lfs.old.unwrap().downloaded);
        assert_eq!(lfs.download, LfsDownload::NotNeeded);
    }

    #[test]
    fn lfs_storage_moves_the_store() {
        let (dir, _, second) = lfs_repo();
        let p = dir.path();
        run_git(p, &["config", "lfs.storage", "elsewhere"]);
        store(p, B, 12); // the default place no longer counts
        let d = commit::file_diff(p, &second, "big.bin", None, "none", None).unwrap();
        assert!(!d.lfs.unwrap().new.unwrap().downloaded);
        let moved = p
            .join(".git/elsewhere/objects")
            .join(&B[0..2])
            .join(&B[2..4]);
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::write(moved.join(B), vec![b'x'; 12]).unwrap();
        let d = commit::file_diff(p, &second, "big.bin", None, "none", None).unwrap();
        assert!(d.lfs.unwrap().new.unwrap().downloaded);
    }

    #[test]
    fn a_linked_worktree_reads_the_shared_store() {
        let (dir, _, second) = lfs_repo();
        let p = dir.path();
        store(p, B, 12);
        let linked = tempfile::tempdir().unwrap();
        let wt = linked.path().join("wt");
        // `--no-checkout`: with git-lfs installed globally, checking out `big.bin`
        // would run the smudge filter, which tries to download B and aborts.
        run_git(
            p,
            &["worktree", "add", "-q", "--no-checkout", "--detach", wt.to_str().unwrap()],
        );
        let d = commit::file_diff(&wt, &second, "big.bin", None, "none", None).unwrap();
        assert!(d.lfs.unwrap().new.unwrap().downloaded);
    }

    #[test]
    fn without_git_lfs_there_is_no_button() {
        let (dir, _, second) = lfs_repo();
        let p = dir.path();
        let raw = |from: &str| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(p)
                .args(["diff", from, &second, "--", "big.bin"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let text = raw(&format!("{second}~1"));
        let card = |installed: bool| {
            let mut d = crate::engine::cli::parse_diff("big.bin", &text);
            attach_with(p, &mut d, &text, &|| installed);
            d.lfs.unwrap().download
        };
        assert_eq!(card(false), LfsDownload::NoLfs);
        // With git-lfs, the index holds pointer B and `*.bin` is `filter=lfs`:
        // `git lfs pull --include=big.bin` fetches exactly this object.
        assert_eq!(card(true), LfsDownload::Available);

        // An older version is not what `pull` fetches.
        std::fs::write(p.join("big.bin"), pointer(A, 10)).unwrap();
        run_git(p, &["commit", "-q", "-am", "back to a"]);
        assert_eq!(card(true), LfsDownload::NotCheckedOut);

        // Not marked for LFS: pull would skip it.
        std::fs::write(p.join(".gitattributes"), "").unwrap();
        std::fs::write(p.join("big.bin"), pointer(B, 12)).unwrap();
        run_git(p, &["commit", "-q", "-am", "b, no attribute"]);
        assert_eq!(card(true), LfsDownload::NotCheckedOut);
    }

    #[test]
    fn a_text_file_that_quotes_the_spec_gets_no_card() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("notes.md"), format!("See {SPEC_V1}\n")).unwrap();
        run_git(p, &["add", "notes.md"]);
        run_git(p, &["commit", "-q", "-m", "notes"]);
        let d = commit::file_diff(p, &head(p), "notes.md", None, "none", None).unwrap();
        assert!(d.lfs.is_none());
        assert!(!d.hunks.is_empty());
    }

    #[test]
    fn pull_refuses_a_path_it_cannot_name_before_running_anything() {
        let dir = scratch_repo();
        assert!(matches!(pull(dir.path(), "a,b.bin"), Err(Error::Rule(_))));
    }
}
