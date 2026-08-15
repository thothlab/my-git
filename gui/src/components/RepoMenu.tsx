import { For, Show, createSignal } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { openRepoAt, recentRepos, state } from "../store";
import { d } from "../i18n";

const baseName = (p: string) => p.replace(/\/+$/, "").split("/").pop() || p;

// Deterministic accent colour from the repo name — the coloured letter-avatar
// mirrors Android Studio's project switcher.
function avatarColor(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) % 360;
  return `hsl(${h} 55% 45%)`;
}

function Avatar(props: { name: string }) {
  return (
    <span
      class="flex h-4 w-4 shrink-0 items-center justify-center rounded text-[10px] font-bold uppercase text-white"
      style={{ "background-color": avatarColor(props.name) }}
    >
      {props.name.slice(0, 1)}
    </span>
  );
}

export default function RepoMenu() {
  const [menuOpen, setMenuOpen] = createSignal(false);

  const current = () => {
    const p = state()?.repoPath;
    return p ? baseName(p) : null;
  };
  const others = () => recentRepos().filter((p) => p !== state()?.repoPath);

  const pick = async () => {
    setMenuOpen(false);
    const dir = await open({ directory: true, title: d().openRepoTitle() });
    if (typeof dir === "string") await openRepoAt(dir);
  };
  const openRecent = (p: string) => {
    setMenuOpen(false);
    void openRepoAt(p);
  };

  return (
    <div class="relative">
      <button
        class="flex items-center gap-1.5 rounded border border-border bg-bg px-1.5 py-0.5 text-xs hover:bg-bg-muted"
        onClick={() => setMenuOpen((v) => !v)}
        title={state()?.repoPath ?? d().openRepoTitle()}
      >
        <Show
          when={current()}
          fallback={<span class="text-fg-muted">{d().noRepository()}</span>}
        >
          <Avatar name={current()!} />
          <span class="max-w-[10rem] truncate font-medium">{current()}</span>
        </Show>
        <span class="text-fg-muted">▾</span>
      </button>

      <Show when={menuOpen()}>
        <>
          <div class="fixed inset-0 z-20" onClick={() => setMenuOpen(false)} />
          <div class="absolute left-0 top-full z-30 mt-1 max-h-96 w-80 overflow-auto rounded-md border border-border bg-bg py-1 text-xs shadow-lg">
            <button
              class="block w-full px-3 py-1.5 text-left hover:bg-bg-muted"
              onClick={() => void pick()}
            >
              {d().openRepoBtn()}
            </button>

            <Show when={others().length > 0}>
              <div class="mt-1 border-t border-border px-3 py-0.5 text-[10px] uppercase text-fg-muted">
                {d().recentProjects()}
              </div>
              <For each={others()}>
                {(p) => (
                  <button
                    class="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-bg-muted"
                    onClick={() => openRecent(p)}
                    title={p}
                  >
                    <Avatar name={baseName(p)} />
                    <span class="min-w-0 flex-1">
                      <div class="truncate font-medium">{baseName(p)}</div>
                      <div class="truncate text-[10px] text-fg-muted">{p}</div>
                    </span>
                  </button>
                )}
              </For>
            </Show>
          </div>
        </>
      </Show>
    </div>
  );
}
