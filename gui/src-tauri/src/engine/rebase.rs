//! Interactive rebase: a plan the user approved, rewording a commit, squashing a
//! run of commits.
//!
//! Git does the rebasing; this module never replays a commit itself. It writes the
//! approved plan to a file and installs two editors for the one `git rebase
//! --interactive` it starts:
//!
//!  - **`GIT_SEQUENCE_EDITOR` copies the plan over the todo list** git offers:
//!    `cp "$GRAFT_REBASE_TODO"`. git runs an editor through `sh -c '<editor> "$@"'`,
//!    so the path travels in an environment variable and is expanded by the shell —
//!    no quoting of our own for spaces, quotes or `$` in the application data
//!    directory (`~/Library/Application Support/…` has a space on every Mac). Git
//!    for Windows runs editors through its own `sh` with coreutils on `PATH`, so the
//!    same command works there by construction. When the copy fails, `cp` exits
//!    non-zero and git refuses to start the rebase — it never falls back to its own
//!    default todo.
//!  - **`GIT_EDITOR` supplies messages by commit hash** ([`EDITOR_SCRIPT`]): it reads
//!    the last line of `rebase-merge/done` — the step git is on — and, when the plan
//!    has a message for that commit, writes it over the file git asked to edit;
//!    otherwise it leaves git's own text. Keyed by hash, not by position: git calls
//!    the editor once per squash *chain*, and a conflict adds a call at
//!    `--continue`. So `op_continue` / `op_skip` install the same editor while this
//!    rebase is in progress (`ops::drive`); with `GIT_EDITOR=true` a reword that
//!    stopped on a conflict would silently keep its old message.
//!
//! The plan lives **outside the repository**, in the application data directory,
//! under `rebase/<fnv1a of the worktree root>/`: `todo`, `editor.sh`, `msg/<hash>`,
//! `head` (HEAD when the rebase started — the same value git keeps in
//! `rebase-merge/orig-head`, which is how a plan is recognised as belonging to the
//! rebase in progress) and `comment` (see below). It is removed when the rebase is
//! over — by the call that started it, by abort, and by [`sweep`] on the next state
//! read when the rebase was finished or aborted from a terminal.
//!
//! **The comment character.** A message delivered through an editor goes through
//! git's `strip` cleanup, which deletes every line starting with the comment
//! character — a message line `#123 fixed` would vanish. The plan therefore picks a
//! character that starts no line of any message involved ([`comment_char`]) and every
//! git call of this rebase gets `-c core.commentChar=<it>`.
//!
//! What is refused before anything is written: a range with a merge commit (a plain
//! rebase would flatten it), a commit that is not an ancestor of HEAD, uncommitted
//! changes to tracked files, an operation already in progress, and a plan that does
//! not name every commit of the range exactly once — git treats a commit missing from
//! the todo as dropped, so a dialog that went stale while a new commit landed would
//! silently delete it. Hashes must be full hex: that also keeps the todo free of
//! injected lines and `msg/<hash>` inside its directory.
//!
//! Why a dirty tree is refused rather than auto-stashed: the stash would hide the
//! user's changes for the whole rebase, including an `edit` stop where they would
//! expect to see them, and the changelist store, synced against that clean
//! snapshot, would forget which list the stashed files belonged to.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::engine::cli::{fnv1a, CliEngine};
use crate::engine::exec;
use crate::engine::ops;
use crate::error::{Error, Result};
use crate::model::{
    OperationKind, RebaseAction, RebaseBlock, RebaseCommit, RebaseRange, RebaseStep,
};

/// Most commits a plan may hold — the dialog lists every one of them.
pub const MAX_STEPS: usize = 1000;

/// Longest message a step may carry, in bytes.
pub const MAX_MESSAGE: usize = 100_000;

/// Candidates for `core.commentChar`, in order of preference.
const COMMENT_CHARS: [char; 12] = ['#', ';', '@', '!', '%', '&', '|', ':', '~', '^', '*', '='];

/// The commit message editor installed as `GIT_EDITOR` (see the module doc).
///
/// `$1` is the file git asks to edit. The step in progress is the last non-comment
/// line of `done`: `<verb> <full hash>`. A hash that is not plain hex, or has no
/// message in the plan, leaves git's own text untouched.
const EDITOR_SCRIPT: &str = r#"# Graft: commit messages of an interactive rebase it started.
# $1 is the message file git asks to edit.
target=$1
line=$(sed -e '/^[#;@!%&|:~^*=]/d' -e '/^[[:space:]]*$/d' "$GRAFT_REBASE_DONE" 2>/dev/null | tail -n 1)
rest=${line#* }
hash=${rest%% *}
case $hash in
  ''|*[!0-9a-f]*) exit 0 ;;
esac
[ -f "$GRAFT_REBASE_MSGS/$hash" ] || exit 0
cat "$GRAFT_REBASE_MSGS/$hash" > "$target"
"#;

/// `git -C <repo> <args>`; on failure carry both streams (rebase prints its
/// `CONFLICT` lines on stdout).
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    exec::git(repo, args).run()?.checked_both()
}

/// The full hash of a revision naming a commit.
fn resolve(repo: &Path, rev: &str) -> Result<String> {
    Ok(git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ],
    )?
    .trim()
    .to_string())
}

fn count(repo: &Path, extra: &[&str], spec: &[String]) -> Result<usize> {
    let mut args = vec!["rev-list", "--count"];
    args.extend_from_slice(extra);
    args.push("--end-of-options");
    args.extend(spec.iter().map(String::as_str));
    let out = git(repo, &args)?;
    out.trim()
        .parse()
        .map_err(|_| Error::Parse(format!("rev-list --count said {:?}", out.trim())))
}

/// `HEAD ^<base>`, or `HEAD` alone for a range starting at a root commit.
fn range_spec(head: &str, base: Option<&str>) -> Vec<String> {
    let mut spec = vec![head.to_string()];
    if let Some(b) = base {
        spec.push(format!("^{b}"));
    }
    spec
}

/// How many commits of the range the current branch's upstream already has. No
/// upstream (or a detached HEAD) — nothing is known to be published.
fn published(repo: &Path, spec: &[String], total: usize) -> Result<u32> {
    let up = exec::git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            "@{upstream}^{commit}",
        ],
    )
    .run()?;
    if !up.success() {
        return Ok(0);
    }
    let up = up.stdout_text().trim().to_string();
    let mut unpublished = spec.to_vec();
    unpublished.push(format!("^{up}"));
    let left = count(repo, &[], &unpublished)?;
    Ok(total.saturating_sub(left) as u32)
}

/// Whether tracked files carry uncommitted changes. Untracked files do not stop a
/// rebase, and they do not count here.
fn tracked_changes(repo: &Path) -> Result<bool> {
    Ok(!git(
        repo,
        &["status", "--porcelain", "-z", "--untracked-files=no"],
    )?
    .trim_matches('\0')
    .is_empty())
}

/// What an interactive rebase from `hash` (inclusive) up to HEAD would replay.
///
/// The range is `hash^..HEAD` (`HEAD` for a root commit), oldest first. It is
/// `blocked` when `hash` is not an ancestor of HEAD, when a merge commit lies in it —
/// then HEAD's first-parent line does not run through `hash` without one, and a
/// plain rebase would flatten it — or when it is longer than [`MAX_STEPS`].
/// `published` and `dirty` are answered even for a blocked range: rewording HEAD by
/// `commit --amend` needs them and does not care about merges.
pub fn range(repo: &Path, hash: &str) -> Result<RebaseRange> {
    let head = resolve(repo, "HEAD")?;
    let target = resolve(repo, hash)?;
    let mut out = RebaseRange {
        head: head.clone(),
        base: None,
        commits: Vec::new(),
        blocked: None,
        dirty: tracked_changes(repo)?,
        published: 0,
    };

    let ancestor = exec::git(
        repo,
        &[
            "merge-base",
            "--is-ancestor",
            "--end-of-options",
            &target,
            &head,
        ],
    )
    .run()?;
    match ancestor.code {
        Some(0) => {}
        Some(1) => {
            out.blocked = Some(RebaseBlock::NotOnBranch);
            return Ok(out);
        }
        _ => return Err(ancestor.fail_both()),
    }

    let parent = exec::git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{target}^1^{{commit}}"),
        ],
    )
    .run()?;
    out.base = parent
        .success()
        .then(|| parent.stdout_text().trim().to_string());

    let spec = range_spec(&head, out.base.as_deref());
    let total = count(repo, &[], &spec)?;
    out.published = published(repo, &spec, total)?;
    if total > MAX_STEPS {
        out.blocked = Some(RebaseBlock::TooMany);
        return Ok(out);
    }
    if count(repo, &["--merges"], &spec)? > 0 {
        out.blocked = Some(RebaseBlock::Merge);
        return Ok(out);
    }

    let mut args = vec![
        "log",
        "--reverse",
        "--topo-order",
        "--no-color",
        "--no-show-signature",
        "--format=%H%x00%h%x00%an%x00%at%x00%s%x00%B%x01",
        "--end-of-options",
    ];
    args.extend(spec.iter().map(String::as_str));
    let raw = git(repo, &args)?;
    for record in raw.split('\u{1}') {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let f: Vec<&str> = record.splitn(6, '\0').collect();
        if f.len() != 6 {
            return Err(Error::Parse(format!("rebase range record: {record:?}")));
        }
        let author_at = f[3]
            .trim()
            .parse()
            .map_err(|_| Error::Parse(format!("rebase range timestamp: {:?}", f[3])))?;
        out.commits.push(RebaseCommit {
            hash: f[0].to_string(),
            short_hash: f[1].to_string(),
            author: f[2].to_string(),
            author_at,
            subject: f[4].to_string(),
            message: f[5].trim_end().to_string(),
        });
    }
    if out.commits.len() != total || out.commits.first().map(|c| c.hash.as_str()) != Some(&target) {
        return Err(Error::Parse(format!(
            "rebase range from {target}: git listed {} commits, counted {total}",
            out.commits.len()
        )));
    }
    Ok(out)
}

// ── the plan ────────────────────────────────────────────────────────────────

/// A plan turned into what git reads: the todo text, and the messages the editor
/// hands out, keyed by the hash of the step at which git opens the editor.
#[derive(Debug, PartialEq)]
pub(crate) struct Compiled {
    pub todo: String,
    pub messages: Vec<(String, String)>,
}

fn is_full_hash(h: &str) -> bool {
    (h.len() == 40 || h.len() == 64) && h.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check_message(m: &str) -> Result<()> {
    if m.trim().is_empty() {
        return Err(Error::Rule("a commit message cannot be empty".into()));
    }
    if m.len() > MAX_MESSAGE {
        return Err(Error::Rule(format!(
            "a commit message is limited to {MAX_MESSAGE} bytes"
        )));
    }
    if m.contains('\0') {
        return Err(Error::Rule(
            "a commit message cannot contain a NUL byte".into(),
        ));
    }
    Ok(())
}

fn verb(a: RebaseAction) -> &'static str {
    match a {
        RebaseAction::Pick => "pick",
        RebaseAction::Reword => "reword",
        RebaseAction::Edit => "edit",
        RebaseAction::Squash => "squash",
        RebaseAction::Fixup => "fixup",
        RebaseAction::Drop => "drop",
    }
}

fn melds(a: RebaseAction) -> bool {
    matches!(a, RebaseAction::Squash | RebaseAction::Fixup)
}

/// Validate a plan on its own terms and turn it into a todo and a message map.
///
/// Where a message lands — the one rule of this module worth reading twice:
///
///  - a chain is a kept commit plus the `squash` / `fixup` steps right after it;
///    `drop` steps are written at the end of the todo (they are no-ops wherever they
///    stand), so they never split a chain;
///  - a chain carries at most one message — the head's `reword` text, or one on a
///    melded step — and it becomes the message of the commit the chain produces;
///  - a chain with a `squash`: git opens the editor once, after its last step, so
///    the message is keyed by that step's hash (a `reword` head is written as `pick`:
///    the chain's message replaces its text anyway);
///  - a chain of `fixup`s only: git never opens the editor for it, so the head is
///    written as `reword` and the message keyed by the head — fixups keep the head's
///    message. An `edit` head cannot be both, and such a plan is refused.
pub(crate) fn compile(steps: &[RebaseStep]) -> Result<Compiled> {
    if steps.is_empty() {
        return Err(Error::Rule(
            "a rebase plan needs at least one commit".into(),
        ));
    }
    if steps.len() > MAX_STEPS {
        return Err(Error::Rule(format!(
            "a rebase plan holds at most {MAX_STEPS} commits"
        )));
    }
    let mut seen = HashSet::new();
    for s in steps {
        if !is_full_hash(&s.hash) {
            return Err(Error::Rule(format!("not a full commit hash: {:?}", s.hash)));
        }
        if !seen.insert(s.hash.to_ascii_lowercase()) {
            return Err(Error::Rule(format!(
                "commit {} appears twice in the plan",
                s.hash
            )));
        }
        match (s.action, &s.message) {
            (RebaseAction::Reword, None) => {
                return Err(Error::Rule(format!(
                    "the reworded commit {} needs a message",
                    &s.hash[..7]
                )))
            }
            (RebaseAction::Pick | RebaseAction::Edit | RebaseAction::Drop, Some(_)) => {
                return Err(Error::Rule(
                    "only reword, squash and fixup steps carry a message".into(),
                ))
            }
            _ => {}
        }
        if let Some(m) = &s.message {
            check_message(m)?;
        }
    }

    let kept: Vec<&RebaseStep> = steps
        .iter()
        .filter(|s| s.action != RebaseAction::Drop)
        .collect();
    let Some(first) = kept.first() else {
        return Err(Error::Rule(
            "a rebase plan must keep at least one commit".into(),
        ));
    };
    if melds(first.action) {
        return Err(Error::Rule(
            "the first kept commit has nothing before it to be squashed into".into(),
        ));
    }

    let hash = |s: &RebaseStep| s.hash.to_ascii_lowercase();
    let mut lines = Vec::with_capacity(steps.len());
    let mut messages = Vec::new();
    let mut i = 0;
    while i < kept.len() {
        let mut j = i + 1;
        while j < kept.len() && melds(kept[j].action) {
            j += 1;
        }
        let chain = &kept[i..j];
        let head = chain[0];
        let mut texts = chain.iter().filter_map(|s| s.message.as_ref());
        let message = texts.next();
        if texts.next().is_some() {
            return Err(Error::Rule(format!(
                "the commits melded into {} carry more than one message; keep one",
                &head.hash[..7]
            )));
        }
        let mut head_verb = verb(head.action);
        if let Some(m) = message {
            if chain[1..].iter().any(|s| s.action == RebaseAction::Squash) {
                let last = chain[chain.len() - 1];
                messages.push((hash(last), m.clone()));
                if head.action == RebaseAction::Reword {
                    head_verb = "pick";
                }
            } else if head.action == RebaseAction::Edit {
                return Err(Error::Rule(format!(
                    "the message of commits fixed up into the edited commit {} cannot be \
                     planned; change it at the stop",
                    &head.hash[..7]
                )));
            } else {
                head_verb = "reword";
                messages.push((hash(head), m.clone()));
            }
        }
        lines.push(format!("{head_verb} {}", hash(head)));
        for s in &chain[1..] {
            lines.push(format!("{} {}", verb(s.action), hash(s)));
        }
        i = j;
    }
    for s in steps.iter().filter(|s| s.action == RebaseAction::Drop) {
        lines.push(format!("drop {}", hash(s)));
    }
    Ok(Compiled {
        todo: lines.join("\n") + "\n",
        messages,
    })
}

/// A `core.commentChar` that starts no line of any of `texts`, so git's `strip`
/// cleanup removes only its own comments. `#` when it is free — the default.
pub(crate) fn comment_char<'a>(texts: impl Iterator<Item = &'a str> + Clone) -> char {
    COMMENT_CHARS
        .iter()
        .copied()
        .find(|c| !texts.clone().any(|t| t.lines().any(|l| l.starts_with(*c))))
        .unwrap_or('#')
}

// ── plan files ──────────────────────────────────────────────────────────────

/// The files of one repository's plan.
pub(crate) struct Plan {
    dir: PathBuf,
    comment: char,
}

impl Plan {
    fn todo(&self) -> PathBuf {
        self.dir.join("todo")
    }
    fn editor(&self) -> PathBuf {
        self.dir.join("editor.sh")
    }
    fn msgs(&self) -> PathBuf {
        self.dir.join("msg")
    }

    /// `core.commentChar=<c>`, for `-c` on every git call of this rebase.
    pub(crate) fn comment_config(&self) -> String {
        format!("core.commentChar={}", self.comment)
    }

    /// The environment that installs the message editor; with `sequence`, also the
    /// sequence editor that copies the plan over the todo — otherwise `true`, since
    /// only the start of a rebase asks for the todo.
    pub(crate) fn env(&self, repo: &Path, sequence: bool) -> Result<Vec<(String, String)>> {
        let done = CliEngine::new(repo)
            .git_paths(&["rebase-merge/done"])?
            .pop()
            .ok_or_else(|| Error::Parse("rev-parse --git-path returned nothing".into()))?;
        let text = |p: PathBuf| p.to_string_lossy().into_owned();
        Ok(vec![
            (
                "GIT_SEQUENCE_EDITOR".into(),
                if sequence {
                    r#"cp "$GRAFT_REBASE_TODO""#
                } else {
                    "true"
                }
                .into(),
            ),
            ("GRAFT_REBASE_TODO".into(), text(self.todo())),
            ("GIT_EDITOR".into(), r#"sh "$GRAFT_REBASE_EDITOR""#.into()),
            ("GRAFT_REBASE_EDITOR".into(), text(self.editor())),
            ("GRAFT_REBASE_DONE".into(), text(done)),
            ("GRAFT_REBASE_MSGS".into(), text(self.msgs())),
        ])
    }
}

/// `<data>/rebase/<fnv1a of the canonical worktree root>`.
pub(crate) fn plan_dir(data_dir: &Path, repo: &Path) -> PathBuf {
    let root = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    data_dir
        .join("rebase")
        .join(fnv1a(root.to_string_lossy().as_bytes()))
}

fn io(what: &str, e: std::io::Error) -> Error {
    Error::Io(format!("{what}: {e}"))
}

fn remove_dir(dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io("removing the rebase plan", e)),
    }
}

/// Write the plan of a rebase about to start from `head`, replacing any older one.
pub(crate) fn write_plan(
    data_dir: &Path,
    repo: &Path,
    head: &str,
    compiled: &Compiled,
    comment: char,
) -> Result<Plan> {
    let plan = Plan {
        dir: plan_dir(data_dir, repo),
        comment,
    };
    remove_dir(&plan.dir)?;
    let w = |p: PathBuf, text: &str| {
        std::fs::write(p, text).map_err(|e| io("writing the rebase plan", e))
    };
    std::fs::create_dir_all(plan.msgs()).map_err(|e| io("creating the rebase plan", e))?;
    w(plan.todo(), &compiled.todo)?;
    w(plan.editor(), EDITOR_SCRIPT)?;
    w(plan.dir.join("head"), &format!("{head}\n"))?;
    w(plan.dir.join("comment"), &comment.to_string())?;
    for (hash, message) in &compiled.messages {
        let mut text = message.clone();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        w(plan.msgs().join(hash), &text)?;
    }
    Ok(plan)
}

/// Start `git rebase --interactive` with the plan's editors installed.
pub(crate) fn launch(repo: &Path, plan: &Plan, base: Option<&str>) -> Result<()> {
    let vars = plan.env(repo, true)?;
    let env: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let config = plan.comment_config();
    let mut args = vec![
        "-c",
        config.as_str(),
        "rebase",
        "--interactive",
        "--no-rebase-merges",
    ];
    match base {
        Some(b) => {
            args.push("--end-of-options");
            args.push(b);
        }
        None => args.push("--root"),
    }
    exec::git(repo, &args).env(&env).run()?.checked_both()?;
    Ok(())
}

/// The plan of the rebase in progress, if this application started it: a plan
/// exists for the repository and its `head` is the `orig-head` git keeps. A rebase
/// started elsewhere later — or none at all — gets `None`.
pub(crate) fn resume(data_dir: &Path, repo: &Path) -> Result<Option<Plan>> {
    let dir = plan_dir(data_dir, repo);
    let Ok(head) = std::fs::read_to_string(dir.join("head")) else {
        return Ok(None);
    };
    let paths = CliEngine::new(repo).git_paths(&["rebase-merge", "rebase-merge/orig-head"])?;
    if !paths[0].is_dir() {
        return Ok(None);
    }
    let orig = std::fs::read_to_string(&paths[1]).unwrap_or_default();
    if orig.trim() != head.trim() {
        return Ok(None);
    }
    let comment = std::fs::read_to_string(dir.join("comment"))
        .ok()
        .and_then(|s| s.chars().next())
        .unwrap_or('#');
    Ok(Some(Plan { dir, comment }))
}

/// Remove the plan unless the rebase it belongs to is still in progress. Called
/// after every start and abort, and on each state read — which is how a rebase
/// finished or aborted in a terminal loses its plan too.
pub fn sweep(data_dir: &Path, repo: &Path) -> Result<()> {
    let dir = plan_dir(data_dir, repo);
    if !dir.exists() || resume(data_dir, repo)?.is_some() {
        return Ok(());
    }
    remove_dir(&dir)
}

// ── operations ──────────────────────────────────────────────────────────────

fn ensure_calm(repo: &Path) -> Result<()> {
    let kind = ops::detect_kind(repo)?;
    if kind != OperationKind::None {
        return Err(Error::Rule(format!(
            "a {} is in progress; finish or abort it first",
            match kind {
                OperationKind::Merge => "merge",
                OperationKind::Rebase => "rebase",
                OperationKind::CherryPick => "cherry-pick",
                OperationKind::Revert => "revert",
                OperationKind::Bisect => "bisect",
                OperationKind::None => "",
            }
        )));
    }
    Ok(())
}

fn refuse_blocked(r: &RebaseRange) -> Result<()> {
    match r.blocked {
        None => Ok(()),
        Some(RebaseBlock::NotOnBranch) => Err(Error::Rule(
            "the commit is not on the current branch; check out a branch that contains it".into(),
        )),
        Some(RebaseBlock::Merge) => Err(Error::Rule(
            "a merge commit lies between this commit and HEAD; a rebase would flatten it".into(),
        )),
        Some(RebaseBlock::TooMany) => Err(Error::Rule(format!(
            "more than {MAX_STEPS} commits would be replayed"
        ))),
    }
}

/// Check the plan against the range, write it, run it, and sweep.
fn run_plan(repo: &Path, data_dir: &Path, r: &RebaseRange, steps: &[RebaseStep]) -> Result<()> {
    refuse_blocked(r)?;
    if r.dirty {
        return Err(Error::Rule(
            "tracked files have uncommitted changes; commit or stash them before rewriting history"
                .into(),
        ));
    }
    let compiled = compile(steps)?;
    let planned: HashSet<String> = steps.iter().map(|s| s.hash.to_ascii_lowercase()).collect();
    let actual: HashSet<String> = r
        .commits
        .iter()
        .map(|c| c.hash.to_ascii_lowercase())
        .collect();
    if planned != actual {
        return Err(Error::Rule(
            "the history changed since this plan was made; open it again".into(),
        ));
    }
    let texts = steps
        .iter()
        .filter_map(|s| s.message.as_deref())
        .chain(r.commits.iter().map(|c| c.message.as_str()));
    let plan = write_plan(data_dir, repo, &r.head, &compiled, comment_char(texts))?;
    let result = launch(repo, &plan, r.base.as_deref());
    let swept = sweep(data_dir, repo);
    result?;
    swept
}

/// Replay `hash` (inclusive) up to HEAD according to `steps`, oldest first.
pub fn start(repo: &Path, data_dir: &Path, hash: &str, steps: &[RebaseStep]) -> Result<()> {
    ensure_calm(repo)?;
    let r = range(repo, hash)?;
    run_plan(repo, data_dir, &r, steps)
}

/// Replace the message of HEAD and nothing else: `--only` without paths commits
/// HEAD's own tree, so whatever the user has staged stays staged and out of it.
fn amend_message(repo: &Path, message: &str) -> Result<()> {
    exec::git(
        repo,
        &[
            "commit",
            "--amend",
            "--only",
            "--allow-empty",
            "--no-edit",
            "-F",
            "-",
        ],
    )
    .env(&[("GIT_EDITOR", "true")])
    .input(message.as_bytes())
    .run()?
    .checked_both()?;
    Ok(())
}

/// Give one commit a new message. HEAD is amended; an older commit is rewritten by
/// replaying the range after it with a single `reword`.
pub fn reword(repo: &Path, data_dir: &Path, hash: &str, message: &str) -> Result<()> {
    ensure_calm(repo)?;
    check_message(message)?;
    let head = resolve(repo, "HEAD")?;
    let target = resolve(repo, hash)?;
    if target == head {
        return amend_message(repo, message);
    }
    let r = range(repo, &target)?;
    let steps: Vec<RebaseStep> = r
        .commits
        .iter()
        .map(|c| {
            let this = c.hash == target;
            RebaseStep {
                hash: c.hash.clone(),
                action: if this {
                    RebaseAction::Reword
                } else {
                    RebaseAction::Pick
                },
                message: this.then(|| message.to_string()),
            }
        })
        .collect();
    run_plan(repo, data_dir, &r, &steps)
}

/// Meld a run of consecutive commits on HEAD's line into one with `message`.
///
/// The oldest of them is found by `merge-base --octopus` — for commits on one line
/// that is the oldest, and for anything else it is a commit outside the selection,
/// which is then refused. The oldest is kept, the others are fixed up into it, and
/// the chain's message is the one given (keyed per [`compile`]).
pub fn squash(repo: &Path, data_dir: &Path, hashes: &[String], message: &str) -> Result<()> {
    ensure_calm(repo)?;
    check_message(message)?;
    if hashes.len() < 2 {
        return Err(Error::Rule("squashing needs at least two commits".into()));
    }
    let mut chosen = HashSet::new();
    for h in hashes {
        if !chosen.insert(resolve(repo, h)?) {
            return Err(Error::Rule(format!("commit {h} is selected twice")));
        }
    }
    let mut args = vec!["merge-base", "--octopus", "--end-of-options"];
    args.extend(chosen.iter().map(String::as_str));
    let oldest = git(repo, &args)?.trim().to_string();
    let not_a_run = || {
        Error::Rule(
            "only commits that follow one another on the current branch can be squashed".into(),
        )
    };
    if !chosen.contains(&oldest) {
        return Err(not_a_run());
    }
    let r = range(repo, &oldest)?;
    refuse_blocked(&r)?;
    let n = chosen.len();
    if r.commits.len() < n || !r.commits[..n].iter().all(|c| chosen.contains(&c.hash)) {
        return Err(not_a_run());
    }
    let steps: Vec<RebaseStep> = r
        .commits
        .iter()
        .enumerate()
        .map(|(i, c)| RebaseStep {
            hash: c.hash.clone(),
            action: match i {
                0 => RebaseAction::Pick,
                i if i < n => RebaseAction::Fixup,
                _ => RebaseAction::Pick,
            },
            message: (i == n - 1).then(|| message.to_string()),
        })
        .collect();
    run_plan(repo, data_dir, &r, &steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
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
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_file(p: &Path, file: &str, text: &str, message: &str) -> String {
        std::fs::write(p.join(file), text).unwrap();
        git(p, &["add", file]);
        git(p, &["commit", "-q", "-m", message]);
        git(p, &["rev-parse", "HEAD"])
    }

    /// `init` plus A (b.txt), B (c.txt), C (d.txt) — three commits touching
    /// different files, so any order replays cleanly.
    fn repo_abc() -> (tempfile::TempDir, [String; 3]) {
        let dir = scratch_repo();
        let p = dir.path();
        let a = commit_file(p, "b.txt", "b\n", "A");
        let b = commit_file(p, "c.txt", "c\n", "B");
        let c = commit_file(p, "d.txt", "d\n", "C");
        (dir, [a, b, c])
    }

    /// Subjects from HEAD down to the root.
    fn subjects(p: &Path) -> Vec<String> {
        git(p, &["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn message(p: &Path, rev: &str) -> String {
        git(p, &["log", "-1", "--format=%B", rev])
    }

    fn step(hash: &str, action: RebaseAction) -> RebaseStep {
        RebaseStep {
            hash: hash.to_string(),
            action,
            message: None,
        }
    }

    fn with_message(hash: &str, action: RebaseAction, m: &str) -> RebaseStep {
        RebaseStep {
            hash: hash.to_string(),
            action,
            message: Some(m.to_string()),
        }
    }

    fn data() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn rule(r: Result<impl std::fmt::Debug>) -> String {
        match r {
            Err(Error::Rule(m)) => m,
            other => panic!("expected a domain refusal, got {other:?}"),
        }
    }

    use RebaseAction::*;

    // ── compile ──────────────────────────────────────────────────────────────

    const H1: &str = "1111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222";
    const H3: &str = "3333333333333333333333333333333333333333";
    const H4: &str = "4444444444444444444444444444444444444444";

    #[test]
    fn compile_writes_the_todo_in_plan_order_with_drops_last() {
        let c = compile(&[
            step(H2, Pick),
            step(H1, Drop),
            step(H3, Edit),
            step(H4, Fixup),
        ])
        .unwrap();
        assert_eq!(
            c.todo,
            format!("pick {H2}\nedit {H3}\nfixup {H4}\ndrop {H1}\n")
        );
        assert!(c.messages.is_empty());
    }

    #[test]
    fn compile_keys_each_message_where_git_opens_the_editor() {
        // a reword: its own hash
        let c = compile(&[with_message(H1, Reword, "new"), step(H2, Pick)]).unwrap();
        assert_eq!(c.todo, format!("reword {H1}\npick {H2}\n"));
        assert_eq!(c.messages, vec![(H1.to_string(), "new".to_string())]);

        // a chain with a squash: its last member, and the head is a plain pick
        let c = compile(&[
            with_message(H1, Reword, "whole"),
            step(H2, Squash),
            step(H3, Fixup),
            step(H4, Pick),
        ])
        .unwrap();
        assert_eq!(
            c.todo,
            format!("pick {H1}\nsquash {H2}\nfixup {H3}\npick {H4}\n")
        );
        assert_eq!(c.messages, vec![(H3.to_string(), "whole".to_string())]);

        // a chain of fixups only: git never opens the editor, so the head rewords
        let c = compile(&[
            step(H1, Pick),
            step(H2, Fixup),
            with_message(H3, Fixup, "all"),
        ])
        .unwrap();
        assert_eq!(c.todo, format!("reword {H1}\nfixup {H2}\nfixup {H3}\n"));
        assert_eq!(c.messages, vec![(H1.to_string(), "all".to_string())]);

        // a drop between does not split the chain
        let c = compile(&[
            step(H1, Pick),
            step(H2, Drop),
            with_message(H3, Squash, "m"),
        ])
        .unwrap();
        assert_eq!(c.todo, format!("pick {H1}\nsquash {H3}\ndrop {H2}\n"));
        assert_eq!(c.messages, vec![(H3.to_string(), "m".to_string())]);
    }

    #[test]
    fn compile_refuses_what_git_would_misread() {
        let cases: Vec<(Vec<RebaseStep>, &str)> = vec![
            (vec![], "at least one"),
            (vec![step(H1, Drop), step(H2, Drop)], "keep at least one"),
            (vec![step(H1, Drop), step(H2, Squash)], "nothing before it"),
            (vec![step(H1, Fixup)], "nothing before it"),
            (vec![step(H1, Pick), step(H1, Pick)], "twice"),
            (vec![step("abc1234", Pick)], "full commit hash"),
            (
                vec![step(&format!("{}\nexec rm", &H1[..40]), Pick)],
                "full commit hash",
            ),
            (vec![step(H1, Reword)], "needs a message"),
            (vec![with_message(H1, Reword, "  \n")], "cannot be empty"),
            (vec![with_message(H1, Pick, "x")], "only reword"),
            (
                vec![with_message(H1, Reword, "a"), with_message(H2, Squash, "b")],
                "more than one message",
            ),
            (
                vec![step(H1, Edit), with_message(H2, Fixup, "m")],
                "edited commit",
            ),
            (
                vec![with_message(H1, Reword, &"x".repeat(MAX_MESSAGE + 1))],
                "limited",
            ),
        ];
        for (steps, want) in cases {
            let m = rule(compile(&steps));
            assert!(m.contains(want), "{steps:?}: {m}");
        }
    }

    #[test]
    fn the_comment_character_avoids_every_message_line() {
        assert_eq!(comment_char(["plain", "text\nhere"].into_iter()), '#');
        assert_eq!(comment_char(["body\n#123 fixed"].into_iter()), ';');
        assert_eq!(comment_char(["#a", ";b\n@c"].into_iter()), '!');
    }

    // ── range ────────────────────────────────────────────────────────────────

    #[test]
    fn range_lists_the_commits_from_the_one_asked_up_to_head() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let r = range(p, &b).unwrap();
        assert_eq!(r.blocked, None);
        assert_eq!(r.base.as_deref(), Some(a.as_str()));
        let hashes: Vec<&str> = r.commits.iter().map(|c| c.hash.as_str()).collect();
        assert_eq!(
            hashes,
            vec![b.as_str(), c.as_str()],
            "oldest first, inclusive"
        );
        assert_eq!(r.commits[0].subject, "B");
        assert_eq!(r.commits[0].message, "B");
        assert!(!r.dirty);
        assert_eq!(r.published, 0, "no upstream");

        let root = git(p, &["rev-list", "--max-parents=0", "HEAD"]);
        let r = range(p, &root).unwrap();
        assert_eq!(r.base, None, "a root commit has no base");
        assert_eq!(r.commits.len(), 4);
    }

    #[test]
    fn range_is_blocked_by_a_merge_and_off_the_branch() {
        let (dir, [a, ..]) = repo_abc();
        let p = dir.path();
        git(p, &["checkout", "-q", "-b", "side", &a]);
        let side = commit_file(p, "s.txt", "s\n", "side");
        git(p, &["checkout", "-q", "main"]);
        assert_eq!(
            range(p, &side).unwrap().blocked,
            Some(RebaseBlock::NotOnBranch)
        );

        git(p, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
        let r = range(p, &a).unwrap();
        assert_eq!(r.blocked, Some(RebaseBlock::Merge));
        assert!(r.commits.is_empty());

        let d = data();
        let m = rule(reword(p, d.path(), &a, "new"));
        assert!(m.contains("merge commit"), "{m}");
        let steps = vec![step(&a, Pick)];
        assert!(rule(start(p, d.path(), &a, &steps)).contains("merge commit"));
        assert_eq!(subjects(p)[0], "merge side", "nothing was rewritten");
    }

    #[test]
    fn range_counts_what_the_upstream_already_has() {
        let (dir, [a, b, _c]) = repo_abc();
        let p = dir.path();
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git(
            p,
            &["push", "-q", "origin", &format!("{b}:refs/heads/main")],
        );
        git(p, &["fetch", "-q", "origin"]);
        git(
            p,
            &["branch", "-q", "--set-upstream-to=origin/main", "main"],
        );
        assert_eq!(
            range(p, &a).unwrap().published,
            2,
            "A and B are on origin/main"
        );
        let head = git(p, &["rev-parse", "HEAD"]);
        assert_eq!(range(p, &head).unwrap().published, 0);
    }

    // ── plans ────────────────────────────────────────────────────────────────

    #[test]
    fn a_plan_reorders_and_drops() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&c, Pick), step(&b, Drop), step(&a, Pick)],
        )
        .unwrap();
        assert_eq!(subjects(p), vec!["A", "C", "init"]);
        assert!(!p.join("c.txt").exists(), "B's file went with it");
        assert!(p.join("d.txt").exists() && p.join("b.txt").exists());
        assert_eq!(ops::detect_state(p).unwrap().kind, OperationKind::None);
        assert!(
            !plan_dir(d.path(), p).exists(),
            "the plan is removed once done"
        );
    }

    #[test]
    fn a_squash_chain_takes_the_planned_message() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[
                step(&a, Pick),
                with_message(&b, Squash, "One\n\nfrom three"),
                step(&c, Fixup),
            ],
        )
        .unwrap();
        assert_eq!(subjects(p), vec!["One", "init"]);
        assert_eq!(message(p, "HEAD"), "One\n\nfrom three");
        for f in ["b.txt", "c.txt", "d.txt"] {
            assert!(p.join(f).exists(), "{f}");
        }
    }

    #[test]
    fn a_squash_without_a_message_keeps_both_texts_and_no_comments() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&a, Pick), step(&b, Squash), step(&c, Pick)],
        )
        .unwrap();
        assert_eq!(subjects(p).len(), 3);
        let m = message(p, "HEAD~1");
        assert!(m.starts_with('A') && m.contains('B'), "{m:?}");
        assert!(
            !m.contains("combination"),
            "git's comment lines are stripped: {m:?}"
        );
    }

    #[test]
    fn a_fixup_keeps_the_head_message() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&a, Pick), step(&b, Fixup), step(&c, Pick)],
        )
        .unwrap();
        assert_eq!(subjects(p), vec!["C", "A", "init"]);
        assert!(p.join("c.txt").exists());
    }

    #[test]
    fn squash_melds_a_run_and_refuses_a_gap() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        let m = rule(squash(p, d.path(), &[a.clone(), c.clone()], "x"));
        assert!(m.contains("follow one another"), "{m}");

        squash(p, d.path(), &[b.clone(), a.clone()], "A and B").unwrap();
        assert_eq!(subjects(p), vec!["C", "A and B", "init"]);
        assert!(p.join("b.txt").exists() && p.join("c.txt").exists());
    }

    #[test]
    fn a_deep_reword_changes_one_message_and_keeps_the_trees() {
        let (dir, [a, b, _c]) = repo_abc();
        let p = dir.path();
        let d = data();
        let tree = git(p, &["rev-parse", "HEAD^{tree}"]);
        reword(p, d.path(), &b, "B, renamed\n\n#123 keeps its hash line").unwrap();
        assert_eq!(subjects(p), vec!["C", "B, renamed", "A", "init"]);
        assert_eq!(
            message(p, "HEAD~1"),
            "B, renamed\n\n#123 keeps its hash line",
            "a line starting with # survives: the rebase ran with another comment char"
        );
        assert_eq!(git(p, &["rev-parse", "HEAD^{tree}"]), tree);
        assert_eq!(git(p, &["rev-parse", "HEAD~2"]), a, "A was not rewritten");
    }

    #[test]
    fn rewording_a_root_commit_uses_root() {
        let (dir, _) = repo_abc();
        let p = dir.path();
        let d = data();
        let root = git(p, &["rev-list", "--max-parents=0", "HEAD"]);
        reword(p, d.path(), &root, "the beginning").unwrap();
        assert_eq!(subjects(p), vec!["C", "B", "A", "the beginning"]);
    }

    /// The reword step itself conflicts; the message still lands, through the
    /// editor `op_continue` installs — `GIT_EDITOR=true` would keep the old text.
    #[test]
    fn a_reword_that_stops_on_a_conflict_gets_its_message_at_continue() {
        let dir = scratch_repo();
        let p = dir.path();
        let a = commit_file(p, "a.txt", "two\n", "A");
        let b = commit_file(p, "a.txt", "three\n", "B");
        let d = data();

        let r = start(
            p,
            d.path(),
            &a,
            &[step(&a, Drop), with_message(&b, Reword, "B reworded")],
        );
        assert!(
            matches!(r, Err(Error::Git { .. })),
            "the conflict surfaces: {r:?}"
        );
        let state = ops::detect_state(p).unwrap();
        assert_eq!(state.kind, OperationKind::Rebase);
        assert_eq!(state.edit_stop, None, "a conflict is not an edit stop");
        assert!(
            plan_dir(d.path(), p).exists(),
            "the plan stays while the rebase does"
        );

        std::fs::write(p.join("a.txt"), "three\n").unwrap();
        git(p, &["add", "a.txt"]);
        ops::op_continue(p, Some(d.path())).unwrap();

        assert_eq!(ops::detect_state(p).unwrap().kind, OperationKind::None);
        assert_eq!(subjects(p), vec!["B reworded", "init"]);
        assert!(!plan_dir(d.path(), p).exists());
    }

    #[test]
    fn an_edit_step_stops_and_continue_finishes() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&a, Pick), step(&b, Edit), step(&c, Pick)],
        )
        .unwrap();

        let state = ops::detect_state(p).unwrap();
        assert_eq!(state.kind, OperationKind::Rebase);
        assert_eq!(state.edit_stop.as_deref(), Some(b.as_str()));
        assert!(state.conflicted.is_empty());

        std::fs::write(p.join("c.txt"), "c, amended\n").unwrap();
        git(p, &["commit", "-q", "-a", "--amend", "--no-edit"]);
        ops::op_continue(p, Some(d.path())).unwrap();

        assert_eq!(ops::detect_state(p).unwrap().kind, OperationKind::None);
        assert_eq!(subjects(p), vec!["C", "B", "A", "init"]);
        assert_eq!(
            std::fs::read_to_string(p.join("c.txt")).unwrap(),
            "c, amended\n"
        );
        assert!(!plan_dir(d.path(), p).exists());
    }

    #[test]
    fn abort_removes_the_plan() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&a, Edit), step(&b, Pick), step(&c, Pick)],
        )
        .unwrap();
        assert!(plan_dir(d.path(), p).exists());
        ops::op_abort(p, Some(d.path())).unwrap();
        assert!(!plan_dir(d.path(), p).exists());
        assert_eq!(git(p, &["rev-parse", "HEAD"]), c);
    }

    /// Finished from a terminal: the next state read finds no rebase and removes
    /// the plan. A plan whose `head` is not the rebase in progress is not ours.
    #[test]
    fn sweep_removes_a_plan_the_rebase_no_longer_needs() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&a, Edit), step(&b, Pick), step(&c, Pick)],
        )
        .unwrap();
        sweep(d.path(), p).unwrap();
        assert!(plan_dir(d.path(), p).exists(), "still in progress");

        std::fs::write(plan_dir(d.path(), p).join("head"), format!("{a}\n")).unwrap();
        assert!(
            resume(d.path(), p).unwrap().is_none(),
            "another rebase's plan"
        );

        git(p, &["rebase", "--abort"]);
        std::fs::write(plan_dir(d.path(), p).join("head"), format!("{c}\n")).unwrap();
        sweep(d.path(), p).unwrap();
        assert!(!plan_dir(d.path(), p).exists());
    }

    /// When the sequence editor cannot copy the plan, git must not run its own
    /// default todo. `rebase.autoSquash` with a `fixup!` commit makes that default
    /// visible: it would meld the fixup into A.
    #[test]
    fn a_failed_copy_of_the_plan_stops_git_before_its_default_todo() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        git(p, &["config", "rebase.autoSquash", "true"]);
        let f = commit_file(p, "b.txt", "b2\n", "fixup! A");
        let d = data();
        let steps: Vec<RebaseStep> = [&a, &b, &c, &f].iter().map(|h| step(h, Pick)).collect();
        let plan = write_plan(d.path(), p, &f, &compile(&steps).unwrap(), '#').unwrap();
        std::fs::remove_file(plan.todo()).unwrap();

        let r = launch(p, &plan, Some(&git(p, &["rev-parse", "HEAD~4"])));
        match r {
            Err(Error::Git { stderr, .. }) => assert!(stderr.contains("editor"), "{stderr}"),
            other => panic!("expected git to refuse, got {other:?}"),
        }
        assert_eq!(ops::detect_state(p).unwrap().kind, OperationKind::None);
        assert_eq!(git(p, &["rev-parse", "HEAD"]), f, "nothing was replayed");
        assert_eq!(
            subjects(p)[0],
            "fixup! A",
            "the default todo would have melded it"
        );
        assert!(
            !git(p, &["reflog", "--format=%gs"]).contains("rebase"),
            "no rebase step reached the reflog"
        );
        sweep(d.path(), p).unwrap();
        assert!(!plan_dir(d.path(), p).exists(), "a refused start leaves no plan");
    }

    /// The data directory and the repository both carry a space, a quote and a `$`.
    #[test]
    fn paths_with_spaces_and_quotes_reach_git_intact() {
        let outer = tempfile::tempdir().unwrap();
        let p = outer.path().join("my repo 'q\" $HOME");
        std::fs::create_dir(&p).unwrap();
        git(&p, &["init", "-q", "-b", "main"]);
        git(&p, &["config", "user.email", "t@example.com"]);
        git(&p, &["config", "user.name", "Test"]);
        git(&p, &["config", "commit.gpgsign", "false"]);
        let a = commit_file(&p, "a.txt", "1\n", "A");
        commit_file(&p, "b.txt", "2\n", "B");
        let d = outer.path().join("Application Support 'x\" `y` $z");
        reword(&p, &d, &a, "A, reworded").unwrap();
        assert_eq!(subjects(&p), vec!["B", "A, reworded"]);
    }

    #[test]
    fn a_stale_plan_or_a_dirty_tree_is_refused_before_git_runs() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        // the dialog listed A..C; a commit landed on top since
        let n = commit_file(p, "n.txt", "n\n", "new");
        let m = rule(start(
            p,
            d.path(),
            &a,
            &[step(&a, Pick), step(&b, Pick), step(&c, Drop)],
        ));
        assert!(m.contains("history changed"), "{m}");
        assert_eq!(git(p, &["rev-parse", "HEAD"]), n);

        std::fs::write(p.join("a.txt"), "dirty\n").unwrap();
        let m = rule(reword(p, d.path(), &b, "x"));
        assert!(m.contains("uncommitted"), "{m}");
        assert!(!plan_dir(d.path(), p).exists());
    }

    /// Rewording HEAD is `commit --amend --only`: what is staged stays staged and
    /// out of the commit, and a dirty tree does not stop it.
    #[test]
    fn rewording_head_leaves_the_index_alone() {
        let (dir, _) = repo_abc();
        let p = dir.path();
        let d = data();
        let tree = git(p, &["rev-parse", "HEAD^{tree}"]);
        std::fs::write(p.join("staged.txt"), "staged\n").unwrap();
        git(p, &["add", "staged.txt"]);
        std::fs::write(p.join("a.txt"), "unstaged\n").unwrap();
        let head = git(p, &["rev-parse", "HEAD"]);

        reword(p, d.path(), &head, "C, reworded\n\n#keep").unwrap();

        assert_eq!(message(p, "HEAD"), "C, reworded\n\n#keep");
        assert_eq!(
            git(p, &["rev-parse", "HEAD^{tree}"]),
            tree,
            "the tree is HEAD's own"
        );
        assert_eq!(git(p, &["diff", "--cached", "--name-only"]), "staged.txt");
        assert_eq!(git(p, &["diff", "--name-only"]), "a.txt");
        assert_eq!(subjects(p).len(), 4, "amended, not added");
    }

    #[test]
    fn an_operation_in_progress_is_refused() {
        let (dir, [a, b, c]) = repo_abc();
        let p = dir.path();
        let d = data();
        start(
            p,
            d.path(),
            &a,
            &[step(&a, Edit), step(&b, Pick), step(&c, Pick)],
        )
        .unwrap();
        assert!(rule(reword(p, d.path(), &a, "x")).contains("in progress"));
        assert!(rule(squash(p, d.path(), &[b.clone(), c.clone()], "x")).contains("in progress"));
    }
}
