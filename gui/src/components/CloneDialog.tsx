import { Show, createEffect, createSignal, onCleanup } from "solid-js";
import { open as pickFolder } from "@tauri-apps/plugin-dialog";
import { errText, onCloneProgress, repoClone, repoCloneCancel } from "../api";
import { d } from "../i18n";
import { openRepoAt, registerModalSource, state } from "../store";
import { folderNameFromUrl, httpLogin, isPlainHttp, joinDest } from "./cloneRules";
import { DISABLED_CLASS } from "./IconButton";

/**
 * "Clone…": an address, the folder to clone into, the new folder's name, and the
 * progress of `git clone` while it runs.
 *
 * Seams worth naming:
 *
 *  - **Not through `run()`.** A clone changes no open repository and returns a
 *    path, not `RepoState`; a refusal (a token in the address, a folder that is not
 *    empty) or git's failure stays in this dialog, next to what was typed. On
 *    success the result is opened by `openRepoAt` — the path "Open…" takes, which
 *    also puts it at the top of the recent repositories.
 *  - **The name follows the address until the reader edits it**, then stays theirs.
 *  - **Escape stops a running clone instead of closing over it**: the dialog is the
 *    only place its progress and its Stop live. The backend removes what the
 *    stopped clone had made.
 *  - The listener for `repo-clone-progress` is attached before the command starts,
 *    and detached when it ends, whichever way it ends.
 *  - The chosen parent folder is remembered (`localStorage.cloneParent`).
 */

const PARENT_KEY = "cloneParent";

const [open, setOpen] = createSignal(false);
const [running, setRunning] = createSignal(false);

registerModalSource(open);

export function openCloneDialog(): void {
  setOpen(true);
}

export default function CloneDialog() {
  return (
    <Show when={open()}>
      <CloneView />
    </Show>
  );
}

function readParent(): string {
  try {
    const p = localStorage.getItem(PARENT_KEY);
    if (p) return p;
  } catch {
    // no storage: fall through to the open repository's folder
  }
  const repo = state()?.repoPath;
  return repo ? repo.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "") : "";
}

function CloneView() {
  let box: HTMLDivElement | undefined;
  let urlInput: HTMLInputElement | undefined;
  const [url, setUrl] = createSignal("");
  const [parent, setParent] = createSignal(readParent());
  const [name, setName] = createSignal("");
  const [nameEdited, setNameEdited] = createSignal(false);
  const [progress, setProgress] = createSignal("");
  const [error, setError] = createSignal("");
  const [note, setNote] = createSignal("");

  createEffect(() => {
    if (open()) queueMicrotask(() => urlInput?.focus());
  });
  // Unmounted by a mode switch or the like while running: the clone is stopped
  // rather than left finishing into a folder nobody will open.
  onCleanup(() => {
    if (running()) void repoCloneCancel();
  });

  const onUrl = (v: string) => {
    setUrl(v);
    if (!nameEdited()) setName(folderNameFromUrl(v));
  };

  const choose = async () => {
    const dir = await pickFolder({
      directory: true,
      title: d().cloneParentDialog(),
      defaultPath: parent() || undefined,
    });
    if (typeof dir === "string") setParent(dir);
    queueMicrotask(() => box?.focus());
  };

  const missing = (): string | null => {
    if (!url().trim()) return d().cloneUrlLabel();
    if (!parent()) return d().cloneNeedParent();
    if (!name().trim()) return d().cloneNeedName();
    return null;
  };

  const start = async () => {
    if (running() || missing()) return;
    setRunning(true);
    setError("");
    setNote("");
    setProgress(d().cloneRunning());
    const unlisten = await onCloneProgress((e) => setProgress(e.line));
    try {
      const path = await repoClone(url().trim(), parent(), name().trim());
      if (path === null) {
        setNote(d().cloneCancelled());
        return;
      }
      try {
        localStorage.setItem(PARENT_KEY, parent());
      } catch {
        // a convenience only
      }
      // Not running any more before the view goes: its clean-up would "stop" a
      // clone that already finished.
      setRunning(false);
      setOpen(false);
      await openRepoAt(path);
    } catch (e) {
      setError(errText(e));
    } finally {
      unlisten();
      setRunning(false);
      setProgress("");
    }
  };

  const stop = () => {
    if (running()) void repoCloneCancel();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      if (running()) stop();
      else setOpen(false);
    } else if (e.code === "Enter" || e.code === "NumpadEnter") {
      e.preventDefault();
      void start();
    }
  };

  const input =
    "mt-1 w-full rounded border border-border bg-bg-muted px-2 py-1 text-sm text-fg outline-none focus:border-accent disabled:opacity-60";

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        role="dialog"
        aria-label={d().cloneTitle()}
        class="w-[min(36rem,92vw)] rounded-lg border border-border bg-bg p-4 text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="mb-3 text-sm font-semibold">{d().cloneTitle()}</div>

        <label class="mb-1 block text-xs text-fg-muted">
          {d().cloneUrlLabel()}
          <input
            ref={urlInput}
            class={`${input} font-mono text-xs`}
            value={url()}
            placeholder={d().remoteUrlPlaceholder()}
            spellcheck={false}
            disabled={running()}
            onInput={(e) => onUrl(e.currentTarget.value)}
          />
        </label>
        <div class="mb-3 text-[0.6875rem] text-fg-subtle">{d().remoteUrlNote()}</div>
        <Show when={isPlainHttp(url())}>
          <div class="-mt-2 mb-3 text-[0.6875rem] text-warn">{d().remoteHttpWarn()}</div>
        </Show>
        <Show when={httpLogin(url())}>
          {(user) => (
            <div class="-mt-2 mb-3 text-[0.6875rem] text-warn">{d().remoteLoginNote(user())}</div>
          )}
        </Show>

        <div class="mb-3 text-xs text-fg-muted">
          {d().cloneParentLabel()}
          <div class="mt-1 flex items-center gap-2">
            <span
              class="min-w-0 flex-1 truncate rounded border border-border bg-bg-muted px-2 py-1 font-mono text-xs text-fg"
              title={parent()}
            >
              {parent() || <span class="italic text-fg-muted">{d().cloneParentNone()}</span>}
            </span>
            <button
              class={`shrink-0 rounded border border-border px-2.5 py-1 text-xs hover:bg-bg-muted ${DISABLED_CLASS}`}
              disabled={running()}
              onClick={() => void choose()}
            >
              {d().cloneChooseParent()}
            </button>
          </div>
        </div>

        <label class="mb-1 block text-xs text-fg-muted">
          {d().cloneNameLabel()}
          <input
            class={input}
            value={name()}
            spellcheck={false}
            disabled={running()}
            onInput={(e) => {
              setName(e.currentTarget.value);
              setNameEdited(e.currentTarget.value !== "");
            }}
          />
        </label>
        <Show when={parent() && name().trim()}>
          <div class="mb-3 truncate text-[0.6875rem] text-fg-subtle" title={joinDest(parent(), name().trim())}>
            {d().cloneDest(joinDest(parent(), name().trim()))}
          </div>
        </Show>

        <Show when={running() || progress()}>
          <div class="mb-3 truncate rounded bg-bg-muted px-2 py-1 font-mono text-[0.6875rem] text-fg-muted" title={progress()}>
            {progress()}
          </div>
        </Show>
        <Show when={note()}>
          <div class="mb-3 text-xs text-fg-subtle">{note()}</div>
        </Show>
        <Show when={error()}>
          <pre class="mb-3 max-h-32 overflow-auto whitespace-pre-wrap rounded border border-danger/40 bg-danger/10 p-2 font-mono text-[0.6875rem] text-danger">
            {error()}
          </pre>
        </Show>

        <div class="flex items-center gap-2">
          <span class="text-[0.6875rem] text-fg-subtle">{d().cloneKeys()}</span>
          <span class="ml-auto" />
          <Show
            when={running()}
            fallback={
              <>
                <button
                  class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
                  onClick={() => setOpen(false)}
                >
                  {d().cancel()}
                </button>
                <button
                  class={`rounded bg-accent px-3 py-1 text-sm text-white ${DISABLED_CLASS}`}
                  disabled={missing() !== null}
                  title={missing() ?? undefined}
                  onClick={() => void start()}
                >
                  {d().cloneStart()}
                </button>
              </>
            }
          >
            <button class="rounded bg-danger px-3 py-1 text-sm text-white" onClick={stop}>
              {d().cloneStop()}
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
}
