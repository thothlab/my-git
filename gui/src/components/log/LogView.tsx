import { createSignal, onCleanup, onMount } from "solid-js";
import { d } from "../../i18n";
import Resizer, { RowResizer } from "../Resizer";
import CommitDetailsPane from "./CommitDetailsPane";
import LogTable from "./LogTable";
import { PanelChrome, PanelNote } from "./PanelChrome";

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

  const clampDetails = (w: number) => {
    const total = bottomEl?.clientWidth ?? window.innerWidth;
    return Math.round(Math.min(Math.max(w, MIN_DETAILS), Math.max(MIN_DETAILS, total - MIN_DIFF)));
  };
  const setDetailsW = (w: number) => setDetailsWSig(clampDetails(w));
  const setRatio = (r: number) => setRatioSig(clamp01(r));

  onMount(() => {
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
          <CommitDetailsPane selected={selected} />
        </div>
        <Resizer
          getWidth={detailsW}
          setWidth={setDetailsW}
          onCommit={() => localStorage.setItem(DETAILS_W_KEY, String(detailsW()))}
        />
        <div class="min-w-0 flex-1 overflow-hidden border-l border-border">
          {/* TODO(prd): task 10 wires DiffView into this frame as the diff source. */}
          <PanelChrome id="diff" title={d().diffTitle()} handlers={{}}>
            <PanelNote title={d().historyPending()} hint={d().selectCommitHint()} />
          </PanelChrome>
        </div>
      </div>
    </div>
  );
}

// Keep both halves of the vertical split usable regardless of window height.
const clamp01 = (r: number) => Math.min(0.85, Math.max(0.15, r));
