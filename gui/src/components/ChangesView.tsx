import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import {
  changelistCreate,
  changelistDelete,
  changelistRename,
  changelistSetActive,
  fileRollback,
  filesMove,
  listRollback,
  type ChangelistView,
  type FileStatus,
} from "../api";
import {
  baseName,
  buildFileTree,
  treeDirPaths,
  type FileTreeNode,
} from "./pathTree";
import {
  busy,
  checked,
  confirmAction,
  groupByDir,
  isChecked,
  promptText,
  refresh,
  run,
  selectedListId,
  selectedPath,
  setChecked,
  setSelectedListId,
  setSelectedPath,
  showIgnored,
  state,
  statusMeta,
  toggleChecked,
  toggleGroupByDir,
  toggleShowIgnored,
} from "../store";
import { d } from "../i18n";
import SidebarFooter from "./SidebarFooter";

// Which paths a drag carries: the checked set if the dragged row is part of it,
// otherwise just that one file.
let dragPaths: string[] = [];

type Menu = { x: number; y: number; file?: string; list?: ChangelistView };
const [menu, setMenu] = createSignal<Menu | null>(null);
// id of the changelist currently under a file drag (for drop highlight)
const [dragOverId, setDragOverId] = createSignal<string | null>(null);

// Collapsed tree nodes (changelist ids; directory keys added in group-by mode).
const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());
const isCollapsed = (key: string) => collapsed().has(key);
const toggleCollapsed = (key: string) =>
  setCollapsed((prev) => {
    const n = new Set(prev);
    n.has(key) ? n.delete(key) : n.add(key);
    return n;
  });

// ── Directory-tree grouping ──────────────────────────────────────────────────
//
// The rule itself (split on `/`, collapse single-child chains) lives in
// `pathTree.ts` and is shared with the commit's file list. Only the alias below
// belongs to this panel.

type TreeNode = FileTreeNode<FileStatus>;

export default function ChangesView() {
  onMount(() => {
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    onCleanup(() => window.removeEventListener("click", close));
  });

  const lists = () => state()?.changelists ?? [];
  const totalFiles = () => lists().reduce((n, c) => n + c.files.length, 0);
  // A user-created (named) changelist must stay visible even with no files, or a
  // just-created list vanishes on a clean tree. Default/Unversioned don't count.
  const hasUserLists = () => lists().some((c) => !c.isDefault && !c.isUnversioned);

  const newList = async () => {
    const name = await promptText(d().newChangelist(), "");
    if (name && name.trim()) await run(changelistCreate(name.trim()));
  };

  const rollbackChecked = async () => {
    const paths = [...checked()];
    if (paths.length === 0) return;
    if (await confirmAction(d().rollbackConfirm(paths.length))) {
      await run(fileRollback(paths));
      setChecked(new Set<string>());
    }
  };

  const collapseAll = () => {
    const keys = new Set<string>();
    for (const cl of lists()) {
      keys.add(cl.id);
      if (groupByDir() && !cl.isIgnored) {
        for (const p of treeDirPaths(buildFileTree(cl.files))) keys.add(`dir:${cl.id}:${p}`);
      }
    }
    setCollapsed(keys);
  };
  const expandAll = () => setCollapsed(new Set<string>());

  return (
    <div class="flex h-full flex-col">
      <div class="flex items-center gap-0.5 border-b border-border px-1 py-1 text-fg-muted">
        <TbBtn title={d().refreshTip()} onClick={() => void refresh()} disabled={busy()}>
          ↻
        </TbBtn>
        <TbBtn
          title={d().rollbackTip()}
          onClick={() => void rollbackChecked()}
          disabled={checked().size === 0}
        >
          ↺
        </TbBtn>
        <TbBtn title={d().expandAll()} onClick={expandAll}>
          <IconExpandAll />
        </TbBtn>
        <TbBtn title={d().collapseAll()} onClick={collapseAll}>
          <IconCollapseAll />
        </TbBtn>
        <ViewOptionsMenu />
        <TbBtn title={d().newChangelist()} onClick={() => void newList()} class="ml-auto">
          ＋
        </TbBtn>
      </div>

      <div class="flex-1 overflow-auto py-1">
        <Show
          when={totalFiles() > 0 || hasUserLists()}
          fallback={
            <div class="p-4 text-center text-xs text-fg-muted">
              {d().cleanTree()}
            </div>
          }
        >
          <For each={lists()}>{(cl) => <ListNode cl={cl} />}</For>
        </Show>
      </div>

      <SidebarFooter />
      <ContextMenu />
    </div>
  );
}

function ListNode(props: { cl: ChangelistView }) {
  const cl = () => props.cl;
  const isActive = () => state()?.activeChangelistId === cl().id;
  const isSelected = () => selectedListId() === cl().id;
  const listCollapsed = () => isCollapsed(cl().id);
  const tree = createMemo(() => buildFileTree(cl().files));
  const treeMode = () => groupByDir() && !cl().isIgnored;

  const allChecked = () =>
    cl().files.length > 0 && cl().files.every((f) => isChecked(f.path));

  const toggleAll = () => {
    const paths = cl().files.map((f) => f.path);
    setChecked((prev) => {
      const n = new Set(prev);
      if (allChecked()) paths.forEach((p) => n.delete(p));
      else paths.forEach((p) => n.add(p));
      return n;
    });
  };

  const canDrop = () => !cl().isUnversioned && dragPaths.length > 0;
  const onDragOver = (e: DragEvent) => {
    if (!canDrop()) return;
    e.preventDefault(); // allow drop
    setDragOverId(cl().id);
  };
  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragOverId(null);
    if (cl().isUnversioned || dragPaths.length === 0) return;
    void run(filesMove(dragPaths, cl().id));
  };

  // Whole changelist block is the drop target (header + its files), not just the
  // header row — dropping onto an empty list must work too.
  return (
    <div
      class="rounded"
      classList={{ "bg-accent/10 ring-1 ring-inset ring-accent/50": dragOverId() === cl().id }}
      onDragOver={onDragOver}
      onDrop={onDrop}
    >
      <div
        class="flex items-center gap-1 px-2 py-0.5 text-xs"
        classList={{ "bg-accent/10": isSelected() && dragOverId() !== cl().id }}
        onClick={() => setSelectedListId(cl().id)}
        onContextMenu={(e) => {
          e.preventDefault();
          setSelectedListId(cl().id);
          if (cl().isUnversioned) return; // synthetic lists have no list actions
          setMenu({ x: e.clientX, y: e.clientY, list: cl() });
        }}
      >
        <Disclosure
          show={cl().files.length > 0}
          collapsed={listCollapsed()}
          onToggle={() => toggleCollapsed(cl().id)}
        />
        <Show when={!cl().isUnversioned && cl().files.length > 0}>
          <span class="flex h-4 w-4 shrink-0 items-center justify-center">
            <input
              type="checkbox"
              class="accent-accent"
              checked={allChecked()}
              onClick={(e) => e.stopPropagation()}
              onChange={toggleAll}
            />
          </span>
        </Show>
        <span class="truncate font-semibold">{cl().name}</span>
        <span class="shrink-0 text-fg-muted">({cl().files.length})</span>
        <Show when={isActive()}>
          <span class="shrink-0 rounded bg-accent/20 px-1 text-[10px] text-accent">
            {d().active()}
          </span>
        </Show>
      </div>

      <Show when={!listCollapsed()}>
        <Show
          when={treeMode()}
          fallback={
            <For each={cl().files}>
              {(f) => <FileRow file={f} listId={cl().id} depth={1} name={f.path} />}
            </For>
          }
        >
          <TreeChildren node={tree()} listId={cl().id} depth={1} />
        </Show>
      </Show>
    </div>
  );
}

function FileRow(props: { file: FileStatus; listId: string; depth: number; name: string }) {
  const f = () => props.file;
  const m = () => statusMeta(f().status);
  const selected = () => selectedPath() === f().path;
  // Ignored rows are read-only: no checkbox, no drag, no diff-select, no menu —
  // so an ignored path can never be checked, committed, or diffed.
  const readOnly = () => f().status === "ignored";

  return (
    <div
      class="flex cursor-default items-center gap-1 py-0.5 pr-2 font-mono text-xs hover:bg-bg-muted"
      classList={{ "bg-accent/15": selected() && !readOnly() }}
      style={{ "padding-left": `${8 + props.depth * 16}px` }}
      draggable={!readOnly()}
      onDragStart={() => {
        if (readOnly()) return;
        dragPaths = isChecked(f().path) ? [...checked()] : [f().path];
      }}
      onDragEnd={() => {
        dragPaths = [];
        setDragOverId(null);
      }}
      onClick={() => {
        if (readOnly()) return;
        setSelectedPath(f().path);
        setSelectedListId(props.listId);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        if (readOnly()) return;
        setSelectedPath(f().path);
        setMenu({ x: e.clientX, y: e.clientY, file: f().path });
      }}
    >
      <span class="w-4 shrink-0" />
      <span class="flex h-4 w-4 shrink-0 items-center justify-center">
        <Show when={!readOnly()}>
          <input
            type="checkbox"
            class="accent-accent"
            checked={isChecked(f().path)}
            onClick={(e) => e.stopPropagation()}
            onChange={() => toggleChecked(f().path)}
          />
        </Show>
      </span>
      <span class={`w-3 shrink-0 text-center font-bold ${m().cls}`} title={f().status}>
        {m().letter}
      </span>
      <span class="truncate" classList={{ "text-fg-muted": readOnly() }} title={f().path}>
        {props.name}
      </span>
    </div>
  );
}

// One directory node in the grouped tree: a disclosure + folder + name, with its
// children (subdirs then files) rendered one level deeper.
function DirNode(props: { dir: TreeNode; listId: string; depth: number }) {
  const key = () => `dir:${props.listId}:${props.dir.path}`;
  const dirCollapsed = () => isCollapsed(key());
  return (
    <>
      <div
        class="flex cursor-default items-center gap-1 py-0.5 pr-2 text-xs hover:bg-bg-muted"
        style={{ "padding-left": `${8 + props.depth * 16}px` }}
        onClick={() => toggleCollapsed(key())}
      >
        <Disclosure show={true} collapsed={dirCollapsed()} onToggle={() => toggleCollapsed(key())} />
        <FolderIcon />
        <span class="truncate text-fg-subtle">{props.dir.name}</span>
      </div>
      <Show when={!dirCollapsed()}>
        <TreeChildren node={props.dir} listId={props.listId} depth={props.depth + 1} />
      </Show>
    </>
  );
}

// Renders a tree node's children: subdirectories first, then its own files.
function TreeChildren(props: { node: TreeNode; listId: string; depth: number }) {
  return (
    <>
      <For each={props.node.dirs}>
        {(dir) => <DirNode dir={dir} listId={props.listId} depth={props.depth} />}
      </For>
      <For each={props.node.files}>
        {(f) => (
          <FileRow file={f} listId={props.listId} depth={props.depth} name={baseName(f.path)} />
        )}
      </For>
    </>
  );
}

function FolderIcon() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 16 16"
      fill="currentColor"
      class="shrink-0 text-fg-subtle"
    >
      <path d="M1.5 4A1.5 1.5 0 0 1 3 2.5h2.6l1.4 1.4H13A1.5 1.5 0 0 1 14.5 5.4v6.1A1.5 1.5 0 0 1 13 13H3a1.5 1.5 0 0 1-1.5-1.5z" />
    </svg>
  );
}

function ContextMenu() {
  const m = menu;

  const moveTargets = () =>
    (state()?.changelists ?? []).filter((c) => !c.isUnversioned);

  const rollbackFile = async (path: string) => {
    if (await confirmAction(d().revertFileConfirm(path)))
      await run(fileRollback([path]));
  };
  const rollbackList = async (cl: ChangelistView) => {
    if (cl.files.length && (await confirmAction(d().revertListConfirm(cl.name))))
      await run(listRollback(cl.id));
  };
  const rename = async (cl: ChangelistView) => {
    const name = await promptText(d().renameChangelist(), cl.name);
    if (name && name.trim()) await run(changelistRename(cl.id, name.trim()));
  };
  const del = async (cl: ChangelistView) => {
    if (await confirmAction(d().deleteListConfirm(cl.name), false))
      await run(changelistDelete(cl.id));
  };

  return (
    <Show when={m()}>
      {(mm) => (
        <div
          class="fixed z-40 min-w-44 rounded-md border border-border bg-bg py-1 text-xs shadow-lg"
          style={{ left: `${mm().x}px`, top: `${mm().y}px` }}
          onClick={(e) => e.stopPropagation()}
        >
          <Show when={mm().file}>
            {(path) => (
              <>
                <div class="px-3 py-1 text-[10px] uppercase text-fg-muted">{d().moveTo()}</div>
                <For each={moveTargets()}>
                  {(c) => (
                    <MenuItem
                      label={c.name}
                      onClick={() => {
                        setMenu(null);
                        void run(filesMove([path()], c.id));
                      }}
                    />
                  )}
                </For>
                <Divider />
                <MenuItem
                  label={d().revertToHead()}
                  danger
                  onClick={() => {
                    setMenu(null);
                    void rollbackFile(path());
                  }}
                />
              </>
            )}
          </Show>

          <Show when={mm().list}>
            {(cl) => (
              <>
                <MenuItem
                  label={d().makeActive()}
                  onClick={() => {
                    setMenu(null);
                    void run(changelistSetActive(cl().id));
                  }}
                />
                <Show when={!cl().isUnversioned}>
                  <MenuItem
                    label={d().renameItem()}
                    onClick={() => {
                      setMenu(null);
                      void rename(cl());
                    }}
                  />
                  <Show when={!cl().isDefault}>
                    <MenuItem
                      label={d().deleteList()}
                      onClick={() => {
                        setMenu(null);
                        void del(cl());
                      }}
                    />
                  </Show>
                  <Divider />
                  <MenuItem
                    label={d().revertListToHead()}
                    danger
                    onClick={() => {
                      setMenu(null);
                      void rollbackList(cl());
                    }}
                  />
                </Show>
              </>
            )}
          </Show>
        </div>
      )}
    </Show>
  );
}

function MenuItem(props: { label: string; danger?: boolean; onClick: () => void }) {
  return (
    <button
      class="block w-full px-3 py-1 text-left hover:bg-bg-muted"
      classList={{ "text-danger": props.danger }}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

function Divider() {
  return <div class="my-1 border-t border-border" />;
}

// A compact icon button for the CHANGES toolbar.
function TbBtn(props: {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
  class?: string;
  children: any;
}) {
  return (
    <button
      class={`flex h-6 w-6 items-center justify-center rounded text-sm hover:bg-bg-muted hover:text-fg disabled:opacity-30 disabled:hover:bg-transparent ${props.class ?? ""}`}
      classList={{ "bg-accent/15 text-accent": props.active }}
      title={props.title}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      {props.children}
    </button>
  );
}

// Rotating filled triangle used as the changelist/directory disclosure. Reserves
// a fixed-width gutter so rows with and without an arrow line up in one column.
function Disclosure(props: { show: boolean; collapsed: boolean; onToggle: () => void }) {
  return (
    <span
      class="flex h-4 w-4 shrink-0 cursor-default items-center justify-center text-fg-subtle"
      onClick={(e) => {
        if (props.show) {
          e.stopPropagation();
          props.onToggle();
        }
      }}
    >
      <Show when={props.show}>
        <svg
          width="13"
          height="13"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          style={{
            transform: props.collapsed ? "none" : "rotate(90deg)",
            transition: "transform 0.12s",
          }}
        >
          <path d="M6 4l4 4-4 4" />
        </svg>
      </Show>
    </span>
  );
}

// Android-Studio-style toolbar glyphs: two chevrons pointing down = expand all,
// two chevrons converging = collapse all.
function IconExpandAll() {
  // chevrons pointing apart (up over down) — Android Studio's expand-all glyph
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M4 6l4-3 4 3" />
      <path d="M4 10l4 3 4-3" />
    </svg>
  );
}
function IconCollapseAll() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M4 3l4 3 4-3" />
      <path d="M4 13l4-3 4 3" />
    </svg>
  );
}

// Group-by-directory toggle (folder glyph).
function IconTree() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
      <path d="M1.5 4A1.5 1.5 0 0 1 3 2.5h2.6l1.4 1.4H13A1.5 1.5 0 0 1 14.5 5.4v6.1A1.5 1.5 0 0 1 13 13H3a1.5 1.5 0 0 1-1.5-1.5z" />
    </svg>
  );
}

// Show-ignored toggle (eye glyph).
function IconEye() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M1 8s2.6-4.5 7-4.5S15 8 15 8s-2.6 4.5-7 4.5S1 8 1 8z" />
      <circle cx="8" cy="8" r="2" />
    </svg>
  );
}

// View-options dropdown (Android Studio's "eye" menu): Group By / Show toggles.
function ViewOptionsMenu() {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="relative">
      <TbBtn
        title={d().viewOptionsTip()}
        onClick={() => setOpen((v) => !v)}
        active={open() || groupByDir() || showIgnored()}
      >
        <IconEye />
      </TbBtn>
      <Show when={open()}>
        <>
          <div class="fixed inset-0 z-20" onClick={() => setOpen(false)} />
          <div class="absolute left-0 top-full z-30 mt-1 min-w-44 rounded-md border border-border bg-bg py-1 text-xs text-fg shadow-lg">
            <div class="px-3 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-fg-subtle">
              {d().groupByHeader()}
            </div>
            <MenuToggle
              checked={groupByDir()}
              label={d().directory()}
              onClick={() => toggleGroupByDir()}
            />
            <div class="mt-1 border-t border-border px-3 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-fg-subtle">
              {d().showHeader()}
            </div>
            <MenuToggle
              checked={showIgnored()}
              label={d().ignoredFiles()}
              onClick={() => void toggleShowIgnored()}
            />
          </div>
        </>
      </Show>
    </div>
  );
}

function MenuToggle(props: { checked: boolean; label: string; onClick: () => void }) {
  return (
    <button
      class="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-bg-muted"
      onClick={props.onClick}
    >
      <span class="w-3 text-center text-accent">{props.checked ? "✓" : ""}</span>
      <span>{props.label}</span>
    </button>
  );
}
