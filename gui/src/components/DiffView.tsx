import { For, Show, createResource, createSignal, onCleanup } from "solid-js";
import {
  diffFile,
  hunkRevert,
  hunkStage,
  hunkUnstage,
  type DiffBase,
  type DiffLine,
  type Hunk,
  type RepoState,
} from "../api";
import { confirmAction, run, selectedPath } from "../store";
import { d } from "../i18n";
import { beginDrag } from "./Resizer";

const SPLIT_RATIO_KEY = "diffSplitRatio";
const clampRatio = (r: number) => Math.min(0.8, Math.max(0.2, r));

export default function DiffView() {
  const [base, setBase] = createSignal<DiffBase>("worktree");
  const [split, setSplit] = createSignal(true);

  // Function, not a module constant, so labels track the active locale.
  const bases = (): { id: DiffBase; label: string }[] => [
    { id: "worktree", label: d().unstaged() },
    { id: "index", label: d().staged() },
    { id: "head", label: d().vsHead() },
  ];

  const source = () => {
    const p = selectedPath();
    return p ? { path: p, base: base() } : null;
  };
  const [diff, { refetch }] = createResource(source, (s) =>
    diffFile(s.path, s.base),
  );

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

  const hasSideBySide = () =>
    split() &&
    !diff.loading &&
    !diff.error &&
    !!diff() &&
    !diff()!.binary &&
    diff()!.hunks.length > 0;

  return (
    <Show
      when={selectedPath()}
      fallback={
        <div class="flex h-full items-center justify-center text-xs text-fg-muted">
          {d().selectFileHint()}
        </div>
      }
    >
      <div class="flex h-full flex-col">
        <div class="flex items-center gap-2 border-b border-border px-2 py-1 text-xs">
          <span class="truncate font-mono" title={selectedPath()!}>
            {selectedPath()}
          </span>
          <div class="ml-auto flex overflow-hidden rounded border border-border">
            <For each={bases()}>
              {(b) => (
                <button
                  class="px-1.5 py-0.5"
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
          <button
            class="rounded border border-border px-1.5 py-0.5 hover:bg-bg-muted"
            onClick={() => setSplit((v) => !v)}
            title="Side-by-side / unified"
          >
            {split() ? "▥ split" : "▤ unified"}
          </button>
        </div>

        <div class="relative min-h-0 flex-1" ref={wrapEl}>
          <div class="absolute inset-0 overflow-auto font-mono text-xs leading-tight">
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
                    {diff()?.binary ? d().binaryFile() : d().noChangesForBase()}
                  </div>
                }
              >
                <Show
                  when={diff()!.hunks.length > 0}
                  fallback={<div class="p-3 text-fg-muted">{d().noChangesForBase()}</div>}
                >
                  <For each={diff()!.hunks}>
                    {(h) => (
                      <HunkView
                        hunk={h}
                        base={base()}
                        split={split()}
                        ratio={ratio()}
                        onStage={() => act(() => hunkStage(h.patch))}
                        onUnstage={() => act(() => hunkUnstage(h.patch))}
                        onRevert={async () => {
                          if (await confirmAction(d().revertHunkConfirm()))
                            await act(() => hunkRevert(h.patch));
                        }}
                      />
                    )}
                  </For>
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

function HunkView(props: {
  hunk: Hunk;
  base: DiffBase;
  split: boolean;
  ratio: number;
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
            <HunkBtn label="Stage" onClick={props.onStage} />
            <HunkBtn label="Revert" danger onClick={props.onRevert} />
          </Show>
          <Show when={props.base === "index"}>
            <HunkBtn label="Unstage" onClick={props.onUnstage} />
          </Show>
        </div>
      </div>
      <Show when={props.split} fallback={<Unified lines={props.hunk.lines} />}>
        <SideBySide lines={props.hunk.lines} ratio={props.ratio} />
      </Show>
    </div>
  );
}

function HunkBtn(props: { label: string; danger?: boolean; onClick: () => void }) {
  return (
    <button
      class="rounded border px-1 text-[11px]"
      classList={{
        "border-danger/50 text-danger hover:bg-danger/10": props.danger,
        "border-border hover:bg-bg": !props.danger,
      }}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

function lineBg(o: string) {
  return o === "+"
    ? "bg-success/15"
    : o === "-"
      ? "bg-danger/15"
      : "";
}

function Unified(props: { lines: DiffLine[] }) {
  return (
    <div>
      <For each={props.lines}>
        {(l) => (
          <div class={`flex ${lineBg(l.origin)}`}>
            <span class="w-10 shrink-0 select-none pr-1 text-right text-fg-muted">
              {l.oldNo ?? ""}
            </span>
            <span class="w-10 shrink-0 select-none pr-2 text-right text-fg-muted">
              {l.newNo ?? ""}
            </span>
            <span class="w-3 shrink-0 select-none text-fg-muted">{l.origin}</span>
            <span class="whitespace-pre">{l.content || " "}</span>
          </div>
        )}
      </For>
    </div>
  );
}

type Row = { left?: DiffLine; right?: DiffLine };

function toRows(lines: DiffLine[]): Row[] {
  const rows: Row[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];
  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let i = 0; i < n; i++) rows.push({ left: dels[i], right: adds[i] });
    dels = [];
    adds = [];
  };
  for (const l of lines) {
    if (l.origin === "-") dels.push(l);
    else if (l.origin === "+") adds.push(l);
    else {
      flush();
      rows.push({ left: l, right: l });
    }
  }
  flush();
  return rows;
}

function SideBySide(props: { lines: DiffLine[]; ratio: number }) {
  const rows = () => toRows(props.lines);
  return (
    <div>
      <For each={rows()}>
        {(r) => (
          <div class="flex">
            <Cell line={r.left} side="old" frac={props.ratio} />
            <Cell line={r.right} side="new" frac={1 - props.ratio} />
          </div>
        )}
      </For>
    </div>
  );
}

function Cell(props: { line?: DiffLine; side: "old" | "new"; frac: number }) {
  const no = () =>
    props.side === "old" ? props.line?.oldNo : props.line?.newNo;
  const bg = () => (props.line ? lineBg(props.line.origin) : "bg-bg-muted/40");
  return (
    <div class={`flex min-w-0 ${bg()}`} style={{ width: `${props.frac * 100}%` }}>
      <span class="w-10 shrink-0 select-none pr-2 text-right text-fg-muted">
        {no() ?? ""}
      </span>
      <span class="min-w-0 flex-1 whitespace-pre-wrap break-all">
        {props.line ? props.line.content || " " : ""}
      </span>
    </div>
  );
}
