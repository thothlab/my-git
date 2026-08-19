import { ErrorBoundary, createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { busy, busyLabel, error, openInitial, refresh, setViewMode, viewMode } from "./store";
import { d } from "./i18n";
import Toolbar from "./components/Toolbar";
import ChangesView from "./components/ChangesView";
import DiffView from "./components/DiffView";
import CommitPanel from "./components/CommitPanel";
import StatusBar from "./components/StatusBar";
import Resizer from "./components/Resizer";
import BranchTree from "./components/log/BranchTree";
import LogView from "./components/log/LogView";
import { registerHotkey, startHotkeys } from "./hotkeys";
import { ModalHost } from "./components/Modals";

const LEFT_WIDTH_KEY = "leftPanelWidth";
const TREE_WIDTH_KEY = "logTreeWidth";
const MIN_LEFT = 220; // changes panel never narrower than this
const MIN_DIFF = 360; // preferred minimum for the diff panel
const MIN_TREE = 180; // branch tree in the Log mode
const MIN_LOG_RIGHT = 540; // commit files 220 + diff 320 (PRD minimums)

// Keep the left panel within [min, innerWidth - minRight] so a resized (or stale
// persisted) width can never collapse the right side after the window shrinks.
// Invariant: the window's minWidth (720, see tauri.conf.json) >= both sums —
// 220 + 360 in the Changes mode, 180 + 540 in the Log mode, where the Log sum is
// exactly 720. Dividers take no layout width, so that equality holds. If the
// window is ever forced narrower, the left minimum wins (outer max).
const clampWidth = (w: number, min: number, minRight: number) =>
  Math.round(Math.min(Math.max(w, min), Math.max(min, window.innerWidth - minRight)));

export default function App() {
  const isLog = () => viewMode() === "log";
  // Each mode remembers its own left column: the branch tree and the changes
  // list have different natural widths and different minimums.
  const clampFor = (w: number, log: boolean) =>
    log ? clampWidth(w, MIN_TREE, MIN_LOG_RIGHT) : clampWidth(w, MIN_LEFT, MIN_DIFF);
  const [changesW, setChangesW] = createSignal(
    clampFor(Number(localStorage.getItem(LEFT_WIDTH_KEY)) || 288, false),
  );
  const [treeW, setTreeW] = createSignal(
    clampFor(Number(localStorage.getItem(TREE_WIDTH_KEY)) || 240, true),
  );
  const leftW = () => (isLog() ? treeW() : changesW());
  const setLeftWidth = (w: number) =>
    isLog() ? setTreeW(clampFor(w, true)) : setChangesW(clampFor(w, false));
  const persistLeftWidth = () =>
    localStorage.setItem(isLog() ? TREE_WIDTH_KEY : LEFT_WIDTH_KEY, String(leftW()));

  onMount(async () => {
    // re-clamp when the window is resized, so the right side keeps its minimum
    const onResize = () => {
      setChangesW((w) => clampFor(w, false));
      setTreeW((w) => clampFor(w, true));
    };
    window.addEventListener("resize", onResize);
    onCleanup(() => window.removeEventListener("resize", onResize));

    onCleanup(startHotkeys());
    registerHotkey("Digit1", () => setViewMode("changes"));
    registerHotkey("Digit2", () => setViewMode("log"));

    await openInitial();
    // resync on window focus — external git activity between interactions
    const unlisten = await getCurrentWindow().onFocusChanged(({ payload }) => {
      if (payload) void refresh();
    });
    onCleanup(unlisten);
  });

  // A render throw anywhere below shows a recoverable panel instead of a blank
  // window — the UI never silently "crashes".
  return (
    <ErrorBoundary
      fallback={(err, reset) => (
        <div class="flex h-full flex-col items-center justify-center gap-3 bg-bg p-6 text-center text-fg">
          <div class="text-sm font-semibold text-danger">{d().uiCrashTitle()}</div>
          <pre class="max-w-full overflow-auto whitespace-pre-wrap rounded border border-border bg-bg-muted p-3 text-left font-mono text-xs">
            {String(err?.message ?? err)}
          </pre>
          <div class="flex gap-2">
            <button
              class="rounded bg-accent px-3 py-1 text-sm text-white"
              onClick={() => {
                void refresh();
                reset();
              }}
            >
              {d().reloadState()}
            </button>
            <button
              class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
              onClick={() => location.reload()}
            >
              {d().reloadWindow()}
            </button>
          </div>
        </div>
      )}
    >
      <div class="flex h-full flex-col bg-bg text-fg">
        <Toolbar />
        <div class="h-0.5">
          <Show when={busy()}>
            <div class="busybar" />
          </Show>
        </div>
        {/* A long operation says which one it is, not just "something runs". */}
        <Show when={busy() && busyLabel()}>
          <div class="border-b border-border bg-bg-muted px-3 py-0.5 text-xs text-fg-muted">
            {busyLabel()}
          </div>
        </Show>

        <Show when={error()}>
          <pre class="max-h-32 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-xs text-danger">
            {error()}
          </pre>
        </Show>

        <div class="flex min-h-0 flex-1">
          <aside
            class="shrink-0 overflow-hidden border-r border-border"
            style={{ width: `${leftW()}px` }}
          >
            <Show when={isLog()} fallback={<ChangesView />}>
              <BranchTree />
            </Show>
          </aside>
          <Resizer getWidth={leftW} setWidth={setLeftWidth} onCommit={persistLeftWidth} />
          <main class="min-w-0 flex-1 overflow-hidden">
            {/* Switching modes unmounts the panels; the Changes state (checked
                files, selected changelist) lives in the store and survives. */}
            <Show when={isLog()} fallback={<DiffView />}>
              <LogView />
            </Show>
          </main>
        </div>

        <Show when={!isLog()}>
          <footer class="border-t border-border bg-bg-muted">
            <CommitPanel />
          </footer>
        </Show>
        <StatusBar />

        <ModalHost />
      </div>
    </ErrorBoundary>
  );
}
