import { For, Show, createMemo, createSignal } from "solid-js";
import { d } from "../../i18n";
import { PanelBtn, PanelChrome, PanelNote } from "./PanelChrome";

/**
 * Commit list panel — shell only. Columns, the graph layer and virtualisation
 * are task 08; this file owns the frame, the filter field and the keyboard.
 *
 * `undefined` = history not requested yet (the Log mode has just been opened and
 * there is no backend behind it), `[]` = the repository genuinely has no
 * commits. The two states read differently to the user, so they are not merged.
 */
export interface LogRow {
  hash: string;
  subject: string;
}
// TODO(prd): task 08 replaces this with logStore.commits().
const commits = (): LogRow[] | undefined => undefined;

export default function LogTable(props: { onSelect?: (hash: string | null) => void }) {
  const [filter, setFilter] = createSignal("");
  const [selected, setSelected] = createSignal(0);
  const rows = createMemo<LogRow[]>(() => commits() ?? []);

  const clamp = (i: number) => Math.max(0, Math.min(i, rows().length - 1));
  const select = (i: number) => {
    const n = clamp(i);
    setSelected(n);
    props.onSelect?.(rows()[n]?.hash ?? null);
  };

  return (
    <PanelChrome
      id="commits"
      title={d().logTitle()}
      handlers={{
        moveSelection: (delta) => select(selected() + delta),
        moveToEdge: (edge) => select(edge === -1 ? 0 : rows().length - 1),
      }}
      toolbar={
        <>
          <input
            class="w-40 rounded border border-border bg-bg px-1.5 py-0.5 text-xs outline-none focus:border-accent"
            placeholder={d().filterCommits()}
            value={filter()}
            onInput={(e) => setFilter(e.currentTarget.value)}
          />
          <PanelBtn label="⟳" tip={d().refreshTip()} disabled disabledTip={d().actionPending()} />
          <PanelBtn label="⋯" tip={d().viewOptionsTip()} disabled disabledTip={d().actionPending()} />
        </>
      }
    >
      <Show
        when={commits() !== undefined}
        fallback={<PanelNote title={d().historyPending()} hint={d().focusHint()} />}
      >
        <Show
          when={rows().length > 0}
          fallback={<PanelNote title={d().noCommitsTitle()} hint={d().noCommitsHint()} />}
        >
          <For each={rows()}>
            {(row, i) => (
              <div
                class="flex h-[22px] cursor-default items-center gap-2 px-2 font-mono text-xs"
                classList={{
                  "bg-accent/20 text-fg": selected() === i(),
                  "text-fg-muted": selected() !== i(),
                }}
                onClick={() => select(i())}
              >
                <span class="text-fg-subtle">{row.hash.slice(0, 7)}</span>
                <span class="truncate">{row.subject}</span>
              </div>
            )}
          </For>
        </Show>
      </Show>
    </PanelChrome>
  );
}
