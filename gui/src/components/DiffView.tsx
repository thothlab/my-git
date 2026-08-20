import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  createEffect,
  onCleanup,
  untrack,
} from "solid-js";
import {
  commitFileDiff,
  commitsCompareDiff,
  diffFile,
  hunkRevert,
  hunkStage,
  hunkUnstage,
  type DiffBase,
  type DiffLine,
  type FileDiff,
  type Hunk,
  type RepoState,
  type WhitespaceMode,
} from "../api";
import { confirmAction, run, selectedPath } from "../store";
import { d } from "../i18n";
import { beginDrag } from "./Resizer";
import { registerHotkey } from "../hotkeys";
import {
  BIG_DIFF_LINES,
  FOLD_CONTEXT,
  buildView,
  pairChanged,
  sideLabels,
  wordSegments,
  type DiffSource,
  type HighlightMode,
  type Item,
  type Row,
  type SideLabel,
} from "./diff/model";
import { DISABLED_CLASS } from "./IconButton";

export type { DiffSource, HighlightMode } from "./diff/model";

/** What a hosting panel can drive from its keyboard handlers. */
export interface DiffApi {
  count: () => number;
  next: () => void;
  prev: () => void;
  scrollLines: (delta: number) => void;
  scrollToEdge: (edge: -1 | 1) => void;
}

const SPLIT_RATIO_KEY = "diffSplitRatio";
const WS_KEY = "diffWhitespace";
const HL_KEY = "diffHighlight";
const LINE_PX = 16;
/**
 * The widest gap a click may open, and the ceiling on the context that follows
 * from it.
 *
 * Both are the same number for a reason: the context asked for is the gap plus
 * the fold's own margin, so capping the gap caps the request. A gap wider than
 * this is not refused because the lines are uninteresting but because `-U`
 * widens every hunk of the file at once — the reader would be asking for the
 * whole file through a panel that summarises far smaller ones (BIG_DIFF_LINES).
 */
const MAX_EXPAND_GAP = 1000;
const MAX_CONTEXT = MAX_EXPAND_GAP + FOLD_CONTEXT;
/**
 * Ceiling on the payload that comes *back*, as opposed to the one asked for.
 *
 * `MAX_EXPAND_GAP` bounds the request, and that is not the same thing: `-U`
 * widens every hunk of the file at once, so a file with twenty changes and
 * nine-hundred-line gaps answers a single click with tens of thousands of
 * lines — several times over the number at which the panel refuses to render a
 * file at all. The "big diff" summary cannot catch it, because it is judged by
 * the first, un-widened answer on purpose (asking to see more must not show
 * less). So the widened answer is measured on arrival.
 *
 * The floor is what is already drawn: a payload no larger than the one on
 * screen is never refused, or a reader who chose "show it whole" on a file
 * above the threshold could not open a single gap in it.
 */
const expandedLinesCeiling = (drawn: number) => Math.max(BIG_DIFF_LINES, drawn);
/**
 * Diff lines in a payload, counted without building its view — the same number
 * `buildView` reports as `lines`, which is the budget the "big diff" summary is
 * judged by. A payload refused for its size must not be walked row by row first.
 */
const payloadLines = (f: FileDiff) => f.hunks.reduce((n, h) => n + h.lines.length, 0);
const clampRatio = (r: number) => Math.min(0.8, Math.max(0.2, r));

const readWs = (): WhitespaceMode => {
  const v = localStorage.getItem(WS_KEY);
  return v === "trailing" || v === "all" ? v : "none";
};
const readHl = (): HighlightMode => {
  const v = localStorage.getItem(HL_KEY);
  return v === "lines" || v === "none" ? v : "words";
};

/**
 * The one diff panel of the application: it serves the Changes mode (working
 * tree / index, with hunk staging) and the Log mode (a file inside a commit or
 * a comparison of two revisions). The source arrives as a prop; with no prop
 * the panel falls back to the selected working-tree file, which is what the
 * Changes mode has always shown.
 *
 * `api` hands the navigation out to a hosting panel (see `diff/DiffPanel`), so
 * the keyboard layer stays the single owner of key handling.
 */
export default function DiffView(props: { source?: DiffSource | null; api?: (a: DiffApi) => void }) {
  // Whether the host drives the source is fixed by the call site, not by the
  // value: a host that passes `null` means "nothing selected", not "fall back".
  const hosted = "source" in props;

  const [base, setBase] = createSignal<DiffBase>("worktree");
  const [split, setSplit] = createSignal(true);
  const [ws, setWsSig] = createSignal<WhitespaceMode>(readWs());
  const [hl, setHlSig] = createSignal<HighlightMode>(readHl());
  const setWs = (m: WhitespaceMode) => {
    setWsSig(m);
    localStorage.setItem(WS_KEY, m);
  };
  const setHl = (m: HighlightMode) => {
    setHlSig(m);
    localStorage.setItem(HL_KEY, m);
  };

  // Function, not a module constant, so labels track the active locale.
  const bases = (): { id: DiffBase; label: string }[] => [
    { id: "worktree", label: d().unstaged() },
    { id: "index", label: d().staged() },
    { id: "head", label: d().vsHead() },
  ];

  const source = (): DiffSource | null => {
    if (hosted) return props.source ?? null;
    const p = selectedPath();
    return p ? { kind: "worktree", path: p, base: base() } : null;
  };

  /**
   * Unchanged lines asked for around each change (R46i, D04).
   *
   * `undefined` is the historical request — no `-U` at all. A gap *between*
   * hunks holds lines git never put in the patch, so revealing them is not a
   * fold to open but the same file asked for again with enough context to cover
   * the widest gap the reader has opened. Per file: it is reset with the source
   * below, like every other per-file reading state.
   */
  const [context, setContext] = createSignal<number | undefined>(undefined);
  /**
   * The gaps the reader asked to see, as line ranges rather than gap indexes.
   *
   * A wider payload re-cuts the file: two hunks that were separated by the gap
   * arrive merged, and the index the reader clicked names a different gap — or
   * none. Line numbers survive that; they are also what tells one gap from the
   * rest, which is the whole point: opening a gap must open *that* gap and not
   * every folded region in the file.
   */
  const [revealed, setRevealed] = createSignal<GapRange[]>([]);
  const expandGap = (index: number) => {
    const v = view();
    if (!v || index <= 0) return;
    const gap = v.gaps[index];
    if (gap <= 0) return;
    // First of two ceilings, and the cheap one: a gap this wide is the file
    // itself and there is no point asking git for it. The size that actually
    // arrives is checked separately, on arrival — see `expandedLinesCeiling`.
    if (gap > MAX_EXPAND_GAP) {
      setNote(d().gapTooLarge(gap, MAX_EXPAND_GAP));
      return;
    }
    setNote("");
    setRevealed((r) => [...r, gapRange(v.hunks[index - 1].hunk, v.hunks[index].hunk)]);
    setContext((c) => Math.min(MAX_CONTEXT, Math.max(c ?? 3, gap + FOLD_CONTEXT)));
  };
  /** The reader asked for more context than the file was first shown with. */
  const widened = () => context() !== undefined;

  // Every signal the request depends on is read here, synchronously, before any
  // await: past the first await the tracking scope is gone.
  const request = () => {
    const s = source();
    return s ? { src: s, ws: ws(), context: context() } : null;
  };

  // Guard against a stale answer (R32i.3): a slower request for the file the
  // user has already left must never overwrite the newer one.
  let seq = 0;
  const [diff, { refetch }] = createResource(request, async (r) => {
    const mine = ++seq;
    const got = await fetchDiff(r.src, r.ws, r.context);
    // A newer request has started meanwhile: this answer belongs to a file the
    // user has already left, and publishing it would show a foreign diff. The
    // superseded fetch simply never settles.
    if (mine !== seq) return new Promise<FileDiff>(() => {});
    return got;
  });

  const act = async (fn: () => Promise<RepoState>) => {
    await run(fn());
    refetch();
  };

  // Split ratio: fraction of width given to the "old" (left) side, shared across
  // every side-by-side row and driven by one draggable overlay handle.
  const [ratio, setRatio] = createSignal(
    clampRatio(Number(localStorage.getItem(SPLIT_RATIO_KEY)) || 0.5),
  );
  const persistRatio = () => localStorage.setItem(SPLIT_RATIO_KEY, String(ratio()));

  // Safety net: the split handle lives inside <Show>, so a refresh landing
  // mid-drag can tear it down while it holds the cursor override.
  onCleanup(() => {
    document.body.style.cursor = "";
  });

  let wrapEl: HTMLDivElement | undefined;
  let scrollEl: HTMLDivElement | undefined;
  const onSplitDown = (e: PointerEvent) => {
    e.preventDefault();
    beginDrag(
      e.currentTarget as HTMLElement,
      e.pointerId,
      (ev) => {
        const rect = wrapEl?.getBoundingClientRect();
        if (!rect || rect.width === 0) return;
        setRatio(clampRatio((ev.clientX - rect.left) / rect.width));
      },
      persistRatio,
    );
  };

  /**
   * The payload the panel has agreed to draw, which is not always the last one
   * that arrived.
   *
   * A widened answer over `expandedLinesCeiling` is dropped rather than
   * summarised: what the reader was already looking at stays on screen, so
   * "show more" never turns into "shown less". The request is then rolled back
   * to the last accepted context, so the file does not stay pinned to a width
   * the panel refuses to draw — every later refetch (staging, rollback) would
   * otherwise pay for it again.
   */
  const [shown, setShown] = createSignal<FileDiff | null>(null);
  /** Request identity of the accepted payload, and what it cost to draw. */
  let acceptedKey: string | null = null;
  let acceptedContext: number | undefined = undefined;
  let acceptedRevealed: GapRange[] = [];
  let drawnLines = 0;
  const view = createMemo(() => {
    const f = shown();
    return f && !f.binary ? buildView(f) : null;
  });

  // Per-file UI state: current difference, revealed folds, "show it whole".
  const [current, setCurrent] = createSignal(-1);
  const [note, setNote] = createSignal("");
  const [opened, setOpened] = createSignal<Set<string>>(new Set<string>());
  const [whole, setWhole] = createSignal(false);
  const [baseLines, setBaseLines] = createSignal<number | null>(null);
  const anchors = new Map<number, HTMLElement>();
  // Keyed on the file and the whitespace mode, not on the whole request: asking
  // for more context is the reader *staying* in this file, and resetting here
  // would throw away the position they widened the context to look at.
  let shownKey: string | null = null;
  createEffect(() => {
    const s = source();
    const key = s ? `${JSON.stringify(s)}|${ws()}` : null;
    if (key === shownKey) return;
    shownKey = key;
    setCurrent(-1);
    setNote("");
    setOpened(new Set<string>());
    setWhole(false);
    setContext(undefined);
    setRevealed([]);
    setBaseLines(null);
    acceptedKey = null;
    acceptedContext = undefined;
    acceptedRevealed = [];
    drawnLines = 0;
    anchors.clear();
  });

  createEffect(() => {
    const f = diff();
    if (!f) return;
    const ctx = untrack(context);
    const ceiling = expandedLinesCeiling(drawnLines);
    const got = payloadLines(f);
    // Only a *widening* is refused. The same request coming back larger (the
    // file itself grew under a stage or a rollback) is the truth about the file
    // and has to be drawn, or the panel would sit on a stale patch; volume there
    // is the "big diff" summary's business.
    if (ctx !== undefined && ctx !== acceptedContext && got > ceiling && untrack(shown)) {
      setNote(d().expandTooLarge(got, ceiling));
      setRevealed(acceptedRevealed);
      setContext(acceptedContext);
      return;
    }
    const key = `${shownKey}|${ctx ?? ""}`;
    // The rolled-back request lands here again with the very payload already on
    // screen; re-publishing it would rebuild every row for nothing.
    if (key === acceptedKey && untrack(shown)) return;
    acceptedKey = key;
    acceptedContext = ctx;
    acceptedRevealed = untrack(revealed);
    drawnLines = got;
    setShown(f);
  });


  /**
   * Asking for a gap is asking to *see* those lines. The wider payload arrives
   * with its long unchanged runs folded again by the same rule that folds
   * everything else, so without this a click on a gap would trade one collapsed
   * row for another and the lines would still be hidden.
   *
   * Only the folds that cover a gap the reader opened, and only ever added to
   * what is already open: opening every fold in the file would reveal regions
   * nobody asked about, and replacing the set would undo it again on the next
   * payload.
   */
  createEffect(() => {
    const v = view();
    const want = revealed();
    if (!v || want.length === 0) return;
    const ids: string[] = [];
    for (const h of v.hunks)
      for (const it of h.items)
        if (it.kind === "fold" && it.rows.some((r) => inAnyGap(want, r))) ids.push(it.id);
    if (ids.length === 0) return;
    setOpened((prev) => {
      const next = new Set(prev);
      const before = next.size;
      for (const id of ids) next.add(id);
      return next.size === before ? prev : next;
    });
  });

  /**
   * How long the file was **as first asked for**.
   *
   * The "big diff" summary is a guard against a payload too large to render, and
   * the reader's own request for more context must not trip it: revealing lines
   * would then hide the diff behind a warning, and "show more" would show less.
   * So the threshold is judged by the first, un-widened answer for this file.
   */
  createEffect(() => {
    const v = view();
    if (v && context() === undefined) setBaseLines(v.lines);
  });

  const goto = (i: number) => {
    setCurrent(i);
    anchors.get(i)?.scrollIntoView({ block: "center" });
  };
  // In the summary of a big diff there is nothing to scroll to, so a jump
  // reveals the diff first instead of doing nothing.
  const reveal = () => {
    if (bigDiff()) setWhole(true);
  };
  const api: DiffApi = {
    count: () => view()?.count ?? 0,
    next: () => {
      const n = view()?.count ?? 0;
      if (n === 0) return setNote(d().diffNoDifferences());
      if (current() + 1 >= n) return setNote(d().diffAtLast());
      setNote("");
      reveal();
      goto(current() + 1);
    },
    prev: () => {
      const n = view()?.count ?? 0;
      if (n === 0) return setNote(d().diffNoDifferences());
      if (current() <= 0) return setNote(d().diffAtFirst());
      setNote("");
      reveal();
      goto(current() - 1);
    },
    scrollLines: (delta) => scrollEl?.scrollBy({ top: delta * LINE_PX }),
    scrollToEdge: (edge) =>
      scrollEl?.scrollTo({ top: edge < 0 ? 0 : scrollEl.scrollHeight }),
  };
  props.api?.(api);

  // Standalone (Changes mode) the panel is not in the focus cycle, so its
  // navigation needs application shortcuts. Hosted, the panel's own handlers
  // carry them and a second registration would throw.
  if (!hosted) {
    registerHotkey("ArrowDown", () => api.next());
    registerHotkey("ArrowUp", () => api.prev());
  }

  const bigDiff = () => (baseLines() ?? 0) > BIG_DIFF_LINES && !whole();
  const hasSideBySide = () =>
    split() && !diff.loading && !diff.error && !!view() && view()!.hunks.length > 0 && !bigDiff();

  const labels = createMemo<{ left: SideLabel; right: SideLabel } | null>(() => {
    const s = source();
    return s
      ? sideLabels(s, {
          working: d().workingTreeSide(),
          index: d().indexSide(),
          head: d().headSide(),
          noParent: d().noParentSide(),
        })
      : null;
  });

  const stageable = () => {
    const s = source();
    // A patch produced with whitespace ignored does not apply; offering to stage
    // it would break the working feature this panel already carries.
    return s?.kind === "worktree" && ws() === "none";
  };

  const sizeText = (n?: number) => (n == null ? d().sizeUnknown() : d().bytes(n));

  return (
    <Show
      when={source()}
      fallback={
        <div class="flex h-full items-center justify-center text-xs text-fg-muted">
          {d().selectFileHint()}
        </div>
      }
    >
      <div class="flex h-full flex-col">
        <div class="flex flex-wrap items-center gap-2 border-b border-border px-2 py-1 text-xs">
          <span class="min-w-0 truncate font-mono" title={source()!.path}>
            {source()!.path}
          </span>
          <Show when={!hosted}>
            <div class="ml-auto flex shrink-0 overflow-hidden rounded border border-border">
              <For each={bases()}>
                {(b) => (
                  <button
                    class="whitespace-nowrap px-1.5 py-0.5"
                    classList={{
                      "bg-accent text-white": base() === b.id,
                      "hover:bg-bg-muted": base() !== b.id,
                    }}
                    onClick={() => setBase(b.id)}
                  >
                    {b.label}
                  </button>
                )}
              </For>
            </div>
          </Show>

          <div
            class="flex shrink-0 items-center gap-1"
            classList={{ "ml-auto": hosted }}
          >
            <span class="text-fg-muted">
              {/* "0 differences" before the answer arrives is a claim gated on
                  missing data, not a count. */}
              {view() ? d().diffCount(view()!.count) : ""}
            </span>
            <button
              class="rounded border border-border px-1.5 py-0.5 hover:bg-bg-muted"
              title={d().diffPrevTip()}
              onClick={() => api.prev()}
            >
              ↑
            </button>
            <button
              class="rounded border border-border px-1.5 py-0.5 hover:bg-bg-muted"
              title={d().diffNextTip()}
              onClick={() => api.next()}
            >
              ↓
            </button>
          </div>

          <label class="flex shrink-0 items-center gap-1 text-fg-muted">
            {d().diffWhitespace()}
            <select
              class="rounded border border-border bg-bg px-1 py-0.5 text-fg"
              value={ws()}
              onChange={(e) => setWs(e.currentTarget.value as WhitespaceMode)}
            >
              <option value="none">{d().wsNone()}</option>
              <option value="trailing">{d().wsTrailing()}</option>
              <option value="all">{d().wsAll()}</option>
            </select>
          </label>

          <label class="flex shrink-0 items-center gap-1 text-fg-muted">
            {d().diffHighlight()}
            <select
              class="rounded border border-border bg-bg px-1 py-0.5 text-fg"
              value={hl()}
              onChange={(e) => setHl(e.currentTarget.value as HighlightMode)}
            >
              <option value="words">{d().hlWords()}</option>
              <option value="lines">{d().hlLines()}</option>
              <option value="none">{d().hlNone()}</option>
            </select>
          </label>

          <button
            class="w-20 shrink-0 whitespace-nowrap rounded border border-border px-1.5 py-0.5 text-center hover:bg-bg-muted"
            onClick={() => setSplit((v) => !v)}
            title="Side-by-side / unified"
          >
            {split() ? "▥ split" : "▤ unified"}
          </button>
        </div>

        {/* Which revision each side shows, and which of them cannot be edited. */}
        <Show when={labels()}>
          <div class="flex items-center gap-2 border-b border-border bg-bg-subtle px-2 py-0.5 text-[11px] text-fg-muted">
            <Show when={split()} fallback={
              <span class="truncate font-mono">
                {labels()!.left.text} → {labels()!.right.text}
              </span>
            }>
              <span class="flex min-w-0 items-center gap-1" style={{ width: `${ratio() * 100}%` }}>
                <SideTag label={labels()!.left} />
              </span>
              <span class="flex min-w-0 flex-1 items-center gap-1">
                <SideTag label={labels()!.right} />
              </span>
            </Show>
          </div>
        </Show>

        <Show when={note() || shown()?.mergeFirstParent}>
          <div class="border-b border-border px-2 py-0.5 text-[11px] text-warn">
            {note() || d().mergeFirstParentNote()}
          </div>
        </Show>

        <div class="relative min-h-0 flex-1" ref={wrapEl}>
          <div
            ref={scrollEl}
            class="absolute inset-0 overflow-auto font-mono text-xs leading-tight"
          >
            <Show
              when={!diff.loading && !diff.error}
              fallback={
                <div class="p-3 text-fg-muted">
                  {diff.error ? d().diffUnavailable() : "…"}
                </div>
              }
            >
              <Show
                when={shown() && !shown()!.binary}
                fallback={
                  <div class="p-3 text-fg-muted">
                    {shown()?.binary
                      ? d().binarySizes(sizeText(shown()!.oldSize), sizeText(shown()!.newSize))
                      : d().noChangesForBase()}
                  </div>
                }
              >
                <Show
                  when={view()!.hunks.length > 0}
                  fallback={<div class="p-3 text-fg-muted">{d().noChangesForBase()}</div>}
                >
                  <Show
                    when={!bigDiff()}
                    fallback={
                      <div class="flex flex-col items-start gap-2 p-3 text-fg-muted">
                        <span>{d().bigDiffNote(view()!.lines)}</span>
                        <span>{d().diffCount(view()!.count)}</span>
                        <button
                          class="rounded border border-border px-2 py-0.5 hover:bg-bg-muted"
                          onClick={() => setWhole(true)}
                        >
                          {d().showWholeDiff()}
                        </button>
                      </div>
                    }
                  >
                    <For each={view()!.hunks}>
                      {(hv, i) => (
                        <>
                          <Show when={view()!.gaps[i()] > 0}>
                            <button
                              class="w-full bg-bg-muted px-2 py-0.5 text-center text-fg-subtle hover:bg-accent/10 hover:text-accent"
                              title={d().expandGapTip()}
                              onClick={() => expandGap(i())}
                            >
                              ⋯ {d().expandGap(view()!.gaps[i()])}
                            </button>
                          </Show>
                          <HunkBody
                            hunk={hv.hunk}
                            items={hv.items}
                            split={split()}
                            ratio={ratio()}
                            highlight={hl()}
                            current={current()}
                            opened={opened()}
                            anchor={(idx, el) => anchors.set(idx, el)}
                            onOpen={(id) =>
                              setOpened((s) => {
                                const next = new Set(s);
                                next.add(id);
                                return next;
                              })
                            }
                            stageable={stageable()}
                            widened={widened()}
                            base={source()!.kind === "worktree" ? (source() as { base: DiffBase }).base : null}
                            onStage={() => act(() => hunkStage(hv.hunk.patch))}
                            onUnstage={() => act(() => hunkUnstage(hv.hunk.patch))}
                            onRevert={async () => {
                              const ask = widened()
                                ? d().revertHunkWideConfirm()
                                : d().revertHunkConfirm();
                              if (await confirmAction(ask))
                                await act(() => hunkRevert(hv.hunk.patch));
                            }}
                          />
                        </>
                      )}
                    </For>
                  </Show>
                </Show>
              </Show>
            </Show>
          </div>

          {/* One draggable divider for the whole side-by-side view. Absolute over
              the viewport (not the scrolling content), positioned at the split
              ratio; it doubles as the vertical divider line. */}
          <Show when={hasSideBySide()}>
            <div
              role="separator"
              aria-orientation="vertical"
              class="group absolute inset-y-0 z-20 flex w-2 -translate-x-1/2 cursor-col-resize items-stretch justify-center"
              style={{ left: `${ratio() * 100}%` }}
              onPointerDown={onSplitDown}
            >
              <div class="w-px bg-border transition-colors group-hover:bg-accent" />
            </div>
          </Show>
        </div>
      </div>
    </Show>
  );
}

/** Lines a gap hides, in both numberings; an empty range on a side the file
 * does not have (an added or a deleted file numbers only one of them). */
type GapRange = { oldFrom: number; oldTo: number; newFrom: number; newTo: number };

const firstNo = (h: Hunk, side: "old" | "new") => {
  for (const l of h.lines) {
    const n = side === "old" ? l.oldNo : l.newNo;
    if (n != null) return n;
  }
  return null;
};
const lastNo = (h: Hunk, side: "old" | "new") => {
  for (let i = h.lines.length - 1; i >= 0; i--) {
    const n = side === "old" ? h.lines[i].oldNo : h.lines[i].newNo;
    if (n != null) return n;
  }
  return null;
};

/** The lines git left out between two hunks, named by number so the request can
 * be recognised again in a payload cut differently. */
function gapRange(prev: Hunk, next: Hunk): GapRange {
  const side = (s: "old" | "new"): [number, number] => {
    const a = lastNo(prev, s);
    const b = firstNo(next, s);
    return a != null && b != null ? [a + 1, b - 1] : [1, 0]; // [1, 0] matches nothing
  };
  const [oldFrom, oldTo] = side("old");
  const [newFrom, newTo] = side("new");
  return { oldFrom, oldTo, newFrom, newTo };
}

/** Does this row lie inside one of the gaps the reader opened? */
function inAnyGap(ranges: GapRange[], row: Row): boolean {
  const o = row.left?.oldNo;
  const n = row.right?.newNo;
  return ranges.some(
    (r) =>
      (o != null && o >= r.oldFrom && o <= r.oldTo) ||
      (n != null && n >= r.newFrom && n <= r.newTo),
  );
}

function fetchDiff(
  src: DiffSource,
  ws: WhitespaceMode,
  context?: number,
): Promise<FileDiff> {
  if (src.kind === "commit") return commitFileDiff(src.hash, src.path, ws, context);
  if (src.kind === "compare")
    return commitsCompareDiff(src.from, src.to, src.path, ws, context);
  return diffFile(src.path, src.base, ws, context);
}

function SideTag(props: { label: SideLabel }) {
  return (
    <>
      <span class="truncate font-mono">{props.label.text}</span>
      <Show when={props.label.readOnly}>
        <span class="shrink-0 rounded border border-border px-1 text-fg-subtle">
          {d().readOnlySide()}
        </span>
      </Show>
    </>
  );
}

function HunkBody(props: {
  hunk: Hunk;
  items: Item[];
  split: boolean;
  ratio: number;
  highlight: HighlightMode;
  current: number;
  opened: Set<string>;
  anchor: (idx: number, el: HTMLElement) => void;
  onOpen: (id: string) => void;
  stageable: boolean;
  /** The payload was asked for with more context than the file was first shown
   * with, so a hunk here can cover what used to be two. */
  widened: boolean;
  base: DiffBase | null;
  onStage: () => void;
  onUnstage: () => void;
  onRevert: () => void;
}) {
  // Expanding the context merges neighbouring changes into one hunk, so Stage,
  // Unstage and Revert act on a wider region than the one the reader saw before
  // expanding. The buttons say so rather than quietly doing more (R46i).
  const label = (base: string) => (props.widened ? `${base} · ${d().hunkWide()}` : base);
  const tip = () =>
    !props.stageable ? d().hunkWhitespaceTip() : props.widened ? d().hunkWideTip() : undefined;
  return (
    <div class="border-b border-border">
      <div class="flex items-center gap-2 bg-bg-muted px-2 py-0.5 text-accent">
        <span class="truncate">{props.hunk.header}</span>
        <div class="ml-auto flex gap-1">
          <Show when={props.base === "worktree"}>
            <HunkBtn
              label={label("Stage")}
              disabled={!props.stageable}
              tip={tip()}
              onClick={props.onStage}
            />
            <HunkBtn
              label={label("Revert")}
              danger
              disabled={!props.stageable}
              tip={tip()}
              onClick={props.onRevert}
            />
          </Show>
          <Show when={props.base === "index"}>
            <HunkBtn
              label={label("Unstage")}
              tip={props.widened ? d().hunkWideTip() : undefined}
              onClick={props.onUnstage}
            />
          </Show>
        </div>
      </div>
      <For each={props.items}>
        {(it) => (
          <Show
            when={it.kind === "row" || props.opened.has((it as { id: string }).id)}
            fallback={
              <button
                class="w-full bg-bg-subtle px-2 py-0.5 text-center text-fg-subtle hover:bg-bg-muted"
                onClick={() => props.onOpen((it as { id: string }).id)}
              >
                ⋯ {d().foldedLines((it as { hidden: number }).hidden)}
              </button>
            }
          >
            <For each={it.kind === "row" ? [it.row] : it.rows}>
              {(row) => (
                <RowView
                  row={row}
                  split={props.split}
                  ratio={props.ratio}
                  highlight={props.highlight}
                  active={row.diff >= 0 && row.diff === props.current}
                  anchor={props.anchor}
                />
              )}
            </For>
          </Show>
        )}
      </For>
    </div>
  );
}

function HunkBtn(props: {
  label: string;
  danger?: boolean;
  disabled?: boolean;
  tip?: string;
  onClick: () => void;
}) {
  return (
    <button
      class={`rounded border px-1 text-[11px] ${DISABLED_CLASS}`}
      classList={{
        "border-danger/50 text-danger hover:bg-danger/10": props.danger,
        "border-border hover:bg-bg": !props.danger,
      }}
      disabled={props.disabled}
      title={props.tip}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

/** Which line is added, removed or unchanged is the diff itself, not an
 * emphasis option: the tint stays in all three highlight modes, and only the
 * intra-line emphasis (see `Text`) is what "none" switches off. */
function lineBg(o: string | undefined) {
  return o === "+" ? "bg-success/15" : o === "-" ? "bg-danger/15" : "";
}

/** One paired row, in either layout. The anchor lands on the row that opens a
 * difference, which is what the previous / next buttons scroll to. */
function RowView(props: {
  row: Row;
  split: boolean;
  ratio: number;
  highlight: HighlightMode;
  active: boolean;
  anchor: (idx: number, el: HTMLElement) => void;
}) {
  const segs = createMemo(() =>
    props.highlight === "words" && pairChanged(props.row)
      ? wordSegments(props.row.left!.content, props.row.right!.content)
      : null,
  );
  return (
    <div
      class="flex"
      classList={{ "ring-1 ring-inset ring-accent": props.active && props.row.first }}
      ref={(el) => {
        if (props.row.first && props.row.diff >= 0) props.anchor(props.row.diff, el);
      }}
    >
      <Show
        when={props.split}
        fallback={
          <UnifiedRow row={props.row} highlight={props.highlight} segs={segs()} />
        }
      >
        <Cell
          line={props.row.left}
          side="old"
          frac={props.ratio}
          highlight={props.highlight}
          segs={segs()?.left}
        />
        <Cell
          line={props.row.right}
          side="new"
          frac={1 - props.ratio}
          highlight={props.highlight}
          segs={segs()?.right}
        />
      </Show>
    </div>
  );
}

type Seg = { text: string; changed: boolean };

function Text(props: {
  content: string;
  origin?: string;
  highlight: HighlightMode;
  segs?: Seg[];
}) {
  // Class strings are built here rather than through `classList`: an empty key
  // in classList reaches `DOMTokenList.toggle("")`, which throws.
  const emphasis = () =>
    props.origin === "+" ? "bg-success/40" : props.origin === "-" ? "bg-danger/40" : "";
  return (
    <Show
      when={props.segs && props.segs.length > 0}
      fallback={
        <span
          class={`whitespace-pre-wrap break-all ${
            props.highlight === "lines" ? emphasis() : ""
          }`}
        >
          {props.content || " "}
        </span>
      }
    >
      <span class="whitespace-pre-wrap break-all">
        <For each={props.segs}>
          {(s) => <span class={s.changed ? emphasis() : ""}>{s.text}</span>}
        </For>
      </span>
    </Show>
  );
}

function UnifiedRow(props: {
  row: Row;
  highlight: HighlightMode;
  segs: { left: Seg[]; right: Seg[] } | null;
}) {
  const sides = () =>
    props.row.diff < 0
      ? [{ line: props.row.left, segs: undefined as Seg[] | undefined }]
      : [
          { line: props.row.left, segs: props.segs?.left },
          { line: props.row.right, segs: props.segs?.right },
        ].filter((s) => !!s.line);
  return (
    <div class="min-w-0 flex-1">
      <For each={sides()}>
        {(s) => (
          <div class={`flex ${lineBg(s.line!.origin)}`}>
            <span class="w-10 shrink-0 select-none pr-1 text-right text-fg-muted">
              {s.line!.oldNo ?? ""}
            </span>
            <span class="w-10 shrink-0 select-none pr-2 text-right text-fg-muted">
              {s.line!.newNo ?? ""}
            </span>
            <span class="w-3 shrink-0 select-none text-fg-muted">{s.line!.origin}</span>
            <Text
              content={s.line!.content}
              origin={s.line!.origin}
              highlight={props.highlight}
              segs={s.segs}
            />
          </div>
        )}
      </For>
    </div>
  );
}

function Cell(props: {
  line?: DiffLine;
  side: "old" | "new";
  frac: number;
  highlight: HighlightMode;
  segs?: Seg[];
}) {
  const no = () => (props.side === "old" ? props.line?.oldNo : props.line?.newNo);
  const bg = () => (props.line ? lineBg(props.line.origin) : "bg-bg-muted/40");
  return (
    <div class={`flex min-w-0 ${bg()}`} style={{ width: `${props.frac * 100}%` }}>
      <span class="w-10 shrink-0 select-none pr-2 text-right text-fg-muted">
        {no() ?? ""}
      </span>
      <span class="w-3 shrink-0 select-none text-fg-muted">
        {props.line ? props.line.origin : ""}
      </span>
      <span class="min-w-0 flex-1">
        <Show when={props.line} fallback={<span> </span>}>
          <Text
            content={props.line!.content}
            origin={props.line!.origin}
            highlight={props.highlight}
            segs={props.segs}
          />
        </Show>
      </span>
    </div>
  );
}
