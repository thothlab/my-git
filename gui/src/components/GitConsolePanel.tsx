import { For, Match, Show, Switch, createEffect, createResource, createSignal, on, onCleanup } from "solid-js";
import { gitExec, journalList, journalOutput, type JournalSummary } from "../api";
import { d } from "../i18n";
import { busy, error, registerModalSource, runWithOutput } from "../store";
import { afterRepoChange } from "./log/actions/repoRefresh";
import { formatArgv, splitShellArgs } from "./gitConsoleCommand";

/**
 * The git console: the journal of every git command this process ran, and a
 * command line to run one more.
 *
 * The journal lives in Rust (`engine/exec.rs`) — every git process the
 * application starts is recorded there with its origin: a person's action
 * (a mutation, a command typed here) or the application reading state for
 * itself (snapshot, log, branch tree…). "Mine" is the default tab because the
 * question a person opens this with is "what did I just do"; "All" answers
 * "what did Graft do". The two origins live in separate rings (1000 user
 * actions, 2000 background reads), so the frequent background never pushes
 * "Mine" out; "All" is their merge by id. A successful background read keeps
 * only 16 KB of each stream, anything else 256 KB — the truncation note says
 * which. A command typed below is itself a journal entry — the console shows
 * it from there, expanded, rather than keeping a second list.
 *
 * The input is not a terminal — no shell, no pipes, no other binaries,
 * non-interactive (see `CliEngine::exec_raw`: no `$EDITOR`, no credential
 * prompt, stdin closed) — it exists so the user does not have to leave the
 * window for the git commands the panels do not expose a button for.
 *
 * Fresh entries are polled once a second while the panel is open, and only
 * then: `journal_list(after)` returns what is new, reads process memory and
 * never runs git, so the poll cannot feed the journal it reads. A push event
 * would cost a serialization per git process for a panel that is closed
 * almost all of the time.
 *
 * Mounted next to `StashPanel`, for the same reason: a console belongs to
 * neither of the window's two modes, so `App` owns it and the toolbar, the
 * app menu and the error banner open the one instance.
 */

type Tab = "mine" | "all";

/** Entries the panel keeps in memory — both journal rings together (1000 user
 *  actions + 2000 background reads, `engine/exec.rs`). */
const KEEP = 3000;
const POLL_MS = 1000;

const [open, setOpen] = createSignal(false);
const [tab, setTab] = createSignal<Tab>("mine");
const [expandedId, setExpandedId] = createSignal<number | null>(null);
/** Entry the panel was opened to show (banner "show output"): scrolled to, outlined. */
const [focusId, setFocusId] = createSignal<number | null>(null);
const [cmdHistory, setCmdHistory] = createSignal<string[]>([]);

registerModalSource(open);

/**
 * Open the console. With `entryId` — a failed run named by the error banner —
 * it opens on "All" (the failure may be a background read) with that entry
 * expanded and in view.
 */
export function openGitConsole(entryId?: unknown): void {
  if (typeof entryId === "number") {
    setTab("all");
    setExpandedId(entryId);
    setFocusId(entryId);
  } else {
    // "Mine" by default on every plain open: a banner link that switched to
    // "All" once should not decide what the next open shows.
    setTab("mine");
    setFocusId(null);
  }
  setOpen(true);
}

export default function GitConsolePanel() {
  return (
    <Show when={open()}>
      <GitConsoleView />
    </Show>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");
/** Local wall-clock time of a run — from local parts, like every date in the UI. */
const clock = (ms: number) => {
  const t = new Date(ms);
  return `${pad(t.getHours())}:${pad(t.getMinutes())}:${pad(t.getSeconds())}`;
};
const failed = (e: JournalSummary) => e.exitCode !== 0;

function GitConsoleView() {
  let input: HTMLInputElement | undefined;
  let log: HTMLDivElement | undefined;
  const [text, setText] = createSignal("");
  const [inputError, setInputError] = createSignal("");
  const [entries, setEntries] = createSignal<JournalSummary[]>([]);
  let historyIdx = -1; // -1 = not browsing history (the live draft)
  let draft = "";

  // Which tab's list is on screen. A poll that started under another tab (or
  // before a reset) answers a question nobody is asking any more — dropped.
  let generation = 0;
  let lastId: number | null = null;
  /** Generation of the poll in flight; one at a time per list. */
  let inflight: number | null = null;
  /** A poll was asked for while one was in flight — run it once that lands. */
  let again = false;

  const atBottom = () => !log || log.scrollHeight - log.scrollTop - log.clientHeight < 32;
  const scrollToBottom = () =>
    queueMicrotask(() => {
      if (log) log.scrollTop = log.scrollHeight;
    });

  const poll = async (opts: { stick?: boolean } = {}): Promise<void> => {
    if (inflight === generation) {
      again ||= !!opts.stick;
      return;
    }
    const gen = generation;
    inflight = gen;
    const mine = tab() === "mine";
    try {
      const rows = await journalList(mine, lastId);
      // Ids only grow; anything at or below the last one shown is already there.
      const fresh = lastId === null ? rows : rows.filter((r) => r.id > lastId!);
      if (gen === generation && fresh.length > 0) {
        const stick = opts.stick || atBottom();
        lastId = fresh[fresh.length - 1].id;
        setEntries((es) => [...es, ...fresh].slice(-KEEP));
        if (stick) scrollToBottom();
      }
    } catch {
      // The journal read touches no repository and has no failure worth a
      // banner; the next tick asks again.
    } finally {
      if (inflight === gen) inflight = null;
    }
    if (again && gen === generation) {
      again = false;
      await poll({ stick: true });
    }
  };

  const revealFocus = () =>
    queueMicrotask(() => {
      const id = focusId();
      if (id === null || !log) return;
      log.querySelector(`[data-journal-id="${id}"]`)?.scrollIntoView({ block: "nearest" });
    });

  createEffect(
    on(tab, async () => {
      generation += 1;
      lastId = null;
      again = false;
      setEntries([]);
      await poll({ stick: true });
      revealFocus();
    }),
  );

  const timer = setInterval(() => void poll(), POLL_MS);
  onCleanup(() => clearInterval(timer));

  createEffect(() => {
    if (open()) queueMicrotask(() => input?.focus());
  });

  const close = () => setOpen(false);

  const submit = async () => {
    const raw = text().trim();
    if (!raw || busy()) return;
    const parsed = splitShellArgs(raw);
    setText("");
    setInputError("");
    historyIdx = -1;
    draft = "";
    setCmdHistory((h) => [...h, raw]);
    if (!parsed.ok) {
      setInputError(d().gitConsoleBadInput(parsed.error));
      return;
    }
    if (parsed.args.length === 0) return;
    const result = await runWithOutput(gitExec(parsed.args), d().phaseGitExec());
    if (result) {
      setFocusId(null);
      setExpandedId(result.journalId);
    } else {
      // git never ran (no repository open, or it could not be started): there
      // is no journal entry to open, so the reason is shown here.
      setInputError(error());
    }
    await poll({ stick: true });
    afterRepoChange();
  };

  // The application's global shortcuts stand down inside an input (hotkeys.ts
  // rule 2) — so plain ArrowUp/ArrowDown here are ours alone, unlike anywhere
  // a bare arrow key already means something.
  const onInputKeyDown = (e: KeyboardEvent) => {
    const h = cmdHistory();
    if (e.code === "Enter") {
      e.preventDefault();
      void submit();
      return;
    }
    if (e.code === "ArrowUp" && h.length > 0) {
      e.preventDefault();
      if (historyIdx === -1) draft = text();
      historyIdx = Math.max(0, (historyIdx === -1 ? h.length : historyIdx) - 1);
      setText(h[historyIdx]);
      return;
    }
    if (e.code === "ArrowDown" && historyIdx !== -1) {
      e.preventDefault();
      historyIdx += 1;
      setText(historyIdx >= h.length ? ((historyIdx = -1), draft) : h[historyIdx]);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      close();
    }
  };

  const tabButton = (t: Tab, label: () => string, tip: () => string) => (
    <button
      class={`rounded px-2 py-0.5 text-xs ${
        tab() === t ? "bg-bg-muted text-fg" : "text-fg-subtle hover:text-fg"
      }`}
      aria-pressed={tab() === t}
      title={tip()}
      onClick={() => {
        setFocusId(null);
        setTab(t);
      }}
    >
      {label()}
    </button>
  );

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        tabindex={-1}
        class="flex h-[min(32rem,84vh)] w-[min(60rem,94vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="text-sm font-semibold">{d().gitConsole()}</span>
          <div class="flex items-center gap-0.5 rounded border border-border p-0.5">
            {tabButton("mine", () => d().gitConsoleMine(), () => d().gitConsoleMineTip())}
            {tabButton("all", () => d().gitConsoleAll(), () => d().gitConsoleAllTip())}
          </div>
          <span class="min-w-0 truncate text-xs text-fg-subtle">{d().gitConsoleHint()}</span>
          <button
            class="ml-auto shrink-0 rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>

        <div ref={log} class="min-h-0 flex-1 overflow-auto">
          <Show
            when={entries().length > 0}
            fallback={
              <Empty text={tab() === "mine" ? d().gitConsoleEmptyMine() : d().gitConsoleEmptyAll()} />
            }
          >
            <For each={entries()}>{(e) => <Entry e={e} />}</For>
          </Show>
        </div>

        <Show when={inputError()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-t border-border px-3 py-1 font-mono text-[0.6875rem] text-danger">
            {inputError()}
          </pre>
        </Show>
        <div class="flex items-center gap-2 border-t border-border px-3 py-2">
          <span class="shrink-0 font-mono text-xs text-fg-subtle">$</span>
          <input
            ref={input}
            class="min-w-0 flex-1 bg-transparent font-mono text-xs outline-none placeholder:text-fg-subtle"
            placeholder={d().gitConsolePlaceholder()}
            value={text()}
            disabled={busy()}
            onInput={(e) => {
              setText(e.currentTarget.value);
              setInputError("");
            }}
            onKeyDown={onInputKeyDown}
          />
        </div>
      </div>
    </div>
  );
}

function Entry(props: { e: JournalSummary }) {
  const expanded = () => expandedId() === props.e.id;
  const command = () => `git ${formatArgv(props.e.argv)}`;
  const exit = () =>
    props.e.exitCode === null ? d().gitConsoleNotStarted() : d().gitConsoleExit(props.e.exitCode);
  return (
    <div
      data-journal-id={props.e.id}
      class={`border-b border-border font-mono text-[0.6875rem] ${failed(props.e) ? "bg-danger/5" : ""} ${
        focusId() === props.e.id ? "outline outline-1 -outline-offset-1 outline-accent" : ""
      }`}
    >
      <button
        class="flex w-full items-baseline gap-2 px-3 py-1 text-left hover:bg-bg-muted"
        aria-expanded={expanded()}
        title={d().gitConsoleCwd(props.e.repo)}
        onClick={() => {
          setFocusId(null);
          setExpandedId(expanded() ? null : props.e.id);
        }}
      >
        <span class="shrink-0 text-fg-subtle">{expanded() ? "▾" : "▸"}</span>
        <span
          class={`min-w-0 flex-1 truncate ${
            failed(props.e) ? "text-danger" : props.e.origin === "user" ? "text-fg" : "text-fg-muted"
          }`}
        >
          {command()}
        </span>
        <span class="shrink-0 text-fg-subtle">
          {clock(props.e.startedAt)} · {d().gitConsoleDuration(props.e.durationMs)} ·{" "}
        </span>
        <span class={`shrink-0 ${failed(props.e) ? "text-danger" : "text-fg-subtle"}`}>{exit()}</span>
      </button>
      <Show when={expanded()}>
        <EntryOutput id={props.e.id} command={command()} repo={props.e.repo} />
      </Show>
    </div>
  );
}

function EntryOutput(props: { id: number; command: string; repo: string }) {
  const [out] = createResource(() => props.id, journalOutput);
  return (
    <div class="space-y-1 px-3 pb-2 pl-7">
      <pre class="whitespace-pre-wrap text-fg-muted">$ {props.command}</pre>
      <div class="text-fg-subtle">{d().gitConsoleCwd(props.repo)}</div>
      <Switch>
        <Match when={out.loading}>
          <div class="text-fg-subtle">{d().gitConsoleLoading()}</div>
        </Match>
        <Match when={out.error || !out()}>
          <div class="text-fg-subtle">{d().gitConsoleEvicted()}</div>
        </Match>
        <Match when={out()}>
          {(o) => (
            <>
              <Show when={o().stdout}>
                <pre class="whitespace-pre-wrap text-fg">{o().stdout}</pre>
              </Show>
              <Show when={o().stdoutTruncated}>
                <div class="text-warn">{d().gitConsoleTruncated(Math.round(o().limitBytes / 1024))}</div>
              </Show>
              <Show when={o().stderr}>
                <pre class="whitespace-pre-wrap text-warn">{o().stderr}</pre>
              </Show>
              <Show when={o().stderrTruncated}>
                <div class="text-warn">{d().gitConsoleTruncated(Math.round(o().limitBytes / 1024))}</div>
              </Show>
              <Show when={!o().stdout && !o().stderr}>
                <div class="text-fg-subtle">{d().gitConsoleNoOutput()}</div>
              </Show>
            </>
          )}
        </Match>
      </Switch>
    </div>
  );
}

function Empty(props: { text: string }) {
  return <div class="p-4 text-center text-xs text-fg-muted">{props.text}</div>;
}
