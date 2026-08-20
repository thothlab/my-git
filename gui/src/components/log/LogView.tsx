import { createMemo, createSignal, onCleanup, onMount } from "solid-js";
import Resizer, { RowResizer } from "../Resizer";
import DiffPanel from "../diff/DiffPanel";
import type { DiffSource } from "../diff/model";
import { focusPanel } from "../../hotkeys";
import { commits } from "../../logStore";
import CommitDetailsPane from "./CommitDetailsPane";
import { compareTarget } from "./actions/compareSelection";
import { selectedCommitFile } from "./commitFileSelection";
import LogTable from "./LogTable";

/**
 * Right-hand area of the Log mode: the commit list on top, below it a
 * horizontal split of "commit details + files | diff". The branch tree lives in
 * the window's existing left aside, so the outer `aside | Resizer | main`
 * skeleton stays as it is.
 *
 * Minimum widths (PRD): tree 180 + files 220 + diff 320 = 720, exactly the
 * window's minWidth. Dividers take no layout width, so nothing is clipped there.
 */
const MIN_DETAILS = 220;
const MIN_DIFF = 320;
const SPLIT_KEY = "logSplitRatio";
const DETAILS_W_KEY = "logDetailsWidth";

export default function LogView() {
  let bodyEl: HTMLDivElement | undefined;
  let bottomEl: HTMLDivElement | undefined;

  const [ratio, setRatioSig] = createSignal(clamp01(Number(localStorage.getItem(SPLIT_KEY)) || 0.55));
  const [detailsW, setDetailsWSig] = createSignal(Number(localStorage.getItem(DETAILS_W_KEY)) || 360);
  const [selected, setSelected] = createSignal<string | null>(null);

  /**
   * What the diff panel shows — the last link of the cascade
   * branch → commit → file → diff.
   *
   * Two sources, one selection: a comparison of two revisions replaces the
   * selected commit as the subject of the details panel, so the file picked
   * there belongs to the comparison and the diff must be the one between its
   * two sides. Outside a comparison the file belongs to a commit and is shown
   * against that commit's first parent.
   *
   * `parent` is looked up in the loaded rows rather than derived: `DiffSource`
   * asks for the parent's hash, and `null` there means "root commit" — a value
   * that must be established, not guessed. A commit whose row is not loaded
   * (nothing selects one today) yields no source at all rather than a diff
   * claiming a root commit.
   */
  const diffSource = createMemo<DiffSource | null>(() => {
    const file = selectedCommitFile();
    if (!file) return null;
    const cmp = compareTarget();
    if (cmp) return { kind: "compare", path: file.path, from: cmp.from, to: cmp.to };
    const row = commits().find((c) => c.hash === file.hash);
    if (!row) return null;
    return { kind: "commit", path: file.path, hash: file.hash, parent: row.parents[0] ?? null };
  });

  const clampDetails = (w: number) => {
    const total = bottomEl?.clientWidth ?? window.innerWidth;
    return Math.round(Math.min(Math.max(w, MIN_DETAILS), Math.max(MIN_DETAILS, total - MIN_DIFF)));
  };
  const setDetailsW = (w: number) => setDetailsWSig(clampDetails(w));
  const setRatio = (r: number) => setRatioSig(clamp01(r));

  onMount(() => {
    // The commit list starts with the keyboard, or the mode has no keyboard at
    // all: the key layer routes Arrow / Home / End / Enter to the *focused*
    // panel, and nothing focused one on entry. Every key then fell through to
    // the window, the cursor never left the newest commit, and the panel read
    // as "the keyboard does not work here" — which is how it was reported.
    // Queued so it runs after the panels below have registered themselves.
    queueMicrotask(() => focusPanel("commits"));
    setDetailsWSig((w) => clampDetails(w));
    const onResize = () => setDetailsWSig((w) => clampDetails(w));
    window.addEventListener("resize", onResize);
    onCleanup(() => window.removeEventListener("resize", onResize));
  });

  return (
    <div ref={bodyEl} class="flex h-full min-h-0 min-w-0 flex-col">
      <div class="min-h-0 overflow-hidden" style={{ height: `${ratio() * 100}%` }}>
        <LogTable onSelect={setSelected} />
      </div>

      <RowResizer
        container={() => bodyEl}
        setRatio={setRatio}
        onCommit={() => localStorage.setItem(SPLIT_KEY, String(ratio()))}
      />

      <div ref={bottomEl} class="flex min-h-0 flex-1 border-t border-border">
        <div class="min-w-0 shrink-0 overflow-hidden" style={{ width: `${detailsW()}px` }}>
          <CommitDetailsPane selected={selected} compare={compareTarget} />
        </div>
        <Resizer
          getWidth={detailsW}
          setWidth={setDetailsW}
          onCommit={() => localStorage.setItem(DETAILS_W_KEY, String(detailsW()))}
        />
        <div class="min-w-0 flex-1 overflow-hidden border-l border-border">
          <DiffPanel source={diffSource()} />
        </div>
      </div>
    </div>
  );
}

// Keep both halves of the vertical split usable regardless of window height.
const clamp01 = (r: number) => Math.min(0.85, Math.max(0.15, r));
