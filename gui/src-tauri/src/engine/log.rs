//! Reading history and laying out graph lanes.
//!
//! Owns the `git log` format, the parse of `%D` and the streaming lane algorithm;
//! the frontend sees only [`LogPage`]. Output is parsed **by NUL separators**
//! (`%x00` between fields, `%x01` between records) — a commit subject may contain
//! anything, spaces included.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use crate::engine::cli::parse_refs;
use crate::error::{Error, Result};
use crate::model::{
    LaneEdge, LaneEdgeKind, LogCommit, LogCursor, LogFilter, LogOrder, LogPage,
};

/// First slot of `LogCursor.open_lanes`: not a lane but a header naming the history
/// the cursor was cut from — `@<tip hash>:<filter fingerprint>`.
///
/// The cursor crosses to the frontend and comes back unchanged, possibly after a
/// fetch, a rebase or a new commit. Without the header `skip` would point into a
/// stream that has moved and the open lines would continue commits that are no
/// longer there: edges would be drawn, no error raised, and the picture would simply
/// be wrong. With it, a stale cursor is refused and the caller reloads from the top.
const CURSOR_HEADER: char = '@';

/// Fields of one log record, in the order the format string writes them.
const FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%D%x00%s%x01";

/// The lane budget of prd_02 §Решения, written as a number rather than left "as it
/// comes out". Lanes past it are still laid out and reported with their true index —
/// collapsing them into a "+N" marker is the drawing side's decision (task 08); this
/// module only says, through `LogPage.lane_overflow`, that the page went past the
/// budget. Also the size of the colour palette: colour is the lane index modulo this,
/// so neighbouring lanes never share a colour.
const LANE_BUDGET: usize = 12;

/// Run `git -C <repo> <args>` and hand back raw stdout. A git failure keeps its
/// command and stderr verbatim — the reason reaches the UI unfolded.
fn git(repo: &Path, args: &[String]) -> Result<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    if !out.status.success() {
        return Err(Error::Git {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(out.stdout)
}

fn git_text(repo: &Path, args: &[String]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(repo, args)?).to_string())
}

fn s(v: &str) -> String {
    v.to_string()
}

/// Configured remote names, so `origin/main` is told apart from a local branch
/// that merely contains a slash (`feature/x`).
fn remotes(repo: &Path) -> Result<Vec<String>> {
    Ok(git_text(repo, &[s("remote")])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Newest commit of the history this filter selects, or `None` for a repository
/// with no commits. Part of the cursor header — it moves whenever the history the
/// user is paging through moves.
fn tip(repo: &Path, filter: &LogFilter) -> Result<Option<String>> {
    let mut args = vec![s("log"), s("--max-count=1"), s("--format=%H")];
    args.extend(filter_args(filter));
    let out = git_text(repo, &args)?;
    Ok(out.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()))
}

/// Fingerprint of the filter, so a cursor cut under one filter is not applied under
/// another. FNV-1a over the very argument list that produced the page — a filter
/// field that changes nothing in the query changes nothing here either.
fn fingerprint(filter: &LogFilter) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in filter_args(filter).join("\u{1}").bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn cursor_header(repo: &Path, filter: &LogFilter) -> Result<String> {
    Ok(format!(
        "{CURSOR_HEADER}{}:{}",
        tip(repo, filter)?.unwrap_or_default(),
        fingerprint(filter)
    ))
}

/// The lanes a cursor carries, once its header is checked against the history as it
/// is now. An empty cursor is a fresh start, not a stale one.
fn open_lanes_of(repo: &Path, filter: &LogFilter, cursor: Option<&LogCursor>) -> Result<Vec<String>> {
    let open = match cursor {
        Some(c) if !c.open_lanes.is_empty() => &c.open_lanes,
        _ => return Ok(Vec::new()),
    };
    let head = open[0].as_str();
    if !head.starts_with(CURSOR_HEADER) || head != cursor_header(repo, filter)? {
        return Err(Error::Rule(
            "log cursor was cut from a different history (it moved, or the filter changed) — reload the log from the first page".into(),
        ));
    }
    Ok(open[1..].to_vec())
}

/// A filter that keeps only some commits leaves holes in the history: the parent
/// of a shown commit may itself be filtered out, so an edge drawn between two
/// adjacent rows would be an invention. The graph is switched off for exactly
/// these filters (prd_02 R17i.2); branch and ordering keep history contiguous.
///
/// TODO(prd): `LogPage` has no field for this — the signal reaching the UI is that
/// every row comes back with an empty `edges`. An explicit `graphSuppressed: bool`
/// belongs in `model.rs`, which is task 01's zone.
fn breaks_history(filter: &LogFilter) -> bool {
    filter.text.is_some()
        || !filter.authors.is_empty()
        || filter.since.is_some()
        || filter.until.is_some()
        || !filter.paths.is_empty()
}

fn looks_like_hash(text: &str) -> bool {
    let t = text.trim();
    (4..=40).contains(&t.len()) && t.chars().all(|c| c.is_ascii_hexdigit())
}

/// Revision selector and filter flags shared by the page query.
fn filter_args(filter: &LogFilter) -> Vec<String> {
    let mut a = Vec::new();
    a.push(match filter.order {
        LogOrder::Date => s("--date-order"),
        LogOrder::Topo => s("--topo-order"),
    });
    match &filter.branch {
        // A named branch is a single starting point; without one the graph covers
        // every ref, minus the stash — stash commits are not history the user browses.
        Some(b) if !b.trim().is_empty() => a.push(b.trim().to_string()),
        _ => {
            // `--all` means every ref under refs/, which includes the stash and the
            // notes ref — neither is history the user browses, and both showed up
            // as phantom commits until excluded.
            a.push(s("--exclude=refs/stash"));
            a.push(s("--exclude=refs/notes/*"));
            a.push(s("--all"));
        }
    }
    let text = filter.text.as_deref().filter(|t| !t.is_empty());
    // Repeated `--author` is git's own OR: a commit is kept if any of them
    // matches. Blank entries are dropped rather than passed on — `--author=`
    // matches everything, so one empty row in the list would silently undo the
    // whole filter.
    let authors: Vec<&str> = filter
        .authors
        .iter()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .collect();
    if let Some(text) = text {
        a.push(format!("--grep={text}"));
    }
    for author in &authors {
        a.push(format!("--author={author}"));
    }
    // The pattern flags govern `--author` as much as `--grep`: git matches an author
    // as a regular expression by default, so a name with a dot or a bracket in it
    // would over-match or fail outright while the user typed no pattern at all.
    // They are global to the query, so they cover every `--author` at once.
    if text.is_some() || !authors.is_empty() {
        if filter.regex {
            a.push(s("--extended-regexp"));
        } else {
            a.push(s("--fixed-strings"));
        }
        if !filter.match_case {
            a.push(s("--regexp-ignore-case"));
        }
    }
    // `@<unix>` is git's own epoch date form, so no locale-dependent formatting.
    if let Some(since) = filter.since {
        a.push(format!("--since=@{since}"));
    }
    if let Some(until) = filter.until {
        a.push(format!("--until=@{until}"));
    }
    if !filter.paths.is_empty() {
        a.push(s("--"));
        for p in &filter.paths {
            a.push(p.clone());
        }
    }
    a
}

/// One parsed record before lanes are laid out.
struct Row {
    hash: String,
    parents: Vec<String>,
    author: String,
    author_email: String,
    author_at: i64,
    decor: String,
    subject: String,
}

fn parse_rows(out: &str) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for rec in out.split('\u{1}') {
        let rec = rec.trim_start_matches('\n');
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\u{0}').collect();
        if f.len() < 7 {
            return Err(Error::Parse(format!(
                "log record has {} fields, expected 7",
                f.len()
            )));
        }
        rows.push(Row {
            hash: f[0].trim().to_string(),
            parents: f[1].split_whitespace().map(|p| p.to_string()).collect(),
            author: f[2].to_string(),
            author_email: f[3].to_string(),
            author_at: f[4].trim().parse().unwrap_or(0),
            decor: f[5].to_string(),
            subject: f[6].to_string(),
        });
    }
    Ok(rows)
}

fn short(hash: &str) -> String {
    hash.chars().take(7).collect()
}

/// Streaming lane layout.
///
/// `lanes[k]` holds the hash a lane is waiting for; `None` is a free slot. The page
/// starts from the lanes left open at its upper boundary (`LogCursor.open_lanes`,
/// where an empty string marks a free slot) and hands the same shape to the next
/// page, so the second page continues the first without a break.
///
/// Colour is the lane index modulo [`LANE_BUDGET`]: the lane a line occupies is
/// decided by the commit that opened it and never changes while the line lives, so
/// the same commit gets the same colour on every reload and across page boundaries,
/// and two lines drawn side by side always differ in colour.
struct Lanes {
    lanes: Vec<Option<String>>,
    overflow: bool,
}

fn color_of(lane: usize) -> u8 {
    (lane % LANE_BUDGET) as u8
}

impl Lanes {
    fn new(open: &[String]) -> Self {
        Self {
            lanes: open
                .iter()
                .map(|h| if h.is_empty() { None } else { Some(h.clone()) })
                .collect(),
            overflow: false,
        }
    }

    fn find(&self, hash: &str) -> Option<usize> {
        self.lanes.iter().position(|l| l.as_deref() == Some(hash))
    }

    fn claim_free(&mut self) -> usize {
        match self.lanes.iter().position(|l| l.is_none()) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    fn open_count(&self) -> usize {
        self.lanes.iter().filter(|l| l.is_some()).count()
    }

    /// Place one commit and produce the edges leaving its row downwards.
    fn place(&mut self, row: &Row) -> (u16, Vec<LaneEdge>) {
        let lane = match self.find(&row.hash) {
            Some(i) => i,
            None => self.claim_free(),
        };
        self.lanes[lane] = None; // the line has arrived; where it goes next is decided below
        let before: Vec<bool> = self.lanes.iter().map(|l| l.is_some()).collect();

        let mut edges = Vec::new();
        for (i, parent) in row.parents.iter().enumerate() {
            let target = match self.find(parent) {
                // Two children of the same parent share one lane: the second child's
                // line bends into the lane already waiting, instead of opening a
                // duplicate that would never be closed.
                Some(t) => t,
                None if i == 0 => lane,
                None => self.claim_free(),
            };
            self.lanes[target] = Some(parent.clone());
            let kind = if i > 0 {
                LaneEdgeKind::Merge
            } else if target == lane {
                LaneEdgeKind::Straight
            } else {
                LaneEdgeKind::Branch
            };
            edges.push(LaneEdge {
                from_lane: lane as u16,
                to_lane: target as u16,
                kind,
                color: color_of(target),
            });
        }
        // Lanes untouched by this commit keep running straight past its row.
        for (k, open) in before.iter().enumerate() {
            if *open && !edges.iter().any(|e| e.to_lane as usize == k) {
                edges.push(LaneEdge {
                    from_lane: k as u16,
                    to_lane: k as u16,
                    kind: LaneEdgeKind::Straight,
                    color: color_of(k),
                });
            }
        }
        if self.open_count() > LANE_BUDGET || self.lanes.len() > LANE_BUDGET {
            self.overflow = true;
        }
        (lane as u16, edges)
    }

    /// Open lines at the lower boundary, positions preserved (`""` = free slot).
    fn open_lanes(&self) -> Vec<String> {
        let last = self.lanes.iter().rposition(|l| l.is_some());
        match last {
            None => Vec::new(),
            Some(n) => self.lanes[..=n]
                .iter()
                .map(|l| l.clone().unwrap_or_default())
                .collect(),
        }
    }
}

/// One page of history, continuing the graph from `cursor` when given.
///
/// Pages must be asked for in order: the lane layout is streaming, and a jump to an
/// arbitrary offset has no open lines to continue from (prd_02 §Решения).
pub fn page(repo: &Path, filter: &LogFilter, cursor: Option<&LogCursor>, limit: u32) -> Result<LogPage> {
    let skip = cursor.map(|c| c.skip).unwrap_or(0);
    let mut injected = false;
    let mut args = vec![s("log"), s(FORMAT), format!("--max-count={limit}")];
    if skip > 0 {
        args.push(format!("--skip={skip}"));
    }
    // `filter_args` already keeps `--` and the paths last.
    args.extend(filter_args(filter));

    let mut rows = parse_rows(&git_text(repo, &args)?)?;
    let more = limit > 0 && rows.len() as u32 >= limit;

    // A search string that looks like a hash is looked up as a hash as well (R20i):
    // `--grep` only sees the message, and the hash is not part of it.
    if skip == 0 {
        if let Some(text) = filter.text.as_deref().filter(|t| looks_like_hash(t)) {
            if let Some(extra) = commit_by_hash(repo, text.trim())? {
                if !rows.iter().any(|r| r.hash == extra.hash) {
                    // The page never grows past `limit`: the injected row takes the
                    // place of the last one, which stays in the stream and opens the
                    // next page — the offset below counts only the rows actually kept.
                    if rows.len() as u32 >= limit {
                        rows.pop();
                    }
                    rows.insert(0, extra);
                    injected = true;
                }
            }
        }
    }

    let fetched = rows.len() as u32 - u32::from(injected);
    let remotes = remotes(repo)?;
    let graph = !breaks_history(filter);
    let mut lanes = Lanes::new(&open_lanes_of(repo, filter, cursor)?);
    let mut commits = Vec::with_capacity(rows.len());
    for row in &rows {
        let (lane, edges) = if graph { lanes.place(row) } else { (0, Vec::new()) };
        commits.push(LogCommit {
            hash: row.hash.clone(),
            short_hash: short(&row.hash),
            parents: row.parents.clone(),
            author: row.author.clone(),
            author_email: row.author_email.clone(),
            author_at: row.author_at,
            subject: row.subject.clone(),
            refs: parse_refs(&row.decor, &remotes),
            lane,
            edges,
        });
    }

    let next_cursor = match more {
        false => None,
        true => {
            // The header goes in even when no line is open: `skip` alone is enough to
            // read a moved history wrongly.
            let mut open = vec![cursor_header(repo, filter)?];
            if graph {
                open.extend(lanes.open_lanes());
            }
            Some(LogCursor { skip: skip + fetched, open_lanes: open })
        }
    };
    Ok(LogPage { commits, next_cursor, lane_overflow: lanes.overflow })
}

/// One commit read by hash, or `None` when the string names no revision here.
///
/// "Not a revision" and "git could not answer" are different answers: with
/// `--verify --quiet` git reports an unknown revision by its exit code and says
/// nothing, while a repository it cannot read still explains itself on stderr. The
/// second case is an error — folding it into `None` would show a typo where the
/// repository is broken.
fn commit_by_hash(repo: &Path, rev: &str) -> Result<Option<Row>> {
    let args = ["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")];
    let out = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            return Ok(None);
        }
        return Err(Error::Git { command: args.join(" "), stderr });
    }
    let full = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if full.is_empty() {
        return Ok(None);
    }
    let rows = parse_rows(&git_text(
        repo,
        &[s("log"), s("-1"), s(FORMAT), s("--no-walk"), full],
    )?)?;
    Ok(rows.into_iter().next())
}

/// Distinct commit authors, for the author filter — most prolific first.
///
/// Walks the same history [`page`] walks, refs and exclusions included. `shortlog
/// --all` reads more than that: an author existing only in a stash commit would be
/// offered by the filter and then select nothing.
pub fn authors(repo: &Path) -> Result<Vec<String>> {
    let mut args = vec![s("log"), s("--format=%an")];
    args.extend(filter_args(&LogFilter::default()));
    let out = git_text(repo, &args)?;

    let mut count: HashMap<&str, usize> = HashMap::new();
    for name in out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        *count.entry(name).or_default() += 1;
    }
    let mut list: Vec<(&str, usize)> = count.into_iter().collect();
    // by commits, then by name: two authors with the same count keep a stable order
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    Ok(list.into_iter().map(|(n, _)| n.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cli::tests::scratch_repo;
    use crate::model::LogOrder;
    use crate::model::{LaneEdge, LaneEdgeKind};
    use std::process::Command;

    pub(crate) fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn commit(dir: &Path, file: &str, body: &str, msg: &str) {
        std::fs::write(dir.join(file), body).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-m", msg]);
    }


    /// main: init - second - m2 - merge ; feature: (from second) - f
    fn branched_repo() -> tempfile::TempDir {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "b.txt", "b\n", "second");
        run(p, &["checkout", "-q", "-b", "feature"]);
        commit(p, "f.txt", "f\n", "on feature");
        run(p, &["checkout", "-q", "main"]);
        commit(p, "m.txt", "m\n", "on main");
        run(p, &["merge", "--no-ff", "-m", "merge feature", "feature"]);
        dir
    }

    #[test]
    fn merge_row_carries_a_merge_edge_and_the_side_line_bends_back() {
        let dir = branched_repo();
        let page = page(dir.path(), &LogFilter::default(), None, 20).unwrap();
        let by_subject = |s: &str| page.commits.iter().find(|c| c.subject == s).unwrap().clone();

        let merge = by_subject("merge feature");
        assert_eq!(merge.parents.len(), 2, "merge has two parents");
        assert_eq!(merge.lane, 0, "the line reaching the tip stays in the leftmost lane");
        let merge_edges: Vec<&LaneEdge> = merge.edges.iter().filter(|e| e.kind == LaneEdgeKind::Merge).collect();
        assert_eq!(merge_edges.len(), 1, "one edge per extra parent: {:?}", merge.edges);
        assert_eq!(merge_edges[0].from_lane, 0);
        assert_eq!(merge_edges[0].to_lane, 1, "the second parent opens a lane of its own");
        assert!(merge.edges.iter().any(|e| e.kind == LaneEdgeKind::Straight && e.to_lane == 0));

        let side = by_subject("on feature");
        assert_eq!(side.lane, 1, "the feature commit sits in the lane the merge opened");
        let back: Vec<&LaneEdge> = side.edges.iter().filter(|e| e.kind == LaneEdgeKind::Branch).collect();
        assert_eq!(back.len(), 1, "its parent is already awaited in lane 0: {:?}", side.edges);
        assert_eq!((back[0].from_lane, back[0].to_lane), (1, 0));

        let second = by_subject("second");
        assert_eq!(second.lane, 0, "both lines converged before it");
        assert!(second.edges.iter().all(|e| e.from_lane == 0 && e.to_lane == 0), "one line below the fork: {:?}", second.edges);

        // colour follows the lane and never collides between two lanes drawn side by side
        assert_ne!(merge_edges[0].color, merge.edges.iter().find(|e| e.to_lane == 0).unwrap().color);
        assert!(!page.lane_overflow, "two lanes are well under the budget of twelve");
    }

    #[test]
    fn second_page_continues_the_lanes_of_the_first() {
        let dir = branched_repo();
        let p = dir.path();
        let whole = page(p, &LogFilter::default(), None, 20).unwrap();
        assert_eq!(whole.commits.len(), 5);

        let first = page(p, &LogFilter::default(), None, 2).unwrap();
        assert_eq!(first.commits.len(), 2);
        let cursor = first.next_cursor.clone().expect("a full page promises more");
        assert_eq!(cursor.skip, 2);
        assert!(cursor.open_lanes[0].starts_with('@'), "slot 0 is the header: {:?}", cursor.open_lanes);
        assert_eq!(cursor.open_lanes.len(), 3, "header plus the two lines the merge left open: {:?}", cursor.open_lanes);
        assert_eq!(cursor.open_lanes[2], whole.commits[2].hash, "lane 1 waits for the feature commit");

        let second = page(p, &LogFilter::default(), Some(&cursor), 2).unwrap();
        let side = &second.commits[0];
        assert_eq!(side.subject, "on feature");
        assert_eq!(side.lane, 1, "the lane opened on the previous page is still lane 1");
        // the same rows, laid out in one go, must agree with the paged layout
        for (a, b) in whole.commits[2..4].iter().zip(second.commits.iter()) {
            assert_eq!(a.hash, b.hash);
            assert_eq!(a.lane, b.lane, "lane of {} differs across paging", a.subject);
            let colors = |c: &LogCommit| c.edges.iter().map(|e| (e.from_lane, e.to_lane, e.color)).collect::<Vec<_>>();
            assert_eq!(colors(a), colors(b), "edges of {} differ across paging", a.subject);
        }

        // reloading the same page twice gives the same colours (spec: lane colour is stable)
        let again = page(p, &LogFilter::default(), None, 20).unwrap();
        let seen = |pg: &LogPage| pg.commits.iter().map(|c| (c.hash.clone(), c.lane, c.edges.iter().map(|e| e.color).collect::<Vec<_>>())).collect::<Vec<_>>();
        assert_eq!(seen(&whole), seen(&again));
    }

    fn commit_as(dir: &Path, file: &str, msg: &str, name: &str, email: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        run(dir, &["add", file]);
        run(dir, &["-c", &format!("user.name={name}"), "-c", &format!("user.email={email}"), "commit", "-m", msg]);
    }

    #[test]
    fn each_filter_narrows_the_page_and_a_gap_making_filter_drops_the_edges() {
        let dir = branched_repo();
        let p = dir.path();
        commit_as(p, "o.txt", "by other", "Other One", "other@example.com");
        let subjects = |f: &LogFilter| {
            page(p, f, None, 50).unwrap().commits.iter().map(|c| c.subject.clone()).collect::<Vec<_>>()
        };
        let all = subjects(&LogFilter::default());
        assert_eq!(all.len(), 6, "everything: {all:?}");

        // branch keeps history contiguous, so the graph stays on
        let branch = LogFilter { branch: Some("feature".into()), ..Default::default() };
        assert_eq!(subjects(&branch), vec!["on feature", "second", "init"]);
        let bp = page(p, &branch, None, 50).unwrap();
        assert!(bp.commits[0].edges.iter().any(|e| e.kind == LaneEdgeKind::Straight), "branch filter keeps edges");

        // text, author, date and paths all leave holes — node only, no edges
        let cases: Vec<(&str, LogFilter, Vec<&str>)> = vec![
            ("text", LogFilter { text: Some("on m".into()), ..Default::default() }, vec!["on main"]),
            ("text is case-insensitive by default", LogFilter { text: Some("ON MAIN".into()), ..Default::default() }, vec!["on main"]),
            ("text with case", LogFilter { text: Some("ON MAIN".into()), match_case: true, ..Default::default() }, vec![]),
            ("regex", LogFilter { text: Some("^on (main|feature)$".into()), regex: true, ..Default::default() }, vec!["on main", "on feature"]),
            ("regex off treats the pattern literally", LogFilter { text: Some("^on (main|feature)$".into()), ..Default::default() }, vec![]),
            ("author", LogFilter { authors: vec!["Other One".into()], ..Default::default() }, vec!["by other"]),
            ("paths", LogFilter { paths: vec!["f.txt".into()], ..Default::default() }, vec!["on feature"]),
        ];
        for (label, f, expect) in cases {
            let got = subjects(&f);
            assert_eq!(got, expect, "filter {label}");
            for c in page(p, &f, None, 50).unwrap().commits {
                assert!(c.edges.is_empty(), "filter {label} breaks history: no edges for {}", c.subject);
                assert_eq!(c.lane, 0);
            }
        }

        // combined filters narrow further than each alone
        let both = LogFilter { authors: vec!["Test".into()], text: Some("on".into()), ..Default::default() };
        assert_eq!(subjects(&both), vec!["on main", "on feature", "second"], "\"second\" contains \"on\" too — the text filter is a substring match");

        // a date window cuts by time
        let newest = page(p, &LogFilter::default(), None, 1).unwrap().commits[0].author_at;
        let since = LogFilter { since: Some(newest), ..Default::default() };
        assert!(subjects(&since).contains(&"by other".to_string()));
        let until = LogFilter { until: Some(newest - 1), ..Default::default() };
        assert!(!subjects(&until).contains(&"by other".to_string()), "the newest commit falls outside the window");
    }

    #[test]
    fn a_hash_like_search_finds_the_commit_by_hash_as_well() {
        let dir = branched_repo();
        let p = dir.path();
        let target = page(p, &LogFilter::default(), None, 50).unwrap()
            .commits.iter().find(|c| c.subject == "second").unwrap().clone();
        let prefix = target.hash[..8].to_string();

        let f = LogFilter { text: Some(prefix.clone()), ..Default::default() };
        let got = page(p, &f, None, 50).unwrap();
        assert_eq!(got.commits.len(), 1, "no message contains that hex string");
        assert_eq!(got.commits[0].hash, target.hash);

        // the injected row takes the place of the last one instead of growing the page,
        // and the row it displaced opens the next page rather than disappearing
        commit(p, "n1.txt", "n1\n", &format!("note {prefix} one"));
        commit(p, "n2.txt", "n2\n", &format!("note {prefix} two"));
        let full = page(p, &f, None, 2).unwrap();
        assert_eq!(full.commits.len(), 2, "a page never exceeds its limit");
        assert_eq!(full.commits[0].hash, target.hash, "the hash match comes first");
        assert_eq!(full.commits[1].subject, format!("note {prefix} two"));
        let cursor = full.next_cursor.clone().expect("the grep stream has more");
        assert_eq!(cursor.skip, 1, "only the kept stream row counts towards the offset");
        let next = page(p, &f, Some(&cursor), 2).unwrap();
        assert_eq!(next.commits[0].subject, format!("note {prefix} one"), "the displaced row is not lost");

        // a hex string naming nothing simply matches nothing
        let f = LogFilter { text: Some("deadbeef".into()), ..Default::default() };
        assert!(page(p, &f, None, 50).unwrap().commits.is_empty());
    }

    #[test]
    fn invalid_regex_returns_the_git_reason() {
        let dir = scratch_repo();
        let f = LogFilter { text: Some("(unclosed".into()), regex: true, ..Default::default() };
        match page(dir.path(), &f, None, 10) {
            Err(Error::Git { command, stderr }) => {
                assert!(command.contains("--grep=(unclosed"), "the failing command is named: {command}");
                assert!(!stderr.is_empty(), "git's own reason reaches the UI");
            }
            other => panic!("expected a git error with its stderr, got {other:?}"),
        }
    }

    #[test]
    fn subject_with_newlines_and_odd_bytes_does_not_break_the_parse() {
        let dir = scratch_repo();
        let p = dir.path();
        let msg = "line one\nstill the subject\ttab %x00 %H\n\nbody paragraph";
        std::fs::write(p.join("w.txt"), "w\n").unwrap();
        run(p, &["add", "w.txt"]);
        run(p, &["commit", "-m", msg]);

        let page = page(p, &LogFilter::default(), None, 10).unwrap();
        assert_eq!(page.commits.len(), 2, "two commits, not one per message line");
        let top = &page.commits[0];
        assert!(top.subject.starts_with("line one"), "subject: {:?}", top.subject);
        assert!(top.subject.contains("%x00"), "a literal placeholder in the text stays text: {:?}", top.subject);
        assert!(!top.subject.contains("body paragraph"), "the body is not the subject");
        assert_eq!(top.parents.len(), 1);
        assert_eq!(page.commits[1].subject, "init");
    }

    #[test]
    fn empty_repository_gives_an_empty_page_and_no_authors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-b", "main"]);
        let page = page(p, &LogFilter::default(), None, 10).unwrap();
        assert!(page.commits.is_empty());
        assert!(page.next_cursor.is_none());
        assert!(authors(p).unwrap().is_empty());
    }

    #[test]
    fn authors_lists_every_author_once() {
        let dir = scratch_repo();
        let p = dir.path();
        commit_as(p, "o.txt", "by other", "Other One", "other@example.com");
        commit_as(p, "o2.txt", "by other again", "Other One", "other@example.com");
        let a = authors(p).unwrap();
        assert_eq!(a.len(), 2, "two distinct authors: {a:?}");
        assert_eq!(a[0], "Other One", "the most prolific author comes first");
        assert!(a.contains(&"Test".to_string()));
    }

    fn commit_at(dir: &Path, file: &str, msg: &str, stamp: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        run(dir, &["add", file]);
        let out = Command::new("git").arg("-C").arg(dir)
            .env("GIT_AUTHOR_DATE", stamp).env("GIT_COMMITTER_DATE", stamp)
            .args(["commit", "-m", msg]).output().expect("spawn git");
        assert!(out.status.success(), "commit failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn date_order_interleaves_branches_and_topo_order_keeps_chains_together() {
        let dir = scratch_repo();
        let p = dir.path();
        run(p, &["checkout", "-q", "-b", "feature"]);
        commit_at(p, "f.txt", "F", "@2400 +0000");
        run(p, &["checkout", "-q", "main"]);
        commit_at(p, "m1.txt", "M1", "@2200 +0000");
        commit_at(p, "m2.txt", "M2", "@2600 +0000");
        run(p, &["merge", "--no-ff", "-m", "merge", "feature"]);

        let pos = |order: LogOrder, subject: &str| {
            let f = LogFilter { order, ..Default::default() };
            page(p, &f, None, 20).unwrap().commits.iter().position(|c| c.subject == subject).unwrap()
        };
        // by date the side commit falls between the two main commits
        assert!(pos(LogOrder::Date, "M2") < pos(LogOrder::Date, "F"));
        assert!(pos(LogOrder::Date, "F") < pos(LogOrder::Date, "M1"));
        // topologically the main chain is not cut open by the side commit
        let (m2, m1) = (pos(LogOrder::Topo, "M2"), pos(LogOrder::Topo, "M1"));
        assert_eq!(m1, m2 + 1, "M1 follows M2 with nothing in between");
    }

    #[test]
    fn more_than_twelve_open_lanes_raise_the_overflow_flag() {
        let dir = scratch_repo();
        let p = dir.path();
        for n in 0..14 {
            run(p, &["checkout", "-q", "-b", &format!("b{n}"), "main"]);
            // explicit stamps: every tip must be newer than every mid, otherwise the
            // date order of same-second commits decides how many lanes are open at once
            commit_at(p, &format!("x{n}.txt"), &format!("mid {n}"), &format!("@{} +0000", 1000 + n * 10));
            commit_at(p, &format!("y{n}.txt"), &format!("tip {n}"), &format!("@{} +0000", 2000 + n * 10));
        }
        let wide = page(p, &LogFilter::default(), None, 50).unwrap();
        assert!(wide.lane_overflow, "fourteen side lines exceed the budget of twelve");
        assert!(wide.commits.iter().any(|c| c.lane >= 12), "lanes past the budget are still reported honestly");

        let narrow = page(branched_repo().path(), &LogFilter::default(), None, 50).unwrap();
        assert!(!narrow.lane_overflow, "two lines stay inside the budget");
    }

    #[test]
    fn an_author_name_with_metacharacters_is_matched_literally() {
        let dir = scratch_repo();
        let p = dir.path();
        commit_as(p, "o.txt", "by pattern", "A. B (x)", "ab@example.com");
        let f = LogFilter { authors: vec!["A. B (x)".into()], ..Default::default() };
        let got = page(p, &f, None, 20).unwrap();
        assert_eq!(got.commits.len(), 1, "the name is a name, not a pattern: {:?}", got.commits);
        assert_eq!(got.commits[0].subject, "by pattern");

        // and the same string as a regular expression is a pattern again
        let f = LogFilter { authors: vec!["A. B \\(x\\)".into()], regex: true, ..Default::default() };
        assert_eq!(page(p, &f, None, 20).unwrap().commits.len(), 1);
    }

    #[test]
    fn two_authors_are_ored_not_anded() {
        let dir = scratch_repo();
        let p = dir.path();
        commit_as(p, "a.txt", "by other", "Other One", "other@example.com");
        commit_as(p, "b.txt", "by third", "Third Person", "third@example.com");
        let f = LogFilter {
            authors: vec!["Other One".into(), "Third Person".into()],
            ..Default::default()
        };
        let got: Vec<String> = page(p, &f, None, 50)
            .unwrap()
            .commits
            .into_iter()
            .map(|c| c.subject)
            .collect();
        assert_eq!(
            got,
            vec!["by third".to_string(), "by other".to_string()],
            "repeated --author is git's own OR; anding them would leave nothing: {got:?}"
        );
    }

    /// A blank entry is `--author=`, which matches everything: one empty row in
    /// the list would silently undo the whole filter.
    #[test]
    fn a_blank_author_entry_does_not_widen_the_filter() {
        let dir = scratch_repo();
        let p = dir.path();
        commit_as(p, "a.txt", "by other", "Other One", "other@example.com");
        let f = LogFilter {
            authors: vec!["Other One".into(), "   ".into()],
            ..Default::default()
        };
        let got = page(p, &f, None, 50).unwrap();
        assert_eq!(got.commits.len(), 1, "{:?}", got.commits);
        assert_eq!(got.commits[0].subject, "by other");
    }

    #[test]
    fn stash_and_notes_commits_stay_out_of_the_log() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "dirty\n").unwrap();
        run(p, &["stash", "push", "-m", "stashed work"]);
        run(p, &["notes", "add", "-m", "a note", "HEAD"]);

        let page = page(p, &LogFilter::default(), None, 20).unwrap();
        let subjects: Vec<&str> = page.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["init"], "only real history: {subjects:?}");
    }

    #[test]
    fn a_limit_of_zero_ends_the_log_instead_of_promising_more() {
        let dir = scratch_repo();
        let page = page(dir.path(), &LogFilter::default(), None, 0).unwrap();
        assert!(page.commits.is_empty());
        assert!(page.next_cursor.is_none(), "an empty answer must not ask the caller to come back");
    }

    #[test]
    fn a_truncated_record_is_an_error_not_a_shorter_log() {
        let good = "h\u{0}p\u{0}Ann\u{0}a@e\u{0}1700000000\u{0}\u{0}subject\u{1}";
        assert_eq!(parse_rows(good).unwrap().len(), 1);

        // one field short — the record is not silently skipped, which would hand the
        // user a log with a commit missing from it and no sign of why
        let truncated = "h\u{0}p\u{0}Ann\u{0}a@e\u{0}1700000000\u{0}subject\u{1}";
        match parse_rows(truncated) {
            Err(Error::Parse(m)) => assert!(m.contains("6"), "the reason names what was found: {m}"),
            other => panic!("expected a parse error, got {:?}", other.map(|r| r.len())),
        }
        match parse_rows(&format!("{good}{truncated}")) {
            Err(Error::Parse(_)) => {}
            other => panic!("a bad record among good ones is still an error, got {:?}", other.map(|r| r.len())),
        }
    }

    #[test]
    fn a_cursor_cut_from_a_moved_history_is_refused() {
        let dir = branched_repo();
        let p = dir.path();
        let first = page(p, &LogFilter::default(), None, 2).unwrap();
        let cursor = first.next_cursor.clone().unwrap();
        // the same cursor on the same history keeps working
        assert!(page(p, &LogFilter::default(), Some(&cursor), 2).is_ok());

        commit(p, "later.txt", "later\n", "arrived after the page");
        match page(p, &LogFilter::default(), Some(&cursor), 2) {
            Err(Error::Rule(m)) => assert!(m.contains("reload"), "the reason tells the caller what to do: {m}"),
            other => panic!("a cursor into a moved history must be refused, got {:?}", other.map(|p| p.commits.len())),
        }

        // and a cursor cut under one filter is not accepted under another
        let fresh = page(p, &LogFilter::default(), None, 2).unwrap().next_cursor.unwrap();
        let other = LogFilter { branch: Some("feature".into()), ..Default::default() };
        assert!(page(p, &other, Some(&fresh), 2).is_err(), "the filter is part of what the cursor was cut from");
    }

    #[test]
    fn an_author_seen_only_in_a_stash_is_not_offered() {
        let dir = scratch_repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "dirty\n").unwrap();
        run(p, &["-c", "user.name=Stash Only", "-c", "user.email=s@example.com", "stash", "push", "-m", "wip"]);

        let a = authors(p).unwrap();
        assert_eq!(a, vec!["Test".to_string()], "the log does not show stash commits, so the filter must not offer their author: {a:?}");
    }

    #[test]
    fn an_unreadable_repository_is_not_reported_as_a_missing_revision() {
        let dir = scratch_repo();
        // a real repo, a hex string naming nothing in it
        assert!(commit_by_hash(dir.path(), "deadbeef").unwrap().is_none());
        // no repo at all — git has something to say, and it is not "no such commit"
        match commit_by_hash(&dir.path().join("no-such-directory"), "deadbeef") {
            Err(Error::Git { stderr, .. }) => assert!(!stderr.is_empty(), "git's own reason survives"),
            other => panic!("expected a git error, got {:?}", other.map(|r| r.map(|c| c.hash))),
        }
    }

    #[test]
    fn page_reads_linear_history_newest_first() {
        let dir = scratch_repo();
        let p = dir.path();
        commit(p, "b.txt", "b\n", "second");
        commit(p, "c.txt", "c\n", "third");
        run(p, &["tag", "v1"]);

        let page = page(p, &LogFilter::default(), None, 10).unwrap();
        let subjects: Vec<&str> = page.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["third", "second", "init"]);

        let head = &page.commits[0];
        assert_eq!(head.short_hash, head.hash[..7].to_string());
        assert_eq!(head.author, "Test");
        assert_eq!(head.author_email, "t@example.com");
        assert!(head.author_at > 1_600_000_000);
        assert_eq!(head.parents, vec![page.commits[1].hash.clone()]);
        assert!(page.commits[2].parents.is_empty(), "root commit has no parents");

        // decorations reach the row; their kinds are the business of
        // `cli::parse_refs`, tested there
        let names: Vec<&str> = head.refs.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"v1"), "labels of the tip: {names:?}");
        assert!(page.commits[1].refs.is_empty(), "an undecorated commit carries no labels");

        assert!(page.next_cursor.is_none(), "3 commits under a limit of 10 end the log");
        assert!(!page.lane_overflow);
    }
}
