import { createSignal, onMount, onCleanup, For, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openRepo, repoState, errText, type RepoState } from "./api";

// task_01 shell: proves the IPC pipeline (open repo → status → grouped files) and
// the theme tokens. Real ChangesView/DiffView/CommitPanel land in later tasks.
export default function App() {
  const [state, setState] = createSignal<RepoState | null>(null);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const refresh = async () => {
    setBusy(true);
    try {
      setState(await repoState());
      setError("");
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  onMount(async () => {
    // default to the repo containing the working directory; a picker comes later
    try {
      setState(await openRepo("."));
      setError("");
    } catch (e) {
      setError(errText(e));
    }
    // resync when the window regains focus (external git activity between sessions)
    const unlisten = await getCurrentWindow().onFocusChanged(({ payload }) => {
      if (payload) void refresh();
    });
    onCleanup(unlisten);
  });

  return (
    <div class="flex h-full flex-col bg-bg text-fg">
      <header class="flex items-center gap-3 border-b border-border bg-bg-muted px-3 py-2">
        <span class="font-semibold">my-git GUI</span>
        <Show when={state()}>
          {(s) => (
            <span class="text-fg-muted">
              {s().branch}
              <Show when={s().upstream}>
                {" "}
                <span class="text-warn">
                  ↑{s().ahead} ↓{s().behind}
                </span>
              </Show>
            </span>
          )}
        </Show>
        <button
          class="ml-auto rounded border border-border px-2 py-0.5 hover:bg-bg"
          onClick={() => void refresh()}
          disabled={busy()}
        >
          {busy() ? "…" : "Refresh"}
        </button>
      </header>

      <Show when={error()}>
        <pre class="whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-xs text-danger">
          {error()}
        </pre>
      </Show>

      <main class="flex-1 overflow-auto p-3">
        <Show when={state()} fallback={<div class="text-fg-muted">Открываю репозиторий…</div>}>
          {(s) => (
            <For each={s().changelists}>
              {(cl) => (
                <div class="mb-3">
                  <div class="mb-1 font-semibold">
                    {cl.name} <span class="text-fg-muted">({cl.files.length})</span>
                  </div>
                  <For each={cl.files}>
                    {(f) => (
                      <div class="pl-3 font-mono text-xs">
                        <span class="mr-2 uppercase text-fg-muted">{f.status[0]}</span>
                        {f.path}
                      </div>
                    )}
                  </For>
                </div>
              )}
            </For>
          )}
        </Show>
      </main>
    </div>
  );
}
