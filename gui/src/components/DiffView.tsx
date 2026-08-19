import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  createEffect,
  onCleanup,
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

  // Every signal the request depends on is read here, synchronously, before any
  // await: past the first await the tracking scope is gone.
  const request = () => {
    const s = source();
    return s ? { src: s, ws: ws() } : null;
  };

  // Guard against a stale answer (R32i.3): a slower request for the file the
  // user has already left must never overwrite the newer one.
  let seq = 0;
  const [diff, { refetch }] = createResource(request, async (r) => {
    const mine = ++seq;
    const got = await fetchDiff(r.src, r.ws);
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

  const view = createMemo(() => {
    const f = diff();
    return f && !f.binary ? buildView(f) : null;
  });

  // Per-file UI state: current difference, revealed folds, "show it whole".
  const [current, setCurrent] = createSignal(-1);
  const [note, setNote] = createSignal("");
  const [opened, setOpened] = createSignal<Set<string>>(new Set<string>());
  const [whole, setWhole] = createSignal(false);
  const anchors = new Map<number, HTMLElement>();
  createEffect(() => {
    const r = request();
    void r?.src;
    setCurrent(-1);
    setNote("");
    setOpened(new Set<string>());
    setWhole(false);
    anchors.clear();
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

  const bigDiff = () => !!view() && view()!.lines > BIG_DIFF_LINES && !whole();
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

        <Show when={note() || diff()?.mergeFirstParent}>
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
                when={diff() && !diff()!.binary}
                fallback={
                  <div class="p-3 text-fg-muted">
                    {diff()?.binary
                      ? d().binarySizes(sizeText(diff()!.oldSize), sizeText(diff()!.newSize))
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
                            <div
                              class="bg-bg-muted px-2 py-0.5 text-center text-fg-subtle"
                              title={d().gapTip()}
                            >
                              ⋯ {d().gapLines(view()!.gaps[i()])}
                            </div>
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
                            base={source()!.kind === "worktree" ? (source() as { base: DiffBase }).base : null}
                            onStage={() => act(() => hunkStage(hv.hunk.patch))}
                            onUnstage={() => act(() => hunkUnstage(hv.hunk.patch))}
                            onRevert={async () => {
                              if (await confirmAction(d().revertHunkConfirm()))
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

function fetchDiff(src: DiffSource, ws: WhitespaceMode): Promise<FileDiff> {
  if (src.kind === "commit") return commitFileDiff(src.hash, src.path, ws);
  if (src.kind === "compare") return commitsCompareDiff(src.from, src.to, src.path, ws);
  return diffFile(src.path, src.base, ws);
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
  base: DiffBase | null;
  onStage: () => void;
  onUnstage: () => void;
  onRevert: () => void;
}) {
  return (
    <div class="border-b border-border">
      <div class="flex items-center gap-2 bg-bg-muted px-2 py-0.5 text-accent">
        <span class="truncate">{props.hunk.header}</span>
        <div class="ml-auto flex gap-1">
          <Show when={props.base === "worktree"}>
            <HunkBtn
              label="Stage"
              disabled={!props.stageable}
              tip={props.stageable ? undefined : d().hunkWhitespaceTip()}
              onClick={props.onStage}
            />
            <HunkBtn
              label="Revert"
              danger
              disabled={!props.stageable}
              tip={props.stageable ? undefined : d().hunkWhitespaceTip()}
              onClick={props.onRevert}
            />
          </Show>
          <Show when={props.base === "index"}>
            <HunkBtn label="Unstage" onClick={props.onUnstage} />
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
      class="rounded border px-1 text-[11px] disabled:cursor-not-allowed disabled:opacity-40"
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
