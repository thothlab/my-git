import { onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openRepo } from "./api";
import { error, refresh, run } from "./store";
import Toolbar from "./components/Toolbar";
import ChangesView from "./components/ChangesView";
import DiffView from "./components/DiffView";
import CommitPanel from "./components/CommitPanel";
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

  return (
    <div class="flex h-full flex-col bg-bg text-fg">
      <Toolbar />

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

      <ModalHost />
    </div>
  );
}
