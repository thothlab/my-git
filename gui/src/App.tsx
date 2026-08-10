import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openRepo } from "./api";
import { busy, error, refresh, run, state } from "./store";
import ChangesView from "./components/ChangesView";
import DiffView from "./components/DiffView";
import CommitPanel from "./components/CommitPanel";
import { ModalHost } from "./components/Modals";

type Theme = "auto" | "light" | "dark";

export default function App() {
  const [theme, setTheme] = createSignal<Theme>(
    (localStorage.getItem("theme") as Theme) || "auto",
  );
  createEffect(() => {
    const t = theme();
    const dark =
      t === "dark" ||
      (t === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("theme", t);
  });
  const cycleTheme = () =>
    setTheme((t) => (t === "auto" ? "light" : t === "light" ? "dark" : "auto"));

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
      <header class="flex items-center gap-3 border-b border-border bg-bg-muted px-3 py-1.5 text-sm">
        <span class="font-semibold">my-git</span>
        <Show when={state()}>
          {(s) => (
            <span class="flex items-center gap-2 text-fg-muted">
              <span class="rounded bg-bg px-1.5 py-0.5 font-mono text-xs">
                {s().detached ? "detached" : s().branch}
              </span>
              <Show when={s().upstream}>
                <span class="font-mono text-xs text-warn" title={s().upstream ?? ""}>
                  ↑{s().ahead} ↓{s().behind}
                </span>
              </Show>
            </span>
          )}
        </Show>

        <div class="ml-auto flex items-center gap-2">
          <button
            class="rounded border border-border px-2 py-0.5 text-xs hover:bg-bg"
            title="Тема: auto → light → dark"
            onClick={cycleTheme}
          >
            {theme()}
          </button>
          <button
            class="rounded border border-border px-2 py-0.5 text-xs hover:bg-bg"
            onClick={() => void refresh()}
            disabled={busy()}
          >
            {busy() ? "…" : "↻"}
          </button>
        </div>
      </header>

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
