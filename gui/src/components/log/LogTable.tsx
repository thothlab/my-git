import { createVirtualizer } from "@tanstack/solid-virtual";
import { For, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import { commitContains, type LogCommit, type RefLabel } from "../../api";
import { d } from "../../i18n";
import {
  atEnd,
  capped,
  checkNewCommits,
  clearNote,
  columnWidths,
  commits,
  cursorIndex,
  LANE_BUDGET,
  filter,
  graphSuppressed,
  highlight,
  loadMore,
  loadUiState,
  loaded,
  loading,
  loadingMore,
  logError,
  note,
  orderKnown,
  pendingNew,
  refreshLog,
  resetColumnWidth,
  resetLog,
  saveColumnWidth,
  setBranchScope,
  setColumnWidth,
  selectAll,
  selectAt,
  selectedSet,
  setAtTop,
  setOrder,
  showNewCommits,
} from "../../logStore";
import { state } from "../../store";
import { selectedBranch } from "./branchSelection";
import { clearCompare } from "./actions/compareSelection";
import { commitMenuItems } from "./actions/commitActions";
import ContextMenu, { createMenuController, type MenuAnchor } from "./actions/ContextMenu";
import { ActionDialogHost } from "./actions/dialogs";
import OperationBar from "./actions/OperationBar";
import LogGraph, { LANE_W, lanesBelow } from "./LogGraph";
import { PanelBtn, PanelChrome, PanelNote } from "./PanelChrome";

/**
 * Commit list: graph column, subject with reference labels, author, relative
 * date. Virtualised, paged and multi-selectable.
 *
 * Three rules here are not style but paid-for bugs (PRD prd_02 §Риски):
 *
 *  1. **One Virtualizer per component, never inside a memo.** Re-creating it
 *     resets the scroll position; `count` is a getter so the read stays tracked
 *     without wrapping the instance in reactivity.
 *  2. **A row reads its commit through `createMemo(() => commits()[vi.index])`.**
 *     Capturing the value at render time freezes rows: the list updates and the
 *     screen keeps the old text. This project hit it twice.
 *  3. **The row height is a constant and is applied to the row element**, not
 *     merely estimated. The download anchor and the graph geometry both depend
 *     on it being exactly this many pixels.
 */

/** Row height in px (PRD §Решения). Constant, in the element and in the graph. */
const ROW_H = 22;
const DEFAULT_AUTHOR_W = 150;
const DEFAULT_DATE_W = 120;
const MIN_COL_W = 60;
/** Rows from the end at which the next page is asked for. */
const PREFETCH_ROWS = 40;
/** Room kept at the right of the graph column for the overflow slot — the shared
 * lane of everything past the budget — and for the "+N" marker beside it. */
const GRAPH_GUTTER = 28;
/** Reference labels shown before the rest collapse into "+N". */
const REFS_SHOWN = 3;

export default function LogTable(props: { onSelect?: (hash: string | null) => void }) {
  // The scroll element is a signal, not a plain ref: the virtualizer is created
  // during setup, before the element exists, and has to be told when it appears.
  const [scrollEl, setScrollEl] = createSignal<HTMLDivElement | null>(null);
  const [menuOpen, setMenuOpen] = createSignal(false);

  // Context menu. Its targets are resolved once, when it opens: a right-click
  // inside the selection acts on the whole selection (and the items say how
  // many), a right-click outside it acts on that row alone and leaves the
  // selection untouched (PRD История 51).
  const menu = createMenuController();
  const [menuTargets, setMenuTargets] = createSignal<LogCommit[]>([]);
  const [menuContains, setMenuContains] = createSignal<boolean | null>(null);

  /**
   * The element of a row, for the keyboard path of the menu. Asked of the DOM
   * rather than kept in a map: rows are virtualised, so a map keyed on the row
   * index outlives the rows it describes — twenty thousand detached nodes held
   * for the session, and an anchor rectangle of all zeros for the ones already
   * recycled.
   */
  const rowElement = (index: number): HTMLElement | null =>
    scrollEl()?.querySelector<HTMLElement>(`[data-row="${index}"]`) ?? null;

  const openMenuAt = (index: number, at: MenuAnchor) => {
    const rows = commits();
    const row = rows[index];
    if (!row) return;
    const sel = selectedSet();
    const targets = sel.has(row.hash) ? rows.filter((c) => sel.has(c.hash)) : [row];
    setMenuTargets(targets);
    setMenuContains(null);
    if (targets.length === 1) {
      const hash = targets[0].hash;
      // Asked before the item is offered, so cherry-pick is disabled with its
      // reason rather than failing on the click. A failed question is answered
      // "not contained": git then refuses the pick itself, verbatim, which is
      // better than an item stuck on "checking…" forever.
      void commitContains(hash)
        .then((v) => menuTargets()[0]?.hash === hash && setMenuContains(v))
        .catch(() => setMenuContains(false));
    }
    menu.open(at);
  };

  // History is asked for only once the repository is actually open: `state()` is
  // null until `repo_open` has answered, and a `log_page` sent before that comes
  // back with "repository not open" — an error about our own timing, shown to
  // the reader as if the repository were broken.
  //
  // The same effect distinguishes the two reasons the repository state changes.
  // A different repository invalidates pages, cursor, selection and the panel's
  // persisted widths, so everything is dropped and read again. The same
  // repository having merely advanced (fetch, pull, a commit) must not move the
  // rows under a reader who has scrolled away — that is offered, not applied.
  let lastRepo: string | null = null;
  createEffect(() => {
    const s = state();
    if (!s) return;
    if (s.repoPath !== lastRepo) {
      lastRepo = s.repoPath;
      void loadUiState();
      // A new repository starts unscoped. Carrying the previous repository's
      // branch over would ask for the history of a branch that need not exist
      // here; the tree drops its own selection on the same event, so the two do
      // not fight over what "selected" means.
      setBranchScope(null, false);
      resetLog();
      return;
    }
    void checkNewCommits();
  });

  // R15i: picking a branch in the tree scopes the log to it, and picking the
  // top row scopes it back to HEAD. The tree only writes the choice down; this
  // is the read, and it restarts the log from its first page — a filter change
  // invalidates the cursor, the pages and the lane state behind them.
  createEffect(on(selectedBranch, (b) => setBranchScope(b), { defer: true }));

  const authorW = () => columnWidths().author ?? DEFAULT_AUTHOR_W;
  const dateW = () => columnWidths().date ?? DEFAULT_DATE_W;

  // The graph column is a constant width: room for the whole lane budget, plus a
  // gutter for the overflow slot and its marker. Deriving it from the data — even
  // once, from the first page — was worse than the shift it prevented: a history
  // whose tip is linear froze the column at one lane and every branch and merge
  // below collapsed into the overflow slot. A column that never moves and always
  // has room is the only version that keeps both promises.
  const graphW = () => LANE_BUDGET * LANE_W + GRAPH_GUTTER;

  const grid = () => `${graphW()}px minmax(0,1fr) ${authorW()}px ${dateW()}px`;

  const rowCount = () => commits().length;
  // Scrolling a row into view is the list's business; the panel only asks. The
  // function arrives once the virtualised list has mounted.
  const [scrollToRow, setScrollToRow] = createSignal<((i: number) => void) | null>(null);

  createEffect(() => props.onSelect?.(commits()[cursorIndex()]?.hash ?? null));

  const move = (delta: number, mode: "single" | "range" = "single") => {
    if (rowCount() === 0) return;
    const i = Math.max(0, Math.min(cursorIndex() + delta, rowCount() - 1));
    selectAt(i, mode);
    scrollToRow()?.(i);
  };

  const onRowClick = (index: number, e: MouseEvent) => {
    selectAt(index, e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "single");
    // A deliberate new selection leaves comparison mode. Tied to the click, not
    // to the cursor: a reload moves the cursor on its own and would throw away
    // a comparison the reader had just asked for.
    clearCompare();
  };

  const noteText = () => {
    switch (note()) {
      case "selection-lost":
        return d().selectionLostNote();
      case "searching":
        return d().searchingNote();
      case "no-more-matches":
        return d().noMoreMatches();
      case "search-capped":
        return d().searchCappedNote();
      default:
        return "";
    }
  };

  const filterActive = () => {
    const f = filter();
    return !!(f.text || f.author || f.branch || f.since || f.until || f.paths.length);
  };

  return (
    <PanelChrome
      id="commits"
      title={d().logTitle()}
      handlers={{
        moveSelection: (delta) => move(delta),
        moveToEdge: (edge) => move(edge === -1 ? -rowCount() : rowCount()),
        contextMenu: () => {
          const i = cursorIndex();
          if (i < 0) return;
          // Rows are virtualised: an element scrolled out of the list is still
          // in the map but no longer in the document, and its rectangle is all
          // zeros — which would pin the menu to the window corner.
          const r = rowElement(i)?.getBoundingClientRect();
          openMenuAt(i, r ? { x: Math.round(r.left + 40), y: Math.round(r.bottom) } : { x: 160, y: 160 });
        },
        onKey: (e) => {
          if ((e.metaKey || e.ctrlKey) && e.code === "KeyA") {
            selectAll();
            return true;
          }
          if (e.shiftKey && (e.code === "ArrowDown" || e.code === "ArrowUp")) {
            move(e.code === "ArrowDown" ? 1 : -1, "range");
            return true;
          }
          return false;
        },
      }}
      toolbar={
        <>
          <Show when={selectedSet().size > 1}>
            <span class="mr-1 text-xs text-fg-muted">{d().selectedCommits(selectedSet().size)}</span>
          </Show>
          <Show when={pendingNew() > 0}>
            <button
              class="rounded border border-accent px-1.5 py-0.5 text-xs text-accent hover:bg-accent/10"
              title={d().newCommitsTip()}
              onClick={showNewCommits}
            >
              {d().newCommitsBtn(pendingNew())}
            </button>
          </Show>
          <PanelBtn label="⟳" tip={d().refreshTip()} onClick={() => void refreshLog()} />
          <PanelBtn
            label={filter().order === "date" ? "⇅" : "⑂"}
            tip={`${d().orderTip()}: ${filter().order === "date" ? d().orderDate() : d().orderTopo()}`}
            onClick={() => setOrder(filter().order === "date" ? "topo" : "date")}
          />
          <div class="relative">
            <PanelBtn label="⋯" tip={d().viewOptionsTip()} onClick={() => setMenuOpen(!menuOpen())} />
            <Show when={menuOpen()}>
              <div
                class="absolute right-0 top-6 z-20 w-64 rounded border border-border bg-bg p-1 shadow-lg"
                onMouseLeave={() => setMenuOpen(false)}
              >
                <div class="px-2 py-1 text-[11px] uppercase text-fg-muted">
                  {d().highlightHeader()}
                </div>
                {/* The panel's convention (PanelChrome): an action that does not
                    exist yet is disabled and its tooltip says why. A live switch
                    over an empty predicate is worse than a greyed one — it
                    persists a setting that cannot change anything on screen. */}
                <label
                  class="flex cursor-not-allowed items-center gap-2 rounded px-2 py-1 text-xs opacity-40"
                  title={d().highlightPending()}
                >
                  <input type="checkbox" checked={highlight()} disabled />
                  {d().highlightMine()}
                </label>
              </div>
            </Show>
          </div>
        </>
      }
    >
      {/* The list must not render before the ordering is known, or the rows
          arrive in one order and are re-sorted under the reader. */}
      <Show when={orderKnown()}>
        <div class="flex h-full min-h-0 flex-col">
          {/* Mounted here, not in the layout: the layout file belongs to another
              task, and an operation strip that is not mounted would leave a
              conflicted repository with no way out of the panel. Do not mount a
              second one. */}
          <OperationBar />
          <ActionDialogHost />
          <Show when={menu.anchor()}>
            {(a) => (
              <ContextMenu
                anchor={a()}
                items={() => commitMenuItems(menuTargets(), menuContains())}
                onClose={menu.close}
              />
            )}
          </Show>
          <div
            class="grid shrink-0 select-none items-center border-b border-border bg-bg-subtle text-[11px] text-fg-muted"
            style={{ "grid-template-columns": grid(), height: "20px" }}
          >
            <div />
            <div class="truncate px-2">{d().colSubject()}</div>
            <ColHeader
              label={d().colAuthor()}
              width={authorW}
              onResize={(w) => setColumnWidth("author", w)}
              onCommit={(w) => saveColumnWidth("author", w)}
              onReset={() => resetColumnWidth("author")}
            />
            <ColHeader
              label={d().colDate()}
              width={dateW}
              onResize={(w) => setColumnWidth("date", w)}
              onCommit={(w) => saveColumnWidth("date", w)}
              onReset={() => resetColumnWidth("date")}
            />
          </div>

          {/* A reload over an existing list keeps the rows on screen, so the fact
              that it is happening has to be said somewhere. */}
          <Show when={noteText() || ((logError() || loading()) && rowCount() > 0)}>
            <div
              class="flex shrink-0 items-center gap-2 border-b border-border bg-bg-muted px-2 py-1 text-xs text-fg-subtle"
              onClick={clearNote}
            >
              {noteText() || logError() || d().loadingHistory()}
            </div>
          </Show>

          {/* The scroll element is mounted from the first render and never taken
              away — not while the first page is in flight, not while a reload
              replaces the rows — so the scroll position is never yanked from
              under the reader. Empty, loading and error states are drawn inside
              it rather than in its place. How the virtualizer gets hold of it is
              explained on VirtualRows. */}
          <div
            ref={setScrollEl}
            class="min-h-0 flex-1 overflow-auto"
            onScroll={(e) => setAtTop(e.currentTarget.scrollTop < 4)}
          >
            <Show when={loaded() && scrollEl()} fallback={<PanelNote title={d().loadingHistory()} />}>
              <Show
                when={rowCount() > 0}
                fallback={
                  <PanelNote
                    title={
                      logError() ||
                      (filterActive() ? d().logNoMatches() : d().noCommitsTitle())
                    }
                    hint={logError() || filterActive() ? undefined : d().noCommitsHint()}
                  />
                }
              >
                <VirtualRows
                  scrollEl={scrollEl()!}
                  grid={grid()}
                  graphW={graphW()}
                  capacity={LANE_BUDGET}
                  suppressed={graphSuppressed()}
                  onRowClick={onRowClick}
                  onRowMenu={(index, e) => openMenuAt(index, { x: e.clientX, y: e.clientY })}
                  register={(fn) => setScrollToRow(() => fn)}
                />
                <ListEnd />
              </Show>
            </Show>
          </div>
        </div>
      </Show>
    </PanelChrome>
  );
}

/**
 * The virtualised rows.
 *
 * This is a component of its own for one reason: the virtualizer binds to its
 * scroll container while the adapter's own effect runs, and that is too early
 * for a container this component's parent has not created yet. A binding made
 * to nothing is never retried, and the symptom is a list with a correct total
 * height, a working scrollbar and no rows at all. Taking the container as a
 * prop means it exists before the virtualizer does, which is the arrangement
 * the library documents.
 *
 * The instance is still exactly one per mounted list — the container never
 * changes identity while the panel is up — and `count` stays a getter so the
 * read is tracked without a memo around the virtualizer.
 */
function VirtualRows(props: {
  scrollEl: HTMLDivElement;
  grid: string;
  graphW: number;
  capacity: number;
  suppressed: boolean;
  onRowClick: (index: number, e: MouseEvent) => void;
  onRowMenu: (index: number, e: MouseEvent) => void;
  register: (scrollToRow: (i: number) => void) => void;
}) {
  const rowCount = () => commits().length;
  const virt = createVirtualizer({
    get count() {
      return rowCount();
    },
    getScrollElement: () => props.scrollEl,
    estimateSize: () => ROW_H,
    overscan: 16,
  });

  props.register((i) => virt.scrollToIndex(i, { align: "auto" }));

  // Ask for the next page as the end comes into view. Reading the virtual items
  // keeps this tied to actual scrolling rather than to a timer.
  createEffect(() => {
    const items = virt.getVirtualItems();
    const last = items.length ? items[items.length - 1].index : 0;
    if (rowCount() > 0 && last >= rowCount() - PREFETCH_ROWS) void loadMore();
  });

  return (
    <div class="relative w-full" style={{ height: `${virt.getTotalSize()}px` }}>
      <For each={virt.getVirtualItems()}>
        {(vi) => {
          // Read through a memo by index: capturing the value here is what
          // freezes rows when the list updates.
          const row = createMemo(() => commits()[vi.index]);
          const above = createMemo(() => {
            const prev = commits()[vi.index - 1];
            return prev ? lanesBelow(prev) : [];
          });
          return (
            <Show when={row()}>
              <Row
                commit={row()!}
                openAbove={above()}
                top={vi.start}
                grid={props.grid}
                graphW={props.graphW}
                capacity={props.capacity}
                suppressed={props.suppressed}
                selected={selectedSet().has(row()!.hash)}
                current={cursorIndex() === vi.index}
                onClick={(e) => props.onRowClick(vi.index, e)}
                onMenu={(e) => props.onRowMenu(vi.index, e)}
                index={vi.index}
              />
            </Show>
          );
        }}
      </For>
    </div>
  );
}

/** The three terminal states of paging read differently and are not merged. */
function ListEnd() {
  return (
    <div class="px-2 py-1 text-center text-[11px] text-fg-muted">
      <Show when={loadingMore()}>{d().loadingMore()}</Show>
      <Show when={!loadingMore() && capped()}>{d().logCapReached()}</Show>
      <Show when={!loadingMore() && !capped() && atEnd()}>{d().logEnd()}</Show>
    </div>
  );
}

function Row(props: {
  commit: LogCommit;
  openAbove: number[];
  top: number;
  grid: string;
  graphW: number;
  capacity: number;
  suppressed: boolean;
  selected: boolean;
  current: boolean;
  onClick: (e: MouseEvent) => void;
  onMenu: (e: MouseEvent) => void;
  index: number;
}) {
  return (
    <div
      data-row={props.index}
      class="absolute left-0 grid w-full cursor-default select-none items-center text-xs hover:bg-bg-muted/60"
      classList={{
        "bg-accent/25": props.selected,
        "ring-1 ring-inset ring-accent": props.current,
        // Emphasis (R45i) is wired to the toggle but has no inputs yet, see below.
        "text-fg-muted": emphasis(props.commit) === false && highlight(),
      }}
      style={{
        top: `${props.top}px`,
        height: `${ROW_H}px`,
        "grid-template-columns": props.grid,
      }}
      onClick={props.onClick}
      onContextMenu={(e) => {
        e.preventDefault();
        props.onMenu(e);
      }}
    >
      <div class="h-full" title={props.suppressed ? d().graphSuppressedTip() : undefined}>
        <LogGraph
          commit={props.commit}
          openAbove={props.openAbove}
          height={ROW_H}
          width={props.graphW}
          capacity={props.capacity}
        />
      </div>
      <div class="flex min-w-0 items-center gap-1 px-1">
        <Refs refs={props.commit.refs} />
        <span class="truncate">{props.commit.subject}</span>
      </div>
      <div class="truncate px-2 text-fg-subtle">{props.commit.author}</div>
      <div class="truncate px-2 text-fg-subtle" title={absolute(props.commit.authorAt)}>
        {relative(props.commit.authorAt)}
      </div>
    </div>
  );
}

/**
 * Reference labels of one row: the current branch head, other local branches,
 * remote branches and tags each read differently. Past three, the rest collapse
 * into a counted marker whose tooltip lists them.
 */
function Refs(props: { refs: RefLabel[] }) {
  const shown = () => props.refs.slice(0, REFS_SHOWN);
  const rest = () => props.refs.slice(REFS_SHOWN);
  const cls = (r: RefLabel) =>
    r.kind === "head"
      ? "border-accent text-accent"
      : r.kind === "tag"
        ? "border-warn text-warn"
        : r.kind === "remote"
          ? "border-success text-success"
          : "border-border text-fg-subtle";
  return (
    <>
      <For each={shown()}>
        {(r) => (
          <span
            class={`shrink-0 rounded border px-1 text-[10px] leading-4 ${cls(r)}`}
            title={r.name}
          >
            {r.name}
          </span>
        )}
      </For>
      <Show when={rest().length > 0}>
        <span
          class="shrink-0 rounded border border-border px-1 text-[10px] leading-4 text-fg-muted"
          title={rest().map((r) => r.name).join("\n")}
        >
          {d().refsMore(rest().length)}
        </span>
      </Show>
    </>
  );
}

/** Resizable column header: drag the trailing border, double-click resets it. */
function ColHeader(props: {
  label: string;
  width: () => number;
  onResize: (w: number) => void;
  onCommit: (w: number) => void;
  onReset: () => void;
}) {
  const onPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = props.width();
    const at = (m: PointerEvent) => Math.max(MIN_COL_W, startW - (m.clientX - startX));
    const move = (m: PointerEvent) => props.onResize(at(m));
    const up = (m: PointerEvent) => {
      props.onCommit(at(m));
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div class="relative h-full min-w-0">
      <div
        class="absolute left-0 top-0 h-full w-1 cursor-col-resize"
        title={props.label + " — " + d().colResizeTip()}
        onPointerDown={onPointerDown}
        onDblClick={props.onReset}
      />
      <div class="truncate px-2 leading-5">{props.label}</div>
    </div>
  );
}

/**
 * R45i wants commits of the current branch and commits authored by the
 * repository's configured user emphasised. Neither input exists yet: RepoState
 * carries no `user.email` and LogCommit no reachability from HEAD, and both
 * live in `model.rs` / `api.ts`, outside this task's zone. `null` means "not
 * known", so nothing is subdued on a guess; the toggle and its persistence are
 * in place for when the field arrives.
 */
// TODO(prd): needs RepoState.userEmail and a per-commit "reachable from HEAD".
const emphasis = (_c: LogCommit): boolean | null => null;

const two = (n: number) => String(n).padStart(2, "0");
const timeOf = (dt: Date) => `${two(dt.getHours())}:${two(dt.getMinutes())}`;

/** "today 13:02" / "yesterday 09:40" / "11.08.2026" — the full date is the tooltip. */
function relative(unixSeconds: number): string {
  const dt = new Date(unixSeconds * 1000);
  const now = new Date();
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diffDays = Math.round((day(now) - day(dt)) / 86_400_000);
  if (diffDays === 0) return d().todayAt(timeOf(dt));
  if (diffDays === 1) return d().yesterdayAt(timeOf(dt));
  return `${two(dt.getDate())}.${two(dt.getMonth() + 1)}.${dt.getFullYear()}`;
}

const absolute = (unixSeconds: number) => new Date(unixSeconds * 1000).toLocaleString();
