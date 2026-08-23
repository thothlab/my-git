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
 *     that had already drifted apart.
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
  entryPoints: [join(src, "searchPattern.ts"), join(src, "filterValues.ts")],
  outdir: out,
  format: "esm",
  logLevel: "warning",
});

await build({
  entryPoints: [join(here, "..", "src", "components", "pathTree.ts")],
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
} = await load("editRules.js");

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
const hunk = (header, patch) => ({ header, patch });
const payload = (over = {}) => ({
  path: "a.txt",
  binary: false,
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

await rm(out, { recursive: true, force: true });
console.log(failed === 0 ? `\nall green (${process.env.TZ ?? "local"} time zone)` : `\n${failed} FAILED`);
process.exit(failed === 0 ? 0 : 1);
