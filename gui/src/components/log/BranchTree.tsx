import { For, Show, createEffect, createMemo, createResource, createSignal } from "solid-js";
import {
  branchTree,
  errText,
  uiStateGet,
  uiStateSet,
  type BranchNode,
  type UiState,
} from "../../api";
import { d } from "../../i18n";
import { busy, setError, state } from "../../store";
import { PanelBtn, PanelChrome, PanelNote } from "./PanelChrome";
import {
  IconCollapseAll,
  IconExpandAll,
  IconFetch,
  IconPlus,
  IconRefresh,
  IconStar,
} from "../IconButton";
import { setSelectedBranch } from "./branchSelection";
import {
  branchMenuItems,
  checkoutBranch,
  fetchAll,
  newBranchFrom,
} from "./actions/branchActions";
import ContextMenu, { createMenuController } from "./actions/ContextMenu";
import { ActionDialogHost } from "./actions/dialogs";
import { operationActive, repoRevision } from "./actions/repoRefresh";

/**
 * Branch tree panel: HEAD on top, then the Favourites section, then the Local
 * and Remote groups, branch names split into collapsible folders, ahead/behind
 * for tracking branches, and a filter field.
 *
 * **A favourite leaves its group.** It used to be sorted first *inside* its
 * folder, which is invisible from outside the folder — the star read as a button
 * that does nothing. A favourite now rises into its own section at the top of
 * the panel and is not drawn a second time below. The section is folded into
 * folders by the same rule as Local and Remote — a flat list of full names is
 * unreadable once more than a handful of long branches are starred — and it is
 * a *third* group (`fav`), not a re-use of `local`: row keys and the persisted
 * `collapsedFolders` are namespaced by group, so `p2p` folded in Favourites
 * would otherwise fold `p2p` in Local as well.
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

type Group = "local" | "remote" | "fav";

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
  // Keyed on the repository *and* on the action layer's revision counter: a
  // delete, a rename or a checkout does not move `repoPath`, so a source keyed
  // on the path alone would keep a deleted branch on screen until the reader
  // pressed Refresh.
  //
  // `null` while no repository is open, and that is the whole of it: a source
  // that is merely an empty path is still truthy, so the resource would fire on
  // the very first render — before `openInitial()` has answered — and the
  // backend's "repository not open" would be *thrown* out of the resource read,
  // straight into the window's error boundary ("Unknown error"). The Log mode is
  // the only mode this tree mounts in, which is why closing the app in Log mode
  // used to mean opening it on the crash screen.
  const treeKey = () => (state() ? `${state()!.repoPath}#${repoRevision()}` : null);
  const [branches, { refetch: refetchBranches }] = createResource(treeKey, () => branchTree());
  const [ui, { mutate: mutateUi, refetch: refetchUi }] = createResource(treeKey, () =>
    uiStateGet(),
  );
  const [filter, setFilter] = createSignal("");
  const [favOnly, setFavOnly] = createSignal(false);
  const [selectedKey, setSelectedKey] = createSignal<string>("head");
  // The context menu's target is captured when the menu opens: a right-click on
  // a row acts on that row without moving the selection, because the selection
  // is what scopes the log and the reader did not ask for that to change.
  const menu = createMenuController();
  const [menuNode, setMenuNode] = createSignal<BranchNode | null>(null);

  /**
   * The element of a row, for the keyboard path of the menu. Asked of the DOM
   * rather than kept in a map: a row filtered out of the tree leaves a detached
   * node behind, and an anchor taken from one is a rectangle of zeros — the menu
   * then opens in the corner of the window instead of beside the row.
   */
  let listEl: HTMLDivElement | undefined;
  const rowElement = (key: string): HTMLElement | null => {
    const el = listEl?.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(key)}"]`);
    return el?.isConnected ? el : null;
  };

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

  const needle = () => filter().trim().toLowerCase();
  const matches = (b: BranchNode) => !needle() || b.name.toLowerCase().includes(needle());

  /**
   * The groups hold everything that is *not* a favourite: a favourite is drawn
   * once, in the section above, and a second copy inside its folder would make
   * the star look like it merely duplicated the row.
   */
  const groupTree = (group: "local" | "remote") => {
    const list = (branches() ?? []).filter((b) =>
      group === "remote" ? b.isRemote : !b.isRemote && b.fullRef !== DETACHED_REF,
    );
    const picked = list.filter((b) => !favorites().has(favKey(b)) && matches(b));
    return sortTree(buildTree(picked));
  };

  /**
   * Favourites, folded into folders by their full name. Local and remote
   * favourites share the section and are told apart by the folder they land in:
   * `origin/x` nests under `origin`, `feature/x` under `feature`. That is what
   * answers the ambiguity which once forced a flat list of full names.
   */
  const favTree = createMemo(() =>
    sortTree(
      buildTree(
        (branches() ?? []).filter(
          (b) => b.fullRef !== DETACHED_REF && favorites().has(favKey(b)) && matches(b),
        ),
      ),
    ),
  );

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
            favorite: group === "fav",
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

  const favRows = createMemo(() => rowsOf("fav", favTree()));

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
  const rows = createMemo<Row[]>(() => [
    ...headRow(),
    ...favRows(),
    ...localRows(),
    ...remoteRows(),
  ]);

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
    // Enter on a branch is checkout (История 20). A folder has no revision to
    // check out, so there Enter keeps its fold/unfold meaning.
    if (r?.kind === "branch" && r.node && !r.node.isCurrent) void checkoutBranch(r.node);
  };

  const openMenuFor = (node: BranchNode | null, at: { x: number; y: number } | HTMLElement | undefined) => {
    setMenuNode(node);
    if (at && "x" in at) menu.open(at);
    else menu.openAt(at as HTMLElement | undefined);
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
    walk("fav", favTree());
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
  /**
   * Judged on what is *drawn*, not on what was computed: with "favourites only"
   * on, the groups still hold every other branch, they are simply not rendered.
   * Counting them would leave a reader who has starred nothing looking at a
   * panel holding one HEAD line and no explanation at all.
   */
  const nothingMatched = () =>
    loaded() &&
    (branches() ?? []).length > 0 &&
    favRows().length === 0 &&
    (favOnly() || (localRows().length === 0 && remoteRows().length === 0)) &&
    narrowed();

  const refreshAll = () => {
    refetchBranches();
    refetchUi();
  };

  // Fetch lives in the action layer now (it is also a menu item there): the
  // counters come from `%(upstream:track)`, so the tree has to be re-read after
  // the remote-tracking refs move — which `afterRepoChange` does through the
  // revision counter this component's resources are keyed on.

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
        contextMenu: () => {
          const r = current();
          openMenuFor(
            r?.kind === "branch" ? (r.node ?? null) : null,
            rowElement(selectedKey()) ?? undefined,
          );
        },
        onKey,
      }}
      toolbar={
        <>
          <PanelBtn label={<IconRefresh />} tip={d().refreshTip()} onClick={refreshAll} />
          <PanelBtn
            label={<IconFetch />}
            tip={d().fetchPruneTip()}
            disabled={busy() || operationActive()}
            disabledTip={operationActive() ? d().whyOperationRunning() : d().fetching()}
            onClick={() => void fetchAll()}
          />
          <PanelBtn label={<IconExpandAll />} tip={d().expandAllTip()} onClick={expandAllFolders} />
          <PanelBtn label={<IconCollapseAll />} tip={d().collapseAllTip()} onClick={collapseAll} />
          {/* The second star in the panel, and the one that confuses: this is a
              *filter over the list*, not the mark on a row. It is told apart by
              its pressed state and by a tooltip that names the list, never the
              branch. */}
          <PanelBtn
            label={<IconStar filled={favOnly()} />}
            tip={favOnly() ? d().favoritesShowAllTip() : d().favoritesOnlyTip()}
            active={favOnly()}
            onClick={() => setFavOnly((v) => !v)}
          />
          <PanelBtn
            label={<IconPlus />}
            tip={d().newBranchTip()}
            disabled={operationActive()}
            disabledTip={d().whyOperationRunning()}
            onClick={() => {
              const r = current();
              const from = r?.kind === "branch" && r.node ? r.node.name : (state()?.branch ?? "HEAD");
              void newBranchFrom(from, from);
            }}
          />
        </>
      }
    >
      <ActionDialogHost />
      <Show when={menu.anchor()}>
        {(a) => (
          <ContextMenu
            anchor={a()}
            items={() => branchMenuItems(menuNode(), refreshAll)}
            onClose={menu.close}
          />
        )}
      </Show>
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
              <Show
                when={!nothingMatched()}
                fallback={
                  <PanelNote
                    title={favOnly() && !filter().trim() ? d().noFavorites() : d().noMatches()}
                    hint={favOnly() && !filter().trim() ? d().noFavoritesHint() : undefined}
                  />
                }
              >
                <div ref={listEl} class="min-h-0 flex-1 overflow-auto py-1">
                  <For each={headRow()}>
                    {(row) => (
                      <RowView
                        row={row}
                        selected={selectedKey() === row.key}
                        onSelect={() => setSelectedKey(row.key)}
                        onToggleFavorite={() => {}}
                        onContextMenu={(e) => openMenuFor(null, { x: e.clientX, y: e.clientY })}
                      />
                    )}
                  </For>

                  <Show when={favRows().length > 0}>
                    <Section title={d().favoritesSection()}>
                      <For each={favRows()}>
                        {(row) => (
                          <RowView
                            row={row}
                            selected={selectedKey() === row.key}
                            onSelect={() => {
                              setSelectedKey(row.key);
                              if (row.kind === "folder" && row.folderKey) toggleFolder(row.folderKey);
                            }}
                            onToggleFavorite={() => row.node && toggleFavorite(row.node)}
                            onActivate={() => {
                              if (row.node && !row.node.isCurrent) void checkoutBranch(row.node);
                            }}
                            onContextMenu={(e) =>
                              openMenuFor(row.node ?? null, { x: e.clientX, y: e.clientY })
                            }
                          />
                        )}
                      </For>
                    </Section>
                  </Show>

                  {/* "Favourites only" hides the groups rather than emptying
                      them: an empty Local group under a full Favourites section
                      reads as "this repository has no local branches". */}
                  <Show when={!favOnly()}>
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
                                onActivate={() => {
                              if (row.node && !row.node.isCurrent) void checkoutBranch(row.node);
                            }}
                            onContextMenu={(e) =>
                              openMenuFor(row.node ?? null, { x: e.clientX, y: e.clientY })
                            }
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
                                onActivate={() => {
                              if (row.node && !row.node.isCurrent) void checkoutBranch(row.node);
                            }}
                            onContextMenu={(e) =>
                              openMenuFor(row.node ?? null, { x: e.clientX, y: e.clientY })
                            }
                          />
                        )}
                      </For>
                    </Show>
                  </Section>
                  </Show>
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
  onContextMenu?: (e: MouseEvent) => void;
  onActivate?: () => void;
}) {
  const b = () => props.row.node;
  const tracking = () => {
    const n = b();
    if (!n || !n.upstream || n.ahead === null || n.behind === null) return null;
    return { ahead: n.ahead, behind: n.behind, upstream: n.upstream };
  };
  return (
    <div
      data-row-key={props.row.key}
      class="flex cursor-default items-center gap-1 px-2 py-0.5 font-mono"
      style={{ "padding-left": `${8 + props.row.depth * 12}px` }}
      classList={{
        "bg-accent/20 text-fg": props.selected,
        "text-fg-muted": !props.selected,
        "font-semibold": props.row.kind === "head" || props.row.node?.isCurrent,
      }}
      onClick={props.onSelect}
      onDblClick={() => props.onActivate?.()}
      onContextMenu={(e) => {
        e.preventDefault();
        props.onContextMenu?.(e);
      }}
      title={props.row.node?.name ?? props.row.label}
    >
      {/* The mark on the row, not a toolbar button: no frame, no pressed
          background — the header star is the control that filters the list.
          Filled versus outlined is the whole of the state, readable without
          hovering; the previous version differed only in opacity and read as a
          button that does nothing. */}
      <Show when={props.row.kind === "branch"}>
        <button
          class="flex shrink-0 items-center hover:text-warn"
          classList={{
            "text-warn": props.row.favorite,
            "text-fg-subtle": !props.row.favorite,
          }}
          title={props.row.favorite ? d().favoriteRemoveTip() : d().favoriteAddTip()}
          aria-pressed={props.row.favorite}
          onClick={(e) => {
            e.stopPropagation();
            props.onToggleFavorite();
          }}
        >
          <IconStar filled={props.row.favorite} />
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

/**
 * Branch names split on `/` into folders; a single-child chain is one row.
 *
 * Deliberately **not** `pathTree.ts`, which carries the same rule for files.
 * Three differences, none of them cosmetic:
 *
 * - The leaf is a branch, not a file: it is a full row with its own actions,
 *   context menu, and it is *interleaved* with folders in one `children` array —
 *   `sortTree` orders folders and branches against each other (current branch,
 *   then alphabetically). `pathTree` keeps `dirs` and `files` in separate buckets
 *   and always draws folders first, which would hoist every folder above the
 *   current branch.
 * - A folder merges only when its single child is a *folder*; a folder holding
 *   one branch stays a folder. The file version merges while `files.length === 0`
 *   — the same thing said in the file world, but not expressible on a single
 *   `children` array without knowing what a leaf is.
 * - Collapse keys are namespaced by group (`fav:` / `local:` / `remote:`),
 *   because `p2p` under Favourites, under Local and under Remote collapse apart.
 *   File trees key on the bare path.
 *
 * The path *paths* rule is shared in spirit and matched by hand: `allFolderKeys`
 * walks the compacted tree, so a merged chain has one key, exactly like
 * `treeDirPaths`. Change one, look at the other.
 */
function buildTree(list: BranchNode[]): TreeNode[] {
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
 * Current branch first, then everything alphabetically — applied at every level.
 *
 * There is no favourite rank here any more: a favourite is not sorted inside its
 * folder, it is lifted out of the groups entirely into the Favourites section
 * (see the panel docblock). Ranking it here as well would only reorder rows that
 * are no longer in this tree.
 */
function sortTree(nodes: TreeNode[]): TreeNode[] {
  const rank = (n: TreeNode) => (n.kind === "branch" && n.node.isCurrent ? 0 : 1);
  return nodes
    .map((n) => (n.kind === "folder" ? { ...n, children: sortTree(n.children) } : n))
    .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}
