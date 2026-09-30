import { For, Index, Show, createEffect, createMemo, createSignal } from "solid-js";
import { opRebaseStart, type RebaseRange } from "../../api";
import { d } from "../../i18n";
import { busy, registerModalSource, state } from "../../store";
import { DISABLED_CLASS } from "../IconButton";
import { afterRepoChange, runResult } from "../log/actions/repoRefresh";
import {
  ACTIONS,
  ACTION_KEYS,
  fromCommits,
  melds,
  messageSlot,
  moveEntry,
  planProblem,
  preview,
  slotText,
  summary,
  toSteps,
  type Action,
  type PlanEntry,
} from "./rebaseRules";

/**
 * The plan of an interactive rebase (twig port, task 8): the commits from the one
 * the menu was opened on up to HEAD, oldest first — the order git replays them in
 * and the order the list is written to the todo — each with its action, and a
 * preview of the history the plan produces.
 *
 * Git executes the plan; the backend only hands it over (`engine::rebase`). What
 * this overlay owns is the editing: the rules — which row has a message field,
 * whether the plan can start, what the preview shows — are in `rebaseRules.ts`.
 *
 * An overlay in the `DiscardPanel` manner: `z-40` under the store's modals, a modal
 * source, so the application key layer stands down while it is up, and its own
 * `onKeyDown`. Row keys (P R E S F D, arrows, Alt+arrows) act only while the focus
 * is not in a text field — typing "r" into a message must not turn the row into a
 * reword. On the action `<select>` the letters are taken too: the native type-ahead
 * would match the *localised* option labels, which do not start with them.
 */

type Target = { hash: string; shortHash: string; range: RebaseRange; repo: string };

const [target, setTarget] = createSignal<Target | null>(null);
registerModalSource(() => target() !== null);

/** Where the focus was when the overlay opened, to give it back on close. */
let returnFocus: HTMLElement | null = null;

/**
 * Show the plan for rewriting `range` (read from `hash`). The caller has already
 * checked the range and asked about published commits (`commitActions.ts`).
 */
export function openRebasePlan(t: { hash: string; shortHash: string; range: RebaseRange }): void {
  const repo = state()?.repoPath;
  if (!repo || target()) return;
  const el = document.activeElement;
  returnFocus = el instanceof HTMLElement ? el : null;
  setTarget({ ...t, repo });
}

function close(): void {
  setTarget(null);
  const el = returnFocus;
  returnFocus = null;
  if (el?.isConnected) queueMicrotask(() => el.focus());
}

export default function RebasePanel() {
  return (
    <Show when={target()} keyed>
      {(t) => <RebaseView target={t} />}
    </Show>
  );
}

/** Rows are an `Index`, not a `For`: every keystroke in a message replaces the
 *  row's object, and a `For` keyed on it would rebuild the textarea and drop the
 *  focus after each character. */
const isTextField = (t: EventTarget | null) =>
  t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement;

function RebaseView(props: { target: Target }) {
  const [entries, setEntries] = createSignal<PlanEntry[]>(fromCommits(props.target.range.commits));
  const [cursor, setCursor] = createSignal(0);
  const [failure, setFailure] = createSignal("");
  const [running, setRunning] = createSignal(false);
  const rows: HTMLElement[] = [];
  let box: HTMLDivElement | undefined;

  // Another repository opened underneath: this plan belongs to the old one.
  createEffect(() => {
    if (state()?.repoPath !== props.target.repo) close();
  });
  createEffect(() => {
    if (props.target) queueMicrotask(() => rows[0]?.focus() ?? box?.focus());
  });

  const problem = createMemo(() => planProblem(entries()));
  const counts = createMemo(() => summary(entries()));
  const result = createMemo(() => preview(entries()));

  const focusRow = (i: number) => queueMicrotask(() => rows[i]?.focus());
  const setAction = (i: number, action: Action) =>
    setEntries((l) => l.map((e, k) => (k === i ? { ...e, action } : e)));
  const setText = (i: number, text: string) =>
    setEntries((l) => l.map((e, k) => (k === i ? { ...e, text } : e)));
  const move = (from: number, to: number) => {
    const list = entries();
    if (to < 0 || to >= list.length) return;
    setEntries(moveEntry(list, from, to));
    setCursor(to);
    focusRow(to);
  };
  const goTo = (i: number) => {
    const n = entries().length;
    if (n === 0) return;
    const at = Math.max(0, Math.min(n - 1, i));
    setCursor(at);
    focusRow(at);
  };

  const start = async () => {
    if (running() || busy() || problem()) return;
    setRunning(true);
    setFailure("");
    const err = await runResult(
      opRebaseStart(props.target.hash, toSteps(entries())),
      d().phaseRebaseInteractive(),
    );
    setRunning(false);
    // A bisect is not a stop of this rebase (and refuses it before git runs).
    const kind = state()?.operation?.kind ?? "none";
    const stopped = kind !== "none" && kind !== "bisect";
    // Refused before git ran (a stale plan, a dirty tree): the plan stays on
    // screen with the reason. Anything else changed the repository — a finished
    // rebase, or one stopped on a conflict or an `edit`, which the operation
    // strip now drives.
    if (err && !stopped) {
      setFailure(err);
      return;
    }
    close();
    afterRepoChange();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if ((e.code === "Enter" || e.code === "NumpadEnter") && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void start();
      return;
    }
    if (isTextField(e.target)) return;
    const onSelect = e.target instanceof HTMLSelectElement;
    const up = e.code === "ArrowUp";
    const down = e.code === "ArrowDown";
    if (e.altKey && (up || down)) {
      e.preventDefault();
      move(cursor(), cursor() + (up ? -1 : 1));
      return;
    }
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    const action = ACTION_KEYS[e.code];
    if (action) {
      e.preventDefault();
      setAction(cursor(), action);
      return;
    }
    if (onSelect) return; // arrows on a select pick an option
    if (up || down) {
      e.preventDefault();
      goTo(cursor() + (up ? -1 : 1));
    } else if (e.code === "Home") {
      e.preventDefault();
      goTo(0);
    } else if (e.code === "End") {
      e.preventDefault();
      goTo(entries().length - 1);
    }
  };

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        role="dialog"
        aria-label={d().rbTitle(props.target.shortHash)}
        class="flex h-[min(40rem,88vh)] w-[min(64rem,94vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-start gap-2 border-b border-border px-4 py-2">
          <div class="min-w-0 flex-1">
            <div class="text-sm font-semibold">{d().rbTitle(props.target.shortHash)}</div>
            <div class="text-xs text-fg-subtle">{d().rbNote(entries().length)}</div>
          </div>
          <button
            class="rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().cancel()}
          </button>
        </div>

        <div class="flex min-h-0 flex-1">
          <ol class="min-w-0 flex-1 overflow-auto py-1" role="listbox">
            <Index each={entries()}>
              {(e, i) => (
                <li
                  ref={(el) => (rows[i] = el)}
                  role="option"
                  tabindex={-1}
                  aria-selected={cursor() === i}
                  class="border-l-2 px-2 py-1 outline-none"
                  classList={{
                    "border-accent bg-accent/10": cursor() === i,
                    "border-transparent": cursor() !== i,
                    "pl-8": melds(e().action),
                  }}
                  onFocusIn={() => setCursor(i)}
                  onMouseDown={() => setCursor(i)}
                >
                  <div class="flex items-center gap-2 text-xs">
                    <div class="flex shrink-0 gap-0.5">
                      <button
                        class={`rounded border border-border px-1 leading-4 hover:bg-bg-muted ${DISABLED_CLASS}`}
                        title={d().rbMoveUp()}
                        aria-label={d().rbMoveUp()}
                        disabled={i === 0}
                        onClick={() => move(i, i - 1)}
                      >
                        ↑
                      </button>
                      <button
                        class={`rounded border border-border px-1 leading-4 hover:bg-bg-muted ${DISABLED_CLASS}`}
                        title={d().rbMoveDown()}
                        aria-label={d().rbMoveDown()}
                        disabled={i === entries().length - 1}
                        onClick={() => move(i, i + 1)}
                      >
                        ↓
                      </button>
                    </div>
                    <select
                      class="w-56 shrink-0 rounded border border-border bg-bg-muted px-1 py-0.5 text-xs text-fg outline-none focus:border-accent"
                      aria-label={d().rbActionFor(e().shortHash)}
                      value={e().action}
                      onChange={(ev) => setAction(i, ev.currentTarget.value as Action)}
                    >
                      <For each={ACTIONS}>{(a) => <option value={a}>{d().rbAction(a)}</option>}</For>
                    </select>
                    <code class="shrink-0 text-fg-muted">{e().shortHash}</code>
                    <span
                      class="min-w-0 flex-1 truncate"
                      classList={{
                        "text-fg-muted line-through": e().action === "drop",
                        "text-warn": e().action === "edit",
                      }}
                      title={e().original}
                    >
                      {e().subject}
                    </span>
                  </div>
                  <Show when={messageSlot(entries(), i)}>
                    {(slot) => (
                      <label class="mt-1 block pl-14 text-[0.6875rem] text-fg-muted">
                        {slot() === "reword" ? d().rbMessageReword() : d().rbMessageCombined()}
                        <textarea
                          rows={3}
                          autocomplete="off"
                          autocorrect="off"
                          autocapitalize="off"
                          spellcheck={false}
                          class="mt-0.5 w-full resize-y rounded border border-border bg-bg-muted px-2 py-1 font-mono text-xs text-fg outline-none focus:border-accent"
                          value={slotText(entries(), i)}
                          onInput={(ev) => setText(i, ev.currentTarget.value)}
                        />
                      </label>
                    )}
                  </Show>
                </li>
              )}
            </Index>
          </ol>

          <div class="flex w-72 shrink-0 flex-col border-l border-border">
            <div class="border-b border-border px-3 py-1 text-[0.6875rem] font-semibold uppercase tracking-wide text-fg-subtle">
              {d().rbPreviewTitle()}
            </div>
            <ol class="min-h-0 flex-1 overflow-auto py-1">
              <For each={result()}>
                {(c) => (
                  <li class="px-3 py-1 text-xs">
                    <div class="truncate" title={c.subject}>
                      {c.subject}
                    </div>
                    <div class="flex flex-wrap gap-1 text-[0.625rem] text-fg-muted">
                      <code>{c.from.join(" + ")}</code>
                      <Show when={c.reworded}>
                        <span class="text-accent">{d().rbPreviewReworded()}</span>
                      </Show>
                      <Show when={c.stops}>
                        <span class="text-warn">{d().rbPreviewStops()}</span>
                      </Show>
                    </div>
                  </li>
                )}
              </For>
            </ol>
          </div>
        </div>

        <Show when={failure()}>
          <pre class="max-h-28 overflow-auto whitespace-pre-wrap border-t border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {failure()}
          </pre>
        </Show>

        <div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2">
          <span class="text-[0.6875rem] text-fg-subtle">{d().rbKeysHint()}</span>
          <span
            class="ml-auto text-xs"
            classList={{ "text-danger": !!problem(), "text-fg-muted": !problem() }}
            role="status"
          >
            {problem()
              ? d().rbProblem(problem() as string)
              : d().rbSummary(counts().kept, counts().melded, counts().dropped)}
          </span>
          <button
            class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
            onClick={close}
          >
            {d().cancel()}
          </button>
          <button
            class={`rounded bg-accent px-3 py-1 text-sm text-white ${DISABLED_CLASS}`}
            disabled={running() || busy() || !!problem()}
            title={problem() ? d().rbProblem(problem() as string) : d().rbStart()}
            onClick={() => void start()}
          >
            {d().rbStart()}
          </button>
        </div>
      </div>
    </div>
  );
}
