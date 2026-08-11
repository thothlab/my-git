import { ErrorBoundary, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openRepo } from "./api";
import { busy, error, refresh, run } from "./store";
import Toolbar from "./components/Toolbar";
import ChangesView from "./components/ChangesView";
import DiffView from "./components/DiffView";
import CommitPanel from "./components/CommitPanel";
import StatusBar from "./components/StatusBar";
import { ModalHost } from "./components/Modals";

export default function App() {
  onMount(async () => {
    await run(openRepo("."));
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
          <div class="text-sm font-semibold text-danger">Что-то пошло не так в UI</div>
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
              Перечитать состояние
            </button>
            <button
              class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
              onClick={() => location.reload()}
            >
              Перезагрузить окно
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
          <aside class="w-72 shrink-0 border-r border-border">
            <ChangesView />
          </aside>
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
