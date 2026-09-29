import { createVirtualizer } from "@tanstack/solid-virtual";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  on,
  onCleanup,
} from "solid-js";
import {
  errText,
  fileHistory,
  type BackendError,
  type FileHistoryCommit,
  type FileHistoryCursor,
} from "../api";
import { isTypingTarget } from "../hotkeys";
import { d, fmtDateTime } from "../i18n";
import { registerModalSource, scaledPx, state, statusMeta } from "../store";
import { openBlame } from "./blame/BlamePanel";
import DiffView, { type DiffApi } from "./DiffView";
import { sameDiffSource, type DiffSource } from "./diff/model";
import { showCommitInLog } from "./log/actions/showInLog";

/**
 * File history (R05c): every commit that touched one file, renames followed,
 * and the diff of just that file in the chosen commit.
 *
 * An overlay in the `StashPanel` manner, not a panel of the Log mode, and on
 * purpose: it is opened from both modes (a file in Changes, a file of a commit
 * in Log), and a `PanelId` of its own would put the panels behind it into the
 * Tab cycle, while a second `DiffPanel` would register the id `"diff"` twice
 * when opened from Log. As a modal source it silences the keyboard layer, so it
 * answers its own keys — arrows, PageUp/PageDown, Home/End, Enter, Escape,
 * Cmd/Ctrl + Up/Down for the previous/next difference — in `onKeyDown`, not in
 * the capture phase: a store modal on top of it keeps Escape.
 *
 * Seams worth naming:
 *
 *  - **The row cursor is local to the overlay.** It is not a fourth selection of
 *    the window: Enter hands the commit to the log's own selection
 *    (`showCommitInLog`, shared with the blame) and closes the overlay.
 *  - **Blame replaces it rather than stacking on it.** Cmd/Ctrl+B (or the
 *    button) opens the blame of the row's version and closes the history; the
 *    blame's "File history" does the same the other way. Two overlays of one
 *    z-level would fight over focus and over Escape.
 *  - **Each row diffs under its own path.** `--follow` crosses renames, so the
 *    diff source takes `path` / `oldPath` from the row, never the path the
 *    history was opened with.
 *  - **The list is a snapshot.** The backend pins the history to the commit the
 *    first page was read from; a commit made meanwhile shows up on the next open.
 *  - **Merge commits are not listed** — `git log --follow` without `-m`; the
 *    footer says so, so an absent merge does not read as a missing commit.
 */

const PAGE = 200;
/** Two text lines per row. */
const ROW_H = 40;
const rowH = () => scaledPx(ROW_H);
/** Load the next page when the view comes this close to the end. */
const PREFETCH_ROWS = 30;
/** The diff follows the cursor once it stops, not on every step of a held key. */
const SETTLE_MS = 120;

type Target = { path: string; rev: string | null; revLabel: string | null; repo: string };

const [target, setTarget] = createSignal<Target | null>(null);
registerModalSource(() => target() !== null);

/**
 * Open the history of `path`. `rev` is where it starts (`null` — HEAD); the Log
 * mode passes the commit the file was picked in, because the path is the name
 * the file had *there* and may not exist at HEAD. `revLabel` is what the header
 * shows for it.
 */
export function openFileHistory(path: string, rev: string | null = null, revLabel?: string): void {
  const repo = state()?.repoPath;
  if (!repo || !path) return;
  setTarget({ path, rev, revLabel: revLabel ?? (rev ? rev.slice(0, 7) : null), repo });
}

/** The change a path of the Changes panel stands for, if it is one. */
export function changeOf(path: string) {
  return (state()?.changelists ?? []).flatMap((cl) => cl.files).find((x) => x.path === path);
}

/**
 * The name a working-tree path has at HEAD: a staged rename's history is under
 * its old name, and the new one does not exist there yet — a history opened by
 * it from HEAD is empty. Used by the Changes panel and by a working-tree blame.
 */
export function historyPathOf(path: string): string {
  const f = changeOf(path);
  return f?.status === "renamed" && f.oldPath ? f.oldPath : path;
}

/** Close the history — another overlay (the blame) is taking its place. */
export function closeFileHistory(): void {
  setTarget(null);
}

export default function FileHistoryPanel() {
  return (
    <Show when={target()} keyed>
      {(t) => <FileHistoryView target={t} />}
    </Show>
  );
}

const isRule = (e: unknown): boolean =>
  !!e && typeof e === "object" && (e as Partial<BackendError>).kind === "rule";

function FileHistoryView(props: { target: Target }) {
  let box: HTMLDivElement | undefined;
  let diffApi: DiffApi | undefined;
  let scrollToRow: ((i: number) => void) | undefined;
  const [scrollEl, setScrollEl] = createSignal<HTMLDivElement>();

  const [rows, setRows] = createSignal<FileHistoryCommit[]>([]);
  const [cursor, setCursor] = createSignal<FileHistoryCursor | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [error, setError] = createSignal("");
  const [index, setIndex] = createSignal(-1);
  const [shownIndex, setShownIndex] = createSignal(-1);

  // One generation per load from the top: a page that answers after a reload
  // belongs to a list no longer on screen.
  let seq = 0;

  const close = () => setTarget(null);

  const loadFirst = async () => {
    const my = ++seq;
    setLoading(true);
    setError("");
    try {
      const page = await fileHistory(props.target.path, props.target.rev, null, PAGE);
      if (my !== seq) return;
      setRows(page.commits);
      setCursor(page.nextCursor);
      setIndex(page.commits.length ? 0 : -1);
      setShownIndex(page.commits.length ? 0 : -1);
    } catch (e) {
      if (my !== seq) return;
      setRows([]);
      setCursor(null);
      setError(errText(e));
    } finally {
      if (my === seq) setLoading(false);
    }
  };

  const loadMore = async () => {
    const c = cursor();
    if (!c || loading() || loadingMore()) return;
    const my = seq;
    setLoadingMore(true);
    try {
      const page = await fileHistory(props.target.path, props.target.rev, c, PAGE);
      if (my !== seq) return;
      const have = new Set(rows().map((r) => r.hash));
      setRows([...rows(), ...page.commits.filter((r) => !have.has(r.hash))]);
      setCursor(page.nextCursor);
    } catch (e) {
      if (my !== seq) return;
      // The pinned commit is gone (a rewrite plus gc): read from the top once.
      if (isRule(e)) void loadFirst();
      else setError(errText(e));
    } finally {
      if (my === seq) setLoadingMore(false);
    }
  };

  void loadFirst();
  queueMicrotask(() => box?.focus());

  // A repository switched underneath (it cannot be from the keyboard while this
  // is up, but the window may reopen another one): the history is of the old one.
  createEffect(() => {
    if (state()?.repoPath !== props.target.repo) close();
  });

  let settle: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on(index, (i) => {
      clearTimeout(settle);
      settle = setTimeout(() => setShownIndex(i), SETTLE_MS);
    }, { defer: true }),
  );
  onCleanup(() => clearTimeout(settle));

  const current = () => rows()[index()] ?? null;
  const shown = createMemo(() => rows()[shownIndex()] ?? null);
  const source = createMemo<DiffSource | null>(
    () => {
      const c = shown();
      return c
        ? {
            kind: "commit",
            path: c.path,
            hash: c.hash,
            parent: c.parents[0] ?? null,
            oldPath: c.oldPath,
          }
        : null;
    },
    null,
    // A new object per read would re-ask git for the same diff.
    { equals: sameDiffSource },
  );

  const move = (to: number) => {
    const n = rows().length;
    if (n === 0) return;
    const i = Math.max(0, Math.min(to, n - 1));
    setIndex(i);
    scrollToRow?.(i);
  };

  const showInLog = async () => {
    const c = current();
    if (!c) return;
    close();
    await showCommitInLog(c.hash, c.shortHash);
  };

  /** Why the row's version cannot be blamed, or `null`. A row that deleted the
   * file (the old name seen from HEAD, see `engine::file_history`) has no file. */
  const blameReason = (): string | null => {
    const c = current();
    if (!c) return d().fileHistoryEmpty();
    return c.status === "deleted" ? d().blameDeletedReason() : null;
  };
  const blame = () => {
    const c = current();
    if (!c || blameReason()) return;
    openBlame(c.path, c.hash, c.shortHash);
  };

  // Keys that start inside the overlay stop here; the application layer is
  // standing down anyway while a modal source is open.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (isTypingTarget(e.target)) return;
    // A focused button (the diff's toolbar, Close) keeps its own Enter.
    if (e.code === "Enter" && (e.target as HTMLElement).closest?.("button")) return;
    const mod = e.metaKey || e.ctrlKey;
    let handled = true;
    switch (e.code) {
      case "ArrowDown":
        if (mod) diffApi?.next();
        else move(index() + 1);
        break;
      case "ArrowUp":
        if (mod) diffApi?.prev();
        else move(index() - 1);
        break;
      case "PageDown":
        move(index() + 10);
        break;
      case "PageUp":
        move(index() - 10);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(rows().length - 1);
        break;
      case "Enter":
        void showInLog();
        break;
      case "KeyB":
        if (mod) blame();
        else handled = false;
        break;
      default:
        handled = false;
    }
    if (handled) e.preventDefault();
  };

  const revLabel = () => props.target.revLabel;

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        class="flex h-[90vh] w-[94vw] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="truncate font-mono text-sm font-semibold" title={props.target.path}>
            {d().fileHistoryTitle(props.target.path)}
          </span>
          <Show when={revLabel()}>
            <span class="shrink-0 font-mono text-xs text-fg-muted">
              {d().fileHistoryFrom(revLabel()!)}
            </span>
          </Show>
          <Show when={!loading() && !error()}>
            <span class="shrink-0 text-xs text-fg-muted">
              ({d().fileHistoryCount(rows().length, cursor() !== null)})
            </span>
          </Show>
          <button
            class="ml-auto shrink-0 rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted disabled:opacity-50"
            disabled={blameReason() !== null}
            title={blameReason() ?? d().blameItem()}
            onClick={blame}
          >
            {d().blameItem()}
          </button>
          <button
            class="shrink-0 rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted disabled:opacity-50"
            disabled={!current()}
            title={current() ? d().fileHistoryShowInLog() : d().fileHistoryEmpty()}
            onClick={() => void showInLog()}
          >
            {d().fileHistoryShowInLog()}
          </button>
          <button
            class="shrink-0 rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>

        <Show when={error()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {error()}
          </pre>
        </Show>

        <div class="flex min-h-0 flex-1">
          <div
            ref={setScrollEl}
            class="w-[min(26rem,40%)] shrink-0 overflow-auto border-r border-border"
          >
            <Show
              when={!loading()}
              fallback={<Note title={d().fileHistoryLoading()} />}
            >
              <Show
                when={rows().length > 0}
                fallback={
                  <Show when={!error()}>
                    <Note title={d().fileHistoryEmpty()} hint={d().fileHistoryEmptyHint()} />
                  </Show>
                }
              >
                <Show when={scrollEl()}>
                  {(el) => (
                    <Rows
                      scrollEl={el()}
                      rows={rows()}
                      index={index()}
                      historyPath={props.target.path}
                      onPick={(i) => {
                        setIndex(i);
                        box?.focus();
                      }}
                      onOpen={(i) => {
                        setIndex(i);
                        void showInLog();
                      }}
                      onNearEnd={() => void loadMore()}
                      register={(fn) => (scrollToRow = fn)}
                    />
                  )}
                </Show>
                <Show when={loadingMore()}>
                  <div class="px-3 py-1 text-xs text-fg-muted">{d().fileHistoryLoadingMore()}</div>
                </Show>
              </Show>
            </Show>
          </div>

          <div class="min-w-0 flex-1">
            <Show when={source()} fallback={<Note title={d().selectCommitHint()} />}>
              <DiffView source={source()} api={(a) => (diffApi = a)} />
            </Show>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-3 py-1 text-[0.6875rem] text-fg-subtle">
          <span>{d().fileHistoryMergesNote()}</span>
          <span class="ml-auto">{d().fileHistoryKeys()}</span>
        </div>
      </div>
    </div>
  );
}

/**
 * The virtualised rows. Takes the scroll container as a prop for the reason
 * `LogTable`'s `VirtualRows` documents: a virtualizer bound before its container
 * exists shows the right height and no rows.
 */
function Rows(props: {
  scrollEl: HTMLDivElement;
  rows: FileHistoryCommit[];
  index: number;
  historyPath: string;
  onPick: (i: number) => void;
  onOpen: (i: number) => void;
  onNearEnd: () => void;
  register: (scrollToRow: (i: number) => void) => void;
}) {
  const virt = createVirtualizer({
    get count() {
      return props.rows.length;
    },
    getScrollElement: () => props.scrollEl,
    estimateSize: () => rowH(),
    overscan: 12,
  });
  createEffect(on(rowH, () => virt.measure(), { defer: true }));
  props.register((i) => virt.scrollToIndex(i, { align: "auto" }));

  createEffect(() => {
    const items = virt.getVirtualItems();
    const last = items.length ? items[items.length - 1].index : 0;
    if (props.rows.length > 0 && last >= props.rows.length - PREFETCH_ROWS) props.onNearEnd();
  });

  return (
    <div class="relative w-full" style={{ height: `${virt.getTotalSize()}px` }}>
      <For each={virt.getVirtualItems()}>
        {(vi) => {
          const row = createMemo(() => props.rows[vi.index]);
          return (
            <Show when={row()}>
              {(c) => (
                <div
                  class="absolute left-0 flex w-full cursor-default flex-col justify-center gap-0.5 overflow-hidden border-b border-border/50 px-3 text-xs hover:bg-bg-muted"
                  classList={{ "bg-accent/15": props.index === vi.index }}
                  style={{ top: `${vi.start}px`, height: `${rowH()}px` }}
                  onClick={() => props.onPick(vi.index)}
                  onDblClick={() => props.onOpen(vi.index)}
                >
                  <div class="flex min-w-0 items-center gap-2">
                    <span
                      class={`w-3 shrink-0 text-center font-bold ${statusMeta(c().status).cls}`}
                      title={c().status}
                    >
                      {statusMeta(c().status).letter}
                    </span>
                    <span class="shrink-0 font-mono text-fg-muted">{c().shortHash}</span>
                    <span class="truncate" title={c().subject}>
                      {c().subject}
                    </span>
                  </div>
                  <div class="flex min-w-0 items-center gap-2 pl-5 text-[0.6875rem] text-fg-subtle">
                    <span class="shrink-0">{c().author}</span>
                    <span class="shrink-0">{fmtDateTime(c().authorAt)}</span>
                    <For each={c().refs}>
                      {(r) => (
                        <span class="max-w-[8rem] shrink-0 truncate rounded border border-border px-1 font-mono text-fg-muted">
                          {r.name}
                        </span>
                      )}
                    </For>
                    <Show
                      when={c().oldPath}
                      fallback={
                        <Show when={c().path !== props.historyPath}>
                          <span class="truncate font-mono" title={c().path}>
                            {c().path}
                          </span>
                        </Show>
                      }
                    >
                      <span class="truncate font-mono" title={`${c().oldPath} → ${c().path}`}>
                        {d().fileHistoryRenamedFrom(c().oldPath!)}
                      </span>
                    </Show>
                  </div>
                </div>
              )}
            </Show>
          );
        }}
      </For>
    </div>
  );
}

function Note(props: { title: string; hint?: string }) {
  return (
    <div class="flex h-full flex-col items-center justify-center gap-1 p-4 text-center">
      <div class="text-xs text-fg-muted">{props.title}</div>
      <Show when={props.hint}>
        <div class="text-xs text-fg-subtle">{props.hint}</div>
      </Show>
    </div>
  );
}
