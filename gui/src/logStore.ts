import { createSignal } from "solid-js";
import { compilePattern, matchesCommit } from "./components/log/searchPattern";
import {
  emptyLogFilter,
  emptyUiState,
  errText,
  commitsUnreachable,
  logPage,
  uiStateGet,
  uiStateSet,
  type BackendError,
  type LogCommit,
  type LogCursor,
  type LogFilter,
  type LogOrder,
  type LogPage,
  type UiState,
} from "./api";

/**
 * Store of the Log panel: pages and their joining, the cursor, the row cap, the
 * guard against stale answers, the selected commit and the set of selected ones.
 *
 * Four rules this module exists to keep in one place — each one is a bug the
 * project has already paid for (PRD prd_02 §Риски):
 *
 *  1. **Every request carries a monotonic `seq`.** An answer older than the last
 *     one issued is dropped. Without it a fast filter change shows the previous
 *     filter's rows and looks like a semantics bug, which is where the search
 *     for the cause then goes.
 *  2. **Signals are read synchronously, before the first `await`.** Reactivity
 *     does not survive an await boundary, and a store that reads a signal after
 *     one simply stops reacting.
 *  3. **Pages join by hash.** A hash-like search term makes the backend put the
 *     found commit into the first page, so the same commit can legitimately
 *     arrive twice.
 *  4. **The cursor is returned verbatim.** Slot 0 of `openLanes` is a header
 *     naming the history the cursor was cut from; slicing or rebuilding it
 *     turns a detectable "reload me" into a silently wrong graph.
 *
 * History is never read here on its own: nothing runs until {@link ensureLoaded}
 * is called by the Log panel, so opening the app in Changes mode executes no git
 * history command.
 */

/** One page (PRD §Решения, budgets written as numbers). */
export const PAGE_LIMIT = 200;
/** Rows kept in memory; past it loading stops and the panel says so. */
export const ROW_CAP = 20_000;
/** Lanes drawn; the rest collapse into a "+N" marker. */
export const LANE_BUDGET = 12;

const ORDER_KEY = "logOrder";
const DIM_KEY = "logDimNonMatching";

// ── State ────────────────────────────────────────────────────────────────────

const initialFilter = (): LogFilter => {
  const f = emptyLogFilter();
  f.order = localStorage.getItem(ORDER_KEY) === "topo" ? "topo" : "date";
  return f;
};

const [filter, setFilterSignal] = createSignal<LogFilter>(initialFilter());
const [commits, setCommits] = createSignal<LogCommit[]>([]);
const [cursor, setCursor] = createSignal<LogCursor | null>(null);
const [loaded, setLoaded] = createSignal(false);
const [loading, setLoading] = createSignal(false);
const [loadingMore, setLoadingMore] = createSignal(false);
const [laneOverflow, setLaneOverflow] = createSignal(false);
const [logError, setLogError] = createSignal("");
const [noteText, setNote] = createSignal("");
/**
 * A jump between matches is running.
 *
 * A flag of its own, not a value of `note`: the note strip is the channel of
 * the reader's own messages and is cleared by a click on it, so a stray click
 * during a jump used to re-enable the buttons that guard the loop. The two are
 * merged for *reading* — `note()` still answers "searching" while a jump runs,
 * which is the contract the filter bar was written against — but nothing the
 * reader does can lower the flag, and only {@link jumpToMatch}'s `finally`
 * does.
 */
const [searching, setSearching] = createSignal(false);
/**
 * Hashes of loaded commits the current revision cannot reach (R45i, D05).
 *
 * The negative is what travels: everything the log shows belongs to the current
 * branch until git says otherwise, so an answer that has not arrived — or one
 * that failed — subdues nothing instead of subduing every row.
 */
const [offBranch, setOffBranch] = createSignal<Set<string>>(new Set());
/**
 * Rows that stand outside the graph: a commit fetched by hash (D06) and pinned
 * to the top of the list.
 *
 * The order of `commits()` *is* the geometry of the graph — the upper halves of
 * a row's lines are computed from the row above it — so a commit spliced in from
 * a differently filtered page has no place in that geometry: its own edges were
 * computed for another page and are empty here, and drawing the next row's lines
 * against it would be drawing a history that does not exist. Such a row is
 * therefore marked, drawn without a graph, and the row below it starts its lines
 * afresh. The mark is dropped the moment paging reaches the commit for real.
 */
const [offGraph, setOffGraph] = createSignal<Set<string>>(new Set());
/** Non-matching rows drawn muted (the search's third mode, R22i). */
const [dim, setDimSignal] = createSignal(localStorage.getItem(DIM_KEY) === "1");
const [pendingNew, setPendingNew] = createSignal(0);
const [atTop, setAtTop] = createSignal(true);
const [highlight, setHighlightSignal] = createSignal(emptyUiState().logHighlight);
const [columnWidths, setColumnWidthsSignal] = createSignal<Record<string, number>>({});
const [selectedSet, setSelectedSet] = createSignal<Set<string>>(new Set());
const [cursorIndex, setCursorIndex] = createSignal(-1);
const [search, setSearchSignal] = createSignal({ text: "", regex: false, matchCase: false });

export {
  atTop,
  columnWidths,
  commits,
  cursorIndex,
  filter,
  highlight,
  laneOverflow,
  loaded,
  loading,
  loadingMore,
  logError,
  pendingNew,
  search,
  searching,
  selectedSet,
  dim,
  offBranch,
  offGraph,
};

/**
 * The note strip's text. `searching` wins while a jump is in flight so the busy
 * state cannot be dismissed by a click; every other note is the reader's and is
 * dismissable.
 */
export const note = (): string => (searching() ? "searching" : noteText());

/** Turn dimming of non-matching rows on or off (view menu of the log). */
export function setDim(on: boolean): void {
  setDimSignal(on);
  localStorage.setItem(DIM_KEY, on ? "1" : "");
}

/** True once the ordering preference is known: the list must not render before
 * it is, or the rows arrive in one order and are re-sorted under the reader. */
export const orderKnown = () => true;
/** No further page exists behind the loaded ones. */
export const atEnd = () => loaded() && cursor() === null;
/** Loading stopped because the row cap was reached, not because history ended. */
export const capped = () => commits().length >= ROW_CAP && cursor() !== null;
/** Hash of the row the keyboard cursor is on, or null. */
export const selected = (): string | null => commits()[cursorIndex()]?.hash ?? null;

/**
 * Graph suppression is not a field: the backend expresses it as *every* row of
 * the page having no edges **and** lane 0. One row cannot show it — a lone root
 * commit looks the same — so the whole loaded set is asked at once.
 */
export const graphSuppressed = (): boolean => {
  // A pinned row is not part of the page the claim is made about: its edges are
  // empty because it came from another request, and counting it would let one
  // hash search look like "the filter broke the history".
  const rows = commits().filter((c) => !offGraph().has(c.hash));
  if (rows.length === 0) return false;
  return rows.every((c) => c.edges.length === 0 && c.lane === 0);
};

// ── Request guard ────────────────────────────────────────────────────────────

let seq = 0;
/** Request number of the "are there newer commits" probe, guarded separately:
 * it must not invalidate a page in flight, but its own answers still race. */
let probeSeq = 0;
/** How many first-page loads are in flight. Counted rather than compared against
 * `seq`: any other request — a page, a reload of its own — moves `seq` on, and a
 * `finally` that checks `my === seq` then never lowers the flag. The list would
 * say "loading history" for the rest of the session. */
let reloadsInFlight = 0;
/** Anchor of a Shift-range: the last single click, not the moving cursor —
 * otherwise repeated Shift-clicks walk the selection away from its origin. */
let anchorIndex = -1;

const isRule = (e: unknown): boolean =>
  !!e && typeof e === "object" && (e as Partial<BackendError>).kind === "rule";

// ── Loading ──────────────────────────────────────────────────────────────────

/** First load, once per mount of the panel. Does nothing if history is there. */
export function ensureLoaded(): void {
  if (loaded() || loading()) return;
  void reload();
}

/**
 * Drop everything and read the first page again. Called when the open
 * repository changes: pages, cursor, selection and the failure of the previous
 * repository all belong to that repository and none of them survive the switch.
 */
export function resetLog(): void {
  seq++; // answers already in flight belong to the previous repository
  setCommits([]);
  setCursor(null);
  setSelectedSet(new Set<string>());
  setCursorIndex(-1);
  anchorIndex = -1;
  clearOffBranch();
  setOffGraph(new Set<string>());
  setLaneOverflow(false);
  setLoadingMore(false);
  setPendingNew(0);
  setLogError("");
  setNote("");
  setLoaded(false);
  void reload();
}

/**
 * Reload from the first page. `keepSelection` restores the previously selected
 * commit if the reloaded pages still contain it; if they do not, the newest
 * commit is selected and the panel says so instead of clearing silently.
 */
export async function reload(opts: { keepSelection?: boolean } = {}): Promise<void> {
  // read synchronously: everything below this line is past an await
  const f = filter();
  const previous = opts.keepSelection ? selected() : null;
  const my = ++seq;

  reloadsInFlight++;
  setLoading(true);
  clearOffBranch();
  setOffGraph(new Set<string>());
  setLogError("");
  setNote("");
  setPendingNew(0);
  try {
    const page = await logPage(f, null, PAGE_LIMIT);
    if (my !== seq) return;
    applyPage(page, []);
    setLoaded(true);
    if (previous) {
      const i = commits().findIndex((c) => c.hash === previous);
      if (i >= 0) {
        setCursorIndex(i);
        setSelectedSet(new Set([previous]));
      } else {
        selectAt(0, "single");
        setNote("selection-lost");
      }
    } else if (commits().length > 0 && cursorIndex() < 0) {
      selectAt(0, "single");
    }
  } catch (e) {
    if (my !== seq) return;
    setLogError(errText(e));
    setLoaded(true);
  } finally {
    reloadsInFlight--;
    if (reloadsInFlight === 0) setLoading(false);
  }
}

/** Refresh keeps the filter and, when it can, the selection (R23i). */
export const refreshLog = () => reload({ keepSelection: true });

/** Next page, appended below. Idempotent while a page is in flight. */
export async function loadMore(): Promise<void> {
  const c = cursor();
  const f = filter();
  const have = commits();
  if (!c || loading() || loadingMore() || have.length >= ROW_CAP) return;
  const my = ++seq;

  setLoadingMore(true);
  try {
    const page = await logPage(f, c, PAGE_LIMIT);
    if (my !== seq) return;
    applyPage(page, have);
  } catch (e) {
    if (my !== seq) return;
    if (isRule(e)) {
      // The history moved under the cursor (fetch, rebase, a new commit) or the
      // filter changed. The backend refuses rather than drawing edges that are
      // no longer true; reloading from the top is the answer, once — a retry of
      // the same paged call here would loop.
      setLoadingMore(false);
      await reload({ keepSelection: true });
      return;
    }
    setLogError(errText(e));
  } finally {
    // Unconditionally: a page overtaken by a newer request is still a page that
    // stopped loading. Gating this on `my === seq` leaves the flag raised for the
    // rest of the session — no further page is ever asked for, and the foot of
    // the list says "loading more" forever.
    setLoadingMore(false);
  }
}

/** Join a page onto what is loaded. Deduplicates by hash: a hash-like search
 * term puts the found commit into the first page, and it can come back again. */
function applyPage(page: LogPage, before: LogCommit[]): void {
  // A row pinned by a hash search is a stand-in for a commit paging had not
  // reached. Once a page delivers it in its own place, with its own edges, the
  // stand-in goes: keeping it would let dedupe drop the real row and leave the
  // history with a hole where the row below it expected a line from above.
  const pinned = offGraph();
  let kept = before;
  // The keyboard cursor is an index, and dropping a row above it moves every row
  // under it; the hash is what the reader actually stands on.
  const under = selected();
  let dropped = false;
  if (pinned.size > 0) {
    const arriving = new Set(page.commits.map((c) => c.hash).filter((h) => pinned.has(h)));
    if (arriving.size > 0) {
      kept = before.filter((c) => !arriving.has(c.hash));
      dropped = kept.length !== before.length;
      setOffGraph(new Set([...pinned].filter((h) => !arriving.has(h))));
    }
  }
  const seen = new Set(kept.map((c) => c.hash));
  const next = kept.slice();
  for (const c of page.commits) {
    if (seen.has(c.hash)) continue;
    seen.add(c.hash);
    next.push(c);
  }
  setCommits(next.length > ROW_CAP ? next.slice(0, ROW_CAP) : next);
  if (dropped && under) {
    const i = commits().findIndex((c) => c.hash === under);
    if (i >= 0) setCursorIndex(i);
  }
  setCursor(page.nextCursor);
  setLaneOverflow(laneOverflow() || page.laneOverflow);
  void askReachability(page.commits);
}

/**
 * Ask which rows of *one page* are off the current branch, and add them to what
 * is already known.
 *
 * The page, never the whole loaded list: the hashes travel on the command line,
 * and twenty thousand of them are past the argument limit — the request would
 * fail exactly on the histories deep enough to need it. Unioning is sound
 * because the answer for a commit does not change while the log stands: a
 * reload clears the set outright.
 *
 * The guard is an **epoch**, not a per-request number: two pages asked at once
 * are not rivals — their answers are unioned — so a shared counter would let the
 * second page cancel the first, and those rows would stay unmarked for good,
 * nobody asking again. What must be dropped is an answer issued before the set
 * was emptied: it would refill a cleared set with the previous history's hashes.
 * So the epoch moves exactly where the set is cleared, and nowhere else.
 */
let reachEpoch = 0;
/** Empty the set and invalidate every request that is filling it. */
function clearOffBranch(): void {
  reachEpoch++;
  setOffBranch(new Set<string>());
}

async function askReachability(rows: LogCommit[]): Promise<void> {
  if (rows.length === 0) return;
  const my = reachEpoch;
  try {
    const off = await commitsUnreachable(rows.map((c) => c.hash));
    if (my !== reachEpoch) return;
    setOffBranch((prev) => {
      const next = new Set(prev);
      for (const h of off) next.add(h);
      return next;
    });
  } catch {
    // Emphasis is a reading aid: without an answer nothing is subdued, which is
    // the same screen the panel showed before the aid existed.
  }
}

// ── Filter and order ─────────────────────────────────────────────────────────

/** Apply a new filter and reload from the top (task 11 calls this). */
export function applyFilter(f: LogFilter): void {
  setFilterSignal({ ...f });
  setSelectedSet(new Set<string>());
  setCursorIndex(-1);
  anchorIndex = -1;
  void reload();
}

/**
 * Scope the log to a branch picked in the branch tree (R15i). `null` is not
 * "no filter chosen" but "whatever HEAD points at" — the tree's top row and its
 * detached-HEAD node both mean that, and the log then shows the history of the
 * current revision rather than naming a branch that may not exist.
 *
 * `reload` is false only while the panel is setting itself up for a repository:
 * the scope has to be in the filter *before* the first page is asked for, or the
 * first page would be fetched for HEAD and thrown away one tick later. On a
 * change of repository the panel clears the scope outright rather than carrying
 * a branch name across — a name that belonged to the previous repository is not
 * a filter, it is a request for history that does not exist here.
 */
export function setBranchScope(branch: string | null, reload = true): void {
  if ((filter().branch ?? null) === branch) return;
  if (reload) applyFilter({ ...filter(), branch });
  else setFilterSignal({ ...filter(), branch });
}

export function setOrder(order: LogOrder): void {
  if (filter().order === order) return;
  localStorage.setItem(ORDER_KEY, order);
  applyFilter({ ...filter(), order });
}

export function setSearch(s: { text: string; regex: boolean; matchCase: boolean }): void {
  setSearchSignal({ ...s });
}

// ── New commits are offered, not inserted ────────────────────────────────────

/**
 * Called after the repository state changed (fetch, pull, a commit). While the
 * reader is scrolled away from the top, newer commits are counted and offered
 * by a control; inserting them would move the rows under the pointer.
 */
export async function checkNewCommits(): Promise<void> {
  const f = filter();
  const top = commits()[0]?.hash;
  if (!loaded() || !top || loading()) return;
  if (atTop()) {
    await reload({ keepSelection: true });
    return;
  }
  // The probe does not claim `seq` — it must not cancel a page in flight — but it
  // is still an answer that can arrive after the filter or the repository has
  // changed, and "+N new" would then count against a history nobody is looking
  // at. So: discard if any request was issued meanwhile, or if a later probe was.
  const at = seq;
  const myProbe = ++probeSeq;
  try {
    const page = await logPage(f, null, PAGE_LIMIT);
    if (at !== seq || myProbe !== probeSeq) return;
    const i = page.commits.findIndex((c) => c.hash === top);
    setPendingNew(i < 0 ? page.commits.length : i);
  } catch {
    // A probe that fails changes nothing the reader can see; the explicit
    // Refresh button is what reports a real failure.
  }
}

/** The reader asked for the newer commits: reload from the top. */
export function showNewCommits(): void {
  setPendingNew(0);
  void reload({ keepSelection: true });
}

export { setAtTop };
export const clearNote = () => setNote("");

// ── Selection ────────────────────────────────────────────────────────────────

export type SelectMode = "single" | "toggle" | "range";

/**
 * Select by row index in display order. `range` takes every row between the
 * anchor and the index — by position on screen, which is what the reader sees,
 * not by date or hash.
 */
export function selectAt(index: number, mode: SelectMode = "single"): void {
  const rows = commits();
  if (rows.length === 0) return;
  const i = Math.max(0, Math.min(index, rows.length - 1));
  if (mode === "range" && anchorIndex >= 0) {
    const [lo, hi] = anchorIndex <= i ? [anchorIndex, i] : [i, anchorIndex];
    setSelectedSet(new Set(rows.slice(lo, hi + 1).map((c) => c.hash)));
  } else if (mode === "toggle") {
    const next = new Set(selectedSet());
    const h = rows[i].hash;
    if (next.has(h)) next.delete(h);
    else next.add(h);
    setSelectedSet(next);
    anchorIndex = i;
  } else {
    setSelectedSet(new Set([rows[i].hash]));
    anchorIndex = i;
  }
  setCursorIndex(i);
}

/**
 * Select a commit by hash if it is loaded; returns whether it was found.
 * A prefix counts: a hash pasted by a reader is usually the short one, and the
 * rows carry the full form.
 */
export function selectHash(hash: string): boolean {
  const p = hash.toLowerCase();
  const i = commits().findIndex((c) => c.hash.toLowerCase().startsWith(p));
  if (i < 0) return false;
  selectAt(i, "single");
  return true;
}

/** Select every loaded row (Cmd/Ctrl+A inside the panel). */
export function selectAll(): void {
  setSelectedSet(new Set(commits().map((c) => c.hash)));
}

// ── Jumping to search matches ────────────────────────────────────────────────

/**
 * The one search rule, asked here rather than restated: `searchPattern.ts` owns
 * what counts as a match, and the highlighting in the row, the dim predicate
 * and this jump must agree — a second copy is exactly what makes a jump land on
 * a row the highlighting does not consider matched.
 */
const pattern = () => compilePattern(search());
const hits = (subject: string, hash: string) => matchesCommit(pattern(), subject, hash);

/** A full or near-full hash the reader pasted. The backend can fetch such a
 * commit directly, which is the only way to reach one past the row cap. */
const HASH_RE = /^[0-9a-f]{7,40}$/i;

/**
 * Fetch a commit named by hash straight from the backend and put it into the
 * loaded rows (D06).
 *
 * Client-side search cannot see past the loaded pages, and `git` can: a
 * hash-like `LogFilter.text` makes `log_page` splice the commit it names into
 * the first page. The probe is a *separate* request built here — the store's own
 * filter is left alone, so the search toggles still never reach
 * `LogFilter.regex` / `LogFilter.matchCase`, which govern how git matches
 * `--author`.
 *
 * Returns whether the commit was found and selected.
 */
export async function findCommitByHash(text: string): Promise<boolean> {
  const hash = text.trim().toLowerCase();
  if (!HASH_RE.test(hash)) return false;
  if (selectHash(hash)) return true;
  const my = ++seq;
  try {
    const page = await logPage({ ...filter(), text: hash }, null, PAGE_LIMIT);
    if (my !== seq) return false;
    const found = page.commits.find((c) => c.hash.toLowerCase().startsWith(hash));
    if (!found) return false;
    // In front, where the backend itself puts it: the commit is the answer to
    // the question that was asked, not the next row of the history. It is marked
    // as standing outside the graph — the page it came from was computed under
    // another filter, so its lane and edges say nothing about the rows around it.
    setCommits([found, ...commits().filter((c) => c.hash !== found.hash)].slice(0, ROW_CAP));
    setOffGraph(new Set([...offGraph(), found.hash]));
    void askReachability([found]);
    return selectHash(found.hash);
  } catch {
    return false;
  }
}

/**
 * Move to the next (`1`) or previous (`-1`) match of the search text, loading
 * further pages when the match lies beyond what is loaded. Ends by saying what
 * happened: found, no more matches, or stopped at the row cap.
 *
 * The busy flag goes down on **every** exit, including a throw: the filter bar
 * gates its buttons on it, and a flag left up disables them for the rest of the
 * session.
 */
export async function jumpToMatch(dir: 1 | -1): Promise<void> {
  if (pattern().kind !== "ok") return;
  if (searching()) return;
  setNote("");
  setSearching(true);
  try {
    let from = cursorIndex();
    for (;;) {
      const rows = commits();
      if (dir === 1) {
        for (let i = from + 1; i < rows.length; i++) {
          if (hits(rows[i].subject, rows[i].hash)) {
            selectAt(i, "single");
            return;
          }
        }
        from = rows.length - 1;
      } else {
        for (let i = from - 1; i >= 0; i--) {
          if (hits(rows[i].subject, rows[i].hash)) {
            selectAt(i, "single");
            return;
          }
        }
        setNote("no-more-matches");
        return;
      }
      if (capped() || atEnd()) {
        // Nothing more to walk through — but a pasted hash still has the direct
        // route, which is the whole point of D06: the commit is older than the
        // cap, and the backend can name it without loading everything between.
        if (await findCommitByHash(search().text)) return;
        setNote(capped() ? "search-capped" : "no-more-matches");
        return;
      }
      const had = commits().length;
      await loadMore();
      if (commits().length === had) {
        if (await findCommitByHash(search().text)) return;
        setNote(capped() ? "search-capped" : "no-more-matches");
        return;
      }
    }
  } finally {
    setSearching(false);
  }
}

// ── Persisted panel state (.git/graft-ui.json) ───────────────────────────────

/**
 * Read the panel's own slice of the UI state. The file is shared with the
 * branch tree, so every write re-reads it first: a snapshot taken at mount
 * would put back stale favourites written by the other panel meanwhile.
 */
export async function loadUiState(): Promise<void> {
  try {
    const ui = await uiStateGet();
    setColumnWidthsSignal({ ...ui.columnWidths });
    setHighlightSignal(ui.logHighlight);
  } catch {
    // No repository yet, or no state file: defaults are the honest answer.
  }
}

async function patchUiState(patch: (ui: UiState) => UiState): Promise<void> {
  try {
    const ui = await uiStateGet();
    await uiStateSet(patch(ui));
  } catch (e) {
    setLogError(errText(e));
  }
}

/** Column width while the border is being dragged: on screen only. */
export function setColumnWidth(key: string, px: number): void {
  setColumnWidthsSignal({ ...columnWidths(), [key]: px });
}

/** Persist one column's width. Called on drag end, not on every mousemove: the
 * UI state file is shared with the branch tree and rewritten whole. */
export function saveColumnWidth(key: string, px: number): void {
  setColumnWidth(key, px);
  void patchUiState((ui) => ({ ...ui, columnWidths: { ...ui.columnWidths, [key]: px } }));
}

/** Forget a column's width so it falls back to the default (double-click). */
export function resetColumnWidth(key: string): void {
  const next = { ...columnWidths() };
  delete next[key];
  setColumnWidthsSignal(next);
  void patchUiState((ui) => {
    const cw = { ...ui.columnWidths };
    delete cw[key];
    return { ...ui, columnWidths: cw };
  });
}

/** Row emphasis on/off, from the view menu (R45i). */
export function setHighlight(on: boolean): void {
  setHighlightSignal(on);
  void patchUiState((ui) => ({ ...ui, logHighlight: on }));
}
