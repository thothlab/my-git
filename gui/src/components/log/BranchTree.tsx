import { For, Show, createMemo, createSignal } from "solid-js";
import { d } from "../../i18n";
import { state } from "../../store";
import { PanelBtn, PanelChrome, PanelNote } from "./PanelChrome";

/**
 * Branch tree panel — shell only. The tree itself (folders, favourites,
 * ahead/behind, context menu) is task 07; this file owns the panel frame, the
 * keyboard selection and the named states.
 *
 * `undefined` means "not loaded yet", `[]` means "loaded and empty": the empty
 * state must never be inferred from the absence of data, or a repository with
 * remotes would be told it has none while the list is still loading.
 */
// TODO(prd): task 07 replaces these with the real branch lists (branch_tree).
const localBranches = (): string[] | undefined => undefined;
const remoteBranches = (): string[] | undefined => undefined;

interface Row {
  key: string;
  label: string;
  head?: boolean;
}

export default function BranchTree() {
  const [selected, setSelected] = createSignal(0);

  // The one row we can state truthfully without the history backend.
  const headRow = createMemo<Row[]>(() => {
    const s = state();
    if (!s) return [];
    return [
      {
        key: "head",
        label: s.detached ? d().detachedHead(s.branch) : d().onBranch(s.branch),
        head: true,
      },
    ];
  });

  const rows = createMemo<Row[]>(() => [
    ...headRow(),
    ...(localBranches() ?? []).map((b) => ({ key: `l/${b}`, label: b })),
    ...(remoteBranches() ?? []).map((b) => ({ key: `r/${b}`, label: b })),
  ]);

  const clamp = (i: number) => Math.max(0, Math.min(i, rows().length - 1));
  const move = (delta: number) => setSelected((i) => clamp(i + delta));

  return (
    <PanelChrome
      id="branches"
      title={d().branchesTitle()}
      handlers={{
        moveSelection: move,
        moveToEdge: (e) => setSelected(e === -1 ? 0 : clamp(rows().length - 1)),
      }}
      toolbar={
        <>
          <PanelBtn label="⟳" tip={d().refreshTip()} disabled disabledTip={d().actionPending()} />
          <PanelBtn label="▾" tip={d().expandAllTip()} disabled disabledTip={d().actionPending()} />
          <PanelBtn label="▸" tip={d().collapseAllTip()} disabled disabledTip={d().actionPending()} />
          <PanelBtn label="★" tip={d().favoritesOnlyTip()} disabled disabledTip={d().actionPending()} />
          <PanelBtn label="+" tip={d().newBranchTip()} disabled disabledTip={d().actionPending()} />
        </>
      }
    >
      <div class="py-1 text-xs">
        <Section title={d().local()}>
          <Show when={rows().length > 0} fallback={<PanelNote title={d().noBranchesYet()} />}>
            <For each={rows()}>
              {(row, i) => (
                <div
                  class="cursor-default truncate px-2 py-0.5 font-mono"
                  classList={{
                    "bg-accent/20 text-fg": selected() === i(),
                    "text-fg-muted": selected() !== i(),
                    "font-semibold": row.head,
                  }}
                  onClick={() => setSelected(i())}
                >
                  {row.label}
                </div>
              )}
            </For>
          </Show>
          <Show when={localBranches() === undefined}>
            <Pending />
          </Show>
        </Section>

        <Section title={d().remote()}>
          {/* remoteBranches() === undefined → unknown, not "none". */}
          <Show when={remoteBranches() !== undefined} fallback={<Pending />}>
            <Show
              when={(remoteBranches() ?? []).length > 0}
              fallback={<PanelNote title={d().noRemoteBranches()} />}
            >
              <For each={remoteBranches() ?? []}>
                {(b) => <div class="truncate px-2 py-0.5 font-mono text-fg-muted">{b}</div>}
              </For>
            </Show>
          </Show>
        </Section>

        <Section title={d().favorites()}>
          <Pending />
        </Section>
      </div>
    </PanelChrome>
  );
}

function Section(props: { title: string; children: any }) {
  return (
    <div class="mb-1">
      <div class="px-2 py-0.5 text-[10px] uppercase tracking-wide text-fg-subtle">
        {props.title}
      </div>
      {props.children}
    </div>
  );
}

/** Visible "not wired up yet" marker — an honest stub, not invented content. */
function Pending() {
  return <div class="px-2 py-0.5 text-fg-subtle">{d().historyPending()}</div>;
}
