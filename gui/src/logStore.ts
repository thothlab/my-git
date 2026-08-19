import { createSignal } from "solid-js";
import {
  emptyLogFilter,
  emptyUiState,
  errText,
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
const [note, setNote] = createSignal("");
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
  note,
  pendingNew,
  search,
  selectedSet,
};

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
  const rows = commits();
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
  const seen = new Set(before.map((c) => c.hash));
  const next = before.slice();
  for (const c of page.commits) {
    if (seen.has(c.hash)) continue;
    seen.add(c.hash);
    next.push(c);
  }
  setCommits(next.length > ROW_CAP ? next.slice(0, ROW_CAP) : next);
  setCursor(page.nextCursor);
  setLaneOverflow(laneOverflow() || page.laneOverflow);
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

/** Select a commit by hash if it is loaded; returns whether it was found. */
export function selectHash(hash: string): boolean {
  const i = commits().findIndex((c) => c.hash === hash);
  if (i < 0) return false;
  selectAt(i, "single");
  return true;
}

/** Select every loaded row (Cmd/Ctrl+A inside the panel). */
export function selectAll(): void {
  setSelectedSet(new Set(commits().map((c) => c.hash)));
}

// ── Jumping to search matches ────────────────────────────────────────────────

function matcher(): ((subject: string, hash: string) => boolean) | null {
  const s = search();
  if (!s.text) return null;
  if (s.regex) {
    try {
      const re = new RegExp(s.text, s.matchCase ? "" : "i");
      return (subject, hash) => re.test(subject) || re.test(hash);
    } catch {
      return null; // an invalid pattern is reported by the field, not here
    }
  }
  const needle = s.matchCase ? s.text : s.text.toLowerCase();
  return (subject, hash) =>
    (s.matchCase ? subject : subject.toLowerCase()).includes(needle) ||
    hash.startsWith(needle.toLowerCase());
}

/**
 * Move to the next (`1`) or previous (`-1`) match of the search text, loading
 * further pages when the match lies beyond what is loaded. Ends by saying what
 * happened: found, no more matches, or stopped at the row cap.
 */
export async function jumpToMatch(dir: 1 | -1): Promise<void> {
  const hit = matcher();
  if (!hit) return;
  setNote("");
  let from = cursorIndex();
  for (;;) {
    const rows = commits();
    if (dir === 1) {
      for (let i = from + 1; i < rows.length; i++) {
        if (hit(rows[i].subject, rows[i].hash)) {
          selectAt(i, "single");
          return;
        }
      }
      from = rows.length - 1;
    } else {
      for (let i = from - 1; i >= 0; i--) {
        if (hit(rows[i].subject, rows[i].hash)) {
          selectAt(i, "single");
          return;
        }
      }
      setNote("no-more-matches");
      return;
    }
    if (capped()) {
      setNote("search-capped");
      return;
    }
    if (atEnd()) {
      setNote("no-more-matches");
      return;
    }
    setNote("searching");
    const had = commits().length;
    await loadMore();
    if (commits().length === had) {
      setNote(capped() ? "search-capped" : "no-more-matches");
      return;
    }
    setNote("");
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
