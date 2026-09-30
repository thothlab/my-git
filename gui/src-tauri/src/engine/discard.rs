//! Backups taken before a discard, and their restore.
//!
//! Rolling files back to HEAD and reverting a hunk are the everyday actions that
//! destroy work. Before either runs, the working-tree copies of the paths it touches
//! are written into git's object store as a commit under the hidden ref
//! [`DISCARD_REF`]; after it ran, the same paths are recorded again. Restore puts the
//! "before" copies back — but only over files that are still exactly as the discard
//! left them, which is what the "after" commit is for.
//!
//! ## Shape of the chain
//!
//! One discard is **two** commits: *before* (parent: the previous tip, or none) and
//! *after* (parent: always its *before*). Their trees hold the paths that existed on
//! either side, by content, mode and link-ness; a path absent from a tree did not exist
//! then. No path list is kept anywhere a machine reads: every path the discard changed
//! is in the union of the two trees, and `git diff-tree before after` names exactly
//! those. The message is for people reading `git log refs/graft/discard`; the only
//! machine-read parts are two fixed trailers, `Graft-Discard: before|after` and
//! `Graft-Kind: <DiscardKind>`.
//!
//! The chain holds at most [`CHAIN_LIMIT`] commits. The decision to start over is
//! taken for the *before* commit only (it becomes a root), so an *after* never lands
//! as the root of a new chain cut off from its *before*. The old chain becomes
//! unreachable and git may prune it later.
//!
//! ## What is recorded, and how
//!
//! Bytes exactly as they lie on disk: `hash-object --no-filters`, and restore writes
//! the blob back itself. Hashing through the filters (CRLF, LFS, a filter driver) and
//! restoring through git would round-trip; mixing the two would not, and writing raw
//! bytes back needs no pathspec at all. A symlink is recorded as a link (its target
//! text, mode `120000`) and never followed; a file's executable bit becomes `100755`.
//! An untracked directory as `git status` reports it (`dir/`) is expanded into the
//! files git would show under it.
//!
//! The tree is built in a throwaway index (`GIT_INDEX_FILE`), so the user's index and
//! working tree are never touched by a backup. The ref moves by `update-ref` with the
//! old value it was read at.
//!
//! ## Failure
//!
//! A backup that cannot be taken stops the discard: the caller's closure never runs.
//! If the *after* record fails once the discard has already happened, the discard's
//! own error (if any) wins, else the recording error is returned. The *before* commit
//! is then on the chain without a partner: the restore list skips it, and its content
//! is still in `git log -p refs/graft/discard`.
//!
//! The user's index is not restored. See `restore` for why.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::cli::{CliEngine, TempIndex, TMP_COUNTER};
use super::exec;
use crate::error::{Error, Result};
use crate::model::{DiscardEntry, DiscardKind};

/// The hidden ref the backups live under. `engine::log` excludes `refs/graft/*` from
/// `--all`, and the branch tree reads only `refs/heads` and `refs/remotes`.
pub const DISCARD_REF: &str = "refs/graft/discard";

/// Commits one chain holds — a hundred discards — before the next backup starts a
/// new one.
pub const CHAIN_LIMIT: usize = 200;

/// Who the backup commits are by: the application, never the user's identity (which
/// may be unset, and whose commits the log emphasises as "mine").
const IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Graft"),
    ("GIT_AUTHOR_EMAIL", "graft@localhost"),
    ("GIT_COMMITTER_NAME", "Graft"),
    ("GIT_COMMITTER_EMAIL", "graft@localhost"),
];

const ROLE_TRAILER: &str = "Graft-Discard";
const KIND_TRAILER: &str = "Graft-Kind";

/// How many paths go on one `hash-object` command line.
const ARGS_CHUNK: usize = 200;

const MODE_FILE: &str = "100644";
const MODE_EXEC: &str = "100755";
const MODE_LINK: &str = "120000";

fn kind_name(kind: DiscardKind) -> &'static str {
    match kind {
        DiscardKind::Files => "files",
        DiscardKind::List => "list",
        DiscardKind::Hunk => "hunk",
        DiscardKind::Lines => "lines",
        DiscardKind::Restore => "restore",
    }
}

fn kind_of(name: &str) -> Option<DiscardKind> {
    Some(match name {
        "files" => DiscardKind::Files,
        "list" => DiscardKind::List,
        "hunk" => DiscardKind::Hunk,
        "lines" => DiscardKind::Lines,
        "restore" => DiscardKind::Restore,
        _ => return None,
    })
}

/// One path as it lies in the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Absent,
    Blob {
        mode: &'static str,
        oid: String,
    },
    /// A directory where a file is expected. Only reported when directories are not
    /// being expanded (the staleness check), and never equal to a recorded state.
    Dir,
}

// ── reading the working tree ────────────────────────────────────────────────

/// Link target text as bytes, the content git stores for a symlink.
fn link_bytes(path: &Path) -> Result<Vec<u8>> {
    let target = std::fs::read_link(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(target.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        Ok(target.to_string_lossy().replace('\\', "/").into_bytes())
    }
}

fn file_mode(meta: &std::fs::Metadata) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return MODE_EXEC;
        }
    }
    let _ = meta;
    MODE_FILE
}

/// Read `paths` from the working tree: absence, or content id and mode.
///
/// `write` stores the blobs (a backup); without it they are only hashed (the
/// staleness check). `expand` turns a directory into the untracked files under it
/// (a backup of `dir/`); without it a directory is reported as [`Entry::Dir`]. Every
/// path passes [`CliEngine::worktree_entry`] first, so nothing is read through a
/// symlinked directory that leads outside the repository.
fn read_paths(
    repo: &Path,
    paths: &[String],
    write: bool,
    expand: bool,
) -> Result<Vec<(String, Entry)>> {
    let eng = CliEngine::new(repo);
    let mut out: Vec<(String, Entry)> = Vec::new();
    let mut regular: Vec<(usize, &'static str)> = Vec::new();
    let mut queue: Vec<String> = paths.to_vec();
    queue.reverse();
    let mut seen = std::collections::HashSet::new();
    while let Some(rel) = queue.pop() {
        let rel = rel.trim_end_matches('/').to_string();
        if !seen.insert(rel.clone()) {
            continue;
        }
        let abs = eng.worktree_entry(&rel)?;
        let meta = match std::fs::symlink_metadata(&abs) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                out.push((rel, Entry::Absent));
                continue;
            }
            Err(e) => return Err(Error::Io(format!("{rel}: {e}"))),
        };
        if meta.file_type().is_symlink() {
            let bytes = link_bytes(&abs).map_err(|e| Error::Io(format!("{rel}: {e}")))?;
            let mut args = vec!["hash-object"];
            if write {
                args.push("-w");
            }
            args.push("--stdin");
            let oid = exec::git(repo, &args).input(&bytes).run()?.checked()?;
            let oid = String::from_utf8_lossy(&oid).trim().to_string();
            out.push((
                rel,
                Entry::Blob {
                    mode: MODE_LINK,
                    oid,
                },
            ));
        } else if meta.is_file() {
            regular.push((out.len(), file_mode(&meta)));
            out.push((rel, Entry::Absent)); // placeholder, filled below
        } else if meta.is_dir() {
            if expand {
                // Pushed in reverse so they are read in the order git listed them.
                let mut inner = eng.untracked_under(&rel)?;
                inner.reverse();
                queue.extend(inner);
            } else {
                out.push((rel, Entry::Dir));
            }
        } else {
            return Err(Error::Rule(format!(
                "{rel} is not a regular file, a link or a folder, so it cannot be backed up"
            )));
        }
    }

    // Plain paths as arguments, not a pathspec: `hash-object` reads files, and after
    // `--` a name that starts with `-` or contains a newline is taken as it is.
    for chunk in regular.chunks(ARGS_CHUNK) {
        let mut args: Vec<&str> = vec!["hash-object"];
        if write {
            args.push("-w");
        }
        args.extend(["--no-filters", "--"]);
        args.extend(chunk.iter().map(|(i, _)| out[*i].0.as_str()));
        let text = String::from_utf8_lossy(&exec::git(repo, &args).run()?.checked()?).to_string();
        let oids: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if oids.len() != chunk.len() {
            return Err(Error::Parse(format!(
                "hash-object returned {} ids for {} files",
                oids.len(),
                chunk.len()
            )));
        }
        for ((i, mode), oid) in chunk.iter().zip(oids) {
            out[*i].1 = Entry::Blob {
                mode,
                oid: oid.to_string(),
            };
        }
    }
    Ok(out)
}

/// How each of `paths` lies in the working tree right now, one comparable string per
/// path: `-` absent, `dir` a folder, `<mode> <oid>` a file or link by its bytes.
///
/// The read [`with_backup`] takes, minus the writing: nothing goes into the object
/// store. It serves the undo journal's fingerprint (`engine::undo`), which only asks
/// "is this file still byte for byte what it was" — the very question this module's
/// staleness check asks, so it is answered by the same code.
pub(crate) fn describe(repo: &Path, paths: &[String]) -> Result<Vec<(String, String)>> {
    Ok(read_paths(repo, paths, false, false)?
        .into_iter()
        .map(|(p, e)| {
            let text = match e {
                Entry::Absent => "-".to_string(),
                Entry::Dir => "dir".to_string(),
                Entry::Blob { mode, oid } => format!("{mode} {oid}"),
            };
            (p, text)
        })
        .collect())
}

// ── writing the chain ───────────────────────────────────────────────────────

/// The tree of the present entries, built in a throwaway index.
fn write_tree(repo: &Path, entries: &[(String, Entry)]) -> Result<String> {
    let index = TempIndex::new(repo, "graft-discard")?;
    let index_path = index.env_value();
    let env = [("GIT_INDEX_FILE", index_path.as_str())];
    let mut records = Vec::new();
    for (path, e) in entries {
        if let Entry::Blob { mode, oid } = e {
            records.extend_from_slice(format!("{mode} {oid}\t").as_bytes());
            records.extend_from_slice(path.as_bytes());
            records.push(0);
        }
    }
    if !records.is_empty() {
        exec::git(repo, &["update-index", "--add", "-z", "--index-info"])
            .env(&env)
            .input(&records)
            .run()?
            .checked()?;
    }
    let tree = exec::git(repo, &["write-tree"])
        .env(&env)
        .run()?
        .checked()?;
    Ok(String::from_utf8_lossy(&tree).trim().to_string())
}

/// The chain's tip, or `None` when there is no chain yet.
fn tip(repo: &Path) -> Result<Option<String>> {
    let out = exec::git(repo, &["rev-parse", "--verify", "--quiet", DISCARD_REF]).run()?;
    match out.code {
        Some(0) => Ok(Some(out.stdout_text().trim().to_string())),
        Some(1) => Ok(None),
        _ => Err(out.fail_stderr()),
    }
}

fn chain_length(repo: &Path) -> Result<usize> {
    let out = exec::git(repo, &["rev-list", "--count", DISCARD_REF])
        .run()?
        .checked()?;
    let text = String::from_utf8_lossy(&out);
    text.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("rev-list --count said {:?}", text.trim())))
}

/// A path as the message shows it: verbatim unless it holds a control character (a
/// newline in a name would start a new paragraph and could fake a trailer).
fn shown(path: &str) -> String {
    if path.chars().any(char::is_control) {
        path.escape_debug().to_string()
    } else {
        path.to_string()
    }
}

fn message(before: bool, kind: DiscardKind, paths: &[String]) -> String {
    let n = paths.len();
    let files = if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    };
    let what = match kind {
        DiscardKind::Files | DiscardKind::List => format!("rolling back {files}"),
        DiscardKind::Hunk => "reverting a hunk".to_string(),
        DiscardKind::Lines => "reverting chosen lines".to_string(),
        DiscardKind::Restore => format!("restoring {files} from a backup"),
    };
    let subject = if before {
        format!("Graft: backup before {what}")
    } else {
        format!("Graft: state after {what}")
    };
    let list: Vec<String> = paths.iter().map(|p| shown(p)).collect();
    format!(
        "{subject}\n\n{}\n\n{ROLE_TRAILER}: {}\n{KIND_TRAILER}: {}\n",
        list.join("\n"),
        if before { "before" } else { "after" },
        kind_name(kind)
    )
}

fn commit(repo: &Path, tree: &str, parent: Option<&str>, message: &str) -> Result<String> {
    let mut args = vec!["commit-tree", "--no-gpg-sign", tree];
    if let Some(p) = parent {
        args.extend(["-p", p]);
    }
    args.extend(["-F", "-"]);
    let out = exec::git(repo, &args)
        .env(IDENTITY)
        .input(message.as_bytes())
        .run()?
        .checked()?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Move the ref to `new`, but only from `old` (`None`: only if it does not exist).
fn move_ref(repo: &Path, new: &str, old: Option<&str>, reason: &str) -> Result<()> {
    exec::git(
        repo,
        &[
            "update-ref",
            "-m",
            reason,
            DISCARD_REF,
            new,
            old.unwrap_or(""),
        ],
    )
    .run()?
    .checked()?;
    Ok(())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Back up `paths`, run the discard, record what it left.
///
/// Returns the backup when the discard changed anything on disk, `None` when it did
/// not (or when there were no paths). A backup that cannot be taken is an error and
/// `discard` is **not** run.
pub fn with_backup(
    repo: &Path,
    kind: DiscardKind,
    paths: &[String],
    discard: impl FnOnce() -> Result<()>,
) -> Result<Option<DiscardEntry>> {
    with_backup_limit(repo, kind, paths, discard, CHAIN_LIMIT)
}

fn with_backup_limit(
    repo: &Path,
    kind: DiscardKind,
    paths: &[String],
    discard: impl FnOnce() -> Result<()>,
    limit: usize,
) -> Result<Option<DiscardEntry>> {
    if paths.is_empty() {
        discard()?;
        return Ok(None);
    }
    let before_state = read_paths(repo, paths, true, true)?;
    let files: Vec<String> = before_state.iter().map(|(p, _)| p.clone()).collect();
    let old_tip = tip(repo)?;
    let parent = match &old_tip {
        Some(t) if chain_length(repo)? + 2 <= limit => Some(t.as_str()),
        _ => None,
    };
    let msg = message(true, kind, &files);
    let before = commit(repo, &write_tree(repo, &before_state)?, parent, &msg)?;
    move_ref(
        repo,
        &before,
        old_tip.as_deref(),
        msg.lines().next().unwrap_or(""),
    )?;

    let outcome = discard();

    let recorded = (|| -> Result<(String, Vec<(String, Entry)>)> {
        let after_state = read_paths(repo, &files, true, true)?;
        let msg = message(false, kind, &files);
        let after = commit(repo, &write_tree(repo, &after_state)?, Some(&before), &msg)?;
        move_ref(
            repo,
            &after,
            Some(&before),
            msg.lines().next().unwrap_or(""),
        )?;
        Ok((after, after_state))
    })();
    outcome?;
    let (after, after_state) = recorded?;

    let was: HashMap<&str, &Entry> = before_state.iter().map(|(p, e)| (p.as_str(), e)).collect();
    let mut changed: Vec<String> = Vec::new();
    for (p, e) in &after_state {
        if was.get(p.as_str()) != Some(&e) {
            changed.push(p.clone());
        }
    }
    if changed.is_empty() {
        return Ok(None);
    }
    changed.sort();
    Ok(Some(DiscardEntry {
        id: after,
        at: now(),
        kind,
        paths: changed,
    }))
}

// ── reading the chain ───────────────────────────────────────────────────────

/// One commit of the chain as `git log` reports it.
struct Record {
    hash: String,
    parents: String,
    at: i64,
    role: String,
    kind: String,
    /// Paths changed against the first parent.
    paths: Vec<String>,
}

/// Every commit of the current chain, newest first.
fn records(repo: &Path) -> Result<Vec<Record>> {
    if tip(repo)?.is_none() {
        return Ok(Vec::new());
    }
    let format = format!(
        "--format=%x01%H%x00%P%x00%ct%x00%(trailers:key={ROLE_TRAILER},valueonly,separator=%x2C)%x00%(trailers:key={KIND_TRAILER},valueonly,separator=%x2C)"
    );
    let out = exec::git(
        repo,
        &[
            "log",
            "-z",
            "--no-color",
            "--no-renames",
            "--name-status",
            &format,
            DISCARD_REF,
            "--",
        ],
    )
    .run()?
    .checked()?;
    let text = String::from_utf8_lossy(&out).to_string();
    parse_records(&text)
}

/// Parse `records`' output. Fields are NUL-separated and a record opens with `\x01`;
/// the paths that follow come in status/path pairs. A record or a pair that does not
/// fit is an error, not a shorter list.
fn parse_records(text: &str) -> Result<Vec<Record>> {
    let tokens: Vec<&str> = text.split('\0').collect();
    let mut out: Vec<Record> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].trim_start_matches('\n');
        if t.is_empty() {
            i += 1;
            continue;
        }
        if let Some(hash) = t.strip_prefix('\u{1}') {
            if i + 4 >= tokens.len() {
                return Err(Error::Parse(format!("truncated backup record {hash}")));
            }
            let at = tokens[i + 2]
                .trim()
                .parse()
                .map_err(|_| Error::Parse(format!("bad time in backup record {hash}")))?;
            out.push(Record {
                hash: hash.to_string(),
                parents: tokens[i + 1].to_string(),
                at,
                role: tokens[i + 3].trim().to_string(),
                kind: tokens[i + 4].trim().to_string(),
                paths: Vec::new(),
            });
            i += 5;
            continue;
        }
        let rec = out
            .last_mut()
            .ok_or_else(|| Error::Parse("backup log starts without a record".into()))?;
        if !matches!(t, "A" | "M" | "D" | "T") {
            return Err(Error::Parse(format!(
                "unexpected status {t:?} in backup {}",
                rec.hash
            )));
        }
        let path = tokens
            .get(i + 1)
            .ok_or_else(|| Error::Parse(format!("status without a path in backup {}", rec.hash)))?;
        rec.paths.push(path.to_string());
        i += 2;
    }
    Ok(out)
}

/// A restorable backup and the commit it restores from.
struct Pair {
    before: String,
    entry: DiscardEntry,
}

fn pairs(repo: &Path) -> Result<Vec<Pair>> {
    let recs = records(repo)?;
    let mut out = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        if r.role != "after" || r.paths.is_empty() {
            continue;
        }
        let Some(prev) = recs.get(i + 1) else {
            continue;
        };
        if prev.role != "before" || r.parents != prev.hash {
            continue;
        }
        let Some(kind) = kind_of(&r.kind) else {
            continue;
        };
        out.push(Pair {
            before: prev.hash.clone(),
            entry: DiscardEntry {
                id: r.hash.clone(),
                at: r.at,
                kind,
                paths: r.paths.clone(),
            },
        });
    }
    Ok(out)
}

/// The newest `limit` restorable backups, newest first.
pub fn list(repo: &Path, limit: usize) -> Result<Vec<DiscardEntry>> {
    Ok(pairs(repo)?
        .into_iter()
        .take(limit)
        .map(|p| p.entry)
        .collect())
}

fn find(repo: &Path, id: &str) -> Result<Pair> {
    pairs(repo)?
        .into_iter()
        .find(|p| p.entry.id == id)
        .ok_or_else(|| {
            let short: String = id.chars().take(12).collect();
            Error::Rule(format!("backup {short} is no longer in {DISCARD_REF}"))
        })
}

/// One side of a path in `diff-tree`: absent (all-zero mode), or content and mode.
fn side(mode: &str, oid: &str) -> Result<Entry> {
    Ok(match mode {
        "000000" => Entry::Absent,
        MODE_FILE => Entry::Blob {
            mode: MODE_FILE,
            oid: oid.to_string(),
        },
        MODE_EXEC => Entry::Blob {
            mode: MODE_EXEC,
            oid: oid.to_string(),
        },
        MODE_LINK => Entry::Blob {
            mode: MODE_LINK,
            oid: oid.to_string(),
        },
        other => return Err(Error::Parse(format!("unexpected mode {other} in a backup"))),
    })
}

/// Each changed path with its (before, after) sides.
fn changes(repo: &Path, pair: &Pair) -> Result<Vec<(String, Entry, Entry)>> {
    let out = exec::git(
        repo,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            &pair.before,
            &pair.entry.id,
        ],
    )
    .run()?
    .checked()?;
    let text = String::from_utf8_lossy(&out).to_string();
    let tokens: Vec<&str> = text.split('\0').collect();
    let mut v = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if t.is_empty() {
            i += 1;
            continue;
        }
        // ":<src mode> <dst mode> <src oid> <dst oid> <status>", then the path.
        let f: Vec<&str> = t.trim_start_matches(':').split(' ').collect();
        let path = tokens.get(i + 1).filter(|p| !p.is_empty());
        let (Some(path), 5) = (path, f.len()) else {
            return Err(Error::Parse(format!("unexpected diff-tree record {t:?}")));
        };
        v.push((path.to_string(), side(f[0], f[2])?, side(f[1], f[3])?));
        i += 2;
    }
    Ok(v)
}

/// Paths of this backup that no longer are as the discard left them. Restoring over
/// them would lose what was done since.
fn stale_of(repo: &Path, changes: &[(String, Entry, Entry)]) -> Result<Vec<String>> {
    let paths: Vec<String> = changes.iter().map(|c| c.0.clone()).collect();
    let now = read_paths(repo, &paths, false, false)?;
    let now: HashMap<&str, &Entry> = now.iter().map(|(p, e)| (p.as_str(), e)).collect();
    Ok(changes
        .iter()
        .filter(|(p, _, after)| now.get(p.as_str()) != Some(&after))
        .map(|c| c.0.clone())
        .collect())
}

/// Paths of backup `id` changed since the discard — empty when a restore is safe.
pub fn stale_paths(repo: &Path, id: &str) -> Result<Vec<String>> {
    let pair = find(repo, id)?;
    stale_of(repo, &changes(repo, &pair)?)
}

/// Blob contents by id, in one `cat-file --batch`.
fn blobs(repo: &Path, oids: &[&str]) -> Result<HashMap<String, Vec<u8>>> {
    let mut map = HashMap::new();
    if oids.is_empty() {
        return Ok(map);
    }
    let mut input = Vec::new();
    for o in oids {
        input.extend_from_slice(o.as_bytes());
        input.push(b'\n');
    }
    let out = exec::git(repo, &["cat-file", "--batch"])
        .input(&input)
        .run()?
        .checked()?;
    let mut rest = out.as_slice();
    while !rest.is_empty() {
        let nl = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| Error::Parse("cat-file --batch: header without newline".into()))?;
        let header = String::from_utf8_lossy(&rest[..nl]).to_string();
        let f: Vec<&str> = header.split(' ').collect();
        if f.len() != 3 || f[1] != "blob" {
            return Err(Error::Parse(format!("cat-file --batch: {header}")));
        }
        let size: usize = f[2]
            .parse()
            .map_err(|_| Error::Parse(format!("cat-file --batch: {header}")))?;
        let start = nl + 1;
        if rest.len() < start + size + 1 {
            return Err(Error::Parse(format!(
                "cat-file --batch: short blob {}",
                f[0]
            )));
        }
        map.insert(f[0].to_string(), rest[start..start + size].to_vec());
        rest = &rest[start + size + 1..];
    }
    Ok(map)
}

/// Put one path back as `entry` says: remove it, write a file, or make a link.
fn put(abs: &Path, rel: &str, entry: &Entry, bytes: &[u8]) -> Result<()> {
    let io = |e: std::io::Error| Error::Io(format!("{rel}: {e}"));
    let existing = std::fs::symlink_metadata(abs).ok();
    if existing.as_ref().is_some_and(|m| m.is_dir()) {
        return Err(Error::Rule(format!(
            "{rel} is a folder now; move it away to restore the file"
        )));
    }
    let Entry::Blob { mode, .. } = entry else {
        if existing.is_some() {
            std::fs::remove_file(abs).map_err(io)?;
        }
        return Ok(());
    };
    let dir = abs
        .parent()
        .ok_or_else(|| Error::Io(format!("{rel} has no parent directory")))?;
    std::fs::create_dir_all(dir).map_err(io)?;
    #[cfg(unix)]
    if *mode == MODE_LINK {
        use std::os::unix::ffi::OsStrExt;
        if existing.is_some() {
            std::fs::remove_file(abs).map_err(io)?;
        }
        return std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(bytes), abs).map_err(io);
    }
    // Written beside the target and renamed over it: `rename` replaces a link at the
    // path instead of writing through it, and a reader never sees half a file.
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.graft-restore.{}.{n}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&tmp) {
            let m = meta.permissions().mode();
            // Executable where readable, as git checks a 100755 file out.
            let m = if *mode == MODE_EXEC {
                m | ((m & 0o444) >> 2)
            } else {
                m & !0o111
            };
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, abs) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

/// Put the files of backup `id` back as they were before the discard.
///
/// Refused with `Error::Stale` (naming the paths) when any of them changed since the
/// discard, unless `force`. The restore is itself backed up first — a
/// [`DiscardKind::Restore`] entry — so a forced overwrite, like every restore, can be
/// undone from the same list.
///
/// **Only the working tree is restored, never the index.** A rollback to HEAD resets
/// both, but writing an old index state back would overwrite whatever was staged
/// since, with no "after" to check it against; the work itself is the content, and
/// staging it again is one action.
pub fn restore(repo: &Path, id: &str, force: bool) -> Result<Option<DiscardEntry>> {
    let pair = find(repo, id)?;
    let changes = changes(repo, &pair)?;
    let stale = stale_of(repo, &changes)?;
    if !stale.is_empty() && !force {
        return Err(Error::Stale(format!(
            "changed since the rollback: {}",
            stale.join(", ")
        )));
    }
    let oids: Vec<&str> = changes
        .iter()
        .filter_map(|(_, before, _)| match before {
            Entry::Blob { oid, .. } => Some(oid.as_str()),
            _ => None,
        })
        .collect();
    let contents = blobs(repo, &oids)?;
    let eng = CliEngine::new(repo);
    // Every path is re-checked before anything is written: a symlinked folder may
    // have been planted since the backup was taken.
    let targets: Vec<PathBuf> = changes
        .iter()
        .map(|(p, _, _)| eng.worktree_entry(p))
        .collect::<Result<_>>()?;
    let paths: Vec<String> = changes.iter().map(|c| c.0.clone()).collect();
    with_backup(repo, DiscardKind::Restore, &paths, || {
        for ((rel, before, _), abs) in changes.iter().zip(&targets) {
            let bytes = match before {
                Entry::Blob { oid, .. } => contents
                    .get(oid)
                    .map(Vec::as_slice)
                    .ok_or_else(|| Error::Parse(format!("blob {oid} of {rel} was not read")))?,
                _ => &[],
            };
            put(abs, rel, before, bytes)?;
        }
        Ok(())
    })
}

/// The files a patch touches, as git reads the patch (`apply --numstat -z`: no
/// quoting, and a rename names both sides). Nothing is applied.
pub fn patch_paths(repo: &Path, patch: &[u8]) -> Result<Vec<String>> {
    let out = exec::git(repo, &["apply", "--numstat", "-z", "-"])
        .input(patch)
        .run()?
        .checked()?;
    let text = String::from_utf8_lossy(&out).to_string();
    let tokens: Vec<&str> = text.split('\0').collect();
    let mut paths = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if t.is_empty() {
            i += 1;
            continue;
        }
        // "<added>\t<deleted>\t<path>", or "<added>\t<deleted>\t" then source and target.
        let f: Vec<&str> = t.splitn(3, '\t').collect();
        if f.len() != 3 {
            return Err(Error::Parse(format!(
                "unexpected apply --numstat record {t:?}"
            )));
        }
        if f[2].is_empty() {
            let (Some(a), Some(b)) = (tokens.get(i + 1), tokens.get(i + 2)) else {
                return Err(Error::Parse(
                    "apply --numstat: rename without its paths".into(),
                ));
            };
            paths.push(a.to_string());
            paths.push(b.to_string());
            i += 3;
        } else {
            paths.push(f[2].to_string());
            i += 1;
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::model::{AllLines, HunkPick, LinePick};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn read(p: &Path, f: &str) -> String {
        std::fs::read_to_string(p.join(f)).unwrap()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn rollback(p: &Path, paths: &[&str]) -> Option<DiscardEntry> {
        let paths = strings(paths);
        with_backup(p, DiscardKind::Files, &paths, || {
            CliEngine::new(p).rollback(&paths)
        })
        .unwrap()
    }

    #[test]
    fn modified_new_and_deleted_files_come_back_as_they_were() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("gone.txt"), "tracked\n").unwrap();
        git(p, &["add", "gone.txt"]);
        git(p, &["commit", "-m", "gone"]);

        std::fs::write(p.join("a.txt"), "changed\n").unwrap();
        std::fs::create_dir(p.join("new")).unwrap();
        std::fs::write(p.join("new/staged.txt"), "staged\n").unwrap();
        git(p, &["add", "new/staged.txt"]);
        std::fs::write(p.join("untracked.txt"), "loose\n").unwrap();
        std::fs::remove_file(p.join("gone.txt")).unwrap();

        let entry = rollback(p, &["a.txt", "new/staged.txt", "untracked.txt", "gone.txt"])
            .expect("the rollback changed files");
        assert_eq!(read(p, "a.txt"), "one\n");
        assert!(!p.join("new/staged.txt").exists());
        assert!(!p.join("untracked.txt").exists());
        assert_eq!(read(p, "gone.txt"), "tracked\n");
        let mut paths = entry.paths.clone();
        paths.sort();
        assert_eq!(
            paths,
            strings(&["a.txt", "gone.txt", "new/staged.txt", "untracked.txt"])
        );
        assert_eq!(entry.kind, DiscardKind::Files);
        let listed = list(p, 10).unwrap();
        assert_eq!(listed.len(), 1);
        let mut listed_paths = listed[0].paths.clone();
        listed_paths.sort();
        assert_eq!(
            (&listed[0].id, listed[0].kind, &listed_paths),
            (&entry.id, entry.kind, &paths)
        );

        let back = restore(p, &entry.id, false)
            .unwrap()
            .expect("the restore changed files");
        assert_eq!(back.kind, DiscardKind::Restore);
        assert_eq!(read(p, "a.txt"), "changed\n");
        assert_eq!(read(p, "new/staged.txt"), "staged\n");
        assert_eq!(read(p, "untracked.txt"), "loose\n");
        assert!(
            !p.join("gone.txt").exists(),
            "deleted again, as it was before the rollback"
        );
        // The index is not restored: the staged-new file comes back untracked.
        assert_eq!(git(p, &["ls-files", "--", "new"]), "");

        // A restore is itself undoable from the same list.
        assert_eq!(list(p, 10).unwrap()[0].id, back.id);
        restore(p, &back.id, false).unwrap();
        assert_eq!(read(p, "a.txt"), "one\n");
        assert!(!p.join("untracked.txt").exists());
    }

    #[test]
    #[cfg(unix)]
    fn modes_and_links_survive_the_round_trip() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(p.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink("a.txt", p.join("link")).unwrap();
        git(p, &["add", "run.sh", "link"]);
        git(p, &["commit", "-m", "exec and link"]);

        std::fs::write(p.join("run.sh"), "#!/bin/sh\necho mine\n").unwrap();
        std::fs::remove_file(p.join("link")).unwrap();
        symlink("elsewhere.txt", p.join("link")).unwrap();
        // A new link leading out of the repository: backed up as a link, never followed.
        symlink("/etc/hosts", p.join("out")).unwrap();

        let entry = rollback(p, &["run.sh", "link", "out"]).unwrap();
        assert_eq!(
            std::fs::read_link(p.join("link")).unwrap(),
            Path::new("a.txt")
        );
        assert!(std::fs::symlink_metadata(p.join("out")).is_err());

        restore(p, &entry.id, false).unwrap();
        assert_eq!(read(p, "run.sh"), "#!/bin/sh\necho mine\n");
        let mode = std::fs::metadata(p.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "still executable: {mode:o}");
        assert_eq!(
            std::fs::read_link(p.join("link")).unwrap(),
            Path::new("elsewhere.txt")
        );
        assert_eq!(
            std::fs::read_link(p.join("out")).unwrap(),
            Path::new("/etc/hosts")
        );
        assert_eq!(
            git(p, &["status", "--porcelain", "--", "run.sh"]),
            "M run.sh",
            "no mode change"
        );
    }

    #[test]
    fn a_reverted_hunk_comes_back() {
        let dir = scratch_repo();
        let p = dir.path();
        let base: String = (1..=10).map(|n| format!("line{n}\n")).collect();
        std::fs::write(p.join("f.txt"), &base).unwrap();
        git(p, &["add", "f.txt"]);
        git(p, &["commit", "-m", "f"]);
        let edited = base
            .replace("line1\n", "ONE\n")
            .replace("line10\n", "TEN\n");
        std::fs::write(p.join("f.txt"), &edited).unwrap();

        let eng = CliEngine::new(p);
        let diff = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        assert_eq!(diff.hunks.len(), 2);
        let picks = [HunkPick {
            hunk: 1,
            lines: LinePick::All(AllLines::All),
        }];
        let patch = eng
            .selection_patch("f.txt", "worktree", &picks, &diff.digest, None, true)
            .unwrap();
        let paths = patch_paths(p, &patch).unwrap();
        assert_eq!(paths, strings(&["f.txt"]));
        let entry = with_backup(p, DiscardKind::Hunk, &paths, || {
            eng.apply_patch(&patch, false, true)
        })
        .unwrap()
        .unwrap();
        assert!(read(p, "f.txt").contains("line10") && read(p, "f.txt").contains("ONE"));
        assert_eq!(entry.kind, DiscardKind::Hunk);

        restore(p, &entry.id, false).unwrap();
        assert_eq!(read(p, "f.txt"), edited);
    }

    /// Reverting single lines, as `lines_revert` does it: the working-tree diff in
    /// reverse. An unchosen addition stays in the file, an unchosen deletion stays
    /// deleted — and the backup puts the file back as it was.
    #[test]
    fn reverting_chosen_lines_touches_only_them_and_is_restored() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("f.txt"), "c1\nd1\nc2\n").unwrap();
        git(p, &["add", "f.txt"]);
        git(p, &["commit", "-m", "f"]);
        let edited = "c1\na1\na2\nc2\n";
        std::fs::write(p.join("f.txt"), edited).unwrap();

        let eng = CliEngine::new(p);
        let diff = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        // " c1", "-d1", "+a1", "+a2", " c2": revert `+a1` alone.
        let picks = [HunkPick {
            hunk: 0,
            lines: LinePick::Lines(vec![2]),
        }];
        let patch = eng
            .selection_patch("f.txt", "worktree", &picks, &diff.digest, None, true)
            .unwrap();
        let paths = patch_paths(p, &patch).unwrap();
        let entry = with_backup(p, DiscardKind::Lines, &paths, || {
            eng.apply_patch(&patch, false, true)
        })
        .unwrap()
        .unwrap();
        assert_eq!(read(p, "f.txt"), "c1\na2\nc2\n");
        assert_eq!(
            entry.kind,
            DiscardKind::Lines,
            "the kind survives the round trip"
        );
        assert_eq!(list(p, 10).unwrap()[0].kind, DiscardKind::Lines);

        // Now `-d1` alone: the deleted line comes back, `a2` stays.
        let diff = eng.diff_file("f.txt", "worktree", "none", None).unwrap();
        let picks = [HunkPick {
            hunk: 0,
            lines: LinePick::Lines(vec![1]),
        }];
        let patch = eng
            .selection_patch("f.txt", "worktree", &picks, &diff.digest, None, true)
            .unwrap();
        eng.apply_patch(&patch, false, true).unwrap();
        assert_eq!(read(p, "f.txt"), "c1\nd1\na2\nc2\n");

        restore(p, &entry.id, true).unwrap();
        assert_eq!(read(p, "f.txt"), edited);
    }

    /// An untracked file reverted in part is edited, not deleted: the partial
    /// reverse of its all-add diff is a modification. The backup brings it back.
    #[test]
    fn reverting_lines_of_an_untracked_file_edits_it_and_is_restored() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("u.txt"), "u1\nu2\nu3\n").unwrap();

        let eng = CliEngine::new(p);
        let diff = eng.diff_file("u.txt", "worktree", "none", None).unwrap();
        let picks = [HunkPick {
            hunk: 0,
            lines: LinePick::Lines(vec![1]),
        }];
        let patch = eng
            .selection_patch("u.txt", "worktree", &picks, &diff.digest, None, true)
            .unwrap();
        let paths = patch_paths(p, &patch).unwrap();
        assert_eq!(paths, strings(&["u.txt"]));
        let entry = with_backup(p, DiscardKind::Hunk, &paths, || {
            eng.apply_patch(&patch, false, true)
        })
        .unwrap()
        .unwrap();
        assert_eq!(read(p, "u.txt"), "u1\nu3\n");

        restore(p, &entry.id, false).unwrap();
        assert_eq!(read(p, "u.txt"), "u1\nu2\nu3\n");
    }

    #[test]
    fn an_untracked_folder_is_backed_up_file_by_file() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::create_dir_all(p.join("d/e")).unwrap();
        std::fs::write(p.join("d/one.txt"), "1\n").unwrap();
        std::fs::write(p.join("d/e/two.txt"), "2\n").unwrap();
        let paths = strings(&["d/"]);
        let entry = with_backup(p, DiscardKind::Files, &paths, || {
            std::fs::remove_dir_all(p.join("d")).map_err(Error::from)
        })
        .unwrap()
        .unwrap();
        let mut got = entry.paths.clone();
        got.sort();
        assert_eq!(got, strings(&["d/e/two.txt", "d/one.txt"]));
        restore(p, &entry.id, false).unwrap();
        assert_eq!(read(p, "d/e/two.txt"), "2\n");
    }

    #[test]
    fn a_file_changed_after_the_rollback_is_not_overwritten_without_asking() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "lost work\n").unwrap();
        let entry = rollback(p, &["a.txt"]).unwrap();
        assert!(stale_paths(p, &entry.id).unwrap().is_empty());

        std::fs::write(p.join("a.txt"), "newer work\n").unwrap();
        assert_eq!(stale_paths(p, &entry.id).unwrap(), strings(&["a.txt"]));
        match restore(p, &entry.id, false) {
            Err(Error::Stale(m)) => assert!(m.contains("a.txt"), "{m}"),
            other => panic!("expected stale, got {other:?}"),
        }
        assert_eq!(read(p, "a.txt"), "newer work\n", "untouched");

        // Forced, and the overwritten version is itself backed up.
        let forced = restore(p, &entry.id, true).unwrap().unwrap();
        assert_eq!(read(p, "a.txt"), "lost work\n");
        restore(p, &forced.id, false).unwrap();
        assert_eq!(read(p, "a.txt"), "newer work\n");
    }

    #[test]
    fn a_failed_backup_stops_the_rollback() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "precious\n").unwrap();
        // A held ref lock: update-ref cannot move the chain.
        let lock = CliEngine::new(p)
            .git_paths(&["refs/graft/discard.lock"])
            .unwrap()
            .pop()
            .unwrap();
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, "").unwrap();

        let mut ran = false;
        let paths = strings(&["a.txt"]);
        let r = with_backup(p, DiscardKind::Files, &paths, || {
            ran = true;
            CliEngine::new(p).rollback(&paths)
        });
        assert!(r.is_err(), "{r:?}");
        assert!(!ran, "the rollback did not run");
        assert_eq!(read(p, "a.txt"), "precious\n");
    }

    #[test]
    #[cfg(unix)]
    fn a_path_through_a_symlinked_folder_leading_out_is_refused_before_anything_runs() {
        let dir = scratch_repo();
        let outside = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(outside.path().join("x"), "outside\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), p.join("linkdir")).unwrap();

        let mut ran = false;
        let r = with_backup(p, DiscardKind::Files, &strings(&["linkdir/x"]), || {
            ran = true;
            Ok(())
        });
        assert!(matches!(r, Err(Error::Rule(_))), "{r:?}");
        assert!(!ran);
        assert!(tip(p).unwrap().is_none(), "nothing recorded");

        // And a folder swapped for such a link after the rollback stops the restore.
        std::fs::create_dir(p.join("sub")).unwrap();
        std::fs::write(p.join("sub/new.txt"), "mine\n").unwrap();
        let entry = rollback(p, &["sub/new.txt"]).unwrap();
        std::fs::remove_dir(p.join("sub")).unwrap();
        std::os::unix::fs::symlink(outside.path(), p.join("sub")).unwrap();
        assert!(restore(p, &entry.id, true).is_err());
        assert!(
            !outside.path().join("new.txt").exists(),
            "nothing written outside"
        );
    }

    #[test]
    fn a_backup_leaves_the_index_alone() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "staged\n").unwrap();
        git(p, &["add", "a.txt"]);
        std::fs::write(p.join("a.txt"), "staged\nand more\n").unwrap();
        std::fs::write(p.join("b.txt"), "new\n").unwrap();
        let before = (
            git(p, &["diff", "--cached"]),
            git(p, &["status", "--porcelain"]),
        );
        with_backup(p, DiscardKind::Files, &strings(&["a.txt", "b.txt"]), || {
            Ok(())
        })
        .unwrap();
        let after = (
            git(p, &["diff", "--cached"]),
            git(p, &["status", "--porcelain"]),
        );
        assert_eq!(before, after);
    }

    #[test]
    fn the_chain_starts_over_at_its_limit() {
        assert_eq!(CHAIN_LIMIT, 200);
        let dir = scratch_repo();
        let p = dir.path();
        let paths = strings(&["a.txt"]);
        let mut ids = Vec::new();
        for n in 0..4 {
            std::fs::write(p.join("a.txt"), format!("edit {n}\n")).unwrap();
            let e = with_backup_limit(
                p,
                DiscardKind::Files,
                &paths,
                || CliEngine::new(p).rollback(&paths),
                6,
            )
            .unwrap()
            .unwrap();
            ids.push(e.id);
        }
        // Three discards fill a chain of six; the fourth starts a new one.
        assert_eq!(chain_length(p).unwrap(), 2);
        let listed: Vec<String> = list(p, 10).unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(listed, vec![ids[3].clone()]);
        let reachable = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["merge-base", "--is-ancestor", &ids[2], DISCARD_REF])
            .status()
            .unwrap();
        assert!(!reachable.success(), "the old chain is no longer reachable");
        assert!(matches!(restore(p, &ids[0], false), Err(Error::Rule(_))));
    }

    #[test]
    fn backups_are_readable_and_hidden_from_branches() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "x\n").unwrap();
        rollback(p, &["a.txt"]).unwrap();
        let shown = git(p, &["log", "--format=%an <%ae>%n%B", DISCARD_REF]);
        assert!(shown.contains("Graft <graft@localhost>"), "{shown}");
        assert!(
            shown.contains("Graft: backup before rolling back 1 file"),
            "{shown}"
        );
        assert!(shown.contains("\na.txt\n"), "the path is named: {shown}");

        let tree = crate::engine::branches::tree(p).unwrap();
        assert!(
            tree.iter().all(|b| !b.full_ref.starts_with("refs/graft/")),
            "{tree:?}"
        );
        let names: Vec<String> = CliEngine::new(p)
            .branches()
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, strings(&["main"]));
    }

    #[test]
    fn unparsable_backup_records_are_errors() {
        assert!(parse_records("\u{1}h\0p\0notatime\0after\0files\0").is_err());
        assert!(parse_records("\u{1}h\0p\0\u{31}\0after\0files\0\nQ\0x\0").is_err());
        let ok = parse_records("\u{1}h\0p\0\u{31}\0after\0files\0\nA\0x y\0").unwrap();
        assert_eq!(ok[0].paths, strings(&["x y"]));
    }
}
