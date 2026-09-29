import { For, Show, createEffect, createSignal } from "solid-js";
import {
  discardCheck,
  discardList,
  discardRestore,
  errText,
  type DiscardEntry,
  type DiscardOutcome,
} from "../api";
import { d, fmtDateTime } from "../i18n";
import {
  busy,
  confirmAction,
  registerModalSource,
  reportError,
  runWithOutput,
  setNotice,
  state,
} from "../store";
import { DISABLED_CLASS } from "./IconButton";

/**
 * Backups of rolled-back work, and the way back.
 *
 * Every rollback and hunk revert is backed up by the backend first
 * (`engine::discard`, the hidden ref `refs/graft/discard`). This module is the
 * client side of that: the one runner every discard goes through, the notice it
 * leaves ("Rolled back 3 files · Undo"), and the dialog listing older backups.
 *
 * Seams worth naming:
 *
 *  - **A discard returns `DiscardOutcome`, not `RepoState`.** It carries the backup
 *    it took, so it runs through `runWithOutput` — same busy / banner / re-read
 *    contract as `run()`.
 *  - **Staleness is asked before, not caught after.** `run()` flattens an error to
 *    text, so the restore would have nothing to branch on; `discardCheck` is a
 *    read-only question, and the confirmation names the paths it answered with.
 *    The backend still refuses (`stale`) if something changed in between.
 *  - **`z-40`, below the store's modals (`z-50`)**, like the stash panel: the
 *    "restore anyway?" confirmation must render over this dialog.
 */

/** How many backups the dialog lists. The chain holds at most a hundred. */
const LIST_LIMIT = 100;

const [open, setOpen] = createSignal(false);
const [entries, setEntries] = createSignal<DiscardEntry[]>([]);
const [listError, setListError] = createSignal("");
const [selectedId, setSelectedId] = createSignal<string | null>(null);

registerModalSource(open);

async function reload(): Promise<void> {
  if (!state()) {
    setEntries([]);
    return;
  }
  try {
    const list = await discardList(LIST_LIMIT);
    setEntries(list);
    setListError("");
    if (!list.some((e) => e.id === selectedId())) setSelectedId(list[0]?.id ?? null);
  } catch (e) {
    setListError(errText(e));
  }
}

export function openDiscardPanel(): void {
  setOpen(true);
  void reload();
}

/** What the notice and the list say a backup was. */
export function discardLabel(e: DiscardEntry): string {
  switch (e.kind) {
    case "hunk":
      return d().discardedHunk(e.paths[0] ?? "");
    case "restore":
      return d().discardedRestore(e.paths.length);
    case "list":
      return d().discardedList(e.paths.length);
    default:
      return d().discardedFiles(e.paths.length);
  }
}

/**
 * Run a discard (rollback, hunk revert) or a restore, and offer the way back.
 * Every caller of `fileRollback`, `listRollback`, `hunkRevert` goes through here.
 */
export async function runDiscard(p: Promise<DiscardOutcome>, label = ""): Promise<void> {
  const repo = state()?.repoPath;
  const result = await runWithOutput(p, label);
  const backup = result?.backup;
  if (backup && repo) {
    setNotice({
      repo,
      text: () => discardLabel(backup),
      action: { label: () => d().discardUndo(), run: () => void restoreBackup(backup) },
    });
  }
  if (open()) await reload();
}

/**
 * Put a backup's files back. When any of them changed since the discard, the
 * reader is told which and asked; a forced restore backs the current versions up
 * first, so it can be undone from the same list.
 */
export async function restoreBackup(entry: DiscardEntry): Promise<void> {
  setNotice(null);
  let stale: string[];
  try {
    stale = await discardCheck(entry.id);
  } catch (e) {
    reportError(e);
    return;
  }
  const force = stale.length > 0;
  if (force && !(await confirmAction(d().discardStaleConfirm(stale), true))) return;
  await runDiscard(discardRestore(entry.id, force), d().phaseDiscardRestore());
}

export default function DiscardPanel() {
  return (
    <Show when={open()}>
      <DiscardPanelView />
    </Show>
  );
}

function DiscardPanelView() {
  let box: HTMLDivElement | undefined;
  const rows = new Map<string, HTMLButtonElement>();
  const selected = () => entries().find((e) => e.id === selectedId()) ?? null;
  const close = () => setOpen(false);

  createEffect(() => {
    if (open()) queueMicrotask(() => box?.focus());
  });
  createEffect(() => {
    const id = selectedId();
    if (id) rows.get(id)?.scrollIntoView({ block: "nearest" });
  });

  const move = (delta: number) => {
    const list = entries();
    if (list.length === 0) return;
    const at = list.findIndex((e) => e.id === selectedId());
    const next = Math.min(list.length - 1, Math.max(0, (at < 0 ? 0 : at) + delta));
    setSelectedId(list[next].id);
  };

  const doRestore = () => {
    const e = selected();
    if (e && !busy()) void restoreBackup(e).then(() => box?.focus());
  };

  // The application layer does not know about this dialog; keys that start inside
  // it stop here. Escape is not taken in the capture phase on purpose: while the
  // "restore anyway?" confirmation is up, that modal owns Escape.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    switch (e.code) {
      case "Escape":
        e.preventDefault();
        close();
        break;
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
        e.preventDefault();
        move(-entries().length);
        break;
      case "End":
        e.preventDefault();
        move(entries().length);
        break;
      case "Enter":
      case "NumpadEnter":
        e.preventDefault();
        doRestore();
        break;
    }
  };

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        role="dialog"
        aria-label={d().discardsTitle()}
        class="flex h-[min(34rem,84vh)] w-[min(52rem,92vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="text-sm font-semibold">{d().discardsTitle()}</span>
          <span class="text-xs text-fg-muted">({entries().length})</span>
          <button
            class="ml-auto rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>

        <Show when={listError()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {listError()}
          </pre>
        </Show>

        <div class="flex min-h-0 flex-1">
          <div class="w-1/2 shrink-0 overflow-auto border-r border-border py-1" role="listbox">
            <Show
              when={entries().length > 0}
              fallback={<Empty text={d().discardsEmpty()} />}
            >
              <For each={entries()}>
                {(e) => (
                  <button
                    ref={(el) => rows.set(e.id, el)}
                    role="option"
                    aria-selected={e.id === selectedId()}
                    tabindex={-1}
                    class="flex w-full flex-col items-start gap-0.5 px-3 py-1.5 text-left text-xs hover:bg-bg-muted"
                    classList={{ "bg-accent/10": e.id === selectedId() }}
                    onClick={() => {
                      setSelectedId(e.id);
                      box?.focus();
                    }}
                    onDblClick={doRestore}
                  >
                    <span class="w-full truncate font-medium">{discardLabel(e)}</span>
                    <span class="w-full truncate text-[0.6875rem] text-fg-muted">
                      {fmtDateTime(e.at)} · {e.paths.join(", ")}
                    </span>
                  </button>
                )}
              </For>
            </Show>
          </div>

          <div class="flex min-w-0 flex-1 flex-col">
            <div class="border-b border-border px-3 py-1 text-[0.6875rem] font-semibold uppercase tracking-wide text-fg-subtle">
              {d().discardFilesTitle()}
            </div>
            <div class="min-h-0 flex-1 overflow-auto py-1">
              <Show when={selected()} fallback={<Empty text={d().discardSelectOne()} />}>
                {(e) => (
                  <For each={e().paths}>
                    {(path) => (
                      <div class="truncate px-3 py-0.5 text-xs" title={path}>
                        {path}
                      </div>
                    )}
                  </For>
                )}
              </Show>
            </div>
            <div class="border-t border-border px-3 py-1 text-[0.6875rem] text-fg-subtle">
              {d().discardNote()}
            </div>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2">
          <span class="text-[0.6875rem] text-fg-subtle">{d().discardKeysHint()}</span>
          <button
            class={`ml-auto rounded bg-accent px-3 py-1 text-sm text-white ${DISABLED_CLASS}`}
            disabled={busy() || !selected()}
            title={selected() ? d().discardRestoreTip() : d().discardSelectOne()}
            onClick={doRestore}
          >
            {d().discardRestoreBtn()}
          </button>
        </div>
      </div>
    </div>
  );
}

function Empty(props: { text: string }) {
  return <div class="p-4 text-center text-xs text-fg-muted">{props.text}</div>;
}
