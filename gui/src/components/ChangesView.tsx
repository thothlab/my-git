import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
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
  checked,
  confirmAction,
  isChecked,
  promptText,
  run,
  selectedListId,
  selectedPath,
  setChecked,
  setSelectedListId,
  setSelectedPath,
  state,
  statusMeta,
  toggleChecked,
} from "../store";

// Which paths a drag carries: the checked set if the dragged row is part of it,
// otherwise just that one file.
let dragPaths: string[] = [];

type Menu = { x: number; y: number; file?: string; list?: ChangelistView };
const [menu, setMenu] = createSignal<Menu | null>(null);

export default function ChangesView() {
  onMount(() => {
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    onCleanup(() => window.removeEventListener("click", close));
  });

  const lists = () => state()?.changelists ?? [];
  const totalFiles = () => lists().reduce((n, c) => n + c.files.length, 0);

  const newList = async () => {
    const name = await promptText("Новый changelist", "");
    if (name && name.trim()) await run(changelistCreate(name.trim()));
  };

  return (
    <div class="flex h-full flex-col">
      <div class="flex items-center gap-2 border-b border-border px-2 py-1.5">
        <span class="text-xs font-semibold uppercase tracking-wide text-fg-muted">Changes</span>
        <button
          class="ml-auto rounded border border-border px-1.5 text-xs hover:bg-bg-muted"
          title="Новый changelist"
          onClick={newList}
        >
          + список
        </button>
      </div>

      <div class="flex-1 overflow-auto py-1">
        <Show
          when={totalFiles() > 0}
          fallback={
            <div class="p-4 text-center text-xs text-fg-muted">
              Нет изменений — рабочее дерево чистое.
            </div>
          }
        >
          <For each={lists()}>{(cl) => <ListNode cl={cl} />}</For>
        </Show>
      </div>

      <ContextMenu />
    </div>
  );
}

function ListNode(props: { cl: ChangelistView }) {
  const cl = () => props.cl;
  const isActive = () => state()?.activeChangelistId === cl().id;
  const isSelected = () => selectedListId() === cl().id;

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

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    if (cl().isUnversioned || dragPaths.length === 0) return;
    void run(filesMove(dragPaths, cl().id));
  };

  return (
    <div>
      <div
        class="flex items-center gap-1.5 px-2 py-0.5 text-xs"
        classList={{ "bg-accent/10": isSelected() }}
        onClick={() => setSelectedListId(cl().id)}
        onContextMenu={(e) => {
          e.preventDefault();
          setSelectedListId(cl().id);
          setMenu({ x: e.clientX, y: e.clientY, list: cl() });
        }}
        onDragOver={(e) => !cl().isUnversioned && e.preventDefault()}
        onDrop={onDrop}
      >
        <Show when={!cl().isUnversioned}>
          <input
            type="checkbox"
            class="accent-accent"
            checked={allChecked()}
            onClick={(e) => e.stopPropagation()}
            onChange={toggleAll}
          />
        </Show>
        <span class="font-semibold">{cl().name}</span>
        <span class="text-fg-muted">({cl().files.length})</span>
        <Show when={isActive()}>
          <span class="rounded bg-accent/20 px-1 text-[10px] text-accent">активный</span>
        </Show>
        <button
          class="ml-auto px-1 text-fg-muted hover:text-fg"
          title="Действия со списком"
          onClick={(e) => {
            e.stopPropagation();
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            setMenu({ x: r.left, y: r.bottom, list: cl() });
          }}
        >
          ⋯
        </button>
      </div>

      <For each={cl().files}>{(f) => <FileRow file={f} listId={cl().id} />}</For>
    </div>
  );
}

function FileRow(props: { file: FileStatus; listId: string }) {
  const f = () => props.file;
  const m = () => statusMeta(f().status);
  const selected = () => selectedPath() === f().path;

  return (
    <div
      class="flex cursor-default items-center gap-1.5 py-0.5 pl-6 pr-2 font-mono text-xs hover:bg-bg-muted"
      classList={{ "bg-accent/15": selected() }}
      draggable={true}
      onDragStart={() => {
        dragPaths = isChecked(f().path) ? [...checked()] : [f().path];
      }}
      onClick={() => {
        setSelectedPath(f().path);
        setSelectedListId(props.listId);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        setSelectedPath(f().path);
        setMenu({ x: e.clientX, y: e.clientY, file: f().path });
      }}
    >
      <input
        type="checkbox"
        class="accent-accent"
        checked={isChecked(f().path)}
        onClick={(e) => e.stopPropagation()}
        onChange={() => toggleChecked(f().path)}
      />
      <span class={`w-3 text-center font-bold ${m().cls}`} title={f().status}>
        {m().letter}
      </span>
      <span class="truncate" title={f().path}>
        {f().path}
      </span>
    </div>
  );
}

function ContextMenu() {
  const m = menu;

  const moveTargets = () =>
    (state()?.changelists ?? []).filter((c) => !c.isUnversioned);

  const rollbackFile = async (path: string) => {
    if (await confirmAction(`Откатить ${path} к HEAD? Локальные правки будут потеряны.`))
      await run(fileRollback([path]));
  };
  const rollbackList = async (cl: ChangelistView) => {
    if (
      cl.files.length &&
      (await confirmAction(
        `Откатить все файлы списка "${cl.name}" к HEAD? Локальные правки будут потеряны.`,
      ))
    )
      await run(listRollback(cl.id));
  };
  const rename = async (cl: ChangelistView) => {
    const name = await promptText("Переименовать changelist", cl.name);
    if (name && name.trim()) await run(changelistRename(cl.id, name.trim()));
  };
  const del = async (cl: ChangelistView) => {
    if (await confirmAction(`Удалить список "${cl.name}"? Файлы вернутся в Default.`, false))
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
                <div class="px-3 py-1 text-[10px] uppercase text-fg-muted">Переместить в</div>
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
                  label="Откатить к HEAD"
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
                  label="Сделать активным"
                  onClick={() => {
                    setMenu(null);
                    void run(changelistSetActive(cl().id));
                  }}
                />
                <Show when={!cl().isUnversioned}>
                  <MenuItem
                    label="Переименовать…"
                    onClick={() => {
                      setMenu(null);
                      void rename(cl());
                    }}
                  />
                  <Show when={!cl().isDefault}>
                    <MenuItem
                      label="Удалить список"
                      onClick={() => {
                        setMenu(null);
                        void del(cl());
                      }}
                    />
                  </Show>
                  <Divider />
                  <MenuItem
                    label="Откатить список к HEAD"
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
