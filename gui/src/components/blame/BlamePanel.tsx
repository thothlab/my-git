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
  WORKING_TREE,
  errText,
  fileBlame,
  fileBlameBefore,
  type Blame,
  type BlameLine,
  type BlameOrigin,
} from "../../api";
import { isTypingTarget } from "../../hotkeys";
import { d, fmtAgo, fmtDateTime } from "../../i18n";
import { registerModalSource, scaledPx, state } from "../../store";
import DiffView, { type DiffApi } from "../DiffView";
import { sameDiffSource, type DiffSource } from "../diff/model";
import { closeFileHistory, historyPathOf, openFileHistory } from "../FileHistoryPanel";
import { showCommitInLog } from "../log/actions/showInLog";
import {
  AGE_LEVELS,
  ageLevels,
  beforeBlock,
  landingLine,
  maxColumns,
  runStarts,
} from "./blameRules";

/**
 * Blame of one file (R05b): which commit last changed each line, the commit and
 * the file's diff in it for the chosen line, and "blame before this change" —
 * the version the line's commit started from, landing on where the line was.
 *
 * An overlay in the `FileHistoryPanel` manner, for the same reasons: it opens
 * from both modes (a file in Changes, a file of a commit, a row of the file
 * history), and as a modal source it silences the keyboard layer, so it answers
 * its own keys in `onKeyDown` — `hotkeys.ts` stands down while any modal is up
 * and a `registerHotkey` would never fire here.
 *
 * Seams worth naming:
 *
 *  - **The stack of "blame before" steps is local.** Each step keeps its blame,
 *    the chosen line and the scroll position; Escape (or Back) pops one, and
 *    closes the overlay only from the first. Nothing is fetched again on the way
 *    back.
 *  - **The line cursor is not a fourth selection of the window.** Enter hands
 *    the commit to the log's own selection through `showCommitInLog`, the same
 *    helper the file history uses.
 *  - **It replaces the file history instead of stacking on it**, and the other
 *    way round: two overlays of one z-level would fight over focus and Escape.
 *  - **Uncommitted lines** (a working-tree blame) have no commit: the diff is the
 *    working tree against the version before, and "open in log" says why not.
 *  - **Line numbers step back through the diff, not by equality.** The backend
 *    maps the line through the hunks of its commit (`engine::blame::before`); a
 *    line the commit introduced lands on what it replaced, highlighted.
 */

/** One text line per row, the diff editor's line height. */
const ROW_H = 18;
const rowH = () => scaledPx(ROW_H);
/** The diff follows the cursor once it stops, not on every step of a held key. */
const SETTLE_MS = 120;
/** Width of the annotation column, in rem — it scales with the text. */
const GUTTER_REM = 18;

type Target = { path: string; rev: string | null; revLabel: string | null; repo: string };

type Step = {
  /** As asked for: `null` — the working tree. */
  rev: string | null;
  path: string;
  revLabel: string | null;
  blame: Blame | null;
  /** Where "blame before" landed; `null` on the first step. */
  landing: { from: number; to: number; exact: boolean } | null;
  /** The chosen line and the scroll position, kept for the way back. */
  line: number;
  scrollTop: number;
};

const [target, setTarget] = createSignal<Target | null>(null);
registerModalSource(() => target() !== null);

/** Where the focus was when the overlay opened, to give it back on close. */
let returnFocus: HTMLElement | null = null;

/**
 * Open the blame of `path` at `rev` — any revision naming a commit, `null` for
 * the working tree. `revLabel` is what the header shows for it. Closes the file
 * history if that is what it was opened from.
 */
export function openBlame(path: string, rev: string | null = null, revLabel?: string): void {
  const repo = state()?.repoPath;
  if (!repo || !path) return;
  closeFileHistory();
  if (!target()) {
    const el = document.activeElement;
    returnFocus = el instanceof HTMLElement ? el : null;
  }
  setTarget({ path, rev, revLabel: revLabel ?? (rev ? rev.slice(0, 7) : null), repo });
}

/** `restore: false` — another place takes the focus (the log, the history). */
function closeBlame(restore = true): void {
  setTarget(null);
  const el = returnFocus;
  returnFocus = null;
  if (restore && el?.isConnected) queueMicrotask(() => el.focus());
}

export default function BlamePanel() {
  return (
    <Show when={target()} keyed>
      {(t) => <BlameView target={t} />}
    </Show>
  );
}

function BlameView(props: { target: Target }) {
  let box: HTMLDivElement | undefined;
  let diffApi: DiffApi | undefined;
  let scrollToRow: ((i: number, align: "auto" | "center") => void) | undefined;
  const [scrollEl, setScrollEl] = createSignal<HTMLDivElement>();

  const [stack, setStack] = createSignal<Step[]>([
    {
      rev: props.target.rev,
      path: props.target.path,
      revLabel: props.target.revLabel,
      blame: null,
      landing: null,
      line: 1,
      scrollTop: 0,
    },
  ]);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal("");
  /** The chosen line, 1-based. */
  const [line, setLine] = createSignal(1);
  const [shownLine, setShownLine] = createSignal(1);

  // One generation per request: an answer that arrives after Back, or after a
  // newer step, belongs to a screen no longer shown.
  let seq = 0;

  const step = () => stack()[stack().length - 1];
  const blame = () => step().blame;
  const lines = (): BlameLine[] => blame()?.lines ?? [];
  const origins = (): BlameOrigin[] => blame()?.origins ?? [];
  const starts = createMemo(() => runStarts(lines()));
  const ages = createMemo(() => ageLevels(origins()));
  const cols = createMemo(() => maxColumns(lines()));

  const originOf = (n: number): BlameOrigin | null => {
    const l = lines()[n - 1];
    return l ? (origins()[l.origin] ?? null) : null;
  };
  const currentLine = () => lines()[line() - 1] ?? null;
  const current = () => originOf(line());
  const shown = () => originOf(shownLine());

  const patchTop = (patch: Partial<Step>) =>
    setStack((s) => [...s.slice(0, -1), { ...s[s.length - 1], ...patch }]);

  const select = (n: number, align: "auto" | "center" = "auto") => {
    const count = lines().length;
    if (count === 0) return;
    const i = Math.max(1, Math.min(n, count));
    setLine(i);
    scrollToRow?.(i - 1, align);
  };

  const close = closeBlame;

  const loadFirst = async () => {
    const my = ++seq;
    setLoading(true);
    setError("");
    try {
      const b = await fileBlame(props.target.path, props.target.rev);
      if (my !== seq) return;
      patchTop({ blame: b });
      setLine(1);
      setShownLine(1);
    } catch (e) {
      if (my !== seq) return;
      setError(errText(e));
    } finally {
      if (my === seq) setLoading(false);
    }
  };

  void loadFirst();
  queueMicrotask(() => box?.focus());

  // The repository switched underneath: the blame is of the old one.
  createEffect(() => {
    if (state()?.repoPath !== props.target.repo) close(false);
  });

  let settle: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on(
      [line, step],
      ([n]) => {
        clearTimeout(settle);
        settle = setTimeout(() => setShownLine(n), SETTLE_MS);
      },
      { defer: true },
    ),
  );
  onCleanup(() => clearTimeout(settle));

  const source = createMemo<DiffSource | null>(
    () => {
      const o = shown();
      if (!o) return null;
      if (o.uncommitted)
        return {
          kind: "compare",
          path: o.path,
          from: o.previous?.hash ?? "HEAD",
          to: WORKING_TREE,
        };
      const parent = o.parents[0] ?? null;
      return {
        kind: "commit",
        path: o.path,
        hash: o.hash,
        parent,
        // `previous` names the parent git blamed further; it is the rename
        // source only when that parent is the one the diff is taken against.
        oldPath: o.previous && o.previous.hash === parent ? o.previous.path : null,
      };
    },
    null,
    // A new object per read would re-ask git for the same diff.
    { equals: sameDiffSource },
  );

  const beforeReason = (): string | null => {
    const o = current();
    if (!o) return d().blameSelectLine();
    const b = beforeBlock(o);
    if (b === "boundary") return d().blameBeforeBoundary();
    if (b === "created") return d().blameBeforeCreated();
    if (b === "new") return d().blameBeforeNew();
    return null;
  };
  const logReason = (): string | null => {
    const o = current();
    if (!o) return d().blameSelectLine();
    return o.uncommitted ? d().blameOpenInLogUncommitted() : null;
  };

  const stepBack = async () => {
    const l = currentLine();
    const o = current();
    if (!l || !o?.previous) return;
    const my = ++seq;
    setLoading(true);
    setError("");
    const here = { line: line(), scrollTop: scrollEl()?.scrollTop ?? 0 };
    try {
      const r = await fileBlameBefore(o.hash, o.path, l.origLine, o.previous.hash, o.previous.path);
      if (my !== seq) return;
      setStack((s) => [
        ...s.slice(0, -1),
        { ...s[s.length - 1], ...here },
        {
          rev: r.blame.rev,
          path: r.blame.path,
          revLabel: r.blame.rev ? r.blame.rev.slice(0, 7) : null,
          blame: r.blame,
          landing: { from: r.from, to: r.to, exact: r.exact },
          line: 1,
          scrollTop: 0,
        },
      ]);
      const at = landingLine(r, r.blame.lines.length) ?? 1;
      // Both at once: the settled line would otherwise be read in the new blame
      // for a moment and fetch the diff of whatever commit sits there.
      setLine(at);
      setShownLine(at);
      // After the rows of the new step exist.
      requestAnimationFrame(() => select(at, "center"));
    } catch (e) {
      if (my === seq) setError(errText(e));
    } finally {
      if (my === seq) setLoading(false);
    }
  };

  /** Pop one step; `false` when there is nothing to go back to. */
  const back = (): boolean => {
    const s = stack();
    if (s.length <= 1) return false;
    ++seq;
    setLoading(false);
    setError("");
    const prev = s[s.length - 2];
    setStack(s.slice(0, -1));
    setLine(prev.line);
    setShownLine(prev.line);
    requestAnimationFrame(() => {
      const el = scrollEl();
      if (el) el.scrollTop = prev.scrollTop;
    });
    box?.focus();
    return true;
  };

  const openInLog = async () => {
    const o = current();
    if (!o || o.uncommitted) return;
    close(false);
    await showCommitInLog(o.hash, o.shortHash);
  };

  /** The history of the version on screen. A working-tree step starts from
   * HEAD, under the name the file has there (a staged rename is not in HEAD). */
  const history = () => {
    const s = step();
    close(false);
    if (s.rev === null) openFileHistory(historyPathOf(s.path));
    else openFileHistory(s.path, s.rev, s.revLabel ?? undefined);
  };

  const page = () => Math.max(1, Math.floor((scrollEl()?.clientHeight ?? 0) / rowH()) - 1);

  // Keys that start inside the overlay stop here; the application layer is
  // standing down anyway while a modal source is open.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      if (!back()) close();
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
        else select(line() + 1);
        break;
      case "ArrowUp":
        if (mod) diffApi?.prev();
        else select(line() - 1);
        break;
      case "PageDown":
        select(line() + page());
        break;
      case "PageUp":
        select(line() - page());
        break;
      case "Home":
        select(1);
        break;
      case "End":
        select(lines().length);
        break;
      case "Enter":
        void openInLog();
        break;
      case "KeyB":
        if (mod) void stepBack();
        else handled = false;
        break;
      default:
        handled = false;
    }
    if (handled) e.preventDefault();
  };

  const revText = () => {
    const s = step();
    return s.rev === null ? d().blameWorkingTree() : d().blameAt(s.revLabel ?? s.rev.slice(0, 7));
  };
  const landing = () => step().landing;

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        class="flex h-[90vh] w-[94vw] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <Show when={stack().length > 1}>
            <button
              class="shrink-0 rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
              title={d().blameBackTip()}
              onClick={() => back()}
            >
              {d().blameBack(stack().length - 1)}
            </button>
          </Show>
          <span class="truncate font-mono text-sm font-semibold" title={step().path}>
            {d().blameTitle(step().path)}
          </span>
          <span class="shrink-0 font-mono text-xs text-fg-muted" title={step().rev ?? undefined}>
            {revText()}
          </span>
          <Show when={blame() && !blame()!.blocked}>
            <span class="shrink-0 text-xs text-fg-muted">({d().blameLines(lines().length)})</span>
          </Show>
          <Show when={loading() && blame()}>
            <span class="shrink-0 text-xs text-fg-subtle">{d().blameLoading()}</span>
          </Show>
          <button
            class="ml-auto shrink-0 rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={() => close()}
          >
            {d().close()}
          </button>
        </div>

        <Show when={landing()}>
          {(l) => (
            <div class="border-b border-border bg-warn/10 px-4 py-1 text-[0.6875rem] text-fg-muted">
              {l().exact ? d().blameLandedExact(l().from) : d().blameLanded(l().from, l().to)}
            </div>
          )}
        </Show>

        <Show when={error()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {error()}
          </pre>
        </Show>

        <div class="flex min-h-0 flex-1">
          <div ref={setScrollEl} class="min-w-0 flex-1 overflow-auto">
            <Show
              when={blame()}
              fallback={<Note title={loading() ? d().blameLoading() : ""} />}
            >
              {(b) => (
                <Show
                  when={!b().blocked}
                  fallback={<Note title={d().blameBlocked(b().blocked!)} />}
                >
                  <Show when={b().lines.length > 0} fallback={<Note title={d().blameEmptyFile()} />}>
                    <Show when={scrollEl()}>
                      {(el) => (
                        <Rows
                          scrollEl={el()}
                          lines={lines()}
                          origins={origins()}
                          starts={starts()}
                          ages={ages()}
                          cols={cols()}
                          line={line()}
                          landing={landing()}
                          onPick={(n) => {
                            setLine(n);
                            box?.focus();
                          }}
                          register={(fn) => (scrollToRow = fn)}
                        />
                      )}
                    </Show>
                  </Show>
                </Show>
              )}
            </Show>
          </div>

          <div class="flex w-[min(42rem,48%)] shrink-0 flex-col border-l border-border">
            <Show when={current()} fallback={<Note title={d().blameSelectLine()} />}>
              {(o) => (
                <div class="space-y-1 border-b border-border px-3 py-2 text-xs">
                  <div class="break-words text-sm font-semibold">
                    {o().uncommitted ? d().blameUncommitted() : o().summary}
                  </div>
                  <Show
                    when={!o().uncommitted}
                    fallback={<div class="text-fg-muted">{d().blameUncommittedNote()}</div>}
                  >
                    <div class="text-fg-muted">
                      {o().author} &lt;{o().authorEmail}&gt;
                    </div>
                    <div class="text-fg-muted">
                      {fmtDateTime(o().authorAt)} · {fmtAgo(o().authorAt)}
                    </div>
                    <div class="font-mono text-fg-subtle" title={o().hash}>
                      {o().shortHash}
                    </div>
                  </Show>
                  <Show when={o().path !== step().path}>
                    <div class="truncate font-mono text-fg-muted" title={o().path}>
                      {d().blamePathInCommit(o().path)}
                    </div>
                  </Show>
                  <div class="flex flex-wrap gap-2 pt-1">
                    <button
                      class="rounded border border-border px-2 py-0.5 hover:bg-bg-muted disabled:opacity-50"
                      disabled={logReason() !== null}
                      title={logReason() ?? d().blameOpenInLog()}
                      onClick={() => void openInLog()}
                    >
                      {d().blameOpenInLog()}
                    </button>
                    <button
                      class="rounded border border-border px-2 py-0.5 hover:bg-bg-muted disabled:opacity-50"
                      disabled={beforeReason() !== null || loading()}
                      title={beforeReason() ?? d().blameBefore()}
                      onClick={() => void stepBack()}
                    >
                      {d().blameBefore()}
                    </button>
                    <button
                      class="rounded border border-border px-2 py-0.5 hover:bg-bg-muted"
                      onClick={history}
                    >
                      {d().fileHistoryItem()}
                    </button>
                  </div>
                  <Show when={beforeReason()}>
                    <div class="text-[0.6875rem] text-fg-subtle">{beforeReason()}</div>
                  </Show>
                </div>
              )}
            </Show>
            <div class="min-h-0 flex-1">
              <Show when={source()}>
                <DiffView source={source()} api={(a) => (diffApi = a)} />
              </Show>
            </div>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-3 py-1 text-[0.6875rem] text-fg-subtle">
          <span class="ml-auto">{d().blameKeys()}</span>
        </div>
      </div>
    </div>
  );
}

/**
 * The virtualised lines. Takes the scroll container as a prop for the reason
 * `LogTable`'s `VirtualRows` documents: a virtualizer bound before its container
 * exists shows the right height and no rows.
 *
 * The inner box is as wide as the widest line (in `ch` of the monospace font,
 * TABs expanded), so all rows scroll sideways together.
 */
function Rows(props: {
  scrollEl: HTMLDivElement;
  lines: BlameLine[];
  origins: BlameOrigin[];
  starts: boolean[];
  ages: number[];
  cols: number;
  line: number;
  landing: { from: number; to: number; exact: boolean } | null;
  onPick: (n: number) => void;
  register: (scrollToRow: (i: number, align: "auto" | "center") => void) => void;
}) {
  const virt = createVirtualizer({
    get count() {
      return props.lines.length;
    },
    getScrollElement: () => props.scrollEl,
    estimateSize: () => rowH(),
    overscan: 20,
  });
  createEffect(on(rowH, () => virt.measure(), { defer: true }));
  props.register((i, align) => virt.scrollToIndex(i, { align }));

  const digits = () => String(props.lines.length).length;
  const inLanding = (n: number) => {
    const l = props.landing;
    return !!l && !l.exact && n >= l.from && n <= l.to;
  };

  return (
    <div
      class="relative font-mono text-xs"
      style={{
        height: `${virt.getTotalSize()}px`,
        width: `calc(${GUTTER_REM}rem + ${digits() + props.cols + 4}ch)`,
        "min-width": "100%",
      }}
    >
      <For each={virt.getVirtualItems()}>
        {(vi) => {
          const row = createMemo(() => props.lines[vi.index]);
          return (
            <Show when={row()}>
              {(l) => {
                const o = () => props.origins[l().origin];
                const start = () => props.starts[vi.index];
                return (
                  <div
                    class="absolute left-0 flex w-full cursor-default items-center"
                    classList={{
                      "bg-accent/15": props.line === l().line,
                      "bg-warn/15": props.line !== l().line && inLanding(l().line),
                      "hover:bg-bg-muted": props.line !== l().line,
                      "border-t border-border": start() && vi.index > 0,
                    }}
                    style={{ top: `${vi.start}px`, height: `${rowH()}px` }}
                    onClick={() => props.onPick(l().line)}
                  >
                    <div
                      class="relative flex shrink-0 items-center gap-2 self-stretch overflow-hidden border-r border-border pl-2 pr-2 font-sans text-[0.6875rem]"
                      style={{ width: `${GUTTER_REM}rem` }}
                      title={o()?.uncommitted ? d().blameUncommitted() : o()?.summary}
                    >
                      <span
                        class="absolute inset-y-0 left-0"
                        classList={{
                          "bg-accent": !o()?.uncommitted,
                          "bg-warn": !!o()?.uncommitted,
                        }}
                        style={{
                          width: `${scaledPx(3)}px`,
                          opacity: 0.15 + ((props.ages[l().origin] ?? 0) / (AGE_LEVELS - 1)) * 0.7,
                        }}
                      />
                      <Show when={start() && o()}>
                        {(oo) => (
                          <Show
                            when={!oo().uncommitted}
                            fallback={<span class="truncate text-warn">{d().blameUncommitted()}</span>}
                          >
                            <span class="shrink-0 font-mono text-fg-muted">{oo().shortHash}</span>
                            <span class="min-w-0 truncate">{oo().author}</span>
                            <span class="ml-auto shrink-0 text-fg-subtle" title={fmtDateTime(oo().authorAt)}>
                              {fmtAgo(oo().authorAt)}
                            </span>
                          </Show>
                        )}
                      </Show>
                    </div>
                    <div
                      class="shrink-0 select-none pr-2 text-right text-fg-subtle"
                      style={{ width: `${digits() + 2}ch` }}
                    >
                      {l().line}
                    </div>
                    <div class="whitespace-pre pl-1" style={{ "tab-size": "8" }}>
                      {l().text || " "}
                    </div>
                  </div>
                );
              }}
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
