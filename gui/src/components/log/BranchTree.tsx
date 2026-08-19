import { For, Show, createEffect, createMemo, createResource, createSignal } from "solid-js";
import {
  branchTree,
  errText,
  fetchRemote,
  uiStateGet,
  uiStateSet,
  type BranchNode,
  type UiState,
} from "../../api";
import { d } from "../../i18n";
import { busy, run, setError, state } from "../../store";
import { PanelBtn, PanelChrome, PanelNote } from "./PanelChrome";
import { setSelectedBranch } from "./branchSelection";

/**
 * Branch tree panel: HEAD on top, then Local and Remote groups, branch names
 * split into collapsible folders, favourites first inside their group,
 * ahead/behind for tracking branches, and a filter field.
 *
 * Three seams worth knowing before changing anything here:
 *
 *  1. **A row is assembled from two sources.** `branch_tree()` always reports
 *     `isFavorite: false` — favourites and collapsed folders live in
 *     `.git/graft-ui.json` and arrive through `ui_state_get()`. Neither source
 *     alone can draw a row.
 *  2. **`undefined` is not "empty".** The panel distinguishes failed, loading
 *     and loaded-empty; a failed `branch_tree()` leaves a resource `undefined`
 *     forever, so an empty state gated on "is there data" would turn a git
 *     error into the claim that the repository has no branches.
 *  3. **The filter expands folders by derivation, never by writing.** Effective
 *     collapse = persisted collapse minus what the filter reveals. Persisting
 *     the expansion would leave every folder open once the filter is cleared.
 */

/** `full_ref` of the synthetic detached-HEAD node (engine `branches::DETACHED_REF`). */
const DETACHED_REF = "HEAD";

type Group = "local" | "remote";

interface FolderNode {
  kind: "folder";
  /** Last segment(s) of the path — a compacted chain keeps its `a/b` shape. */
  name: string;
  /** Full segment path inside the group, e.g. `p2p/bugfix`. */
  path: string;
  children: TreeNode[];
}
interface BranchLeaf {
  kind: "branch";
  name: string;
  node: BranchNode;
  favorite: boolean;
}
type TreeNode = FolderNode | BranchLeaf;

interface Row {
  key: string;
  kind: "head" | "folder" | "branch";
  label: string;
  depth: number;
  group?: Group;
  folderKey?: string;
  collapsed?: boolean;
  node?: BranchNode;
  favorite?: boolean;
}

/** Favourites are keyed by full ref: local `foo` and `origin/foo` are not one branch. */
const favKey = (b: BranchNode) => b.fullRef;
/** Collapsed folders are keyed per group: `p2p` under Local and under Remote collapse apart. */
const folderKey = (group: Group, path: string) => `${group}:${path}`;

export default function BranchTree() {
  // Sourced on the repository path: opening another repository while the Log
  // mode is on screen must not leave the previous repo's branches rendered.
  const [branches, { refetch: refetchBranches }] = createResource(
    () => state()?.repoPath,
    () => branchTree(),
  );
  const [ui, { mutate: mutateUi, refetch: refetchUi }] = createResource(
    () => state()?.repoPath,
    () => uiStateGet(),
  );
  const [filter, setFilter] = createSignal("");
  const [favOnly, setFavOnly] = createSignal(false);
  const [selectedKey, setSelectedKey] = createSignal<string>("head");

  const favorites = createMemo(() => new Set(ui()?.favorites ?? []));
  const collapsed = createMemo(() => new Set(ui()?.collapsedFolders ?? []));

  /**
   * Write through to `.git/graft-ui.json`. Not routed via `run()`: `ui_state_set`
   * answers with a `UiState`, and `run()` would install it as the repository
   * state. The optimistic mutate keeps the click instant; a rejected write puts
   * the file's own truth back rather than leaving the UI lying.
   */
  const patchUi = async (patch: Partial<UiState>) => {
    const base = ui();
    if (!base) return;
    const next: UiState = { ...base, ...patch };
    mutateUi(next);
    try {
      mutateUi(await uiStateSet(next));
    } catch (e) {
      setError(errText(e));
      refetchUi();
    }
  };

  const toggleFavorite = (b: BranchNode) => {
    const key = favKey(b);
    const list = ui()?.favorites ?? [];
    const next = list.includes(key) ? list.filter((x) => x !== key) : [...list, key];
    void patchUi({ favorites: next });
  };

  const toggleFolder = (key: string) => {
    const list = ui()?.collapsedFolders ?? [];
    const next = list.includes(key) ? list.filter((x) => x !== key) : [...list, key];
    void patchUi({ collapsedFolders: next });
  };

  const setFolder = (key: string, collapse: boolean) => {
    const list = ui()?.collapsedFolders ?? [];
    if (collapse === list.includes(key)) return;
    void patchUi({
      collapsedFolders: collapse ? [...list, key] : list.filter((x) => x !== key),
    });
  };

  const headNode = createMemo(() => branches()?.find((b) => b.fullRef === DETACHED_REF));

  const groupTree = (group: Group) => {
    const list = (branches() ?? []).filter((b) =>
      group === "remote" ? b.isRemote : !b.isRemote && b.fullRef !== DETACHED_REF,
    );
    const picked = list.filter((b) => (favOnly() ? favorites().has(favKey(b)) : true));
    const needle = filter().trim().toLowerCase();
    const matched = needle ? picked.filter((b) => b.name.toLowerCase().includes(needle)) : picked;
    return sortTree(buildTree(matched, favorites()));
  };

  const localTree = createMemo(() => groupTree("local"));
  const remoteTree = createMemo(() => groupTree("remote"));

  /**
   * A filter that hides everything else also has to reveal what is left: with a
   * needle typed, folders ignore their persisted collapse, because every folder
   * still standing contains a match by construction.
   */
  const narrowed = createMemo(() => filter().trim() !== "" || favOnly());
  const expandAll = narrowed;

  const rowsOf = (group: Group, tree: TreeNode[]): Row[] => {
    const out: Row[] = [];
    const walk = (nodes: TreeNode[], depth: number) => {
      for (const n of nodes) {
        if (n.kind === "branch") {
          out.push({
            key: `b:${n.node.fullRef}`,
            kind: "branch",
            label: n.name,
            depth,
            group,
            node: n.node,
            favorite: n.favorite,
          });
          continue;
        }
        const key = folderKey(group, n.path);
        const isCollapsed = !expandAll() && collapsed().has(key);
        out.push({
          key: `f:${key}`,
          kind: "folder",
          label: n.name,
          depth,
          group,
          folderKey: key,
          collapsed: isCollapsed,
        });
        if (!isCollapsed) walk(n.children, depth + 1);
      }
    };
    walk(tree, 0);
    return out;
  };

  const localRows = createMemo(() => rowsOf("local", localTree()));
  const remoteRows = createMemo(() => rowsOf("remote", remoteTree()));

  const headRow = createMemo<Row[]>(() => {
    const s = state();
    const det = headNode();
    if (det) {
      return [{ key: "head", kind: "head", label: d().detachedHead(det.name), depth: 0 }];
    }
    if (!s) return [];
    return [{ key: "head", kind: "head", label: d().onBranch(s.branch), depth: 0 }];
  });

  /** Flat order for the keyboard only — each section renders its own slice. */
  const rows = createMemo<Row[]>(() => [...headRow(), ...localRows(), ...remoteRows()]);

  // A change of repository drops the selection back to HEAD. Keeping the row
  // selected by key alone would carry a branch name into a repository that need
  // not have it, and the log — which takes its scope from this selection — would
  // ask for the history of a branch that is not there.
  let lastRepoPath: string | null = null;
  createEffect(() => {
    const repo = state()?.repoPath ?? null;
    if (repo === lastRepoPath) return;
    lastRepoPath = repo;
    setSelectedKey("head");
  });

  const current = createMemo(() => rows().find((r) => r.key === selectedKey()));

  // Selection drives the log (R15i): a branch row scopes it, HEAD clears the
  // scope. Read by the log store through `branchSelection.ts`.
  createEffect(() => {
    const r = current();
    if (!r) return;
    setSelectedBranch(r.kind === "branch" ? (r.node?.name ?? null) : null);
  });

  const move = (delta: number) => {
    const list = rows();
    if (list.length === 0) return;
    const i = list.findIndex((r) => r.key === selectedKey());
    const next = Math.max(0, Math.min((i < 0 ? 0 : i) + delta, list.length - 1));
    setSelectedKey(list[next].key);
  };

  const activate = () => {
    const r = current();
    if (r?.kind === "folder" && r.folderKey) toggleFolder(r.folderKey);
  };

  /**
   * Panel-scoped keys. Cmd/Ctrl+D is deliberately not a `registerHotkey`: that
   * one is global and throws on a duplicate combination, while favouriting only
   * means anything for the row this panel has selected.
   */
  const onKey = (e: KeyboardEvent): boolean => {
    const r = current();
    if ((e.metaKey || e.ctrlKey) && e.code === "KeyD") {
      if (r?.kind === "branch" && r.node) toggleFavorite(r.node);
      return true;
    }
    if (e.metaKey || e.ctrlKey) return false;
    if (e.code === "ArrowRight" && r?.kind === "folder" && r.folderKey) {
      setFolder(r.folderKey, false);
      return true;
    }
    if (e.code === "ArrowLeft" && r?.kind === "folder" && r.folderKey) {
      setFolder(r.folderKey, true);
      return true;
    }
    return false;
  };

  const allFolderKeys = createMemo(() => {
    const keys: string[] = [];
    const walk = (group: Group, nodes: TreeNode[]) => {
      for (const n of nodes) {
        if (n.kind !== "folder") continue;
        keys.push(folderKey(group, n.path));
        walk(group, n.children);
      }
    };
    walk("local", localTree());
    walk("remote", remoteTree());
    return keys;
  });

  const collapseAll = () => {
    const keys = allFolderKeys();
    const list = ui()?.collapsedFolders ?? [];
    void patchUi({ collapsedFolders: [...new Set([...list, ...keys])] });
  };
  const expandAllFolders = () => {
    const keys = new Set(allFolderKeys());
    void patchUi({ collapsedFolders: (ui()?.collapsedFolders ?? []).filter((k) => !keys.has(k)) });
  };

  const failed = () => branches.error ?? ui.error;
  const loaded = () => branches() !== undefined && ui() !== undefined;
  /** No HEAD node and no refs at all — a repository whose first commit is missing. */
  const noCommits = () => loaded() && (branches() ?? []).length === 0 && !state()?.detached;
  const nothingMatched = () =>
    loaded() &&
    (branches() ?? []).length > 0 &&
    localRows().length === 0 &&
    remoteRows().length === 0 &&
    narrowed();

  const refreshAll = () => {
    refetchBranches();
    refetchUi();
  };

  /**
   * Fetch, then re-read the tree: ahead/behind come from `%(upstream:track)`,
   * so the counters only move once the remote-tracking refs have been updated
   * (R11i.1). `run()` owns the busy label, so the wait is visible.
   */
  const fetchAndRefresh = async () => {
    await run(fetchRemote(), d().fetching());
    refetchBranches();
  };

  return (
    <PanelChrome
      id="branches"
      title={d().branchesTitle()}
      handlers={{
        moveSelection: move,
        moveToEdge: (e) => {
          const list = rows();
          if (list.length > 0) setSelectedKey(list[e === -1 ? 0 : list.length - 1].key);
        },
        activate,
        onKey,
      }}
      toolbar={
        <>
          <PanelBtn label="⟳" tip={d().refreshTip()} onClick={refreshAll} />
          <PanelBtn
            label="↓"
            tip={d().fetchPruneTip()}
            disabled={busy()}
            disabledTip={d().fetching()}
            onClick={() => void fetchAndRefresh()}
          />
          <PanelBtn label="▾" tip={d().expandAllTip()} onClick={expandAllFolders} />
          <PanelBtn label="▸" tip={d().collapseAllTip()} onClick={collapseAll} />
          <PanelBtn
            label={favOnly() ? "★" : "☆"}
            tip={d().favoritesOnlyTip()}
            onClick={() => setFavOnly((v) => !v)}
          />
          <PanelBtn label="+" tip={d().newBranchTip()} disabled disabledTip={d().actionPending()} />
        </>
      }
    >
      <div class="flex h-full min-h-0 flex-col text-xs">
        <div class="shrink-0 border-b border-border p-1">
          <input
            class="w-full rounded border border-border bg-bg-subtle px-1.5 py-0.5 text-xs text-fg outline-none placeholder:text-fg-subtle focus:border-accent"
            placeholder={d().filterBranches()}
            value={filter()}
            onInput={(e) => setFilter(e.currentTarget.value)}
          />
        </div>

        <Show
          when={!failed()}
          fallback={<PanelNote title={d().branchesFailed()} hint={errText(failed())} />}
        >
          <Show when={loaded()} fallback={<PanelNote title={d().loadingHistory()} />}>
            <Show
              when={!noCommits()}
              fallback={<PanelNote title={d().noCommitsTitle()} hint={d().noCommitsHint()} />}
            >
              <Show when={!nothingMatched()} fallback={<PanelNote title={d().noMatches()} />}>
                <div class="min-h-0 flex-1 overflow-auto py-1">
                  <For each={headRow()}>
                    {(row) => (
                      <RowView
                        row={row}
                        selected={selectedKey() === row.key}
                        onSelect={() => setSelectedKey(row.key)}
                        onToggleFavorite={() => {}}
                      />
                    )}
                  </For>

                  <Section title={d().local()}>
                    <Show
                      when={localRows().length > 0}
                      fallback={<Note text={narrowed() ? d().noMatches() : d().noBranchesYet()} />}
                    >
                      <For each={localRows()}>
                        {(row) => (
                          <RowView
                            row={row}
                            selected={selectedKey() === row.key}
                            onSelect={() => {
                              setSelectedKey(row.key);
                              if (row.kind === "folder" && row.folderKey) toggleFolder(row.folderKey);
                            }}
                            onToggleFavorite={() => row.node && toggleFavorite(row.node)}
                          />
                        )}
                      </For>
                    </Show>
                  </Section>

                  <Section title={d().remote()}>
                    <Show
                      when={remoteRows().length > 0}
                      fallback={
                        <Note text={narrowed() ? d().noMatches() : d().noRemoteBranches()} />
                      }
                    >
                      <For each={remoteRows()}>
                        {(row) => (
                          <RowView
                            row={row}
                            selected={selectedKey() === row.key}
                            onSelect={() => {
                              setSelectedKey(row.key);
                              if (row.kind === "folder" && row.folderKey) toggleFolder(row.folderKey);
                            }}
                            onToggleFavorite={() => row.node && toggleFavorite(row.node)}
                          />
                        )}
                      </For>
                    </Show>
                  </Section>
                </div>
              </Show>
            </Show>
          </Show>
        </Show>
      </div>
    </PanelChrome>
  );
}

function RowView(props: {
  row: Row;
  selected: boolean;
  onSelect: () => void;
  onToggleFavorite: () => void;
}) {
  const b = () => props.row.node;
  const tracking = () => {
    const n = b();
    if (!n || !n.upstream || n.ahead === null || n.behind === null) return null;
    return { ahead: n.ahead, behind: n.behind, upstream: n.upstream };
  };
  return (
    <div
      class="flex cursor-default items-center gap-1 px-2 py-0.5 font-mono"
      style={{ "padding-left": `${8 + props.row.depth * 12}px` }}
      classList={{
        "bg-accent/20 text-fg": props.selected,
        "text-fg-muted": !props.selected,
        "font-semibold": props.row.kind === "head" || props.row.node?.isCurrent,
      }}
      onClick={props.onSelect}
      title={props.row.node?.name ?? props.row.label}
    >
      <Show when={props.row.kind === "branch"}>
        <button
          class="shrink-0 text-warn"
          classList={{ "opacity-30": !props.row.favorite }}
          title={d().favoriteTip()}
          onClick={(e) => {
            e.stopPropagation();
            props.onToggleFavorite();
          }}
        >
          {props.row.favorite ? "★" : "☆"}
        </button>
      </Show>
      <Show when={props.row.kind === "folder"}>
        <span class="shrink-0 text-fg-subtle">{props.row.collapsed ? "▸" : "▾"}</span>
      </Show>
      <span class="truncate">{props.row.label}</span>
      <Show when={props.row.node?.isCurrent && props.row.kind === "branch"}>
        <span class="shrink-0 text-accent">•</span>
      </Show>
      <Show when={tracking()}>
        {(t) => (
          <span class="ml-auto shrink-0 text-fg-subtle" title={d().trackingTip(t().upstream)}>
            ↓{t().behind} ↑{t().ahead}
          </span>
        )}
      </Show>
    </div>
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

/** In-section note: a group can be empty while the panel around it is not. */
function Note(props: { text: string }) {
  return <div class="px-2 py-0.5 text-fg-subtle">{props.text}</div>;
}

/** Branch names split on `/` into folders; a single-child chain is one row. */
function buildTree(list: BranchNode[], favs: Set<string>): TreeNode[] {
  const root: FolderNode = { kind: "folder", name: "", path: "", children: [] };
  for (const b of list) {
    const segs = b.name.split("/");
    let cur = root;
    for (let i = 0; i < segs.length - 1; i++) {
      const path = cur.path ? `${cur.path}/${segs[i]}` : segs[i];
      let next = cur.children.find(
        (c): c is FolderNode => c.kind === "folder" && c.path === path,
      );
      if (!next) {
        next = { kind: "folder", name: segs[i], path, children: [] };
        cur.children.push(next);
      }
      cur = next;
    }
    cur.children.push({
      kind: "branch",
      name: segs[segs.length - 1],
      node: b,
      favorite: favs.has(favKey(b)),
    });
  }
  return root.children.map(compact);
}

/** `a` → `b` → `c` with nothing else in between reads better as one row `a/b`. */
function compact(node: TreeNode): TreeNode {
  if (node.kind !== "folder") return node;
  let merged: FolderNode = { ...node, children: node.children.map(compact) };
  while (merged.children.length === 1 && merged.children[0].kind === "folder") {
    const child = merged.children[0] as FolderNode;
    merged = {
      kind: "folder",
      name: `${merged.name}/${child.name}`,
      path: child.path,
      children: child.children,
    };
  }
  return merged;
}

/**
 * Current branch first, then favourites, then everything alphabetically —
 * applied at every level, so a favourite keeps the folder it belongs to instead
 * of being hoisted out of the structure the folders exist to show.
 */
function sortTree(nodes: TreeNode[]): TreeNode[] {
  const rank = (n: TreeNode) => (n.kind === "branch" && n.node.isCurrent ? 0 : n.kind === "branch" && n.favorite ? 1 : 2);
  return nodes
    .map((n) => (n.kind === "folder" ? { ...n, children: sortTree(n.children) } : n))
    .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}
