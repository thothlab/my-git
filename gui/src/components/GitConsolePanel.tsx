import { For, Show, createEffect, createSignal } from "solid-js";
import { gitExec } from "../api";
import { d } from "../i18n";
import { busy, error, registerModalSource, runWithOutput } from "../store";
import { afterRepoChange } from "./log/actions/repoRefresh";
import { splitShellArgs } from "./gitConsoleCommand";

/**
 * A git command line: type a git subcommand, it runs in the open repository,
 * both streams come back. Not a terminal — no shell, no pipes, no other
 * binaries, non-interactive (see `CliEngine::exec_raw`: no `$EDITOR`, no
 * credential prompt, stdin closed) — it exists so the user does not have to
 * leave the window for the git commands the panels do not expose a button
 * for.
 *
 * Mounted next to `StashPanel`, for the same reason: a console belongs to
 * neither of the window's two modes, so `App` owns it and both the toolbar
 * and the app menu open the one instance.
 *
 * A fresh `RepoState` this panel installs is not special — `DiffView` already
 * treats every new `RepoState` as "the world may have moved outside the
 * application" and drops its accepted payload for it (see the effect on
 * `state` there). Running `git reset` in here is exactly that case.
 */

type ConsoleEntry =
  | { kind: "ran"; command: string; stdout: string; stderr: string; exitCode: number }
  | { kind: "error"; command: string; message: string };

const [open, setOpen] = createSignal(false);
const [entries, setEntries] = createSignal<ConsoleEntry[]>([]);
const [cmdHistory, setCmdHistory] = createSignal<string[]>([]);

registerModalSource(open);

export function openGitConsole(): void {
  setOpen(true);
}

export default function GitConsolePanel() {
  return (
    <Show when={open()}>
      <GitConsoleView />
    </Show>
  );
}

function GitConsoleView() {
  let box: HTMLDivElement | undefined;
  let input: HTMLInputElement | undefined;
  let log: HTMLDivElement | undefined;
  const [text, setText] = createSignal("");
  let historyIdx = -1; // -1 = not browsing history (the live draft)
  let draft = "";

  const close = () => setOpen(false);

  createEffect(() => {
    if (open()) queueMicrotask(() => input?.focus());
  });

  createEffect(() => {
    entries();
    queueMicrotask(() => {
      if (log) log.scrollTop = log.scrollHeight;
    });
  });

  const submit = async () => {
    const raw = text().trim();
    if (!raw || busy()) return;
    const parsed = splitShellArgs(raw);
    setText("");
    historyIdx = -1;
    draft = "";
    setCmdHistory((h) => [...h, raw]);
    if (!parsed.ok) {
      setEntries((es) => [...es, { kind: "error", command: raw, message: d().gitConsoleBadInput(parsed.error) }]);
      return;
    }
    if (parsed.args.length === 0) return;
    const result = await runWithOutput(gitExec(parsed.args), d().phaseGitExec());
    setEntries((es) => [
      ...es,
      result
        ? { kind: "ran", command: raw, stdout: result.stdout, stderr: result.stderr, exitCode: result.exitCode }
        : { kind: "error", command: raw, message: error() },
    ]);
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

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        class="flex h-[min(32rem,84vh)] w-[min(60rem,94vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="text-sm font-semibold">{d().gitConsole()}</span>
          <span class="text-xs text-fg-subtle">{d().gitConsoleHint()}</span>
          <button
            class="ml-auto rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>

        <div ref={log} class="min-h-0 flex-1 overflow-auto">
          <Show when={entries().length > 0} fallback={<Empty text={d().gitConsoleEmpty()} />}>
            <For each={entries()}>{(e) => <Entry e={e} />}</For>
          </Show>
        </div>

        <div class="flex items-center gap-2 border-t border-border px-3 py-2">
          <span class="shrink-0 font-mono text-xs text-fg-subtle">$</span>
          <input
            ref={input}
            class="min-w-0 flex-1 bg-transparent font-mono text-xs outline-none placeholder:text-fg-subtle"
            placeholder={d().gitConsolePlaceholder()}
            value={text()}
            disabled={busy()}
            onInput={(e) => setText(e.currentTarget.value)}
            onKeyDown={onInputKeyDown}
          />
        </div>
      </div>
    </div>
  );
}

function Entry(props: { e: ConsoleEntry }) {
  return (
    <div class="border-b border-border px-3 py-2 font-mono text-[11px]">
      <div class="text-fg-subtle">$ {props.e.command}</div>
      <Show when={props.e.kind === "error"}>
        <pre class="whitespace-pre-wrap text-danger">{(props.e as { message: string }).message}</pre>
      </Show>
      <Show when={props.e.kind === "ran"}>
        {(() => {
          const r = props.e as { stdout: string; stderr: string; exitCode: number };
          return (
            <>
              <Show when={r.stdout}>
                <pre class="whitespace-pre-wrap text-fg">{r.stdout}</pre>
              </Show>
              <Show when={r.stderr}>
                <pre class="whitespace-pre-wrap text-warn">{r.stderr}</pre>
              </Show>
              <Show when={r.exitCode !== 0}>
                <div class="text-danger">{d().gitConsoleExit(r.exitCode)}</div>
              </Show>
            </>
          );
        })()}
      </Show>
    </div>
  );
}

function Empty(props: { text: string }) {
  return <div class="p-4 text-center text-xs text-fg-muted">{props.text}</div>;
}
