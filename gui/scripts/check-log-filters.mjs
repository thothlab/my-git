#!/usr/bin/env node
/**
 * Checks the pure cores behind the log filter bar:
 *
 *   - `src/components/log/searchPattern.ts` - the one search rule: spans for
 *     highlighting, the predicate the dim mode and the jump between matches
 *     both ask, and the compiled-pattern cache;
 *   - `src/components/log/filterValues.ts` - day boundaries for the date
 *     filter and repository-relative paths for the path filter;
 *   - `src/components/pathTree.ts` - the shared path layout behind both file
 *     trees (Changes panel and commit details), which used to be two copies
 *     that had already drifted apart;
 *   - `src/components/diff/lineSelection.ts` - which diff lines are chosen for
 *     stage / unstage / revert, and that a choice belongs to one diff;
 *   - `src/components/blame/blameRules.ts` - runs of one commit in the blame
 *     gutter, age shades, whether a line can be blamed further back, the width
 *     of the text column and where "blame before" lands;
 *   - `src/components/conflicts/conflictRules.ts` - git's conflict markers
 *     (merge / diff3 / zdiff3, CRLF, broken and nested blocks, other marker
 *     sizes), the result assembled from per-block decisions, the editor's own
 *     undo and the navigation between blocks;
 *   - `src/components/rebase/rebaseRules.ts` - the interactive rebase plan: chains,
 *     which row carries a message, what is sent, the preview, and whether a log
 *     selection is one unbroken run that can be squashed;
 *   - `src/components/log/bisectMarks.ts` - which bisect mark a log row carries
 *     (first bad, candidate, bad, good, skipped, under test), what the search is
 *     waiting for, and when the repository's own terms replace the UI's words;
 *   - `src/components/coAuthorRules.ts` - whom the co-author picker offers and
 *     where `Co-authored-by:` lines go in a message, cross-checked against
 *     `git interpret-trailers` itself (so `git` must be on PATH);
 *   - `src/components/log/forgeUrl.ts` - which remote the commit links are
 *     about, and the GitHub / GitLab / Bitbucket URLs built from its address
 *     (https, ssh, scp syntax, credentials that must never reach a link);
 *   - `src/components/log/ageColor.ts` - the absolute age step of a commit for
 *     the "by age" graph colouring.
 *
 * Run it:  node scripts/check-log-filters.mjs      (from `gui/`)
 * Another time zone:  TZ=America/Los_Angeles node scripts/check-log-filters.mjs
 *
 * This is not a test runner and does not add one: the project has no frontend
 * runner and is not to grow one (PRD prd_02 interfaces.md). It is a script that
 * imports the very modules the panel imports - esbuild only strips the types -
 * so what passes here is the code that ships, not a copy of it. Exit code is 0
 * when every assertion holds and 1 otherwise.
 */
import { build } from "esbuild";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "..", "src", "components", "log");
const out = await mkdtemp(join(tmpdir(), "graft-log-filters-"));

await build({
  entryPoints: [
    join(src, "searchPattern.ts"),
    join(src, "filterValues.ts"),
    join(src, "bisectMarks.ts"),
    join(src, "forgeUrl.ts"),
    join(src, "ageColor.ts"),
  ],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

// `pathTree.ts`, `gitConsoleCommand.ts`, `cloneRules.ts` and `coAuthorRules.ts` share this directory, so bundling
// them together keeps a flat common base and both outputs land as flat
// basenames the loader below can find.
await build({
  entryPoints: [
    join(here, "..", "src", "components", "pathTree.ts"),
    join(here, "..", "src", "components", "gitConsoleCommand.ts"),
    join(here, "..", "src", "components", "cloneRules.ts"),
    join(here, "..", "src", "components", "coAuthorRules.ts"),
  ],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

// Its own call: esbuild puts outputs under the common base of an entry-point
// list, so bundling this one together with `pathTree.ts` would write it to
// `diff/editRules.js` and the loader below would not find it.
await build({
  entryPoints: [join(here, "..", "src", "components", "diff", "editRules.ts")],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

// Its own call too, for the same reason: `diff/lineSelection.ts` next to
// `editRules.ts` would share its base with nothing else here, and a combined call
// with any other directory would nest the output.
await build({
  entryPoints: [join(here, "..", "src", "components", "diff", "lineSelection.ts")],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

// Its own call, like the two above: `blame/` shares its base with nothing here.
await build({
  entryPoints: [join(here, "..", "src", "components", "blame", "blameRules.ts")],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

// Its own call, like the three above: `conflicts/` shares its base with nothing here.
await build({
  entryPoints: [join(here, "..", "src", "components", "conflicts", "conflictRules.ts")],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

// Its own call too: `rebase/` shares its base with nothing here.
await build({
  entryPoints: [join(here, "..", "src", "components", "rebase", "rebaseRules.ts")],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

const load = (name) => import(pathToFileURL(join(out, name)).href);
const { compilePattern, spansIn, matchesCommit } = await load("searchPattern.js");
const { asInputDate, dayStart, dayEnd, startOfToday, relativeToRepo, toSlash } =
  await load("filterValues.js");
const { baseName, buildFileTree, countFiles, treeDirPaths } = await load("pathTree.js");
const {
  editAvailability,
  draftReduce,
  draftDirty,
  draftShouldWrite,
  countLines,
  lineStartOffset,
  clampCurrent,
  mayOverwrite,
  samePayload,
  drawRows,
  endsEditSession,
  readingSpot,
  editorScrollTop,
  newSideAnchors,
} = await load("editRules.js");
const { splitShellArgs, formatArgv } = await load("gitConsoleCommand.js");
const sel = await load("lineSelection.js");
const blame = await load("blameRules.js");
const cr = await load("conflictRules.js");
const rb = await load("rebaseRules.js");
const bm = await load("bisectMarks.js");
const clone = await load("cloneRules.js");
const co = await load("coAuthorRules.js");
const fu = await load("forgeUrl.js");
const ag = await load("ageColor.js");

let failed = 0;
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) failed++;
  console.log(a === b ? "ok  " : "FAIL", what, a === b ? "" : `${a} != ${b}`);
};
const q = (text, regex = false, matchCase = false) => compilePattern({ text, regex, matchCase });

// -- Highlighting -------------------------------------------------------------
eq(spansIn(q("fix"), "Fix the fixture"), [[0, 3], [8, 11]], "plain, case-insensitive, two spans");
eq(spansIn(q("fix", false, true), "Fix the fixture"), [[8, 11]], "plain, case-sensitive");
eq(spansIn(q("a*", true), "banana").length, 3, "a pattern that can match nothing terminates");
eq(q("[", true).kind, "error", "invalid regular expression is reported");
eq(typeof q("[", true).message, "string", "...with a reason to show in the field");
eq(spansIn(q("[", true), "anything"), [], "invalid pattern highlights nothing");
eq(spansIn(q(""), "anything"), [], "empty search highlights nothing");

// -- One rule for dimming and for jumping ------------------------------------
// Mirrors logStore.matcher(): the two must agree, or the jump lands on a row
// the highlighting does not consider matched.
eq(matchesCommit(q("^feat", true), "feat: x", "deadbeef"), true, "regex matches the subject");
eq(matchesCommit(q("chore"), "feat: x", "deadbeef"), false, "no match is no match");
eq(matchesCommit(q("dead"), "chore", "deadbeef12"), true, "plain: hash matches by prefix");
eq(matchesCommit(q("beef"), "chore", "deadbeef12"), false, "plain: hash does not match mid-string");
eq(matchesCommit(q("beef", true), "chore", "deadbeef12"), true, "regex: hash is matched by the pattern");
eq(matchesCommit(q("DEAD", false, true), "chore", "deadbeef12"), true, "case-sensitive still finds the hash");
eq(matchesCommit(q("["), "a [ b", "deadbeef"), true, "a bracket is literal in plain mode");

// -- The compiled-pattern cache ----------------------------------------------
// Interleaved on purpose: the cache lives in module state, so a check that asks
// one query twice in a row proves nothing. Each assertion below fails if the
// cache stops telling its keys apart and hands back the previous pattern.
const fix = q("fix");
const fox = q("fox");
eq(spansIn(fox, "fox fix"), [[0, 3]], "a new text gets its own pattern");
eq(spansIn(q("fix"), "fox fix"), [[4, 7]], "switching back compiles the earlier text again");
eq(spansIn(fix, "fox fix"), [[4, 7]], "a pattern handed out earlier still works");
eq(spansIn(q("a.", true), "axb"), [[0, 2]], "regex flag: the dot is a wildcard");
eq(spansIn(q("a.", false), "axb"), [], "same text, flag off: the dot is literal");
eq(spansIn(q("a.", true), "axb"), [[0, 2]], "and back again");
eq(spansIn(q("FIX", false, false), "fix"), [[0, 3]], "case flag off matches");
eq(spansIn(q("FIX", false, true), "fix"), [], "same text, case flag on does not");
eq(q("hit") === q("hit"), true, "an unchanged query is compiled once");

// -- Dates (committer-date bounds for the date filter) ------------------------
eq(asInputDate(dayStart("2026-08-20")), "2026-08-20", "date round-trip is stable in the local zone");
eq(asInputDate(dayEnd("2026-08-20")), "2026-08-20", "the upper bound stays on its own day");
eq(dayEnd("2026-08-20") - dayStart("2026-08-20"), 86399, "the named day is taken whole");
eq(dayStart("2026-08-21") > dayEnd("2026-08-20"), true, "days do not overlap");
eq(asInputDate(startOfToday()) === new Date().toLocaleDateString("sv"), true, "today is today");
eq(asInputDate(null), "", "no bound means an empty field");

// -- Paths --------------------------------------------------------------------
eq(toSlash("a\\b"), "a/b", "separators are normalised");
eq(relativeToRepo("/home/u/repo", "/home/u/repo/src/a.ts"), "src/a.ts", "posix path made relative");
eq(relativeToRepo("/home/u/repo", "/home/u/other/a.ts"), null, "outside the repository is refused");
eq(relativeToRepo("/home/u/repo/", "/home/u/repo"), ".", "the root itself is the whole tree");
eq(relativeToRepo("/home/u/repo", "/home/u/repository/a.ts"), null, "a longer sibling name is not inside");
eq(relativeToRepo("C:\\p\\repo", "C:\\p\\repo\\src\\a.ts"), "src/a.ts", "windows separators");
eq(relativeToRepo("c:\\p\\repo", "C:\\P\\Repo\\src\\A.ts"), "src/A.ts", "windows drive and case folded, name kept");
eq(relativeToRepo("C:\\p\\repo", "C:\\p\\other\\a.ts"), null, "windows path outside is refused");
eq(relativeToRepo("/home/u/Repo", "/home/u/repo/a.ts"), null, "case still matters on a case-sensitive path");
eq(relativeToRepo("/vol/Repo", "/vol/repo/a.ts", { ignoreCase: true }), "a.ts", "caller may declare the volume case-insensitive");
eq(relativeToRepo("", "/home/u/repo/a.ts"), null, "no repository, no path");

// -- Path layout (shared by both file trees) ----------------------------------
// Expectations written out by hand from the rule, not from the code: a chain of
// single-child directories is one row, and the collapse keys are the paths of
// the rows that exist - the deepest segment of a merged chain, not every
// intermediate one. The Changes panel used to name all three of `src`,
// `src/components`, `src/components/log` and draw only the last.
const files = (...paths) => paths.map((path) => ({ path }));
const chain = buildFileTree(files("src/components/log/a.ts", "src/components/log/b.ts", "README.md"));
eq(chain.dirs.map((d) => d.name), ["src/components/log"], "a single-child chain is one row");
eq(chain.dirs.map((d) => d.path), ["src/components/log"], "...whose path is the deepest segment");
eq(treeDirPaths(chain), ["src/components/log"], "one row, one collapse key");
eq(chain.files.map((f) => f.path), ["README.md"], "the root keeps its own files");
eq(countFiles(chain), 3, "files are counted through the whole subtree");

const forked = buildFileTree(files("a/b/c.ts", "a/d/e.ts"));
eq(forked.dirs.map((d) => d.name), ["a"], "a directory with two children does not merge");
eq(treeDirPaths(forked), ["a", "a/b", "a/d"], "every drawn row gets a key");
eq(treeDirPaths(buildFileTree(files("top.ts"))), [], "a flat list has no directory rows");
// A directory holding a file *and* one subdirectory stays its own row.
const held = buildFileTree(files("a/keep.ts", "a/b/c.ts"));
eq(treeDirPaths(held), ["a", "a/b"], "a directory with a file of its own does not merge away");
eq(baseName("a/b/c.ts"), "c.ts", "base name of a nested path");
eq(baseName("top.ts"), "top.ts", "base name of a bare name");

// -- When editing the right side is offered (prd_03) ---------------------------
// The three conditions of the PRD, written out by hand: side-by-side view, the
// right side is the working tree (`sideLabels(...).right.readOnly === false`),
// and the file came back editable. A reason key, never a bare `false`.
const cond = (o) => editAvailability({ split: true, readOnly: false, loading: false, blocked: null, ...o });
eq(cond({}), null, "all three conditions met: editing is offered");
eq(cond({ split: false }), "unified", "unified view names itself as the reason");
eq(cond({ readOnly: true }), "read-only", "a revision on the right cannot be edited");
eq(cond({ loading: true }), "loading", "the file has not been read yet");
eq(cond({ blocked: "binary" }), "binary", "a blocked file reports the backend's own key");
eq(cond({ blocked: "too-large" }), "too-large", "...including the size ceiling");
eq(cond({ split: false, blocked: "binary" }), "unified", "the nearest reason wins over a later one");
eq(cond({ readOnly: true, loading: true }), "read-only", "read-only is judged before the read");

// -- The draft between the keyboard and the disk -------------------------------
// The trace that earns its keep is the last one: typing *while a write is in
// flight* must leave the draft dirty when that write lands, or the second
// automatic save never happens and the last words typed stay in the window only
// (PRD §Риски, "вторая автозапись подряд").
const trace = (...events) => events.reduce(draftReduce, { text: "", saved: "", writing: null });
const opened = trace({ kind: "synced", text: "a" });
eq(opened, { text: "a", saved: "a", writing: null }, "opening a file leaves nothing unsaved");
eq(draftDirty(opened), false, "...and nothing to write");
const typed = draftReduce(opened, { kind: "type", text: "ab" });
eq(draftDirty(typed), true, "a keystroke is unsaved at once");
eq(draftShouldWrite(typed), true, "...and is something to write");
const sent = draftReduce(typed, { kind: "sent" });
eq(draftShouldWrite(sent), false, "a write in flight is never doubled");
eq(draftDirty(sent), true, "...while the disk still has the old text");
const typedAgain = draftReduce(sent, { kind: "type", text: "abc" });
eq(draftShouldWrite(typedAgain), false, "typing during a write still waits for it");
const landed = draftReduce(typedAgain, { kind: "ok" });
eq(landed, { text: "abc", saved: "ab", writing: null }, "the write marks clean what it sent, not what is typed now");
eq(draftShouldWrite(landed), true, "so the characters typed meanwhile are written next");
const settled = draftReduce(draftReduce(landed, { kind: "sent" }), { kind: "ok" });
eq(draftDirty(settled), false, "two writes in a row settle the draft");
const refused = draftReduce(draftReduce(typed, { kind: "sent" }), { kind: "fail" });
eq(refused, { text: "ab", saved: "a", writing: null }, "a failed write keeps the typed text and the old disk state");
eq(draftShouldWrite(refused), true, "...and the text is still waiting to be written");
eq(draftReduce(typed, { kind: "ok" }), { text: "ab", saved: "a", writing: null }, "an answer with nothing in flight moves nothing");
eq(draftReduce(typed, { kind: "synced", text: "z" }), { text: "z", saved: "z", writing: null }, "rereading from disk replaces the draft");

// -- Text measurements the editor draws by ------------------------------------
// A textarea puts the caret on the empty line after a trailing newline, so that
// line is drawn and has to be numbered.
eq(countLines(""), 1, "an empty file is one line");
eq(countLines("a"), 1, "one line without a terminator");
eq(countLines("a\n"), 2, "a trailing newline opens a line of its own");
eq(countLines("a\nb"), 2, "two lines, no terminator");
eq(countLines("a\n\nb\n"), 4, "blank lines are counted");
eq(lineStartOffset("a\nbb\nc", 1), 0, "the first line starts at the beginning");
eq(lineStartOffset("a\nbb\nc", 2), 2, "past the first newline");
eq(lineStartOffset("a\nbb\nc", 3), 5, "past the second");
eq(lineStartOffset("a\nbb\nc", 9), 6, "a line the draft no longer has clamps to the end");
eq(lineStartOffset("", 3), 0, "an empty draft has one offset");

// -- The pointer at the current difference, after the payload was replaced -----
// Staging or editing republishes the file with a different number of
// differences, and the pointer is clamped against the list that now exists.
eq(clampCurrent(0, 2), -1, "a file with no differences left has no current one");
eq(clampCurrent(2, 5), 1, "a shortened list points at its own last difference");
eq(clampCurrent(9, 2), 2, "a longer list leaves the pointer where it stood");
eq(clampCurrent(0, -1), -1, "nothing chosen stays nothing chosen");
eq(clampCurrent(9, -1), -1, "...and is not pulled up to the first difference");
eq(clampCurrent(3, 2), 2, "the last difference of an unchanged list is kept");

// -- When an overwrite may go ahead (prd_03) ----------------------------------
// The reread behind "overwrite" can come back blocked. Only `missing` is a
// blockage an overwrite answers - its empty digest is the documented way to
// create the file again. Every other one has an empty digest too, and writing
// with it would be refused as "changed on disk", asking the same question again
// under a reason that is not the true one (rule 3 of prd_03_interfaces.md).
eq(mayOverwrite(null), true, "an editable file is overwritten with the typed text");
eq(mayOverwrite("missing"), true, "a deleted file is created again");
eq(mayOverwrite("binary"), false, "a file replaced by a binary is not written over");
eq(mayOverwrite("too-large"), false, "...nor one that outgrew the ceiling");
eq(mayOverwrite("mixed-eol"), false, "...nor one whose line endings became mixed");

// -- Is the answer that just arrived the one already on screen? ---------------
// Every fresh RepoState makes the panel re-read the file, and most of those
// answers are identical. Republishing one rebuilds a reference-keyed row list
// and takes the scroll position with it, so an alt-tab would jump the reader to
// the top of the file.
// A hunk by its header and a compact spelling of its lines: "-a" is a removal of
// `a` from line 1, "+b" an addition of `b` as line 1.
const hunk = (header, spelled) => ({
  header,
  lines: spelled.split("\n").filter(Boolean).map((l) => ({
    origin: l[0],
    content: l.slice(1),
    oldNo: l[0] === "+" ? null : 1,
    newNo: l[0] === "-" ? null : 1,
  })),
});
const payload = (over = {}) => ({
  path: "a.txt",
  binary: false,
  digest: "d1",
  mergeFirstParent: false,
  hunks: [hunk("@@ -1,2 +1,2 @@", "-a\n+b\n")],
  ...over,
});
eq(samePayload(payload(), payload()), true, "the same patch read twice is the same payload");
eq(samePayload(payload(), null), false, "the first answer replaces nothing shown");
eq(samePayload(null, null), false, "...and two absences are not a match either");
eq(
  samePayload(payload(), payload({ hunks: [hunk("@@ -1,2 +1,2 @@", "-a\n+c\n")] })),
  false,
  "a hunk whose text changed is a different payload",
);
eq(
  samePayload(payload(), payload({ hunks: [hunk("@@ -1,3 +1,3 @@", "-a\n+b\n")] })),
  false,
  "...as is one that moved to other line numbers",
);
eq(samePayload(payload(), payload({ hunks: [] })), false, "a staged hunk leaves a shorter list");
eq(
  samePayload(payload(), payload({ digest: "d2" })),
  false,
  "another digest is another payload: the one on screen is what the next line action sends",
);
eq(samePayload(payload(), payload({ path: "b.txt" })), false, "another file is another payload");
eq(
  samePayload(payload({ binary: true, oldSize: 4 }), payload({ binary: true, oldSize: 9 })),
  false,
  "a binary file is compared by its sizes",
);
eq(
  samePayload(payload(), payload({ mergeFirstParent: true })),
  false,
  "the first-parent note is part of what is drawn",
);

// -- May the rows already drawn stay while a new answer travels? --------------
// A re-read is not a departure. Tearing the rows down for the wait empties the
// scrolling container, the browser clamps its scrollTop to zero, and the reader
// is thrown back to the first hunk - before any payload comparison can help.
const gate = (over = {}) => ({
  loading: false,
  error: false,
  drawnKey: "a.txt|none",
  requestKey: "a.txt|none",
  ...over,
});
eq(drawRows(gate()), true, "a settled answer is drawn");
eq(
  drawRows(gate({ loading: true })),
  true,
  "a re-read of the same request keeps the rows it already drew",
);
eq(
  drawRows(gate({ loading: true, requestKey: "b.txt|none" })),
  false,
  "the wait for another file shows the placeholder instead",
);
eq(
  drawRows(gate({ loading: true, requestKey: "a.txt|all" })),
  false,
  "...and so does the wait for another whitespace mode",
);
eq(
  drawRows(gate({ loading: true, drawnKey: null })),
  false,
  "the very first answer has nothing to keep drawing",
);
eq(
  drawRows(gate({ loading: true, drawnKey: null, requestKey: null })),
  false,
  "two absent keys are not a match",
);
eq(
  drawRows(gate({ error: true })),
  false,
  "a patch left standing under \"diff unavailable\" would read as current",
);
eq(
  drawRows(gate({ error: true, loading: true })),
  false,
  "...including while the next attempt is in flight",
);

// -- What ends an editing session (prd_03) ------------------------------------
// Every exit in `DiffView` goes through `exitEdit`, which passes one of these
// six and does nothing when the answer is false - so each value below is one a
// real call site hands over, and flipping one of these answers changes what the
// panel does.
//
// Losing the caret is not a departure: every button in the window takes the
// focus off the textarea when pressed, so a blur that closes the editor means
// the refresh button drops the reader out of edit mode.
eq(endsEditSession("escape"), true, "Escape in the textarea saves and closes");
eq(endsEditSession("toggle"), true, "...and so does a second press of the edit control");
eq(endsEditSession("unified"), true, "the split/unified button leaves the editable layout");
eq(endsEditSession("source"), true, "selecting another file ends the session on this one");
eq(endsEditSession("blur"), false, "the refresh button takes the caret and nothing else");
eq(endsEditSession("window"), false, "...nor does alt-tabbing out of the window end it");

// -- Where the editor opens when the control is pressed -----------------------
// The rows passed in are every drawn row, each with the new-version line it
// stands on (see `newSideAnchors`), measured against the top
// of the scrolling viewport; a row scrolled past has a negative `top`. The rule
// picks the topmost one still visible, and the panel puts the caret on that line
// and scrolls the textarea so it sits at the same height it had in the diff.
const row = (top, line, height = 16) => ({ top, height, line });

eq(readingSpot([row(0, 1), row(16, 2)]), { line: 1, offset: 0 }, "an unscrolled diff opens at its first line");
eq(
  readingSpot([row(-320, 100), row(-16, 120), row(0, 121), row(16, 122)]),
  { line: 121, offset: 0 },
  "scrolled down, the topmost visible row is the reader's place",
);
eq(
  readingSpot([row(-8, 127), row(8, 128)]),
  { line: 127, offset: -8 },
  "a row half over the top edge still counts, and says how far over",
);
eq(
  readingSpot([row(-16, 40), row(0, 60), row(16, 61)]),
  { line: 60, offset: 0 },
  "a row whose bottom edge sits exactly on the top edge is past",
);
eq(readingSpot([]), null, "nothing drawn - an empty diff or a big-diff summary");
eq(
  readingSpot([row(-64, 10), row(-16, 12)]),
  null,
  "a viewport with no line of the new version in it opens the file at its start",
);

// The scan stops at the answer: the caller measures the DOM lazily, so a rule
// that drained the sequence would cost a layout read per row of the file.
let measured = 0;
function* lazyRows() {
  for (const r of [row(-32, 100), row(-16, 101), row(0, 102), row(16, 103), row(32, 104)]) {
    measured++;
    yield r;
  }
}
eq(readingSpot(lazyRows()), { line: 102, offset: 0 }, "a lazy source answers the same");
eq(measured, 3, "...and nothing below the first visible row was measured for it");

// Only the clamp is asserted. What the arithmetic does between the clamps is
// one line of formula, and an expectation spelled with that same formula would
// hold whatever the formula became - it would check the code against itself.
eq(editorScrollTop({ line: 1, offset: 0 }, 16), 0, "the first line has nothing above it to scroll");
eq(
  editorScrollTop({ line: 5, offset: 300 }, 16),
  0,
  "a line nearer the top than the offset asks for clamps - it is on the first screen anyway",
);

// -- The line a row stands on in the new version ------------------------------
// The input is each row's own `newNo`, `null` where the row draws a deletion.
// Expectations are read off the file the diff describes, not recomputed from the
// same walk: a deletion belongs to the line that closed over the gap it left.
eq(
  newSideAnchors([10, 11, 12]),
  [10, 11, 12],
  "rows that have a number keep it",
);
eq(
  newSideAnchors([10, null, 11]),
  [10, 11, 11],
  "text taken out between lines 10 and 11 belongs to line 11",
);
eq(
  newSideAnchors([7, null, null, 8]),
  [7, 8, 8, 8],
  "two deleted lines in one gap both belong to the line after them",
);
eq(
  newSideAnchors([null, null, 5, 6]),
  [5, 5, 5, 6],
  "a hunk opening with deletions reads forward - there is no previous number",
);
eq(
  newSideAnchors([3, null]),
  [3, 4],
  "a deletion at the end of the file points one past the last line",
);
eq(
  newSideAnchors([null, null]),
  [1, 1],
  "a file deleted whole has only line 1 to point at",
);
eq(newSideAnchors([]), [], "nothing drawn, nothing to place");

// -- Choosing lines for stage / unstage / revert ------------------------------
// Two hunks: [" c", "-d", "+a", "+b", " c"] and [" c", "+x", " c"].
const H = [
  { lines: [" ", "-", "+", "+", " "].map((origin) => ({ origin })) },
  { lines: [" ", "+", " "].map((origin) => ({ origin })) },
];
const at = (hunk, line) => ({ hunk, line });
const none = sel.NO_SELECTION;
eq(sel.selectable({ origin: " " }), false, "a context line cannot be chosen");
eq(sel.selectable({ origin: "-" }) && sel.selectable({ origin: "+" }), true, "a removal and an addition can");
eq(sel.selectableRefs(H).map(sel.lineKey), ["0:1", "0:2", "0:3", "1:1"], "choosable lines in document order");

let s1 = sel.toggle(none, H, at(0, 2), "D", undefined);
eq(s1.keys, ["0:2"], "a click chooses the line");
eq([s1.digest, s1.context], ["D", null], "...and binds the choice to the diff it was made in");
eq(sel.toggle(s1, H, at(0, 0), "D", undefined), s1, "a click on a context line changes nothing");
eq(sel.toggle(s1, H, at(0, 2), "D", undefined).keys, [], "a second click gives the line back");
eq(sel.toggle(s1, H, at(0, 1), "D", undefined).keys, ["0:1", "0:2"], "keys stay in document order");

eq(sel.forPayload(s1, "D"), s1, "the selection holds on the diff it was made in");
eq(sel.forPayload(s1, "E").keys, [], "...and is nothing on any other one");
eq(sel.toggle(s1, H, at(0, 1), "E", 5).keys, ["0:1"], "a click on another diff starts over");
eq(sel.toggle(s1, H, at(0, 1), "E", 5).context, 5, "...with that diff's context");
eq([sel.toggle(s1, H, at(0, 1), "D", 9).context], [undefined], "the context stays the one the lines were chosen at");

const r1 = sel.extendTo(sel.toggle(none, H, at(0, 1), "D", undefined), H, at(1, 1), "D", undefined);
eq(r1.keys, ["0:1", "0:2", "0:3", "1:1"], "shift-click chooses the range, across hunks, skipping context");
const r2 = sel.extendTo(r1, H, at(0, 2), "D", undefined);
eq(r2.keys, ["0:1", "0:2"], "moving the end back gives the lines past it back");
const r3 = sel.extendTo(sel.toggle(r2, H, at(1, 1), "D", undefined), H, at(0, 3), "D", undefined);
eq(r3.keys, ["0:1", "0:2", "0:3", "1:1"], "a range from a new anchor keeps what was chosen before");
eq(sel.extendTo(none, H, at(0, 3), "D", undefined).keys, ["0:3"], "without an anchor shift-click is a click");

const k1 = sel.step(none, H, 1, "D", undefined);
eq(k1.keys, ["0:1"], "the first step chooses the first choosable line");
eq(sel.step(none, H, -1, "D", undefined).keys, ["1:1"], "...or the last one going up");
eq(sel.step(none, H, 1, "D", undefined, at(1, 0)).keys, ["1:1"], "...or the first one from the difference on screen");
const k3 = sel.step(sel.step(k1, H, 1, "D", undefined), H, 1, "D", undefined);
eq(k3.keys, ["0:1", "0:2", "0:3"], "steps extend the range");
eq(sel.step(k3, H, -1, "D", undefined).keys, ["0:1", "0:2"], "a step back shrinks it");
const end = sel.step(sel.step(k3, H, 1, "D", undefined), H, 1, "D", undefined);
eq(end.cursor, { hunk: 1, line: 1 }, "steps stop at the last line instead of wrapping");

eq(sel.picks(r1), [{ hunk: 0, lines: [1, 2, 3] }, { hunk: 1, lines: [1] }], "picks group lines by hunk");
eq(sel.picks(none), [], "nothing chosen, nothing sent");

// -- Git console input splitting ----------------------------------------------
eq(splitShellArgs("status"), { ok: true, args: ["status"] }, "single word");
eq(
  splitShellArgs("commit -m fix"),
  { ok: true, args: ["commit", "-m", "fix"] },
  "plain whitespace splitting",
);
eq(
  splitShellArgs("git status"),
  { ok: true, args: ["status"] },
  "a leading literal 'git' is dropped",
);
eq(
  splitShellArgs('commit -m "fix: two words"'),
  { ok: true, args: ["commit", "-m", "fix: two words"] },
  "double-quoted argument keeps its spaces",
);
eq(
  splitShellArgs("commit -m 'fix: two words'"),
  { ok: true, args: ["commit", "-m", "fix: two words"] },
  "single-quoted argument keeps its spaces",
);
eq(
  splitShellArgs('log --grep="a \\"quoted\\" word"'),
  { ok: true, args: ["log", '--grep=a "quoted" word'] },
  "backslash-escaped quote inside a double-quoted argument",
);
eq(splitShellArgs("  status   --short  "), { ok: true, args: ["status", "--short"] }, "extra whitespace is collapsed");
eq(splitShellArgs(""), { ok: true, args: [] }, "empty input has no arguments");
eq(splitShellArgs('commit -m "unterminated').ok, false, "an unmatched quote is reported, not silently closed");
eq(splitShellArgs("git"), { ok: true, args: [] }, "'git' alone is the leading token, not a subcommand");

// ── formatArgv (the journal's one-line command) ─────────────────────────────
eq(formatArgv(["log", "--oneline", "-n", "5"]), "log --oneline -n 5", "plain words stay unquoted");
eq(formatArgv(["commit", "-m", "two words"]), "commit -m 'two words'", "an argument with a space is quoted");
eq(formatArgv(["tag", ""]), "tag ''", "an empty argument stays visible");
for (const argv of [
  ["commit", "-m", "it's \"quoted\""],
  ["log", "--format=%H%x00%s", "--", ":(literal)a b/c.txt"],
  ["show", "HEAD~1^{commit}", "$HOME", "a\\b"],
]) {
  eq(splitShellArgs(formatArgv(argv)), { ok: true, args: argv }, `round trip ${JSON.stringify(argv)}`);
}

// -- Blame gutter (blameRules.ts) -------------------------------------------
{
  const L = (...o) => o.map((origin) => ({ origin }));
  eq(blame.runStarts(L(0, 0, 1, 1, 0)), [true, false, true, false, true], "a run starts where the commit changes");
  eq(blame.runStarts(L()), [], "no lines, no runs");
  eq(blame.runStarts(L(3)), [true], "a single line starts its run");

  const O = (authorAt, uncommitted = false) => ({ authorAt, uncommitted });
  eq(
    blame.ageLevels([O(100), O(300), O(200)]),
    [0, 4, 2],
    "age shades by rank: oldest 0, newest AGE_LEVELS - 1, the middle between",
  );
  eq(
    blame.ageLevels([O(10), O(20), O(1e9)]),
    [0, 2, 4],
    "by rank, not by distance: a decade-old pair does not collapse into one shade",
  );
  eq(blame.ageLevels([O(5), O(5)]), [4, 4], "one date only: everything is newest");
  eq(blame.ageLevels([O(1), O(0, true), O(2)]), [0, 4, 4], "an uncommitted line is the newest");
  eq(blame.AGE_LEVELS, 5, "five shades");

  const prev = { hash: "a".repeat(40), path: "f.txt" };
  const B = (previous, boundary, uncommitted = false) => ({ previous, boundary, uncommitted });
  eq(blame.beforeBlock(B(prev, false)), null, "a previous version: blame before is offered");
  eq(blame.beforeBlock(B(prev, false, true)), null, "an uncommitted line of a committed file steps into HEAD");
  eq(blame.beforeBlock(B(null, true)), "boundary", "the earliest version says so");
  eq(blame.beforeBlock(B(null, false)), "created", "a file created in the commit says so");
  eq(blame.beforeBlock(B(null, false, true)), "new", "a staged new file is not 'created in this commit'");

  eq(blame.textColumns("abc"), 3, "plain text: one column per character");
  eq(blame.textColumns("\tx"), 9, "a TAB runs to the next stop of eight");
  eq(blame.textColumns("ab\tx"), 9, "a TAB after two characters still stops at eight");
  eq(blame.textColumns("жж"), 2, "a Cyrillic letter is one column, not two bytes");
  eq(blame.maxColumns([{ text: "a" }, { text: "\t\tz" }, { text: "" }]), 17, "the widest line wins");
  eq(blame.maxColumns([]), 0, "no lines, no width");

  eq(blame.landingLine({ from: 3, to: 5 }, 10), 3, "lands on the first line of the range");
  eq(blame.landingLine({ from: 12, to: 12 }, 10), 10, "clamped to the last line there is");
  eq(blame.landingLine({ from: 0, to: 0 }, 10), 1, "never above the first line");
  eq(blame.landingLine({ from: 1, to: 1 }, 0), null, "an empty file has nowhere to land");
}

// -- Conflict markers (R05e) --------------------------------------------------
{
  const M = "<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs 1\n>>>>>>> feat\n";
  const merge = cr.parseConflicts(`top\n${M}bottom\n`);
  eq(merge.ok, true, "merge style: parses");
  eq(merge.conflicts.length, 1, "merge style: one block");
  const b0 = merge.conflicts[0];
  eq([b0.ours, b0.base, b0.theirs], [["ours 1", "ours 2"], null, ["theirs 1"]], "merge style: no base");
  eq([b0.oursLabel, b0.theirsLabel], ["HEAD", "feat"], "labels after the markers");
  eq([b0.start, b0.end], [1, 6], "0-based lines of the opening and closing markers");
  eq(merge.regions.map((r) => r.kind), ["common", "conflict", "common"], "common, block, common");
  eq(merge.regions[2].start, 7, "the common text after the block starts after its closing marker");

  const D3 = "a\n<<<<<<< HEAD\nO\n||||||| merged common ancestors\nB\n=======\nT\n>>>>>>> feat\nz\n";
  const diff3 = cr.parseConflicts(D3);
  eq([diff3.conflicts[0].base, diff3.conflicts[0].baseLabel], [["B"], "merged common ancestors"], "diff3: the base and its label");
  const Z = "<<<<<<< ours\nx\n||||||| 1234abc\n=======\ny\n>>>>>>> theirs\n";
  const z = cr.parseConflicts(Z);
  eq(z.conflicts[0].base, [], "zdiff3: an empty base is an empty list, not a missing one");

  const crlf = cr.parseConflicts(D3.replace(/\n/g, "\r\n"));
  eq(crlf.ok && crlf.conflicts[0].theirs, ["T"], "CRLF: the same block, no \\r in the lines");
  eq(cr.buildResult(crlf, [{ kind: "ours" }]).text, "a\nO\nz\n", "CRLF: the result comes out in \\n");

  const empty = cr.parseConflicts("<<<<<<< HEAD\n=======\nt\n>>>>>>> x\n");
  eq([empty.conflicts[0].ours, empty.conflicts[0].theirs], [[], ["t"]], "an empty side is an empty list");

  const two = cr.parseConflicts(`${M}mid\n${M}`);
  eq(two.conflicts.map((c) => c.index), [0, 1], "several blocks are numbered in order");

  eq(cr.parseConflicts("x\n<<<<<<< a\n1\n<<<<<<< b\n").error, { reason: "nested", line: 4 }, "nested opening marker: error at its line");
  eq(cr.parseConflicts("x\n<<<<<<< a\n1\n=======\n2\n").error, { reason: "unterminated", line: 2 }, "unterminated: error at the opening line");
  eq(cr.parseConflicts("<<<<<<< a\n1\n>>>>>>> b\n").error, { reason: "no-separator", line: 3 }, "closing before the separator");
  eq(cr.parseConflicts("<<<<<<< a\n1\n=======\n2\n=======\n3\n>>>>>>> b\n").error, { reason: "stray-separator", line: 5 }, "a second separator");
  eq(cr.parseConflicts("<<<<<<< a\n=======\n||||||| b\n>>>>>>> c\n").error, { reason: "stray-base", line: 3 }, "a base after the separator");
  eq(cr.parseConflicts("fine\n>>>>>>> b\n").error, { reason: "stray-closing", line: 2 }, "a closing marker with no block");
  eq(cr.parseConflicts("||||||| b\n").error, { reason: "stray-base", line: 1 }, "a base marker with no block");
  eq(cr.parseConflicts("Title\n=======\ntext\n").ok, true, "a bare ======= outside a block is a Markdown underline");
  eq(cr.parseConflicts("<<<<<<<< eight\n>>>>>>>> eight\n").ok, true, "eight characters are content, as for git");
  eq(cr.parseConflicts("<<<<<<<x\n").ok, true, "a marker needs a space before its label");

  const twelve = "<".repeat(12) + " HEAD\no\n" + "=".repeat(12) + "\nt\n" + ">".repeat(12) + " f\n";
  eq(cr.parseConflicts(twelve).error, { reason: "marker-size", line: 1, size: 12 }, "markers of another size: refused, with the size");
  eq(cr.parseConflicts(twelve, 12).conflicts.length, 1, "the same text with conflict-marker-size=12: one block");
  eq(cr.parseConflicts(M, 12).error, { reason: "marker-size", line: 1, size: 7 }, "7-long markers under a 12 attribute: refused too");

  // Assembly.
  const text = `top\n${M}mid\n${M}bottom\n`;
  const p = cr.parseConflicts(text);
  eq(cr.buildResult(p, []).text, text, "no decisions: the text comes back byte for byte");
  eq(cr.buildResult(cr.parseConflicts("a\nb"), []).text, "a\nb", "no final newline stays absent");
  eq(cr.buildResult(cr.parseConflicts(""), []).text, "", "an empty text stays empty");
  eq(cr.buildResult(p, [{ kind: "ours" }]).text, `top\nours 1\nours 2\nmid\n${M}bottom\n`, "take ours in the first block only");
  eq(cr.buildResult(p, [null, { kind: "theirs" }]).text, `top\n${M}mid\ntheirs 1\nbottom\n`, "take theirs in the second only");
  eq(cr.decisionLines(p.conflicts[0], { kind: "both", first: "ours" }), ["ours 1", "ours 2", "theirs 1"], "both, ours first");
  eq(cr.decisionLines(p.conflicts[0], { kind: "both", first: "theirs" }), ["theirs 1", "ours 1", "ours 2"], "both, theirs first");
  eq(cr.decisionLines(p.conflicts[0], { kind: "manual", lines: ["x"] }), ["x"], "manual: the typed lines");
  eq(cr.buildResult(p, [{ kind: "manual", lines: [] }, { kind: "manual", lines: [] }]).text, "top\nmid\nbottom\n", "a block resolved to nothing");
  const built = cr.buildResult(p, [{ kind: "ours" }]);
  eq(built.spans, [{ index: 0, from: 1, to: 3, decided: true }, { index: 1, from: 4, to: 10, decided: false }], "spans: where each block landed");

  // Line picks, in click order.
  const b = diff3.conflicts[0];
  let d = null;
  d = cr.togglePick(b, d, { side: "theirs", index: 0 });
  d = cr.togglePick(b, d, { side: "base", index: 0 });
  d = cr.togglePick(b, d, { side: "ours", index: 0 });
  eq(cr.decisionLines(b, d), ["T", "B", "O"], "lines land in the order they were clicked");
  eq(cr.buildResult(diff3, [d]).text, "a\nT\nB\nO\nz\n", "and the result shows them at once");
  d = cr.togglePick(b, d, { side: "base", index: 0 });
  eq(cr.decisionLines(b, d), ["T", "O"], "a second click drops the line");
  d = cr.togglePick(b, d, { side: "theirs", index: 0 });
  d = cr.togglePick(b, d, { side: "ours", index: 0 });
  eq(d, null, "the last line dropped: the block is undecided again");
  eq(cr.decisionLines(b, cr.togglePick(b, { kind: "ours" }, { side: "theirs", index: 0 })), ["O", "T"], "a click after take-ours adds to ours");
  eq(cr.togglePick(b, { kind: "ours" }, { side: "ours", index: 0 }), null, "and dropping ours' only line leaves nothing");
  eq(cr.asPicks(p.conflicts[0], { kind: "both", first: "theirs" }), [{ side: "theirs", index: 0 }, { side: "ours", index: 0 }, { side: "ours", index: 1 }], "a both-decision as picks keeps its order");
  eq(cr.togglePick(b, { kind: "manual", lines: ["m"] }, { side: "ours", index: 0 }), { kind: "manual", lines: ["m", "O"] }, "a click on a hand-edited block appends");
  eq(cr.togglePick(b, null, { side: "ours", index: 9 }), null, "a line that is not there changes nothing");

  // Typing in the result.
  const dd = [{ kind: "ours" }, null];
  const typed = built.text.replace("ours 2\n", "ours 2 edited\n");
  eq(cr.absorbEdit(p, dd, built, typed), [{ kind: "manual", lines: ["ours 1", "ours 2 edited"] }, null], "an edit inside a decided block becomes its manual decision");
  eq(cr.absorbEdit(p, dd, built, built.text.replace("mid\n", "MID\n")), null, "an edit of common text is baked");
  eq(cr.absorbEdit(p, dd, built, built.text.replace("theirs 1\n", "x\n")), null, "an edit inside an undecided block is baked");
  eq(cr.absorbEdit(p, dd, built, built.text.replace("ours 1\nours 2\n", "")), [{ kind: "manual", lines: [] }, null], "deleting a whole decided block's lines");
  eq(cr.absorbEdit(p, dd, built, built.text.slice(0, -1)), null, "removing the final newline is baked");

  eq(cr.unresolved(p, [{ kind: "ours" }]), [1], "one block left");
  eq(cr.leftoverMarkers(`ok\n${M}`), [2], "leftover markers: the line of each block still marked");
  eq(cr.leftoverMarkers("Title\n=======\n"), [], "an underline is not a leftover");
  eq(cr.leftoverMarkers("a\n<<<<<<< x\n"), [2], "a broken block is a leftover");

  eq(cr.stepConflict([0, 2, 5], 2, 1), 5, "next");
  eq(cr.stepConflict([0, 2, 5], 5, 1), 0, "next wraps");
  eq(cr.stepConflict([0, 2, 5], 0, -1), 5, "previous wraps");
  eq(cr.stepConflict([0, 2, 5], 3, -1), 2, "previous from a resolved block");
  eq(cr.stepConflict([], 0, 1), null, "nothing left, nowhere to go");
  eq(cr.lineOffset("ab\ncd\nef", 2), 6, "offset of a line");
  eq(cr.lineOffset("ab", 5), 2, "past the end: the end");

  // Undo.
  let h = cr.emptyHistory();
  h = cr.historyRecord(h, "v0");
  h = cr.historyRecord(h, "v1");
  const u = cr.historyUndo(h, "v2");
  eq([u.value, u.history], ["v1", { past: ["v0"], future: ["v2"] }], "undo steps back and keeps redo");
  const r = cr.historyRedo(u.history, "v1");
  eq([r.value, r.history], ["v2", { past: ["v0", "v1"], future: [] }], "redo steps forward");
  eq(cr.historyRecord(u.history, "v1b").future, [], "a new change forgets redo");
  eq(cr.historyUndo(cr.emptyHistory(), "x"), null, "nothing to undo");
  eq(cr.historyRecord({ past: ["a", "b"], future: [] }, "c", 2).past, ["b", "c"], "the oldest step falls off");
  eq(cr.coalesces({ kind: "type", at: 0 }, "type", 500), true, "a typing burst is one step");
  eq(cr.coalesces({ kind: "type", at: 0 }, "type", 1500), false, "a pause starts a new step");
  eq(cr.coalesces({ kind: "pick", at: 0 }, "type", 10), false, "typing after a click is its own step");
  eq(cr.CONFLICT_CODES.deletedByUs, "DU", "deleted by us is DU");
}

// -- Interactive rebase plan (rebaseRules.ts) --------------------------------
{
  const commits = ["A", "B", "C", "D"].map((s, i) => ({
    hash: String(i + 1).repeat(40),
    shortHash: String(i + 1).repeat(7),
    subject: s,
    message: `${s}\n\nbody ${s}`,
  }));
  const plan = (...actions) =>
    rb.fromCommits(commits).map((e, i) => ({ ...e, action: actions[i] ?? "pick" }));
  const H = (i) => commits[i].hash;

  eq(rb.fromCommits(commits).map((e) => e.action), ["pick", "pick", "pick", "pick"], "everything is picked at first");
  eq(rb.moveEntry([1, 2, 3], 2, 0), [3, 1, 2], "move up to the top");
  eq(rb.moveEntry([1, 2, 3], 0, 5), [1, 2, 3], "a move past the end changes nothing");
  eq(rb.chainsOf(plan("pick", "squash", "drop", "fixup")), [[0, 1, 3]], "a drop does not split a chain");
  eq(rb.chainsOf(plan("pick", "pick", "fixup", "pick")), [[0], [1, 2], [3]], "chains");

  eq(rb.planProblem(plan("drop", "drop", "drop", "drop")), "noneKept", "nothing kept");
  eq(rb.planProblem(plan("drop", "squash")), "firstMelds", "the oldest kept commit cannot meld");
  eq(rb.planProblem(plan("pick", "squash")), null, "a squash after a pick is fine");

  // Message fields.
  let p = plan("pick", "squash", "fixup", "pick");
  eq([0, 1, 2, 3].map((i) => rb.messageSlot(p, i)), [null, null, "combined", null], "a squash chain's field is on its last row");
  eq(rb.slotText(p, 2), "A\n\nbody A\n\nB\n\nbody B", "prefilled with git's own default: head and squashed, fixups left out");
  eq(rb.toSteps(p).map((s) => s.message ?? null), [null, null, null, null], "an untouched combined message is not sent");
  p = p.map((e, i) => (i === 2 ? { ...e, text: "One" } : e));
  eq(rb.toSteps(p)[2], { hash: H(2), action: "fixup", message: "One" }, "an edited one is, on the row that shows it");
  eq(rb.preview(p)[0].subject, "One", "the preview shows the message typed");
  eq(rb.preview(p)[0].from, ["1111111", "2222222", "3333333"], "and what melds into it");

  p = plan("reword", "squash");
  eq([0, 1].map((i) => rb.messageSlot(p, i)), ["reword", null], "a reworded head speaks for its chain");
  eq(rb.toSteps(p)[0].message, "A\n\nbody A", "a reword is prefilled with the commit's own message");
  p = p.map((e, i) => (i === 0 ? { ...e, text: "  " } : e));
  eq(rb.planProblem(p), "emptyMessage", "an emptied message stops the plan");

  eq([0, 1].map((i) => rb.messageSlot(plan("pick", "fixup"), i)), [null, null], "fixups keep the head's message: no field");

  const pv = rb.preview(plan("edit", "drop", "reword"));
  eq(pv.map((c) => [c.subject, c.stops]), [["A", true], ["C", false], ["D", false]], "preview: dropped commits vanish, edit stops");
  eq(rb.summary(plan("pick", "squash", "drop", "fixup")), { kept: 1, melded: 2, dropped: 1 }, "summary");
  eq(rb.ACTION_KEYS.KeyS, "squash", "S squashes");

  // Squash from the log's selection: by first-parent links, not by rows.
  const a = { hash: "a", parents: ["root"] };
  const b = { hash: "b", parents: ["a"] };
  const c = { hash: "c", parents: ["b"] };
  eq(rb.squashRun([c, b, a]), { ok: true, oldestFirst: ["a", "b", "c"] }, "a run, newest first as the log lists it");
  eq(rb.squashRun([a, c, b]), { ok: true, oldestFirst: ["a", "b", "c"] }, "order on screen does not matter");
  eq(rb.squashRun([c, a]), { ok: false, reason: "gap" }, "a gap");
  eq(rb.squashRun([a]), { ok: false, reason: "tooFew" }, "one commit");
  eq(rb.squashRun([{ hash: "m", parents: ["b", "x"] }, b]), { ok: false, reason: "merge" }, "a merge");
  eq(rb.squashRun([b, { hash: "s", parents: ["a"] }]), { ok: false, reason: "gap" }, "two siblings are no run");
  eq(rb.runOpensRange(["a", "b"], ["a", "b", "c"]), true, "the run opens the range");
  eq(rb.runOpensRange(["a", "b"], ["a", "x", "b"]), false, "something else between");
  eq(rb.squashMessage(["one\n", "", "two"]), "one\n\ntwo", "squash prefill joins the messages");
}

// -- Bisect marks on the log -------------------------------------------------
{
  const H = (c) => c.repeat(40);
  const base = { bad: H("b"), good: [H("a")], skip: [H("c")], current: H("d"), firstBad: null, candidates: [], problem: null };
  const m = bm.bisectMarks(base);
  eq([m.get(H("b")), m.get(H("a")), m.get(H("c")), m.get(H("d"))], ["bad", "good", "skip", "testing"], "every role gets its mark");
  eq(bm.bisectMarks({ ...base, current: H("c") }).get(H("c")), "skip", "an answered commit is not 'under test'");
  const done = bm.bisectMarks({ ...base, firstBad: H("b"), current: H("b") });
  eq(done.get(H("b")), "culprit", "the answer beats the bad mark on the same commit");
  eq(done.has(H("d")), false, "a finished search has nothing under test");
  const amb = bm.bisectMarks({ ...base, skip: [H("c"), H("e")], candidates: [H("c"), H("e"), H("b")], current: H("e") });
  eq([amb.get(H("c")), amb.get(H("e")), amb.get(H("b"))], ["candidate", "candidate", "candidate"], "only skipped left: candidates");
  eq(bm.bisectMarks({ ...base, problem: "BISECT_LOG line 3" }).size, 0, "a broken state draws nothing");
  eq(bm.bisectMarks(null).size, 0, "no bisect, no marks");
  eq(bm.bisectMarks({ ...base, bad: "ABC" }).has("abc"), true, "keys are lower case");
  eq(
    [
      bm.bisectPhase({ ...base, bad: null, good: [] }),
      bm.bisectPhase({ ...base, bad: null }),
      bm.bisectPhase({ ...base, good: [] }),
      bm.bisectPhase(base),
      bm.bisectPhase({ ...base, firstBad: H("b") }),
      bm.bisectPhase({ ...base, candidates: [H("c")] }),
      bm.bisectPhase({ ...base, problem: "x" }),
    ],
    ["waitBoth", "waitBad", "waitGood", "testing", "found", "ambiguous", "broken"],
    "what the strip says the search waits for",
  );
  eq([bm.customTerm("bad", "bad"), bm.customTerm("broken", "bad")], [null, "broken"], "custom terms replace the UI's words");
}

// -- Clone: the folder name proposed for an address ----------------------------
{
  const f = clone.folderNameFromUrl;
  eq(
    [
      f("https://h/a/b.git"),
      f("https://h/a/b"),
      f("https://h/a/b/"),
      f("https://h/a/b.git/"),
      f("git@github.com:org/repo.git"),
      f("host:repo.git"),
      f("ssh://git@h:2222/srv/r.git"),
      f("file:///x/y"),
      f("/x/y/.git"),
      f("/x/y/.git/"),
      f("../sibling/repo.git"),
      f("https://h/a/b.git?x=1#top"),
      f("  https://h/a/B.GIT  "),
      f("C:\\work\\proj"),
    ],
    ["b", "b", "b", "b", "repo", "repo", "r", "y", "y", "y", "repo", "b", "B", "proj"],
    "the folder name is the address's last component, without .git or a trailing slash",
  );
  eq([f(""), f("https://"), f("."), f("/"), f("x/..")], ["", "", "", "", ""], "nothing usable: no name");
  eq(
    [clone.isPlainHttp("http://h/r"), clone.isPlainHttp(" HTTP://h/r"), clone.isPlainHttp("https://h/r")],
    [true, true, false],
    "plain http is told apart",
  );
  eq(
    [
      clone.httpLogin("https://me@bitbucket.org/t/r.git"),
      clone.httpLogin("HTTP://org@dev.azure.com/o/p/_git/r"),
      clone.httpLogin("https://u:p@h/r"),
      clone.httpLogin("https://h/r"),
      clone.httpLogin("ssh://git@h/r"),
      clone.httpLogin("https://h/@scope/pkg"),
    ],
    ["me", "org", null, null, null, null],
    "a plain http(s) login is told apart; a password or no user is not one",
  );
  eq(
    [clone.joinDest("/Users/me/src/", "r"), clone.joinDest("/", "r"), clone.joinDest("", "r")],
    ["/Users/me/src/r", "/r", "r"],
    "the destination shown",
  );
}

// -- Co-authors: whom the picker offers, where the trailers go -----------------
{
  const A = { name: "Ann Lee", email: "ann@example.com" };
  const B = { name: "Bob", email: "Bob@Example.com" };
  const people = [A, B, { name: "Me", email: "ME@example.com" }, { name: "No Mail", email: "" },
    { name: "Local", email: "root" }, { name: "Bad <x>", email: "b@x.y" }];
  eq(co.pickable(people, "", [], "me@example.com").map((p) => p.name), ["Ann Lee", "Bob"],
    "the reader, an empty or local-only address and a malformed name are not offered");
  eq(co.pickable(people, "", [{ name: "B.", email: "bob@example.COM" }], null).map((p) => p.name),
    ["Ann Lee", "Me"], "someone chosen is not offered again, whatever the case of the address");
  eq([co.pickable(people, "EXAMPLE.com", [], null).length, co.pickable(people, "lee", [], null)[0]?.name,
    co.pickable(people, "", [], null, 1).length], [3, "Ann Lee", 1], "the query reads name and address without case; the limit holds");
  eq(co.trailerLine({ name: " Ann Lee ", email: " ann@example.com " }), "Co-authored-by: Ann Lee <ann@example.com>", "the trailer line");

  const w = co.withCoAuthors;
  const cases = [
    ["subject only", "Fix it", [A], "Fix it\n\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["a subject that looks like a trailer is still the subject", "feat: x", [A],
      "feat: x\n\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["subject and body", "Fix it\n\nWhy it broke.", [A, B],
      "Fix it\n\nWhy it broke.\n\nCo-authored-by: Ann Lee <ann@example.com>\nCo-authored-by: Bob <Bob@Example.com>"],
    ["an existing block is joined, no blank line", "Fix\n\nBody\n\nReviewed-by: Z <z@z.z>", [A],
      "Fix\n\nBody\n\nReviewed-by: Z <z@z.z>\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["trailing blank lines and spaces go", "Fix  \n\n\n", [A], "Fix\n\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["a trailer-like line inside the subject paragraph is not a block", "Fix\nKey: v", [A],
      "Fix\nKey: v\n\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["a mixed last paragraph is text", "Fix\n\nSee: the docs\nand more", [A],
      "Fix\n\nSee: the docs\nand more\n\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["git's own line makes a quarter enough", "Fix\n\ntext one\ntext two\nSigned-off-by: Z <z@z.z>", [A],
      "Fix\n\ntext one\ntext two\nSigned-off-by: Z <z@z.z>\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["a continuation line belongs to its trailer", "Fix\n\nNote: one\n  two\nAcked-by: Q", [A],
      "Fix\n\nNote: one\n  two\nAcked-by: Q\nCo-authored-by: Ann Lee <ann@example.com>"],
    ["already credited is not repeated (amend)", "Fix\n\nCo-authored-by: Ann Lee <ann@example.com>", [A, B],
      "Fix\n\nCo-authored-by: Ann Lee <ann@example.com>\nCo-authored-by: Bob <Bob@Example.com>"],
  ];
  for (const [what, msg, ps, want] of cases) eq(w(msg, ps), want, `trailers: ${what}`);

  eq(w("Fix\n\nco-authored-by:   A. Lee <ANN@example.com>", [A]), "Fix\n\nco-authored-by:   A. Lee <ANN@example.com>",
    "credited by address: another name or case is still the same person");
  eq(co.trailersToAdd("Fix\n\nCo-authored-by: Ann Lee <ANN@example.com>", [A, B]), ["Co-authored-by: Bob <Bob@Example.com>"],
    "the preview leaves out whom the message already credits");
  eq(co.trailersToAdd("", [A, B, A]).length, 2, "an empty message: everyone chosen, each address once");
  eq(co.trailersToAdd("Fix\nCo-authored-by: Ann Lee <ann@example.com>", [A]).length, 1,
    "a credit inside the subject paragraph is no trailer, so the preview keeps it");
  eq(w("Fix", []), "Fix", "nobody to add: the message is untouched");
  eq(w("Fix  ", [{ name: "Bad <x>", email: "b@x.y" }]), "Fix  ", "an uncreditable person adds nothing");
  eq(w("Fix\r\n\r\nBody\r\n", [A, A]), "Fix\n\nBody\n\nCo-authored-by: Ann Lee <ann@example.com>",
    "CRLF is read as line breaks; the same person twice is added once");
  eq(w("Fix\n\nCo-authored-by: Ann Lee <ann@example.com>\nand some prose", [A]),
    "Fix\n\nCo-authored-by: Ann Lee <ann@example.com>\nand some prose\n\nCo-authored-by: Ann Lee <ann@example.com>",
    "a trailer-looking line outside a block credits no one, as for git");

  // The same messages through git itself: the placement must be the one
  // `interpret-trailers` makes (compared without the final newline git adds).
  // Neutral config: a user's trailer.* settings must not change the answer.
  const { spawnSync } = await import("node:child_process");
  const env = { ...process.env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" };
  for (const [what, msg, ps] of cases) {
    if (/\n\n\n|  \n/.test(msg)) continue; // trailing blanks: git keeps them, `commit -m` does not
    const args = ["interpret-trailers", "--if-exists", "addIfDifferent", ...ps.flatMap((p) => ["--trailer", co.trailerLine(p)])];
    const r = spawnSync("git", args, { input: msg, env, encoding: "utf8" });
    if (r.status !== 0) {
      failed++;
      console.log("FAIL", "git interpret-trailers ran", r.stderr || r.error);
      continue;
    }
    eq(w(msg, ps), r.stdout.replace(/\n+$/, ""), `same as git interpret-trailers: ${what}`);
  }
}

// -- Forge links: which remote, and the URL built from its address -----------
{
  const H = "0123456789abcdef0123456789abcdef01234567";
  const at = (url) => {
    const r = fu.parseRemote(url);
    return r && `${r.forge} ${r.host} ${r.path.join("/")}`;
  };
  eq(
    [
      at("https://github.com/o/r.git"),
      at("https://github.com/o/r"),
      at("https://github.com/o/r/"),
      at("https://github.com/o/r.git/"),
      at("git@github.com:o/r.git"),
      at("github.com:o/r"),
      at("ssh://git@github.com/o/r.git"),
      at("ssh://git@ssh.github.com:443/o/r.git"),
      at("git+ssh://git@github.com/o/r"),
      at("git://github.com/o/r.git"),
      at("  HTTPS://GitHub.com/o/r.git  "),
      at("https://www.github.com/o/r"),
      at("https://gitlab.com/group/sub/proj.git"),
      at("git@gitlab.com:group/proj.git"),
      at("ssh://git@altssh.gitlab.com:443/g/p.git"),
      at("https://bitbucket.org/ws/repo.git"),
      at("git@bitbucket.org:ws/repo.git"),
      at("https://github.com/o/r.git?x=1#top"),
    ],
    [
      "github github.com o/r", "github github.com o/r", "github github.com o/r", "github github.com o/r",
      "github github.com o/r", "github github.com o/r", "github github.com o/r", "github github.com o/r",
      "github github.com o/r", "github github.com o/r", "github github.com o/r", "github github.com o/r",
      "gitlab gitlab.com group/sub/proj", "gitlab gitlab.com group/proj", "gitlab gitlab.com g/p",
      "bitbucket bitbucket.org ws/repo", "bitbucket bitbucket.org ws/repo", "github github.com o/r",
    ],
    "https, ssh, scp and git addresses of the three hosts; .git, slashes, case, query dropped",
  );
  eq(
    [
      at("https://gitlab.example.com/g/p.git"),
      at("git@git.company.io:g/p.git"),
      at("https://github.com.evil.io/o/r"),
      at("https://evilgithub.com/o/r"),
      at("/srv/git/r.git"),
      at("../sibling/r.git"),
      at("file:///srv/github.com/o/r"),
      at("C:\\repos\\r"),
      at("https://github.com/o"),
      at("https://github.com/o/r/tree/main"),
      at("https://bitbucket.org/ws/a/b"),
      at("https://gitlab.com/solo"),
      at("https://github.com/o/../r"),
      at("ftp://github.com/o/r"),
      at(""),
    ],
    Array(15).fill(null),
    "self-hosted and look-alike hosts, local paths, file://, wrong depth, dot-dot: no link",
  );

  const links = (url, email = "a@b.c", name = "Ann Lee") => fu.forgeLinks(url, H, { name, email });
  eq(links("git@github.com:o/r.git"), {
    forge: "github",
    commit: `https://github.com/o/r/commit/${H}`,
    authorCommits: "https://github.com/o/r/commits?author=a%40b.c",
  }, "GitHub: commit and the author's commits");
  eq(links("https://gitlab.com/g/s/p.git"), {
    forge: "gitlab",
    commit: `https://gitlab.com/g/s/p/-/commit/${H}`,
    authorCommits: "https://gitlab.com/g/s/p/-/commits/HEAD?author=Ann%20Lee",
  }, "GitLab: the /-/ routes, nested groups kept, the author by name under a ref");
  eq(links("https://bitbucket.org/ws/repo"), {
    forge: "bitbucket",
    commit: `https://bitbucket.org/ws/repo/commits/${H}`,
    authorCommits: null,
  }, "Bitbucket: the commit; no author filter to link to");
  eq(
    [
      links("https://user:s3cret@github.com/o/r.git").commit,
      links("https://ghp_TOKEN@github.com/o/r.git").commit,
      links("https://***@github.com/o/r.git").commit,
      links("user:pa55@github.com:o/r.git").commit,
      links("ssh://git:pw@github.com:22/o/r.git").commit,
    ],
    Array(5).fill(`https://github.com/o/r/commit/${H}`),
    "credentials, masked or not, never reach the link",
  );
  eq([fu.forgeLinks("git@github.com:o/r", "zz", { name: "A", email: "a@b.c" }).commit,
    links("git@github.com:o/r", "no mail").authorCommits, links("https://gitlab.com/g/p", "a@b.c", "  ").authorCommits],
    [null, null, null], "no link for a hash, an address or a name that is not one");
  eq(links("https://gitlab.com/g/p", "", "Zoë O'Neil & Co").authorCommits,
    "https://gitlab.com/g/p/-/commits/HEAD?author=Zo%C3%AB%20O'Neil%20%26%20Co",
    "GitLab: the name is encoded, and needs no e-mail");
  eq(links("https://github.com/o/r%20x"), null, "a space in the path is no GitHub repository");
  eq(links("https://github.com/o/r", "a+b@c.d").authorCommits, "https://github.com/o/r/commits?author=a%2Bb%40c.d",
    "the address is encoded");

  const R = (name, url) => ({ name, fetchUrls: url ? [url] : [] });
  const remotes = [R("fork", "f"), R("origin", "o"), R("team/main", "t"), R("team", "t0")];
  eq(
    [
      fu.pickRemote(remotes, "team/main/x")?.name,
      fu.pickRemote(remotes, "team/dev")?.name,
      fu.pickRemote(remotes, "gone/main")?.name,
      fu.pickRemote(remotes, null)?.name,
      fu.pickRemote([R("fork", "f"), R("up", "u")], null)?.name,
      fu.pickRemote([R("origin", null)], null),
      fu.pickRemote([], "origin/main"),
    ],
    ["team/main", "team", "origin", "origin", "fork", null, null],
    "the upstream's remote (longest name), else origin, else the first; none without an address",
  );
}

// -- Graph colour by commit age -------------------------------------------------
{
  const now = Date.UTC(2026, 8, 30, 12, 0, 0);
  const ago = (days) => Math.floor(now / 1000 - days * 86400);
  eq(
    [0.01, 0.99, 1, 6.9, 7, 30.9, 31, 364, 365, 3650].map((d) => ag.ageStep(ago(d), now)),
    [0, 0, 1, 1, 2, 2, 3, 3, 4, 4],
    "steps by days: < 1, < 7, < 31, < 365, older — each bound belongs to the next step",
  );
  eq([ag.ageStep(ago(-5), now), ag.ageStep(now / 1000, now)], [0, 0], "a future date or this very second is the freshest");
  eq([ag.ageStep(NaN, now), ag.ageStep(0, now), ag.ageStep(-1, now), ag.ageStep(ago(1), NaN)], [null, null, null, null],
    "no date, the epoch or no clock: no step, no colour");
  eq(ag.AGE_STEPS.map((s) => s.key), ["day", "week", "month", "year", "older"], "five steps, freshest first");
  eq([ag.ageColor(0), ag.ageColor(4)], ["rgb(var(--age-0))", "rgb(var(--age-4))"], "a step is a theme variable, never a literal");
}

await rm(out, { recursive: true, force: true });
console.log(failed === 0 ? `\nall green (${process.env.TZ ?? "local"} time zone)` : `\n${failed} FAILED`);
process.exit(failed === 0 ? 0 : 1);
