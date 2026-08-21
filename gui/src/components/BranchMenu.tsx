import { For, Show, createMemo, createSignal } from "solid-js";
import { branchCheckout, branchCreate, branchList, fetchRemote, type BranchInfo } from "../api";
import { busy, chooseOption, promptText, run, state } from "../store";
import { d } from "../i18n";
import { baseName, buildFileTree, type FileTreeNode } from "./pathTree";
import { IconFetch } from "./IconButton";

/**
 * The branch picker of the Changes toolbar.
 *
 * Modelled on the IDE popover it is meant to replace: a header row carrying the
 * filter field and the two things a reader reaches for while the list is open —
 * Fetch, and the display options — then the list itself.
 *
 * Two things worth knowing before changing it:
 *
 *  1. **The list is a snapshot taken when the popover opens.** So the in-popover
 *     Fetch has to re-read it; without that the button moves the remote-tracking
 *     refs and the rows on screen stay exactly as they were, which reads as a
 *     button that does nothing.
 *  2. **The display options are window state, not repository state.** They live
 *     in `localStorage` beside `showIgnored` and `groupByDir`, not in
 *     `.git/graft-ui.json`: that file is the *panel's* state, and this popover
 *     belongs to the toolbar of the window.
 *
 * Folding by prefix goes through `pathTree.ts`, unlike the Log mode's branch
 * tree — there is no collapse state and no interleaving of folders with rows
 * here, which is the whole of what made that tree keep its own copy.
 */

const OPT_KEY = "branchMenuOptions";
const RECENT_KEY = "recentBranches";
const RECENT_MAX = 8;

interface Options {
  groupByPrefix: boolean;
  showRemote: boolean;
  showRecent: boolean;
}

const readOptions = (): Options => {
  try {
    const raw = JSON.parse(localStorage.getItem(OPT_KEY) ?? "{}");
    return {
      groupByPrefix: raw.groupByPrefix ?? true,
      showRemote: raw.showRemote ?? true,
      showRecent: raw.showRecent ?? true,
    };
  } catch {
    // A broken preference is not worth a broken picker.
    return { groupByPrefix: true, showRemote: true, showRecent: true };
  }
};

/** Recently checked-out branches, per repository. Written on a checkout that
 * actually happened, so the section never offers a branch nobody switched to. */
const readRecent = (repo: string): string[] => {
  try {
    const all = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "{}");
    const list = all?.[repo];
    return Array.isArray(list) ? list.filter((x: unknown) => typeof x === "string") : [];
  } catch {
    return [];
  }
};

const noteRecent = (repo: string, name: string): void => {
  if (!repo || !name) return;
  let all: Record<string, string[]> = {};
  try {
    const parsed = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "{}");
    if (parsed && typeof parsed === "object") all = parsed;
  } catch {
    /* start over rather than lose the checkout */
  }
  const next = [name, ...readRecent(repo).filter((x) => x !== name)].slice(0, RECENT_MAX);
  all[repo] = next;
  localStorage.setItem(RECENT_KEY, JSON.stringify(all));
};

export default function BranchMenu() {
  const [open, setOpen] = createSignal(false);
  const [branches, setBranches] = createSignal<BranchInfo[]>([]);
  const [filter, setFilter] = createSignal("");
  const [options, setOptions] = createSignal<Options>(readOptions());
  const [settingsOpen, setSettingsOpen] = createSignal(false);
  const [recent, setRecent] = createSignal<string[]>([]);

  const current = () => (state()?.detached ? "detached" : state()?.branch ?? "—");
  const repo = () => state()?.repoPath ?? "";

  const reload = async () => {
    try {
      setBranches(await branchList());
    } catch {
      /* error surfaces via run() elsewhere */
    }
  };

  const toggle = async () => {
    if (!open()) {
      await reload();
      setFilter("");
      setRecent(readRecent(repo()));
      setSettingsOpen(false);
    }
    setOpen((v) => !v);
  };

  const setOption = (patch: Partial<Options>) => {
    const next = { ...options(), ...patch };
    setOptions(next);
    localStorage.setItem(OPT_KEY, JSON.stringify(next));
  };

  const dirty = () =>
    (state()?.changelists ?? []).some((c) => !c.isUnversioned && c.files.length > 0);

  const doCheckout = async (b: BranchInfo) => {
    setOpen(false);
    const target = b.isRemote ? b.name.split("/").slice(1).join("/") : b.name;
    let stash = false;
    if (dirty()) {
      const k = await chooseOption(d().switchDirty(target), [
        { key: "stash", label: d().stashAndSwitch() },
        { key: "switch", label: d().switchAsIs() },
        { key: "cancel", label: d().cancel() },
      ]);
      if (!k || k === "cancel") return;
      stash = k === "stash";
    }
    // Read before the await: `repo()` is a store signal.
    const at = repo();
    await run(branchCheckout(target, stash));
    if (state()?.branch === target) noteRecent(at, target);
  };

  const newBranch = async () => {
    setOpen(false);
    const name = await promptText(d().newBranchFromHead(), "");
    if (name && name.trim()) await run(branchCreate(name.trim()));
  };

  const doFetch = async () => {
    await run(fetchRemote(), d().fetching());
    // The snapshot above is now stale: the counters and the remote-tracking refs
    // are exactly what the fetch moved.
    await reload();
  };

  const shown = () => {
    const f = filter().toLowerCase();
    return branches().filter((b) => b.name.toLowerCase().includes(f));
  };

  const recentShown = createMemo(() => {
    if (!options().showRecent) return [];
    const byName = new Map(shown().map((b) => [b.name, b] as const));
    return recent()
      .map((n) => byName.get(n))
      .filter((b): b is BranchInfo => !!b && !b.isCurrent);
  });

  return (
    <div class="relative">
      <button
        class="rounded border border-border bg-bg px-1.5 py-0.5 font-mono text-xs hover:bg-bg-muted"
        onClick={() => void toggle()}
      >
        ⎇ {current()} ▾
      </button>
      <Show when={open()}>
        <>
          <div class="fixed inset-0 z-20" onClick={() => setOpen(false)} />
          <div class="absolute left-0 top-full z-30 mt-1 w-80 rounded-md border border-border bg-bg text-xs shadow-lg">
            {/* Header: filter, Fetch, display options. Outside the scrolling
                list on purpose — it must stay reachable however far the list of
                branches is scrolled. */}
            <div class="flex items-center gap-1 border-b border-border p-1">
              <input
                class="min-w-0 flex-1 rounded border border-border bg-bg-muted px-1.5 py-0.5 outline-none focus:border-accent"
                placeholder={d().filterBranches()}
                value={filter()}
                onInput={(e) => setFilter(e.currentTarget.value)}
              />
              <button
                class="shrink-0 rounded border border-border p-1 text-fg-subtle hover:bg-bg-muted hover:text-fg disabled:cursor-not-allowed disabled:opacity-60"
                title={d().fetchPruneTip()}
                disabled={busy()}
                onClick={() => void doFetch()}
              >
                <IconFetch />
              </button>
              <div class="relative shrink-0">
                <button
                  class="rounded border border-border p-1 text-fg-subtle hover:bg-bg-muted hover:text-fg"
                  title={d().branchMenuOptionsTip()}
                  aria-haspopup="menu"
                  aria-expanded={settingsOpen()}
                  onClick={() => setSettingsOpen((v) => !v)}
                >
                  <GearIcon />
                </button>
                <Show when={settingsOpen()}>
                  <div class="absolute right-0 top-full z-40 mt-1 w-60 rounded-md border border-border bg-bg py-1 shadow-lg">
                    <OptionItem
                      label={d().optGroupByPrefix()}
                      on={options().groupByPrefix}
                      onClick={() => setOption({ groupByPrefix: !options().groupByPrefix })}
                    />
                    <OptionItem
                      label={d().optShowRemote()}
                      on={options().showRemote}
                      onClick={() => setOption({ showRemote: !options().showRemote })}
                    />
                    <OptionItem
                      label={d().optShowRecent()}
                      on={options().showRecent}
                      onClick={() => setOption({ showRecent: !options().showRecent })}
                    />
                  </div>
                </Show>
              </div>
            </div>

            {/* Any move into the list dismisses the options menu: it is a
                popover inside a popover, and the outer backdrop cannot see it. */}
            <div class="max-h-96 overflow-auto py-1" onClick={() => setSettingsOpen(false)}>
              <button
                class="block w-full px-3 py-1 text-left text-accent hover:bg-bg-muted"
                onClick={() => void newBranch()}
              >
                ＋ {d().newBranchItem()}
              </button>
              <Show when={recentShown().length > 0}>
                {/* Recent is flat whatever the grouping says: the point of it is
                    the last few branches in the order they were used, and a
                    folder tree would put them back in alphabetical order. */}
                <SectionTitle title={d().recentBranches()} />
                <For each={recentShown()}>
                  {(b) => <BranchRow branch={b} depth={0} label={b.name} onPick={doCheckout} />}
                </For>
              </Show>
              <Section
                title={d().local()}
                items={shown().filter((b) => !b.isRemote)}
                grouped={options().groupByPrefix}
                onPick={doCheckout}
              />
              <Show when={options().showRemote}>
                <Section
                  title={d().remote()}
                  items={shown().filter((b) => b.isRemote)}
                  grouped={options().groupByPrefix}
                  onPick={doCheckout}
                />
              </Show>
            </div>
          </div>
        </>
      </Show>
    </div>
  );
}

function OptionItem(props: { label: string; on: boolean; onClick: () => void }) {
  return (
    <button
      class="flex w-full items-center gap-2 px-2 py-1 text-left hover:bg-bg-muted"
      role="menuitemcheckbox"
      aria-checked={props.on}
      onClick={props.onClick}
    >
      <span class="w-3 shrink-0 text-accent">{props.on ? "✓" : ""}</span>
      <span class="truncate">{props.label}</span>
    </button>
  );
}

function SectionTitle(props: { title: string }) {
  return (
    <div class="mt-1 border-t border-border px-3 py-0.5 text-[10px] uppercase text-fg-muted">
      {props.title}
    </div>
  );
}

function Section(props: {
  title: string;
  items: BranchInfo[];
  grouped: boolean;
  onPick: (b: BranchInfo) => void;
}) {
  return (
    <Show when={props.items.length > 0}>
      <SectionTitle title={props.title} />
      <Show
        when={props.grouped}
        fallback={
          <For each={props.items}>
            {(b) => <BranchRow branch={b} depth={0} label={b.name} onPick={props.onPick} />}
          </For>
        }
      >
        <Folder
          node={buildFileTree(props.items.map((b) => ({ path: b.name, branch: b })))}
          depth={0}
          onPick={props.onPick}
        />
      </Show>
    </Show>
  );
}

type Leaf = { path: string; branch: BranchInfo };

/** Always expanded: a popover has no room for a fold state and nothing to
 * persist it in. Folders are structure here, not a control. */
function Folder(props: { node: FileTreeNode<Leaf>; depth: number; onPick: (b: BranchInfo) => void }) {
  const dirs = () => [...props.node.dirs].sort((a, b) => a.name.localeCompare(b.name));
  const files = () => [...props.node.files].sort((a, b) => a.path.localeCompare(b.path));
  return (
    <>
      <For each={dirs()}>
        {(dir) => (
          <>
            <div
              class="truncate py-0.5 pr-3 text-fg-muted"
              style={{ "padding-left": `${12 + props.depth * 12}px` }}
              title={dir.path}
            >
              {dir.name}/
            </div>
            <Folder node={dir} depth={props.depth + 1} onPick={props.onPick} />
          </>
        )}
      </For>
      <For each={files()}>
        {(f) => (
          <BranchRow
            branch={f.branch}
            depth={props.depth}
            label={baseName(f.path)}
            onPick={props.onPick}
          />
        )}
      </For>
    </>
  );
}

function BranchRow(props: {
  branch: BranchInfo;
  depth: number;
  label: string;
  onPick: (b: BranchInfo) => void;
}) {
  return (
    <button
      class="block w-full truncate py-1 pr-3 text-left font-mono hover:bg-bg-muted"
      classList={{ "text-accent": props.branch.isCurrent }}
      style={{ "padding-left": `${12 + props.depth * 12}px` }}
      title={props.branch.name}
      onClick={() => props.onPick(props.branch)}
    >
      {props.branch.isCurrent ? "● " : "  "}
      {props.label}
    </button>
  );
}

function GearIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9v.09a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}
