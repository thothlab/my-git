import { For, Show, createEffect, createResource, createSignal } from "solid-js";
import {
  errText,
  stashApply,
  stashDrop,
  stashFiles,
  stashList,
  stashPop,
  stashPush,
  type RepoState,
  type StashEntry,
} from "../api";
import { d, fmtDateTime } from "../i18n";
import {
  busy,
  confirmAction,
  promptText,
  registerModalSource,
  run,
  statusMeta,
  state,
} from "../store";
import { afterRepoChange } from "./log/actions/repoRefresh";
import { DISABLED_CLASS } from "./IconButton";

/**
 * The stash manager: every stash of the repository, what is in it, and the four
 * things one can do with them.
 *
 * Why it lives in `App` rather than in a panel: the window has two modes and the
 * stashes belong to neither. Mounted next to `ModalHost`, it is reachable from
 * the Changes toolbar and from the branch menu without either mode owning it.
 *
 * Three seams worth naming:
 *
 *  - **The selection is held by hash, not by `stash@{N}`.** git renumbers the
 *    refs after every pop and drop, so a selection kept as a ref would, after a
 *    drop, quietly point at a *different* stash — and `stash_files` has no
 *    staleness check to catch it, unlike apply/pop/drop, which are given the
 *    hash and refuse a list that has moved.
 *  - **`run()` refreshes `RepoState`, which is not the stash list.** Every
 *    mutation here reloads the list explicitly.
 *  - **`z-40`, below the store's modals (`z-50`).** The drop confirmation is a
 *    store modal and has to render *over* this panel; drawn under it, the click
 *    on "Drop" would look like a hang.
 */

const [open, setOpen] = createSignal(false);
const [stashes, setStashes] = createSignal<StashEntry[]>([]);
const [listError, setListError] = createSignal("");
const [selectedHash, setSelectedHash] = createSignal<string | null>(null);

registerModalSource(open);

/** How many stashes the repository holds — the number the Changes panel shows. */
export const stashCount = () => stashes().length;

/**
 * Re-read the list. `report` is false for the background refresh that only feeds
 * the counter: a repository that cannot be read there is already announced by
 * everything else in the window, and a second banner adds nothing.
 */
export async function reloadStashes(report = true): Promise<void> {
  if (!state()) {
    setStashes([]);
    return;
  }
  try {
    const list = await stashList();
    setStashes(list);
    setListError("");
    // The selected stash may be gone (dropped, popped) — the hash says so.
    if (!list.some((s) => s.hash === selectedHash())) {
      setSelectedHash(list[0]?.hash ?? null);
    }
  } catch (e) {
    if (report) setListError(errText(e));
  }
}

export function openStashPanel(): void {
  setOpen(true);
  void reloadStashes();
}

export default function StashPanel() {
  return (
    <Show when={open()}>
      <StashPanelView />
    </Show>
  );
}

function StashPanelView() {
  let box: HTMLDivElement | undefined;
  const selected = () => stashes().find((s) => s.hash === selectedHash()) ?? null;

  // Keyed on the whole entry: the ref alone renumbers, and a source that did not
  // change would keep the previous stash's files on screen after a drop.
  const [files] = createResource(selected, (s) => stashFiles(s.ref));

  const close = () => setOpen(false);

  createEffect(() => {
    if (open()) queueMicrotask(() => box?.focus());
  });

  const label = (s: StashEntry) =>
    `${fmtDateTime(s.at)} · ${s.branch ?? d().stashNoBranch()} · ${s.message || d().stashNoMessage()}`;

  const act = async (p: Promise<RepoState>, phase: string) => {
    await run(p, phase);
    await reloadStashes();
    afterRepoChange({ log: false });
  };

  const doApply = () => {
    const s = selected();
    if (s) void act(stashApply(s.ref, s.hash), d().phaseStashApply());
  };
  const doPop = () => {
    const s = selected();
    if (s) void act(stashPop(s.ref, s.hash), d().phaseStashPop());
  };
  const doDrop = async () => {
    const s = selected();
    if (!s) return;
    if (!(await confirmAction(d().confirmStashDrop(label(s)), true))) return;
    await act(stashDrop(s.ref, s.hash), d().phaseStashDrop());
  };
  const doPush = async () => {
    const message = await promptText(d().stashPushTitle(), "");
    if (message === null) return;
    await act(stashPush(message.trim() || undefined), d().phaseStashPush());
  };

  // The application layer does not know about this panel; keys that start inside
  // it stop here. Escape is not taken in the capture phase on purpose: while a
  // store modal (the drop confirmation) is up, that modal owns Escape.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      close();
    }
  };

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        class="flex h-[min(34rem,84vh)] w-[min(52rem,92vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="text-sm font-semibold">{d().stashesTitle()}</span>
          <span class="text-xs text-fg-muted">({stashes().length})</span>
          <button
            class="ml-auto rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>

        <Show when={listError()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[11px] text-danger">
            {listError()}
          </pre>
        </Show>

        <div class="flex min-h-0 flex-1">
          <div class="w-1/2 shrink-0 overflow-auto border-r border-border py-1">
            <Show
              when={stashes().length > 0}
              fallback={
                <div class="p-4 text-center text-xs text-fg-muted">{d().stashesEmpty()}</div>
              }
            >
              <For each={stashes()}>
                {(s) => (
                  <button
                    class="flex w-full flex-col items-start gap-0.5 px-3 py-1.5 text-left text-xs hover:bg-bg-muted"
                    classList={{ "bg-accent/10": s.hash === selectedHash() }}
                    onClick={() => setSelectedHash(s.hash)}
                  >
                    <span class="flex w-full items-center gap-2">
                      <span class="truncate font-medium">
                        {s.message || d().stashNoMessage()}
                      </span>
                      <Show when={s.fromApp}>
                        <span
                          class="shrink-0 rounded bg-accent/20 px-1 text-[10px] text-accent"
                          title={d().stashFromAppTip()}
                        >
                          {d().stashFromApp()}
                        </span>
                      </Show>
                    </span>
                    <span class="w-full truncate text-[11px] text-fg-muted">
                      {fmtDateTime(s.at)} · {s.branch ?? d().stashNoBranch()} · {s.ref}
                    </span>
                  </button>
                )}
              </For>
            </Show>
          </div>

          <div class="flex min-w-0 flex-1 flex-col">
            <div class="border-b border-border px-3 py-1 text-[11px] font-semibold uppercase tracking-wide text-fg-subtle">
              {d().stashFilesTitle()}
            </div>
            <div class="min-h-0 flex-1 overflow-auto py-1">
              <Show when={selected()} fallback={<Empty text={d().stashSelectOne()} />}>
                <Show when={!files.loading} fallback={<Empty text={d().whyChecking()} />}>
                  <Show
                    when={!files.error}
                    fallback={
                      <pre class="m-2 overflow-auto whitespace-pre-wrap rounded border border-danger/40 bg-danger/10 p-2 font-mono text-[11px] text-danger">
                        {errText(files.error)}
                      </pre>
                    }
                  >
                    <Show
                      when={(files() ?? []).length > 0}
                      fallback={<Empty text={d().stashFilesEmpty()} />}
                    >
                      <For each={files()}>
                        {(f) => (
                          <div class="flex items-center gap-2 px-3 py-0.5 text-xs">
                            <span class={`w-3 shrink-0 text-center ${statusMeta(f.status).cls}`}>
                              {statusMeta(f.status).letter}
                            </span>
                            <span class="truncate" title={f.path}>
                              {f.path}
                            </span>
                          </div>
                        )}
                      </For>
                    </Show>
                  </Show>
                </Show>
              </Show>
            </div>
            <div class="border-t border-border px-3 py-1 text-[11px] text-fg-subtle">
              {d().stashFilesNote()}
            </div>
          </div>
        </div>

        {/* Wraps: the Russian labels are much longer than the English ones, and the
            window may be as narrow as 720px. */}
        <div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2">
          <button
            class={`rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted ${DISABLED_CLASS}`}
            disabled={busy()}
            onClick={() => void doPush()}
          >
            {d().stashPushBtn()}
          </button>
          <button
            class={`ml-auto rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted ${DISABLED_CLASS}`}
            disabled={busy() || !selected()}
            title={selected() ? d().stashApplyTip() : d().stashSelectOne()}
            onClick={doApply}
          >
            {d().stashApplyBtn()}
          </button>
          <button
            class={`rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted ${DISABLED_CLASS}`}
            disabled={busy() || !selected()}
            title={selected() ? d().stashPopTip() : d().stashSelectOne()}
            onClick={doPop}
          >
            {d().stashPopBtn()}
          </button>
          <button
            class={`rounded bg-danger px-3 py-1 text-sm text-white ${DISABLED_CLASS}`}
            disabled={busy() || !selected()}
            title={selected() ? d().stashDropTip() : d().stashSelectOne()}
            onClick={() => void doDrop()}
          >
            {d().stashDropBtn()}
          </button>
        </div>
      </div>
    </div>
  );
}

function Empty(props: { text: string }) {
  return <div class="p-4 text-center text-xs text-fg-muted">{props.text}</div>;
}
