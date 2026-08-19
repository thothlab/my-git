import { For, Show, createMemo, createSignal } from "solid-js";
import { d } from "../../i18n";
import { PanelChrome, PanelNote } from "./PanelChrome";

/**
 * Commit card + changed files — shell only. The card, the file tree and
 * "show all branches" are task 09; this file owns the frame and the keyboard.
 */
export interface CommitFileRow {
  path: string;
}
// TODO(prd): task 09 replaces this with commit_files for the selected commit.
const files = (): CommitFileRow[] | undefined => undefined;

export default function CommitDetailsPane(props: { selected: () => string | null }) {
  const [row, setRow] = createSignal(0);
  const rows = createMemo<CommitFileRow[]>(() => files() ?? []);
  const clamp = (i: number) => Math.max(0, Math.min(i, rows().length - 1));

  return (
    <PanelChrome
      id="details"
      title={d().commitDetailsTitle()}
      handlers={{
        moveSelection: (delta) => setRow((i) => clamp(i + delta)),
        moveToEdge: (edge) => setRow(edge === -1 ? 0 : clamp(rows().length - 1)),
      }}
    >
      <Show
        when={props.selected()}
        fallback={<PanelNote title={d().selectCommitHint()} />}
      >
        <div class="p-2 text-xs">
          <div class="mb-2 font-mono text-fg-muted">{props.selected()}</div>
          <div class="mb-1 text-[10px] uppercase tracking-wide text-fg-subtle">
            {d().changedFiles()}
          </div>
          <Show
            when={files() !== undefined}
            fallback={<div class="text-fg-subtle">{d().historyPending()}</div>}
          >
            <For each={rows()}>
              {(f, i) => (
                <div
                  class="truncate px-1 py-0.5 font-mono"
                  classList={{ "bg-accent/20": row() === i() }}
                  onClick={() => setRow(i())}
                >
                  {f.path}
                </div>
              )}
            </For>
          </Show>
        </div>
      </Show>
    </PanelChrome>
  );
}
