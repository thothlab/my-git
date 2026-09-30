//! Remotes of the open repository, and cloning a new one.
//!
//! ## Which addresses are accepted
//!
//! One rule, [`check_url`], for `remote add`, `remote set-url` and `clone`:
//!
//! - `https://`, `http://`, `ssh://`, `git://`, `file://`, scp syntax
//!   (`[user@]host:path`) and a local path written as one (`/abs`, `./rel`, `../rel`).
//! - **No password or token in the address.** A URL in `.git/config` is read by
//!   every tool that opens the repository, printed by `git remote -v`, and echoed
//!   by git in its own error messages; the journal masks what it can
//!   (`exec::mask_credentials`), but the only safe copy of a token is the one never
//!   written there. So a password in the userinfo is refused, and so is a user name
//!   that looks like a token (`exec::looks_like_token`: `ghp_…`, `glpat-…`,
//!   `x-access-token`, 32+ characters of `[A-Za-z0-9_-]`, … — a bare
//!   `https://ghp_…@github.com` is how a token is usually embedded); an http(s)
//!   query string is refused too (`?access_token=`). A plain login stays —
//!   `git@host`, and the `https://me@bitbucket.org/…` Bitbucket and Azure DevOps hand
//!   out: git asks for its password through the credential helper, and the dialog
//!   says so. Every refusal names the alternative — a credential helper.
//! - **No `<transport>::<address>`.** `ext::` runs an arbitrary command, `fd::`
//!   talks to file descriptors, any other name runs a `git-remote-<name>` program
//!   from the path. None of that is an address a person types into a dialog.
//! - Anything else — a bare word git would take for a relative path — is refused
//!   with how to write a local path, rather than guessed at.
//!
//! Addresses already in the configuration are not re-judged: the list shows them
//! masked and says which carry credentials, so the reader can replace them.
//!
//! ## Clone
//!
//! `git clone --progress` streamed through `exec::Git::stream` — stderr segment by
//! segment, `\r`-redrawn meters included — cancellable by killing the process. The
//! destination is `<parent>/<name>` and must not exist or be an empty folder; after
//! a failure or a cancellation it is put back as it was: removed when the clone
//! created it, emptied when it was an empty folder before. Nothing else is ever
//! deleted — a destination that was not empty is refused before git starts.
//!
//! No prompt can open: the clone is a network run (`exec::Git::network`, the same
//! environment as push / fetch / pull — `GIT_TERMINAL_PROMPT=0`, no inherited
//! askpass, ssh with `BatchMode=yes` unless the user has an ssh command of their
//! own). Why BatchMode at all: started from a terminal, Graft has a controlling
//! tty, and ssh would ask for a passphrase or a host key there — a clone hung on a
//! question in a window the reader is not looking at. Started from Finder there is
//! no tty and ssh fails at once either way; keys held by ssh-agent work in both.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use super::exec;
use crate::error::{Error, Result};
use crate::model::{BackgroundFetch, OperationKind, RemoteInfo};

/// What a refused credential-bearing address is told.
const CREDENTIALS: &str = "do not keep a password or token in the remote address: .git/config, \
     `git remote -v` and git's own error messages all show it. Use a credential helper \
     (git-credential-osxkeychain, `gh auth login`, a password manager's helper) instead";

/// Schemes an address may use.
const SCHEMES: &[&str] = &["https", "http", "ssh", "git", "file"];

// ── addresses ────────────────────────────────────────────────────────────────

/// Refuse an address Graft does not write into a configuration or clone from — see
/// the module docs for the rule. Pure: no git, no file system.
pub fn check_url(url: &str) -> Result<()> {
    let refuse = |why: String| Err(Error::Rule(why));
    if url.trim().is_empty() {
        return refuse("the remote address is empty".into());
    }
    if url.chars().any(char::is_control) {
        return refuse("the remote address contains a control character".into());
    }
    if url.starts_with('-') {
        return refuse("a remote address cannot start with \"-\"".into());
    }
    if url != url.trim() {
        return refuse("the remote address starts or ends with a space".into());
    }
    if let Some(i) = url.find("::") {
        let t = &url[..i];
        if !t.is_empty()
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_'))
        {
            return refuse(format!(
                "\"{t}::\" runs a helper program instead of naming a repository; \
                 use an https, ssh, git or file address, or a local path"
            ));
        }
    }
    if let Some(i) = url.find("://") {
        let scheme = &url[..i];
        let is_scheme = scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            return check_scheme_url(&scheme.to_ascii_lowercase(), &url[i + 3..]);
        }
    }
    if is_local_path(url) {
        return Ok(());
    }
    // scp syntax: a colon before any slash.
    if let Some(colon) = url.find(':') {
        if !url[..colon].contains('/') {
            return check_scp(url, colon);
        }
    }
    refuse(format!(
        "\"{url}\" is not an address Graft recognises: use https://…, ssh://…, \
         user@host:path, or a local path written as /absolute, ./relative or ../relative"
    ))
}

fn is_local_path(url: &str) -> bool {
    if url.starts_with('/')
        || url.starts_with("./")
        || url.starts_with("../")
        || url == "."
        || url == ".."
    {
        return true;
    }
    #[cfg(windows)]
    {
        let b = url.as_bytes();
        if b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\')
        {
            return true;
        }
        if url.starts_with(".\\") || url.starts_with("..\\") || url.starts_with("\\\\") {
            return true;
        }
    }
    false
}

fn check_scheme_url(scheme: &str, rest: &str) -> Result<()> {
    let refuse = |why: String| Err(Error::Rule(why));
    if !SCHEMES.contains(&scheme) {
        return refuse(format!(
            "\"{scheme}://\" is not a transport Graft uses: use https, http, ssh, git or file"
        ));
    }
    if rest.chars().any(char::is_whitespace) {
        return refuse("a URL cannot contain spaces (write them as %20)".into());
    }
    let web = scheme == "https" || scheme == "http";
    if web && (rest.contains('?') || rest.contains('#')) {
        return refuse(format!(
            "a query string or fragment in a repository URL usually carries a token — {CREDENTIALS}"
        ));
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
        None => (None, authority),
    };
    if let Some(user) = userinfo {
        match scheme {
            "https" | "http" | "ssh" if user.contains(':') => return refuse(CREDENTIALS.into()),
            // Not echoed: the part that may be the secret stays out of the message.
            "https" | "http" | "ssh" if exec::looks_like_token(user) => {
                return refuse(format!("the user name looks like a token — {CREDENTIALS}"))
            }
            "https" | "http" | "ssh" if user.is_empty() => {
                return refuse("the user name before \"@\" is empty".into())
            }
            // A plain login (Bitbucket and Azure DevOps put one here): git asks for
            // the password through the credential helper.
            "https" | "http" | "ssh" => {}
            _ => {
                return refuse(format!(
                    "a {scheme}:// address has no user name; remove the part before \"@\""
                ))
            }
        }
    }
    let host = strip_port(hostport);
    let path = &rest[auth_end..];
    if scheme == "file" {
        if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
            return refuse(
                "a file:// address names a folder on this computer: file:///path".into(),
            );
        }
        if path.len() <= 1 {
            return refuse("the file:// address names no folder".into());
        }
        return Ok(());
    }
    if host.is_empty() {
        return refuse(format!("the {scheme}:// address names no host"));
    }
    if path.len() <= 1 {
        return refuse(format!(
            "the {scheme}:// address names no repository on {host}"
        ));
    }
    Ok(())
}

/// `host:port` → `host`; a bracketed IPv6 literal keeps its colons.
fn strip_port(hostport: &str) -> &str {
    if hostport.starts_with('[') {
        return hostport.find(']').map_or(hostport, |i| &hostport[..=i]);
    }
    hostport.rsplit_once(':').map_or(hostport, |(h, _)| h)
}

/// `[user@]host:path`, `colon` being the first colon (no slash before it).
fn check_scp(url: &str, colon: usize) -> Result<()> {
    let refuse = |why: String| Err(Error::Rule(why));
    // `user:secret@host:path` — the first colon is inside the userinfo, and an `@`
    // follows it before any slash.
    let upto_slash = &url[..url.find('/').unwrap_or(url.len())];
    if upto_slash[colon..].contains('@') {
        return refuse(CREDENTIALS.into());
    }
    let hostpart = &url[..colon];
    let host = hostpart.rsplit_once('@').map_or(hostpart, |(_, h)| h);
    if let Some((user, _)) = hostpart.rsplit_once('@') {
        if exec::looks_like_token(user) {
            return refuse(format!("the user name looks like a token — {CREDENTIALS}"));
        }
    }
    if hostpart.chars().any(char::is_whitespace) {
        return refuse("the host of the address contains a space".into());
    }
    if hostpart.ends_with('@') || host.is_empty() {
        return refuse("the address names no host before \":\"".into());
    }
    if hostpart.starts_with('@') {
        return refuse("the user name before \"@\" is empty".into());
    }
    if url[colon + 1..].is_empty() {
        return refuse(format!("the address names no repository on {host}"));
    }
    Ok(())
}

/// Does this configured address carry a password or token by the rule above?
fn carries_credentials(url: &str) -> bool {
    matches!(check_url(url), Err(Error::Rule(m)) if m.contains(CREDENTIALS))
}

/// An address as the list shows it: a credential-bearing one masked, by the same
/// `exec::mask_credentials` the journal uses — scheme URLs and scp syntax alike.
fn shown(url: &str) -> String {
    if carries_credentials(url) {
        exec::mask_credentials(url).into_owned()
    } else {
        url.to_string()
    }
}

// ── names ────────────────────────────────────────────────────────────────────

/// Refuse a remote name git would not accept — git's own rule: `refs/remotes/<name>/x`
/// must be a valid ref (no spaces, no `..`, no `~^:?*[\`, no `.lock` ending, …).
/// A leading `-` is refused on top: `git remote add -- -x` takes it, and every later
/// command naming the remote would read it as an option.
///
/// The exit code is tri-state: 1 is "invalid", anything else a question git failed
/// to answer, kept as `Error::Git`.
pub(crate) fn check_remote_name(repo: &Path, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::Rule("remote name is empty".into()));
    }
    let refuse = |why: &str| {
        Err(Error::Rule(format!(
            "\"{name}\" is not a valid remote name{why}"
        )))
    };
    if name.starts_with('-') {
        return refuse(": it cannot start with \"-\"");
    }
    let out = exec::git(
        repo,
        &["check-ref-format", &format!("refs/remotes/{name}/x")],
    )
    .run()?;
    match out.code {
        Some(0) => Ok(()),
        Some(1) => refuse(": no spaces, \"..\", \"~^:?*[\\\" or a \".lock\" ending"),
        _ => Err(out.fail_stderr()),
    }
}

// ── list ─────────────────────────────────────────────────────────────────────

/// Every remote, in the order `git remote` gives, with its URLs (masked where they
/// carry credentials) and how many remote-tracking branches it has.
///
/// Names come from `git remote` — newline-separated, which is safe: a remote name
/// is a ref component and cannot hold a newline. URLs come from the configuration
/// read with `--null`; a remote name may contain dots, so the key is cut at its
/// known prefix and suffix, never split on `.`.
pub fn list(repo: &Path) -> Result<Vec<RemoteInfo>> {
    let mut out: Vec<RemoteInfo> = names(repo)?
        .iter()
        .map(|n| RemoteInfo {
            name: n.clone(),
            fetch_urls: Vec::new(),
            push_urls: Vec::new(),
            has_credentials: false,
            branches: 0,
        })
        .collect();

    let cfg = exec::git(
        repo,
        &[
            "config",
            "--null",
            "--get-regexp",
            r"^remote\..*\.(url|pushurl)$",
        ],
    )
    .run()?;
    let cfg = match cfg.code {
        Some(0) => cfg.stdout,
        Some(1) => Vec::new(), // no key matched: no URLs configured
        _ => return Err(cfg.fail_stderr()),
    };
    for (name, push, url) in parse_url_config(&cfg)? {
        // A remote in the config that `git remote` did not list (a URL with no other
        // settings is still listed; this is belt and braces) gets its own row.
        let idx = match out.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                out.push(RemoteInfo {
                    name: name.clone(),
                    fetch_urls: Vec::new(),
                    push_urls: Vec::new(),
                    has_credentials: false,
                    branches: 0,
                });
                out.len() - 1
            }
        };
        let r = &mut out[idx];
        r.has_credentials |= carries_credentials(&url);
        let url = shown(&url);
        if push {
            r.push_urls.push(url);
        } else {
            r.fetch_urls.push(url);
        }
    }

    let refs = exec::git(
        repo,
        &["for-each-ref", "--format=%(refname)", "refs/remotes/"],
    )
    .run()?
    .checked()?;
    for full in String::from_utf8_lossy(&refs).lines() {
        let Some(rest) = full.strip_prefix("refs/remotes/") else {
            continue;
        };
        // The longest remote name that prefixes the ref: `a` and `a/b` may both exist.
        let owner = out
            .iter_mut()
            .filter(|r| {
                rest.strip_prefix(r.name.as_str())
                    .is_some_and(|t| t.starts_with('/'))
            })
            .max_by_key(|r| r.name.len());
        if let Some(r) = owner {
            if rest[r.name.len() + 1..] != *"HEAD" {
                r.branches += 1;
            }
        }
    }
    Ok(out)
}

/// `(remote, is_push, url)` records of `config --null --get-regexp`: `key\nvalue\0`,
/// or `key\0` for a key with no value.
fn parse_url_config(raw: &[u8]) -> Result<Vec<(String, bool, String)>> {
    let text = String::from_utf8_lossy(raw);
    let mut v = Vec::new();
    for rec in text.split('\0') {
        if rec.is_empty() {
            continue;
        }
        let (key, value) = rec.split_once('\n').unwrap_or((rec, ""));
        let bad = || Error::Parse(format!("unexpected remote configuration key {key:?}"));
        let inner = key.strip_prefix("remote.").ok_or_else(bad)?;
        let (name, push) = if let Some(n) = inner.strip_suffix(".pushurl") {
            (n, true)
        } else if let Some(n) = inner.strip_suffix(".url") {
            (n, false)
        } else {
            return Err(bad());
        };
        if name.is_empty() {
            return Err(bad());
        }
        v.push((name.to_string(), push, value.to_string()));
    }
    Ok(v)
}

fn names(repo: &Path) -> Result<Vec<String>> {
    let out = exec::git(repo, &["remote"]).run()?.checked()?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn require(repo: &Path, name: &str) -> Result<()> {
    if names(repo)?.iter().any(|n| n == name) {
        Ok(())
    } else {
        Err(Error::Rule(format!("there is no remote \"{name}\"")))
    }
}

// ── changes ──────────────────────────────────────────────────────────────────

fn git(repo: &Path, args: &[&str]) -> Result<()> {
    exec::git(repo, args).run()?.checked_both()?;
    Ok(())
}

/// `git remote add` — the address checked first, nothing fetched.
pub fn add(repo: &Path, name: &str, url: &str) -> Result<()> {
    check_remote_name(repo, name)?;
    check_url(url)?;
    if names(repo)?.iter().any(|n| n == name) {
        return Err(Error::Rule(format!(
            "a remote named \"{name}\" already exists"
        )));
    }
    git(repo, &["remote", "add", "--", name, url])
}

/// `git remote rename`: moves the remote-tracking branches and re-points the
/// branches tracking it.
pub fn rename(repo: &Path, from: &str, to: &str) -> Result<()> {
    require(repo, from)?;
    check_remote_name(repo, to)?;
    if from == to {
        return Err(Error::Rule(format!("the remote is already named \"{to}\"")));
    }
    git(repo, &["remote", "rename", "--", from, to])
}

/// `git remote remove`: deletes its remote-tracking branches and unsets the
/// upstream of every branch that tracked it.
pub fn remove(repo: &Path, name: &str) -> Result<()> {
    require(repo, name)?;
    git(repo, &["remote", "remove", "--", name])
}

/// Change the address a remote fetches from (`push` false) or pushes to (`push`
/// true). An empty push address removes the separate one, and pushes go to the
/// fetch address again.
pub fn set_url(repo: &Path, name: &str, url: &str, push: bool) -> Result<()> {
    require(repo, name)?;
    if push && url.is_empty() {
        let key = format!("remote.{name}.pushurl");
        let out = exec::git(repo, &["config", "--unset-all", &key]).run()?;
        // 5: there was nothing to unset — already the state asked for.
        return match out.code {
            Some(0) | Some(5) => Ok(()),
            _ => Err(out.fail_both()),
        };
    }
    check_url(url)?;
    let mut args = vec!["remote", "set-url"];
    if push {
        args.push("--push");
    }
    args.extend(["--", name, url]);
    git(repo, &args)
}

// ── clone ────────────────────────────────────────────────────────────────────

/// Where a clone goes, and whether that folder was there (empty) before.
#[derive(Debug)]
pub(crate) struct CloneTarget {
    pub parent: PathBuf,
    pub path: PathBuf,
    existed: bool,
}

/// Refuse a folder name that is not one plain name inside the chosen parent.
pub fn check_folder_name(name: &str) -> Result<()> {
    let refuse = |why: &str| {
        Err(Error::Rule(format!(
            "\"{name}\" cannot be the new folder's name{why}"
        )))
    };
    if name.trim().is_empty() {
        return Err(Error::Rule("the new folder's name is empty".into()));
    }
    if name == "." || name == ".." {
        return refuse("");
    }
    if name.contains('/') || name.contains('\\') {
        return refuse(": it has to be one name, without \"/\"");
    }
    if name.chars().any(char::is_control) {
        return refuse(": it contains a control character");
    }
    if name != name.trim() {
        return refuse(": it starts or ends with a space");
    }
    Ok(())
}

/// Resolve `<parent>/<name>` and check it may receive a clone: absent, or an empty
/// folder. A symbolic link is refused even when it points at an empty folder —
/// clean-up after a failure would otherwise act on wherever it points.
pub(crate) fn prepare(parent: &Path, name: &str) -> Result<CloneTarget> {
    check_folder_name(name)?;
    if !parent.is_absolute() {
        return Err(Error::Rule(format!(
            "{} is not an absolute path",
            parent.display()
        )));
    }
    let parent = parent.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            Error::Rule(format!("no such folder: {}", parent.display()))
        }
        _ => Error::Io(format!("{}: {e}", parent.display())),
    })?;
    if !parent.is_dir() {
        return Err(Error::Rule(format!("{} is not a folder", parent.display())));
    }
    let path = parent.join(name);
    let existed = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(Error::Io(format!("{}: {e}", path.display()))),
        Ok(m) if m.is_dir() => {
            if std::fs::read_dir(&path)?.next().is_some() {
                return Err(Error::Rule(format!(
                    "{} already exists and is not empty: choose another folder name",
                    path.display()
                )));
            }
            true
        }
        Ok(_) => {
            return Err(Error::Rule(format!(
                "{} already exists and is not a folder: choose another folder name",
                path.display()
            )))
        }
    };
    Ok(CloneTarget {
        parent,
        path,
        existed,
    })
}

/// Put the destination back as it was before the clone: removed when the clone
/// created it, emptied when it was an empty folder. A helper git started may still
/// be writing for a moment after a cancellation, so a pass that fails is retried.
pub(crate) fn cleanup(t: &CloneTarget) -> Result<()> {
    let pass = || -> std::io::Result<()> {
        match std::fs::symlink_metadata(&t.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        if !t.existed {
            return std::fs::remove_dir_all(&t.path);
        }
        for entry in std::fs::read_dir(&t.path)? {
            let entry = entry?;
            let p = entry.path();
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(&p)?;
            } else {
                std::fs::remove_file(&p)?;
            }
        }
        Ok(())
    };
    let mut last = Ok(());
    for _ in 0..5 {
        last = pass();
        if last.is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    last.map_err(|e| Error::Io(format!("{}: {e}", t.path.display())))
}

/// git's stderr with every `\r`-redrawn meter collapsed to its last state — the
/// text of a failed clone's error, without a hundred "Receiving objects" lines.
fn collapse_meters(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .split('\n')
        .map(|l| l.rsplit('\r').find(|s| !s.trim().is_empty()).unwrap_or(""))
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// How often progress reaches the window: a meter redraws per percent, per phase.
const PROGRESS_EVERY: Duration = Duration::from_millis(80);

/// Clone `url` into `<parent>/<name>`. `Ok(Some(path))` on success, `Ok(None)` when
/// `cancel` was set and the clone stopped; the destination is put back as it was
/// after either a failure or a cancellation ([`cleanup`]). `on_line` receives the
/// progress, masked, at most every [`PROGRESS_EVERY`] (the latest line wins).
pub fn clone(
    parent: &Path,
    name: &str,
    url: &str,
    cancel: &AtomicBool,
    on_line: impl FnMut(&str),
) -> Result<Option<PathBuf>> {
    clone_with(parent, name, url, &[], cancel, on_line)
}

/// [`clone`] with extra options before `--` — the tests' way to slow a clone down
/// (`-u 'sleep …; git-upload-pack'`); the command layer never passes any.
fn clone_with(
    parent: &Path,
    name: &str,
    url: &str,
    extra: &[&str],
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<Option<PathBuf>> {
    check_url(url)?;
    let target = prepare(parent, name)?;
    let dest = target.path.to_string_lossy().to_string();
    let mut args = vec!["clone", "--progress"];
    args.extend_from_slice(extra);
    args.extend(["--", url, dest.as_str()]);

    let mut last: Option<Instant> = None;
    let mut pending: Option<String> = None;
    let mut throttled = |seg: &str| {
        if last.map_or(true, |t| t.elapsed() >= PROGRESS_EVERY) {
            on_line(seg);
            last = Some(Instant::now());
            pending = None;
        } else {
            pending = Some(seg.to_string());
        }
    };
    let res = exec::git(&target.parent, &args)
        .network()
        .stream(cancel, &mut throttled);
    if let Some(p) = pending.take() {
        on_line(&p);
    }

    match res {
        Ok(s) if s.cancelled => {
            cleanup(&target)?;
            Ok(None)
        }
        Ok(s) if s.output.success() => Ok(Some(target.path)),
        Ok(s) => {
            let _ = cleanup(&target);
            Err(s.output.fail_with(collapse_meters(&s.output.stderr)))
        }
        Err(e) => {
            let _ = cleanup(&target);
            Err(e)
        }
    }
}

/// The scheduled background fetch (`repo_fetch_background`): nothing when the
/// repository has no remote or an operation is unfinished, else
/// [`CliEngine::fetch_background`](crate::engine::cli::CliEngine::fetch_background)
/// recorded through [`Undo::perform_background`](crate::engine::undo::Undo::perform_background)
/// — which gives way (`Busy`) when anything already runs on the repository.
pub fn fetch_background(
    undo: &crate::engine::undo::Undo,
    data: Option<&Path>,
    repo: &Path,
) -> Result<BackgroundFetch> {
    let names = exec::git(repo, &["remote"]).run()?.checked()?;
    if String::from_utf8_lossy(&names).trim().is_empty() {
        return Ok(BackgroundFetch::NoRemotes);
    }
    if crate::engine::ops::detect_state(repo)?.kind != OperationKind::None {
        return Ok(BackgroundFetch::Operation);
    }
    match undo.perform_background(data, repo, "fetch_background", || {
        crate::engine::cli::CliEngine::new(repo).fetch_background()
    }) {
        None => Ok(BackgroundFetch::Busy),
        Some(r) => r.map(|()| BackgroundFetch::Fetched),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::branches;
    use crate::engine::cli::tests::scratch_repo;
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn run(dir: &Path, args: &[&str]) {
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
    }

    fn rule(r: Result<()>) -> String {
        match r {
            Err(Error::Rule(m)) => m,
            other => panic!("expected Error::Rule, got {other:?}"),
        }
    }

    #[test]
    fn accepts_the_usual_addresses() {
        for u in [
            "https://github.com/org/repo.git",
            "http://intranet.local/repo",
            "https://host:8443/a/b",
            "ssh://git@github.com/org/repo.git",
            "ssh://host:2222/srv/repo.git",
            "ssh://[::1]:22/repo",
            "git://host/repo.git",
            "file:///srv/git/repo.git",
            "file://localhost/srv/repo",
            "git@github.com:org/repo.git",
            "host:repo",
            "/abs/path/repo.git",
            "/path with spaces/repo",
            "./rel",
            "../sibling/repo",
            "HTTPS://Host/Repo",
            "https://me@bitbucket.org/team/r.git",
            "https://org@dev.azure.com/org/p/_git/r",
            "http://user@host/r",
            "me@host:repo.git",
            "https://abcdefghijklmnopqrstuvwxyz01234@h/r", // 31 characters: a login
        ] {
            assert!(check_url(u).is_ok(), "{u}: {:?}", check_url(u));
        }
    }

    #[test]
    fn refuses_credentials_with_the_helper_named() {
        for u in [
            "https://user:token@github.com/o/r.git",
            "https://ghp_abc123@github.com/o/r",
            "https://host/r.git?access_token=abc",
            "https://host/r.git#frag",
            "ssh://git:secret@host/r",
            "user:secret@host:org/repo.git",
            "https://ghp_abc123@github.com/o/r",
            "https://gho_x@github.com/o/r",
            "https://ghu_x@github.com/o/r",
            "https://ghs_x@github.com/o/r",
            "https://github_pat_11AB@github.com/o/r",
            "https://glpat-xyz@gitlab.com/o/r",
            "https://x-access-token@github.com/o/r",
            "https://oauth2@gitlab.com/o/r",
            "https://x-token-auth@bitbucket.org/o/r",
            "https://abcdefghijklmnopqrstuvwxyz012345@h/r",
            "ssh://ghp_abc@host/r",
            "ghp_abc@host:org/r",
        ] {
            let m = rule(check_url(u));
            assert!(m.contains("credential helper"), "{u}: {m}");
            assert!(carries_credentials(u), "{u}");
        }
        assert!(!carries_credentials("git@github.com:org/repo.git"));
        assert!(!carries_credentials("ssh://git@host/r"));
        assert!(!carries_credentials("https://me@bitbucket.org/team/r.git"));
        let m = rule(check_url("https://ghp_SECRETVALUE@github.com/o/r"));
        assert!(!m.contains("SECRETVALUE"), "the token is not echoed: {m}");
    }

    #[test]
    fn refuses_helper_transports_and_unknown_forms() {
        for u in [
            "ext::sh -c touch% /tmp/pwned",
            "fd::17",
            "persistent-https::host/r",
            "hg::https://host/r",
        ] {
            assert!(rule(check_url(u)).contains("helper program"), "{u}");
        }
        for u in ["ftp://host/r", "rsync://host/r", "javascript://x/y"] {
            assert!(rule(check_url(u)).contains("not a transport"), "{u}");
        }
        for u in [
            "",
            "   ",
            "-uevil",
            "repo",
            "host/path",
            "https://",
            "https://host",
            "https:///path",
            "git://user@host/r",
            "file://otherhost/r",
            "https://host/a b",
            " https://host/r",
            "https://host/r\n",
            "git@:path",
            "host:",
        ] {
            assert!(check_url(u).is_err(), "{u:?} should be refused");
        }
    }

    #[test]
    fn credentialed_addresses_are_shown_masked() {
        assert_eq!(shown("https://u:t@h/r"), "https://***@h/r");
        assert_eq!(shown("user:secret@host:org/r"), "***@host:org/r");
        assert_eq!(shown("git@host:org/r"), "git@host:org/r");
        assert_eq!(
            shown("ssh://git@host/r"),
            "ssh://git@host/r",
            "a user alone is not a secret"
        );
        assert_eq!(
            shown("https://me@bitbucket.org/t/r"),
            "https://me@bitbucket.org/t/r"
        );
        assert_eq!(
            shown("https://ghp_x@github.com/o/r"),
            "https://***@github.com/o/r"
        );
    }

    #[test]
    fn remote_names_follow_gits_rule() {
        let dir = scratch_repo();
        let p = dir.path();
        for ok in ["origin", "up-stream", "team/fork", "a.b"] {
            assert!(check_remote_name(p, ok).is_ok(), "{ok}");
        }
        for bad in ["", "a b", "a..b", "-x", "x.lock", "a:b", "a~b", "a/"] {
            assert!(
                matches!(check_remote_name(p, bad), Err(Error::Rule(_))),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn add_list_set_url_rename_remove() {
        let dir = scratch_repo();
        let p = dir.path();
        assert!(
            list(p).unwrap().is_empty(),
            "no remotes: an empty list, not an error"
        );

        add(p, "origin", "https://h/a.git").unwrap();
        add(p, "team.fork", "git@h:team/a.git").unwrap();
        let l = list(p).unwrap();
        assert_eq!(l.len(), 2);
        let tf = l.iter().find(|r| r.name == "team.fork").unwrap();
        assert_eq!(
            tf.fetch_urls,
            vec!["git@h:team/a.git"],
            "a dotted name parsed whole"
        );
        assert!(tf.push_urls.is_empty());

        assert!(rule(add(p, "origin", "https://h/b.git")).contains("already exists"));
        assert!(rule(add(p, "bad name", "https://h/b.git")).contains("not a valid"));
        assert!(rule(add(p, "cred", "https://u:t@h/b.git")).contains("credential helper"));
        assert!(rule(add(p, "ext", "ext::sh -c id")).contains("helper program"));
        assert_eq!(list(p).unwrap().len(), 2, "refusals changed nothing");

        set_url(p, "origin", "https://h/b.git", false).unwrap();
        set_url(p, "origin", "ssh://git@h/b.git", true).unwrap();
        let o = list(p)
            .unwrap()
            .into_iter()
            .find(|r| r.name == "origin")
            .unwrap();
        assert_eq!(o.fetch_urls, vec!["https://h/b.git"]);
        assert_eq!(o.push_urls, vec!["ssh://git@h/b.git"]);
        set_url(p, "origin", "", true).unwrap();
        set_url(p, "origin", "", true).unwrap(); // nothing left to unset: still fine
        let o = list(p)
            .unwrap()
            .into_iter()
            .find(|r| r.name == "origin")
            .unwrap();
        assert!(
            o.push_urls.is_empty(),
            "pushes follow the fetch address again"
        );
        assert!(rule(set_url(p, "origin", "http://u:p@h/r", false)).contains("credential"));
        assert!(rule(set_url(p, "origin", "https://glpat-x@h/r", false)).contains("credential"));
        set_url(p, "origin", "https://me@bitbucket.org/t/r.git", false).unwrap();
        let o = list(p)
            .unwrap()
            .into_iter()
            .find(|r| r.name == "origin")
            .unwrap();
        assert_eq!(o.fetch_urls, vec!["https://me@bitbucket.org/t/r.git"]);
        assert!(!o.has_credentials, "a login is not a secret");
        set_url(p, "origin", "https://h/b.git", false).unwrap();
        assert!(rule(set_url(p, "nope", "https://h/r", false)).contains("no remote"));

        rename(p, "origin", "upstream").unwrap();
        assert!(rule(rename(p, "upstream", "a..b")).contains("not a valid"));
        assert!(rule(rename(p, "gone", "x")).contains("no remote"));
        let names: Vec<String> = list(p).unwrap().into_iter().map(|r| r.name).collect();
        assert!(names.contains(&"upstream".to_string()) && !names.contains(&"origin".to_string()));

        remove(p, "upstream").unwrap();
        remove(p, "team.fork").unwrap();
        assert!(list(p).unwrap().is_empty());
        assert!(rule(remove(p, "upstream")).contains("no remote"));
    }

    /// A token stored before Graft refused them is listed masked and flagged.
    #[test]
    fn a_stored_token_is_masked_in_the_list() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["remote", "add", "old", "https://me:s3cret@h/r.git"]);
        let l = list(p).unwrap();
        assert_eq!(l[0].fetch_urls, vec!["https://***@h/r.git"]);
        assert!(l[0].has_credentials);
    }

    /// An scp-syntax address with a password, stored before Graft refused them: the
    /// list shows it masked, and so does the journal's copy of the configuration
    /// read behind the list — which used to keep the password verbatim.
    #[test]
    fn a_stored_scp_password_is_masked_in_the_list_and_the_journal() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["remote", "add", "old", "me:s3cret@host:org/r.git"]);
        let l = list(p).unwrap();
        assert_eq!(l[0].fetch_urls, vec!["***@host:org/r.git"]);
        assert!(l[0].has_credentials);

        let repo = p.display().to_string();
        let reads: Vec<_> = exec::journal_list(false, None)
            .into_iter()
            .filter(|e| e.repo == repo && e.argv.first().is_some_and(|a| a == "config"))
            .collect();
        assert!(!reads.is_empty());
        for e in reads {
            let out = exec::journal_output(e.id).unwrap();
            assert!(!out.stdout.contains("s3cret"), "{}", out.stdout);
        }
    }

    /// Removing a remote takes its remote-tracking branches out of the branch tree;
    /// the list counted them first, for the confirmation.
    #[test]
    fn removing_a_remote_removes_its_branches_from_the_tree() {
        let dir = scratch_repo();
        let p = dir.path();
        add(p, "origin", "https://h/a.git").unwrap();
        add(p, "fork", "https://h/b.git").unwrap();
        run(p, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(p, &["update-ref", "refs/remotes/origin/feature", "HEAD"]);
        run(
            p,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        run(p, &["update-ref", "refs/remotes/fork/x", "HEAD"]);
        let l = list(p).unwrap();
        let count = |n: &str| l.iter().find(|r| r.name == n).unwrap().branches;
        assert_eq!(count("origin"), 2, "origin/HEAD is not a branch");
        assert_eq!(count("fork"), 1);

        let remote_refs = |p: &Path| {
            branches::tree(p)
                .unwrap()
                .into_iter()
                .filter(|b| b.is_remote)
                .map(|b| b.full_ref)
                .collect::<Vec<_>>()
        };
        assert_eq!(remote_refs(p).len(), 3);
        remove(p, "origin").unwrap();
        assert_eq!(remote_refs(p), vec!["refs/remotes/fork/x"]);
    }

    // ── clone ──

    /// A bare repository with one commit, to clone from.
    fn bare_source() -> (tempfile::TempDir, PathBuf) {
        let work = scratch_repo();
        let holder = tempfile::tempdir().unwrap();
        let bare = holder.path().join("src.git");
        run(
            holder.path(),
            &[
                "clone",
                "-q",
                "--bare",
                &work.path().display().to_string(),
                "src.git",
            ],
        );
        (holder, bare)
    }

    fn head_of(p: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn clones_a_local_bare_repository_by_path_and_by_file_url() {
        let (_h, bare) = bare_source();
        let parent = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);

        let by_path = clone(parent.path(), "a", &bare.display().to_string(), &no, |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(head_of(&by_path), head_of(&bare));

        let mut lines = Vec::new();
        let url = format!("file://{}", bare.display());
        std::fs::create_dir(parent.path().join("b")).unwrap(); // an empty folder is fine
        let by_url = clone(parent.path(), "b", &url, &no, |l| lines.push(l.to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(head_of(&by_url), head_of(&bare));
        assert!(!lines.is_empty(), "progress arrived while cloning");
        assert!(by_url.is_absolute());
    }

    #[test]
    fn refuses_a_destination_that_is_not_empty() {
        let (_h, bare) = bare_source();
        let parent = tempfile::tempdir().unwrap();
        std::fs::create_dir(parent.path().join("full")).unwrap();
        std::fs::write(parent.path().join("full/keep.txt"), "mine").unwrap();
        std::fs::write(parent.path().join("file"), "x").unwrap();
        let no = AtomicBool::new(false);
        let src = bare.display().to_string();
        match clone(parent.path(), "full", &src, &no, |_| {}) {
            Err(Error::Rule(m)) => assert!(m.contains("not empty"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(
            parent.path().join("full/keep.txt").exists(),
            "nothing was touched"
        );
        assert!(matches!(
            clone(parent.path(), "file", &src, &no, |_| {}),
            Err(Error::Rule(_))
        ));
        for bad in ["", "..", "a/b", "."] {
            assert!(
                matches!(
                    clone(parent.path(), bad, &src, &no, |_| {}),
                    Err(Error::Rule(_))
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            clone(Path::new("relative"), "x", &src, &no, |_| {}),
            Err(Error::Rule(_))
        ));
        assert!(matches!(
            clone(parent.path(), "x", "https://u:t@h/r", &no, |_| {}),
            Err(Error::Rule(_))
        ));
    }

    #[test]
    fn a_failed_clone_leaves_the_destination_as_it_was() {
        let parent = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let missing = parent.path().join("no-such.git").display().to_string();
        assert!(clone(parent.path(), "new", &missing, &no, |_| {}).is_err());
        assert!(
            !parent.path().join("new").exists(),
            "created by the clone: removed"
        );

        std::fs::create_dir(parent.path().join("empty")).unwrap();
        assert!(clone(parent.path(), "empty", &missing, &no, |_| {}).is_err());
        assert!(
            parent.path().join("empty").is_dir(),
            "was there before: kept, empty"
        );
    }

    #[test]
    fn cleanup_removes_what_it_created_and_empties_what_was_there() {
        let parent = tempfile::tempdir().unwrap();
        let created = prepare(parent.path(), "c").unwrap();
        std::fs::create_dir_all(created.path.join(".git/objects")).unwrap();
        std::fs::write(created.path.join(".git/HEAD"), "x").unwrap();
        cleanup(&created).unwrap();
        assert!(!created.path.exists());

        std::fs::create_dir(parent.path().join("e")).unwrap();
        let existed = prepare(parent.path(), "e").unwrap();
        std::fs::create_dir_all(existed.path.join(".git/refs")).unwrap();
        std::fs::write(existed.path.join("f.txt"), "x").unwrap();
        cleanup(&existed).unwrap();
        assert!(existed.path.is_dir());
        assert_eq!(std::fs::read_dir(&existed.path).unwrap().count(), 0);

        cleanup(&created).unwrap(); // nothing there any more: fine
    }

    /// Cancelled while git waits on a slow source: the call returns promptly — it
    /// does not wait for the helper still holding stderr — and the half-made folder
    /// is gone. `file://` so that `-u` is honoured (a plain path takes git's local
    /// shortcut, which never runs upload-pack).
    #[test]
    fn cancelling_a_clone_stops_it_and_removes_the_folder() {
        let (_h, bare) = bare_source();
        let parent = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = {
            let c = Arc::clone(&cancel);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(700));
                c.store(true, Ordering::SeqCst);
            })
        };
        let started = Instant::now();
        let url = format!("file://{}", bare.display());
        let r = clone_with(
            parent.path(),
            "slow",
            &url,
            &["-u", "sleep 20; git-upload-pack"],
            &cancel,
            |_| {},
        )
        .unwrap();
        setter.join().unwrap();
        assert!(r.is_none(), "cancelled");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(!parent.path().join("slow").exists());
    }

    #[test]
    fn meters_collapse_to_their_last_state() {
        let raw = b"Cloning into 'x'...\nReceiving objects:  50% (1/2)\rReceiving objects: 100% (2/2), done.\nfatal: boom\n";
        assert_eq!(
            collapse_meters(raw),
            "Cloning into 'x'...\nReceiving objects: 100% (2/2), done.\nfatal: boom"
        );
    }

    // ---- background fetch ----

    /// `p` (scratch repo on `main`, tracking `origin/main` in a bare remote) and a
    /// second clone `other` of the same remote to push from.
    fn with_remote() -> (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir) {
        let local = scratch_repo();
        let bare = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let p = local.path();
        run(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        run(
            p,
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        run(p, &["push", "-q", "-u", "origin", "main"]);
        run(
            other.path(),
            &["clone", "-q", bare.path().to_str().unwrap(), "."],
        );
        run(other.path(), &["config", "user.email", "o@example.com"]);
        run(other.path(), &["config", "user.name", "Other"]);
        (local, bare, other)
    }

    /// A second remote `up2` of `p`, a bare repository holding `main`.
    fn second_remote(p: &Path) -> tempfile::TempDir {
        let bare = tempfile::tempdir().unwrap();
        run(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        run(p, &["remote", "add", "up2", bare.path().to_str().unwrap()]);
        run(p, &["push", "-q", "up2", "main"]);
        run(p, &["fetch", "-q", "up2"]);
        bare
    }

    fn rev(dir: &Path, r: &str) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--verify", "-q", r])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn a_background_fetch_brings_commits_and_branches_and_keeps_the_undo_chain() {
        use crate::engine::cli::CliEngine;
        use crate::engine::undo::{Hint, Undo};
        use crate::model::JournalOrigin;

        let (local, _bare, other) = with_remote();
        let p = local.path();
        // Two remotes: git then runs a child fetch per remote, and the flags have
        // to reach them.
        let up2 = second_remote(p);
        let data = tempfile::tempdir().unwrap();
        let undo = Undo::default();
        // A step the person can undo, recorded before the fetch.
        undo.perform(
            Some(data.path()),
            p,
            "branch_create",
            Hint::args(["topic"]),
            || CliEngine::new(p).create_branch("topic", None),
        )
        .unwrap();
        let offered = undo.state(Some(data.path()), p).unwrap().undo.id;
        assert!(offered.is_some());

        // The upstream of the current branch moves on, and a new branch appears.
        let o = other.path();
        std::fs::write(o.join("new.txt"), "x\n").unwrap();
        run(o, &["add", "new.txt"]);
        run(o, &["commit", "-q", "-m", "upstream work"]);
        run(o, &["push", "-q", "origin", "main"]);
        run(o, &["push", "-q", "origin", "HEAD:refs/heads/feature"]);
        let u = up2.path().to_str().unwrap();
        run(o, &["push", "-q", u, "HEAD:refs/heads/second"]);
        let tip = rev(o, "HEAD");
        let _ = std::fs::remove_file(p.join(".git/FETCH_HEAD"));

        let got = fetch_background(&undo, Some(data.path()), p).unwrap();
        assert_eq!(got, BackgroundFetch::Fetched);
        assert_eq!(rev(p, "refs/remotes/origin/main"), tip);
        assert_eq!(rev(p, "refs/remotes/origin/feature"), tip);
        assert_eq!(rev(p, "refs/remotes/up2/second"), tip, "every remote");
        // Remote-tracking refs are not in the digest: the step is still offered.
        let s = undo.state(Some(data.path()), p).unwrap();
        assert_eq!(s.undo.id, offered, "{:?}", s.undo.reason);
        // No FETCH_HEAD for a terminal's `git merge FETCH_HEAD` to trip over.
        assert!(!p.join(".git/FETCH_HEAD").exists());

        // Journaled as the application's own work, not the person's.
        let repo = p.display().to_string();
        let fetch = exec::journal_list(false, None)
            .into_iter()
            .filter(|e| e.repo == repo && e.argv.first().map(String::as_str) == Some("fetch"))
            .last()
            .expect("the fetch is journaled");
        assert_eq!(fetch.origin, JournalOrigin::Background);
        assert_eq!(fetch.action, None);

        // A fetch that brings nothing changes nothing either.
        assert_eq!(
            fetch_background(&undo, Some(data.path()), p).unwrap(),
            BackgroundFetch::Fetched
        );
        assert_eq!(undo.state(Some(data.path()), p).unwrap().undo.id, offered);
    }

    #[test]
    fn a_background_fetch_does_not_prune() {
        use crate::engine::undo::Undo;
        let (local, _bare, other) = with_remote();
        let p = local.path();
        let up2 = second_remote(p);
        let u = up2.path().to_str().unwrap();
        let o = other.path();
        run(o, &["push", "-q", "origin", "HEAD:refs/heads/gone"]);
        run(o, &["push", "-q", u, "HEAD:refs/heads/gone2"]);
        run(p, &["fetch", "-q", "--all"]);
        run(o, &["push", "-q", "origin", "--delete", "gone"]);
        run(o, &["push", "-q", u, "--delete", "gone2"]);
        run(p, &["config", "fetch.prune", "true"]);
        fetch_background(&Undo::default(), None, p).unwrap();
        assert!(!rev(p, "refs/remotes/origin/gone").is_empty());
        assert!(!rev(p, "refs/remotes/up2/gone2").is_empty());
    }

    #[test]
    fn a_failed_background_fetch_says_why_first() {
        use crate::engine::undo::Undo;
        let (local, _bare, _other) = with_remote();
        let p = local.path();
        run(
            p,
            &["remote", "add", "broken", "/nonexistent/graft-test-remote"],
        );
        match fetch_background(&Undo::default(), None, p) {
            Err(Error::Git { stderr, .. }) => {
                assert!(!stderr.starts_with("Fetching"), "{stderr}");
                assert!(
                    stderr.contains("does not appear to be a git repository"),
                    "{stderr}"
                );
            }
            other => panic!("expected Error::Git, got {other:?}"),
        }
    }

    #[test]
    fn a_new_tag_from_a_background_fetch_ends_the_chain_as_fetched() {
        use crate::engine::cli::CliEngine;
        use crate::engine::undo::{Hint, Undo};
        use crate::model::UndoReasonCode;

        let (local, _bare, other) = with_remote();
        let p = local.path();
        let data = tempfile::tempdir().unwrap();
        let undo = Undo::default();
        undo.perform(
            Some(data.path()),
            p,
            "branch_create",
            Hint::args(["topic"]),
            || CliEngine::new(p).create_branch("topic", None),
        )
        .unwrap();
        let o = other.path();
        run(o, &["commit", "-q", "--allow-empty", "-m", "tagged"]);
        run(o, &["tag", "-a", "-m", "v1", "v1"]);
        run(o, &["push", "-q", "origin", "main", "v1"]);

        fetch_background(&undo, Some(data.path()), p).unwrap();
        assert!(
            !rev(p, "refs/tags/v1").is_empty(),
            "git's default tag following"
        );
        let s = undo.state(Some(data.path()), p).unwrap();
        assert_eq!(s.undo.id, None);
        assert_eq!(s.undo.reason.map(|r| r.code), Some(UndoReasonCode::Fetched));
    }

    #[test]
    fn a_background_fetch_skips_an_unfinished_operation_and_a_repo_without_remotes() {
        use crate::engine::undo::Undo;
        let bare_repo = scratch_repo();
        assert_eq!(
            fetch_background(&Undo::default(), None, bare_repo.path()).unwrap(),
            BackgroundFetch::NoRemotes
        );

        let (local, _bare, _other) = with_remote();
        let p = local.path();
        run(p, &["checkout", "-q", "-b", "side"]);
        std::fs::write(p.join("a.txt"), "side\n").unwrap();
        run(p, &["commit", "-q", "-am", "side"]);
        run(p, &["checkout", "-q", "main"]);
        std::fs::write(p.join("a.txt"), "main\n").unwrap();
        run(p, &["commit", "-q", "-am", "main"]);
        let merge = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["merge", "-q", "side"])
            .output()
            .unwrap();
        assert!(!merge.status.success(), "the merge conflicts");
        assert_eq!(
            fetch_background(&Undo::default(), None, p).unwrap(),
            BackgroundFetch::Operation
        );
    }
}
