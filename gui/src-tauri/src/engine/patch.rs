//! A patch for a chosen subset of one file's diff — the mechanism behind staging,
//! unstaging and reverting single lines (and whole hunks, which are simply every line
//! of a hunk).
//!
//! Pure: bytes in, bytes out, no git. The caller reads the diff, checks that it is
//! still the one the reader was shown (its fingerprint), and hands the result to
//! `git apply`. Bytes and not text: a line that is not UTF-8 would come back from a
//! lossy round trip as U+FFFD and be written into the index that way.
//!
//! ## One rule, mirrored
//!
//! `git apply` matches one side of a patch against its target and writes the other.
//!
//! * **Forward** (stage: the diff is worktree vs index, applied `--cached`). The old
//!   side is matched. An unselected deletion is a line the target still has and must
//!   keep, so it becomes context; an unselected addition must not reach the target,
//!   so it is dropped.
//! * **Reverse** (unstage: `diff --cached` applied `--cached -R`; revert: the worktree
//!   diff applied `-R` to the working tree). The new side is matched. An unselected
//!   addition is already in the target and stays, so it becomes context; an
//!   unselected deletion is not there, so it is dropped.
//!
//! Either way the matched side comes out **exactly** as git printed it, so its
//! numbers are kept, and only the written side's start moves — by the net line count
//! of the hunks emitted before it. The count check against the hunk header is a guard
//! on that invariant: a diff this parser misread fails here, not in the index.
//!
//! ## Whole-file changes
//!
//! A new file stays a creation only if the written result leaves nothing on the old
//! side of every hunk — that is, all of it was chosen; otherwise it becomes an
//! ordinary modification of a file that exists. The same holds mirrored for a
//! deletion: a partial one leaves lines behind and is no longer a deletion.
//!
//! ## `\ No newline at end of file`
//!
//! The marker belongs to a line on one side or both. The matched side keeps its
//! marker as printed. On the written side only its last line may lack a newline, and
//! it does exactly when the line it came from did. A context line whose two sides
//! then disagree — an unselected deletion of a last line without a newline, with
//! additions chosen after it — is split into a matched line without the newline and
//! a written line with one: appending after a last line needs a newline at its end.

use std::collections::HashSet;

use crate::error::{Error, Result};

const NO_NEWLINE: &[u8] = b"\\ No newline at end of file";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Context,
    Add,
    Del,
}

impl Kind {
    fn marker(self) -> u8 {
        match self {
            Kind::Context => b' ',
            Kind::Add => b'+',
            Kind::Del => b'-',
        }
    }
}

/// One line of a hunk, without its marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub text: Vec<u8>,
    /// Followed by `\ No newline at end of file`.
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkText {
    /// The `@@` line as git printed it.
    pub header: Vec<u8>,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// Everything after the closing `@@`, leading space included.
    pub heading: Vec<u8>,
    pub lines: Vec<Line>,
}

/// The hunks of a one-file diff, in order.
///
/// **The one line numbering of the crate**: `parse_diff` builds `Hunk::lines` from
/// this, and a selection sent back names lines by their index here. Two parsers
/// could disagree on an edge (`diff.suppressBlankEmpty` prints a blank context line
/// as an empty one) and a selection would then stage the line next to the one
/// chosen.
///
/// Lenient, because it also reads commit diffs for display: a line with an unknown
/// marker is context, as it always was. The counts in each header decide where a
/// hunk ends, so an empty line inside it is an empty context line and the empty
/// tail after the last newline is nothing.
pub fn hunks(raw: &[u8]) -> Vec<HunkText> {
    let lines: Vec<&[u8]> = raw.split(|&b| b == b'\n').collect();
    let mut out: Vec<HunkText> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(h) = combined_header(lines[i]) {
            i = combined_body(&lines, i + 1, h, &mut out);
            continue;
        }
        let Some(h) = hunk_header(lines[i]) else {
            i += 1;
            continue;
        };
        let (mut old_left, mut new_left) = (h.old_count, h.new_count);
        let mut hunk = h;
        i += 1;
        while i < lines.len() && (old_left > 0 || new_left > 0 || lines[i].starts_with(b"\\")) {
            let l = lines[i];
            if l.starts_with(b"\\") {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
                i += 1;
                continue;
            }
            if l.starts_with(b"@@") {
                break;
            }
            let (kind, text) = match l.first() {
                Some(b'+') => (Kind::Add, &l[1..]),
                Some(b'-') => (Kind::Del, &l[1..]),
                Some(_) => (Kind::Context, &l[1..]),
                None => (Kind::Context, l),
            };
            if kind != Kind::Add {
                old_left = old_left.saturating_sub(1);
            }
            if kind != Kind::Del {
                new_left = new_left.saturating_sub(1);
            }
            hunk.lines.push(Line {
                kind,
                text: text.to_vec(),
                no_newline: false,
            });
            i += 1;
        }
        out.push(hunk);
    }
    out
}

/// `@@ -a[,b] +c[,d] @@ heading`; a missing count is 1.
fn hunk_header(l: &[u8]) -> Option<HunkText> {
    let rest = l.strip_prefix(b"@@ -")?;
    let close = find(rest, b" @@")?;
    let ranges = std::str::from_utf8(&rest[..close]).ok()?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some(HunkText {
        header: l.to_vec(),
        old_start,
        old_count,
        new_start,
        new_count,
        heading: rest[close + 3..].to_vec(),
        lines: Vec::new(),
    })
}

/// `@@@ -a,b -c,d +e,f @@@` — a conflicted file's combined diff (`diff --cc`). Read
/// for display only, as it always was: the first old range and the new one number
/// the lines, the first marker column decides the kind. `build` refuses such a diff.
fn combined_header(l: &[u8]) -> Option<HunkText> {
    let rest = l.strip_prefix(b"@@@ ")?;
    let close = find(rest, b" @@@")?;
    let ranges = std::str::from_utf8(&rest[..close]).ok()?;
    let start = |sign: char| -> u32 {
        ranges
            .split_whitespace()
            .find_map(|t| t.strip_prefix(sign))
            .and_then(|r| r.split(',').next()?.parse().ok())
            .unwrap_or(1)
    };
    Some(HunkText {
        header: l.to_vec(),
        old_start: start('-'),
        old_count: 0,
        new_start: start('+'),
        new_count: 0,
        heading: rest[close + 4..].to_vec(),
        lines: Vec::new(),
    })
}

/// The lines of a combined hunk, up to the next `@@` or the end; returns where it
/// stopped. The empty tail after the last newline is nothing.
fn combined_body(lines: &[&[u8]], mut i: usize, mut h: HunkText, out: &mut Vec<HunkText>) -> usize {
    while i < lines.len() && !lines[i].starts_with(b"@@") {
        let l = lines[i];
        i += 1;
        if l.is_empty() && i == lines.len() {
            break;
        }
        if l.starts_with(b"\\") {
            if let Some(last) = h.lines.last_mut() {
                last.no_newline = true;
            }
            continue;
        }
        let kind = match l.first() {
            Some(b'+') => Kind::Add,
            Some(b'-') => Kind::Del,
            _ => Kind::Context,
        };
        h.lines.push(Line {
            kind,
            text: l.get(1..).unwrap_or_default().to_vec(),
            no_newline: false,
        });
    }
    out.push(h);
    i
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The file header lines a patch is rebuilt from.
#[derive(Debug, Default)]
struct Header {
    diff_git: Vec<u8>,
    /// `old mode` / `new mode`, verbatim.
    modes: Vec<Vec<u8>>,
    new_file: Option<Vec<u8>>,
    deleted_file: Option<Vec<u8>>,
    /// After `--- ` / `+++ `, verbatim (a trailing tab included).
    old_name: Vec<u8>,
    new_name: Vec<u8>,
}

fn header(raw: &[u8]) -> Result<Header> {
    let mut h = Header::default();
    for l in raw.split(|&b| b == b'\n') {
        if l.starts_with(b"@@") {
            break;
        }
        if l.starts_with(b"diff --cc ") || l.starts_with(b"diff --combined ") {
            return Err(Error::Rule(
                "the file has an unresolved conflict; resolve it before choosing lines".into(),
            ));
        }
        if l.starts_with(b"diff --git ") {
            h.diff_git = l.to_vec();
        } else if l.starts_with(b"old mode ") || l.starts_with(b"new mode ") {
            h.modes.push(l.to_vec());
        } else if l.starts_with(b"new file mode ") {
            h.new_file = Some(l.to_vec());
        } else if l.starts_with(b"deleted file mode ") {
            h.deleted_file = Some(l.to_vec());
        } else if let Some(n) = l.strip_prefix(b"--- ") {
            h.old_name = n.to_vec();
        } else if let Some(n) = l.strip_prefix(b"+++ ") {
            h.new_name = n.to_vec();
        } else if l.starts_with(b"Binary files ") || l.starts_with(b"GIT binary patch") {
            return Err(Error::Rule("a binary file has no lines to choose".into()));
        } else if l.starts_with(b"rename ")
            || l.starts_with(b"copy ")
            || l.starts_with(b"similarity index ")
        {
            return Err(Error::Parse(format!(
                "unexpected header in a one-file diff: {}",
                String::from_utf8_lossy(l)
            )));
        }
    }
    if h.diff_git.is_empty() || h.old_name.is_empty() || h.new_name.is_empty() {
        return Err(Error::Parse("diff without a complete file header".into()));
    }
    Ok(h)
}

/// `a/x` → `b/x` (or back), inside quotes too; `/dev/null` has no such name.
fn swap_prefix(name: &[u8], from: u8, to: u8) -> Result<Vec<u8>> {
    let mut v = name.to_vec();
    let at = usize::from(v.first() == Some(&b'"'));
    if v.get(at) != Some(&from) || v.get(at + 1) != Some(&b'/') {
        return Err(Error::Parse(format!(
            "file name without the {} prefix: {}",
            from as char,
            String::from_utf8_lossy(name)
        )));
    }
    v[at] = to;
    Ok(v)
}

/// Which lines of one hunk: every one, or these indexes into `hunks()`' lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    All,
    Lines(Vec<usize>),
}

/// The patch applying the chosen lines of `raw` (see the module comment), or `None`
/// when the choice holds no change at all — context lines alone, say.
///
/// A choice naming a hunk or a line the diff does not have, or the same hunk twice,
/// is refused (`Error::Rule`): it was made against another diff, and guessing what
/// was meant is how the wrong line gets staged.
pub fn build(raw: &[u8], picks: &[(usize, Pick)], reverse: bool) -> Result<Option<Vec<u8>>> {
    let head = header(raw)?;
    let hunks = hunks(raw);
    let mut order: Vec<&(usize, Pick)> = picks.iter().collect();
    order.sort_by_key(|(h, _)| *h);
    let mut seen = HashSet::new();

    let mut body: Vec<u8> = Vec::new();
    let mut delta: i64 = 0;
    let (mut emitted, mut old_total, mut new_total) = (0usize, 0u32, 0u32);
    for (hi, pick) in order {
        let h = hunks
            .get(*hi)
            .ok_or_else(|| Error::Rule(format!("the diff has no hunk {hi}")))?;
        if !seen.insert(*hi) {
            return Err(Error::Rule(format!("hunk {hi} is chosen twice")));
        }
        let chosen: Option<HashSet<usize>> = match pick {
            Pick::All => None,
            Pick::Lines(v) => {
                if let Some(bad) = v.iter().find(|&&i| i >= h.lines.len()) {
                    return Err(Error::Rule(format!("hunk {hi} has no line {bad}")));
                }
                Some(v.iter().copied().collect())
            }
        };
        let Some(built) = build_hunk(h, chosen.as_ref(), reverse, &mut delta)? else {
            continue;
        };
        body.extend_from_slice(&built.text);
        emitted += 1;
        old_total += built.old_count;
        new_total += built.new_count;
    }
    if emitted == 0 {
        return Ok(None);
    }

    let whole = emitted == hunks.len();
    let creation = head.new_file.is_some() && whole && old_total == 0;
    let deletion = head.deleted_file.is_some() && whole && new_total == 0;
    let mut out: Vec<u8> = Vec::new();
    let mut line = |l: &[u8]| {
        out.extend_from_slice(l);
        out.push(b'\n');
    };
    line(&head.diff_git);
    if head.new_file.is_none() && head.deleted_file.is_none() {
        for m in &head.modes {
            line(m);
        }
    }
    let dev_null = b"/dev/null".as_slice();
    if creation {
        line(head.new_file.as_deref().unwrap_or_default());
        line(b"--- /dev/null");
        line(&[b"+++ ".as_slice(), &head.new_name].concat());
    } else if deletion {
        line(head.deleted_file.as_deref().unwrap_or_default());
        line(&[b"--- ".as_slice(), &head.old_name].concat());
        line(b"+++ /dev/null");
    } else {
        // Neither side is absent any more: the one git printed as /dev/null takes
        // the other side's name.
        let old = if head.old_name == dev_null {
            swap_prefix(&head.new_name, b'b', b'a')?
        } else {
            head.old_name.clone()
        };
        let new = if head.new_name == dev_null {
            swap_prefix(&head.old_name, b'a', b'b')?
        } else {
            head.new_name.clone()
        };
        line(&[b"--- ".as_slice(), &old].concat());
        line(&[b"+++ ".as_slice(), &new].concat());
    }
    out.extend_from_slice(&body);
    Ok(Some(out))
}

struct Built {
    text: Vec<u8>,
    old_count: u32,
    new_count: u32,
}

fn build_hunk(
    h: &HunkText,
    chosen: Option<&HashSet<usize>>,
    reverse: bool,
    delta: &mut i64,
) -> Result<Option<Built>> {
    // What each line becomes. Kept as indexes into the hunk, with the kind it is
    // emitted as.
    let mut kept: Vec<(Kind, &Line)> = Vec::new();
    for (i, l) in h.lines.iter().enumerate() {
        let picked = chosen.is_none_or(|c| c.contains(&i));
        let kind = match (l.kind, picked, reverse) {
            (Kind::Context, _, _) => Kind::Context,
            (k, true, _) => k,
            (Kind::Del, false, false) | (Kind::Add, false, true) => Kind::Context,
            _ => continue,
        };
        kept.push((kind, l));
    }
    if !kept.iter().any(|(k, _)| *k != Kind::Context) {
        return Ok(None);
    }

    let old_count = kept.iter().filter(|(k, _)| *k != Kind::Add).count() as u32;
    let new_count = kept.iter().filter(|(k, _)| *k != Kind::Del).count() as u32;
    let (matched, written, m_start, m_header) = if reverse {
        (new_count, old_count, h.new_start, h.new_count)
    } else {
        (old_count, new_count, h.old_start, h.old_count)
    };
    if matched != m_header {
        return Err(Error::Parse(format!(
            "hunk @@ -{},{} +{},{} @@ holds {matched} lines on the side it is matched by",
            h.old_start, h.old_count, h.new_start, h.new_count
        )));
    }
    let first = if matched > 0 { m_start } else { m_start + 1 } as i64;
    let w_start = if written > 0 {
        first + *delta
    } else {
        first + *delta - 1
    };
    let w_start = u32::try_from(w_start)
        .map_err(|_| Error::Parse("a hunk would start before the file".into()))?;
    *delta += written as i64 - matched as i64;
    let (old_start, new_start) = if reverse {
        (w_start, m_start)
    } else {
        (m_start, w_start)
    };

    let mut text = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@").into_bytes();
    text.extend_from_slice(&h.heading);
    text.push(b'\n');

    let (m_kind, w_kind) = if reverse {
        (Kind::Add, Kind::Del)
    } else {
        (Kind::Del, Kind::Add)
    };
    let last_written = kept.iter().rposition(|(k, _)| *k != m_kind);
    let mut push = |kind: Kind, body: &[u8], no_newline: bool| {
        text.push(kind.marker());
        text.extend_from_slice(body);
        text.push(b'\n');
        if no_newline {
            text.extend_from_slice(NO_NEWLINE);
            text.push(b'\n');
        }
    };
    for (i, (kind, l)) in kept.iter().enumerate() {
        let written_last = Some(i) == last_written;
        match *kind {
            Kind::Context if l.no_newline && !written_last => {
                // Matched side: the last line, as printed. Written side: lines
                // follow it, so it needs its newline there.
                push(m_kind, &l.text, true);
                push(w_kind, &l.text, false);
            }
            Kind::Context => push(Kind::Context, &l.text, l.no_newline),
            k if k == m_kind => push(k, &l.text, l.no_newline),
            k => push(k, &l.text, l.no_newline && written_last),
        }
    }
    Ok(Some(Built {
        text,
        old_count,
        new_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: &[u8]) -> String {
        String::from_utf8_lossy(b).to_string()
    }

    fn built(raw: &str, picks: &[(usize, Pick)], reverse: bool) -> String {
        s(&build(raw.as_bytes(), picks, reverse).unwrap().unwrap())
    }

    const TWO: &str = concat!(
        "diff --git a/f b/f\n",
        "index 1111111..2222222 100644\n",
        "--- a/f\n",
        "+++ b/f\n",
        "@@ -1,4 +1,5 @@ fn top\n",
        " c1\n",
        "-d1\n",
        "+a1\n",
        "+a2\n",
        " c2\n",
        " c3\n",
        "@@ -10,3 +11,3 @@\n",
        " c10\n",
        "-d10\n",
        "+a10\n",
        " c11\n",
    );

    #[test]
    fn hunks_carry_their_lines_and_numbers() {
        let hs = hunks(TWO.as_bytes());
        assert_eq!(hs.len(), 2);
        assert_eq!(
            (
                hs[0].old_start,
                hs[0].old_count,
                hs[0].new_start,
                hs[0].new_count
            ),
            (1, 4, 1, 5)
        );
        assert_eq!(hs[0].heading, b" fn top".to_vec());
        assert_eq!(hs[0].lines.len(), 6);
        assert_eq!(hs[0].lines[1].kind, Kind::Del);
        assert_eq!(hs[1].lines[2].text, b"a10".to_vec());
    }

    #[test]
    fn an_empty_line_inside_a_hunk_is_an_empty_context_line() {
        // diff.suppressBlankEmpty prints a blank context line as nothing at all.
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n c\n\n-x\n+y\n";
        let hs = hunks(raw.as_bytes());
        assert_eq!(hs[0].lines.len(), 4);
        assert_eq!(
            hs[0].lines[1],
            Line {
                kind: Kind::Context,
                text: vec![],
                no_newline: false
            }
        );
    }

    #[test]
    fn staging_all_of_every_hunk_rebuilds_the_diff() {
        let got = built(TWO, &[(0, Pick::All), (1, Pick::All)], false);
        assert_eq!(
            got,
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n\
@@ -1,4 +1,5 @@ fn top\n c1\n-d1\n+a1\n+a2\n c2\n c3\n\
@@ -10,3 +11,3 @@\n c10\n-d10\n+a10\n c11\n"
        );
    }

    #[test]
    fn staging_drops_unchosen_additions_and_keeps_unchosen_deletions() {
        // Line 2 is `+a1` only: `-d1` stays as context, `+a2` goes.
        let got = built(TWO, &[(0, Pick::Lines(vec![2]))], false);
        assert!(
            got.contains("@@ -1,4 +1,5 @@ fn top\n c1\n d1\n+a1\n c2\n c3\n"),
            "{got}"
        );
    }

    #[test]
    fn unstaging_keeps_unchosen_additions_and_drops_unchosen_deletions() {
        // Reverse: `-d1` unchosen is dropped, `+a2` unchosen is context.
        let got = built(TWO, &[(0, Pick::Lines(vec![2]))], true);
        assert!(
            got.contains("@@ -1,4 +1,5 @@ fn top\n c1\n+a1\n a2\n c2\n c3\n"),
            "{got}"
        );
    }

    #[test]
    fn a_later_hunk_moves_by_what_the_earlier_ones_emitted() {
        // Staging only `+a1` of hunk 0 adds one line; hunk 1's written side
        // starts at 10 + 1, not at the 11 git printed for the whole diff.
        let got = built(TWO, &[(0, Pick::Lines(vec![1])), (1, Pick::All)], false);
        // Only `-d1`: one line fewer before hunk 1.
        assert!(
            got.contains("@@ -1,4 +1,3 @@ fn top\n c1\n-d1\n c2\n c3\n"),
            "{got}"
        );
        assert!(got.contains("@@ -10,3 +9,3 @@\n"), "{got}");
        // Reverse: the new side is matched, the old side moves.
        let got = built(TWO, &[(0, Pick::Lines(vec![2, 3])), (1, Pick::All)], true);
        assert!(
            got.contains("@@ -1,3 +1,5 @@ fn top\n c1\n+a1\n+a2\n c2\n c3\n"),
            "{got}"
        );
        assert!(got.contains("@@ -9,3 +11,3 @@\n"), "{got}");
    }

    #[test]
    fn a_hunk_with_nothing_chosen_is_left_out_and_so_is_a_context_only_choice() {
        let got = built(TWO, &[(1, Pick::All)], false);
        assert!(!got.contains("@@ -1,"), "{got}");
        assert!(got.contains("@@ -10,3 +10,3 @@\n"), "{got}");
        assert_eq!(
            build(TWO.as_bytes(), &[(0, Pick::Lines(vec![0, 4]))], false).unwrap(),
            None
        );
    }

    #[test]
    fn a_choice_from_another_diff_is_refused() {
        for picks in [
            vec![(2, Pick::All)],
            vec![(0, Pick::Lines(vec![6]))],
            vec![(0, Pick::All), (0, Pick::Lines(vec![1]))],
        ] {
            assert!(matches!(
                build(TWO.as_bytes(), &picks, false),
                Err(Error::Rule(_))
            ));
        }
    }

    const NEW: &str = "diff --git a/n b/n\nnew file mode 100644\nindex 0000000..3333333\n\
--- /dev/null\n+++ b/n\n@@ -0,0 +1,3 @@\n+l1\n+l2\n+l3\n";

    #[test]
    fn a_new_file_stays_a_creation_when_staged_in_part() {
        let got = built(NEW, &[(0, Pick::Lines(vec![1]))], false);
        assert_eq!(
            got,
            "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,1 @@\n+l2\n"
        );
    }

    #[test]
    fn unstaging_part_of_a_new_file_is_a_modification_of_it() {
        let got = built(NEW, &[(0, Pick::Lines(vec![1]))], true);
        assert_eq!(
            got,
            "diff --git a/n b/n\n--- a/n\n+++ b/n\n@@ -1,2 +1,3 @@\n l1\n+l2\n l3\n"
        );
        let all = built(NEW, &[(0, Pick::All)], true);
        assert!(
            all.contains("new file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,3 @@"),
            "{all}"
        );
    }

    const GONE: &str = "diff --git a/g b/g\ndeleted file mode 100644\nindex 3333333..0000000\n\
--- a/g\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-l1\n-l2\n-l3\n";

    #[test]
    fn a_partly_staged_deletion_is_no_longer_a_deletion() {
        let got = built(GONE, &[(0, Pick::Lines(vec![0]))], false);
        assert_eq!(
            got,
            "diff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -1,3 +1,2 @@\n-l1\n l2\n l3\n"
        );
        let all = built(GONE, &[(0, Pick::All)], false);
        assert!(
            all.contains("deleted file mode 100644\n--- a/g\n+++ /dev/null\n@@ -1,3 +0,0 @@\n"),
            "{all}"
        );
        // Unstaging part of a staged deletion brings those lines back as a file.
        let back = built(GONE, &[(0, Pick::Lines(vec![1]))], true);
        assert!(
            back.contains(
                "deleted file mode 100644\n--- a/g\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-l2\n"
            ),
            "{back}"
        );
    }

    #[test]
    fn a_path_with_a_space_or_a_quote_keeps_git_s_spelling() {
        let raw = "diff --git a/sp ace b/sp ace\nnew file mode 100644\n--- /dev/null\n\
+++ b/sp ace\t\n@@ -0,0 +1,2 @@\n+x\n+y\n";
        let got = built(raw, &[(0, Pick::Lines(vec![0]))], true);
        assert!(got.contains("--- a/sp ace\t\n+++ b/sp ace\t\n"), "{got}");
        let raw =
            "diff --git \"a/q\\\"t\" \"b/q\\\"t\"\ndeleted file mode 100644\n--- \"a/q\\\"t\"\n\
+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n";
        let got = built(raw, &[(0, Pick::Lines(vec![0]))], false);
        assert!(
            got.contains("--- \"a/q\\\"t\"\n+++ \"b/q\\\"t\"\n"),
            "{got}"
        );
    }

    #[test]
    fn mode_lines_travel_with_a_modification() {
        let raw =
            "diff --git a/f b/f\nold mode 100644\nnew mode 100755\nindex 1..2\n--- a/f\n+++ b/f\n\
@@ -1 +1 @@\n-x\n+y\n";
        let got = built(raw, &[(0, Pick::All)], false);
        assert!(
            got.starts_with("diff --git a/f b/f\nold mode 100644\nnew mode 100755\n--- a/f\n"),
            "{got}"
        );
        assert!(got.contains("@@ -1,1 +1,1 @@\n-x\n+y\n"), "{got}");
    }

    const EOF: &str = "diff --git a/e b/e\n--- a/e\n+++ b/e\n@@ -1,2 +1,3 @@\n c\n-x\n\
\\ No newline at end of file\n+y\n+z\n\\ No newline at end of file\n";

    #[test]
    fn the_no_newline_marker_follows_the_line_it_belongs_to() {
        let hs = hunks(EOF.as_bytes());
        assert!(hs[0].lines[1].no_newline && hs[0].lines[3].no_newline);
        assert!(!hs[0].lines[2].no_newline);
        assert_eq!(built(EOF, &[(0, Pick::All)], false), EOF);
    }

    #[test]
    fn keeping_a_last_line_and_appending_after_it_gives_it_a_newline() {
        // Stage `+y` only: `x` stays, on the index side without a newline (as it
        // is), on the written side with one, because `y` now follows it.
        let got = built(EOF, &[(0, Pick::Lines(vec![2]))], false);
        assert!(
            got.ends_with("@@ -1,2 +1,3 @@\n c\n-x\n\\ No newline at end of file\n+x\n+y\n"),
            "{got}"
        );
        // Stage `-x` and `+z`: z keeps its own missing newline.
        let got = built(EOF, &[(0, Pick::Lines(vec![1, 3]))], false);
        assert!(
            got.ends_with("@@ -1,2 +1,2 @@\n c\n-x\n\\ No newline at end of file\n+z\n\\ No newline at end of file\n"),
            "{got}"
        );
    }

    #[test]
    fn unstaging_before_a_last_line_without_newline_mirrors_it() {
        // Reverse: new side matched. Unstage `-x` only (bring x back), keep y, z:
        // x is written, z (matched, last, no newline) stays as printed.
        let got = built(EOF, &[(0, Pick::Lines(vec![1]))], true);
        assert!(
            got.ends_with("@@ -1,4 +1,3 @@\n c\n-x\n y\n z\n\\ No newline at end of file\n"),
            "{got}"
        );
    }

    const CONFLICT: &str = concat!(
        "diff --cc f\n",
        "index 5742e7d,eecbe8e..0000000\n",
        "--- a/f\n",
        "+++ b/f\n",
        "@@@ -1,3 -1,3 +1,7 @@@\n",
        "  a\n",
        "++<<<<<<< HEAD\n",
        " +M\n",
        "++=======\n",
        "+ S\n",
        "++>>>>>>> side\n",
        "  c\n",
    );

    #[test]
    fn a_conflicted_file_is_drawn_but_not_offered() {
        let hs = hunks(CONFLICT.as_bytes());
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].lines.len(), 7);
        assert_eq!((hs[0].old_start, hs[0].new_start), (1, 1));
        assert_eq!(hs[0].lines[1].kind, Kind::Add);
        assert_eq!(hs[0].lines[1].text, b"+<<<<<<< HEAD".to_vec());
        assert!(matches!(
            build(CONFLICT.as_bytes(), &[(0, Pick::All)], false),
            Err(Error::Rule(_))
        ));
    }

    #[test]
    fn a_binary_diff_has_nothing_to_choose() {
        let raw = "diff --git a/b b/b\nindex 1..2 100644\nBinary files a/b and b/b differ\n";
        assert!(matches!(
            build(raw.as_bytes(), &[(0, Pick::All)], false),
            Err(Error::Rule(_))
        ));
    }
}
