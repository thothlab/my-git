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

const load = (name) => import(pathToFileURL(join(out, name)).href);
const { compilePattern, spansIn, matchesCommit } = await load("searchPattern.js");
const { asInputDate, dayStart, dayEnd, startOfToday, relativeToRepo, toSlash } =
  await load("filterValues.js");
const { baseName, buildFileTree, countFiles, treeDirPaths } = await load("pathTree.js");

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

await rm(out, { recursive: true, force: true });
console.log(failed === 0 ? `\nall green (${process.env.TZ ?? "local"} time zone)` : `\n${failed} FAILED`);
process.exit(failed === 0 ? 0 : 1);
