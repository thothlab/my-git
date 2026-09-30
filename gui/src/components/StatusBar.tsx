import { Show, createResource } from "solid-js";
import { state } from "../store";
import { getVersion } from "@tauri-apps/api/app";
import { d } from "../i18n";
import { backgroundFetchError, backgroundFetchRunning } from "../backgroundFetch";

export default function StatusBar() {
  // Version from the bundle, never a literal: the hardcoded one here said
  // "0.1.2" two releases after that stopped being true.
  const [version] = createResource(() => getVersion());
  const total = () =>
    (state()?.changelists ?? []).reduce((n, c) => n + c.files.length, 0);

  return (
    <div class="flex items-center gap-3 border-t border-border bg-bg-muted px-3 py-2 text-xs text-fg-muted">
      <Show when={state()} fallback={<span>—</span>}>
        {(s) => (
          <>
            <span class="truncate font-mono" title={s().repoPath}>
              {s().repoPath}
            </span>
            {/* The last background fetch failed: one quiet line, never a banner
                per tick. The full message is in the tooltip. */}
            <Show when={backgroundFetchError()}>
              {(e) => (
                <span class="min-w-0 truncate text-warn" title={e().text}>
                  {d().bgFetchFailed(e().line)}
                </span>
              )}
            </Show>
            <Show when={backgroundFetchRunning()}>
              <span class="shrink-0" title={d().bgFetchRunningTip()}>
                {d().bgFetchRunning()}
              </span>
            </Show>
            <span class="ml-auto">{d().changesCount(total())}</span>
            <span class="text-fg-muted/70">Graft {version() ?? ""}</span>
          </>
        )}
      </Show>
    </div>
  );
}
