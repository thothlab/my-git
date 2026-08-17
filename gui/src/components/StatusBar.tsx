import { Show } from "solid-js";
import { state } from "../store";
import { d } from "../i18n";

export default function StatusBar() {
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
            <span class="ml-auto">{d().changesCount(total())}</span>
            <span class="text-fg-muted/70">my-git GUI 0.1.1</span>
          </>
        )}
      </Show>
    </div>
  );
}
