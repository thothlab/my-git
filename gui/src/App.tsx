import { ErrorBoundary, createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { busy, error, openInitial, refresh } from "./store";
import { d } from "./i18n";
import Toolbar from "./components/Toolbar";
import ChangesView from "./components/ChangesView";
import DiffView from "./components/DiffView";
import CommitPanel from "./components/CommitPanel";
import StatusBar from "./components/StatusBar";
import Resizer from "./components/Resizer";
import { ModalHost } from "./components/Modals";

const LEFT_WIDTH_KEY = "leftPanelWidth";
const MIN_LEFT = 220; // changes panel never narrower than this
const MIN_DIFF = 360; // preferred minimum for the diff panel

// Keep the left panel within [MIN_LEFT, innerWidth - MIN_DIFF] so a resized (or
// stale persisted) width can never collapse the diff panel after the window shrinks.
// Invariant: the window's minWidth (720, see tauri.conf.json) >= MIN_LEFT + MIN_DIFF
// (580), so both minimums are always satisfiable. If the window is ever forced
// narrower than that, MIN_LEFT wins (outer max) and the diff panel yields — the
// changes panel stays usable rather than both collapsing.
const clampLeft = (w: number) =>
  Math.round(Math.min(Math.max(w, MIN_LEFT), Math.max(MIN_LEFT, window.innerWidth - MIN_DIFF)));

export default function App() {
  const [leftW, setLeftW] = createSignal(
    clampLeft(Number(localStorage.getItem(LEFT_WIDTH_KEY)) || 288),
  );
  const setLeftWidth = (w: number) => setLeftW(clampLeft(w));
  const persistLeftWidth = () => localStorage.setItem(LEFT_WIDTH_KEY, String(leftW()));

  onMount(async () => {
    // re-clamp when the window is resized, so the diff panel keeps its minimum
    const onResize = () => setLeftW((w) => clampLeft(w));
    window.addEventListener("resize", onResize);
    onCleanup(() => window.removeEventListener("resize", onResize));

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

        <Show when={error()}>
          <pre class="max-h-32 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-xs text-danger">
            {error()}
          </pre>
        </Show>

        <div class="flex min-h-0 flex-1">
          <aside class="shrink-0 overflow-hidden" style={{ width: `${leftW()}px` }}>
            <ChangesView />
          </aside>
          <Resizer getWidth={leftW} setWidth={setLeftWidth} onCommit={persistLeftWidth} />
          <main class="min-w-0 flex-1 overflow-hidden">
            <DiffView />
          </main>
        </div>

        <footer class="border-t border-border bg-bg-muted">
          <CommitPanel />
        </footer>
        <StatusBar />

        <ModalHost />
      </div>
    </ErrorBoundary>
  );
}
