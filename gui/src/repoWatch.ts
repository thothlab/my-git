import { createEffect, createRoot, on } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onRepoExternalChange } from "./api";
import { busy, refresh, refreshKeepingError, state } from "./store";
import { afterRepoChange } from "./components/log/actions/repoRefresh";
import { editorOpen } from "./components/diff/editState";

/**
 * Showing what a terminal did to the open repository, without the user coming
 * back to the window.
 *
 * The backend watches the git directory (`src-tauri/src/watch.rs`) and sends
 * `repo-external-change` once per burst of changes that were not Graft's own.
 * This module decides *when* to act on it; the action is the one the focus
 * refresh takes — a re-read of the state, only without clearing the error
 * banner (`refreshKeepingError`) — plus `afterRepoChange()`, because
 * the branch tree reads git through a request of its own that a state refresh
 * does not move.
 *
 * The log is the exception inside `afterRepoChange()`: after a fresh state it is
 * **not** force-reloaded (`log: false`). The fresh state already drives it —
 * `LogTable` answers every same-repository state with `checkNewCommits`, which
 * reloads from the first page (keeping the selection) when the reader is at the
 * top, and *offers* "N new commits" when they have scrolled away. A forced
 * reload there would move the rows under that reader and, for a selection past
 * the first page, select the newest commit instead — an automatic refresh
 * closing what the reader had open. Only when the state part is held back (the
 * editor below) is the log reloaded explicitly, since nothing else would.
 *
 * When:
 * - never while `busy()`: `run()` has no sequence guard, and a refresh started
 *   before a mutation and answered after it would install a state older than
 *   the mutation's. Graft's own writes are already silenced on the backend; an
 *   event that still arrives during a mutation stays pending and runs after it.
 * - never twice at once, and not more often than `FOREGROUND_GAP_MS` with the
 *   window focused, `BACKGROUND_GAP_MS` without: a thirty-commit rebase in the
 *   terminal is a handful of bursts, and it becomes one or two refreshes. The
 *   background is the main case — the terminal focused, Graft beside it — so it
 *   is refreshed too, only less eagerly.
 * - not while the window is minimized or hidden: nobody sees it, and every
 *   refresh starts git. It waits for focus or visibility.
 * - the *state* part not while the file editor is open: an external commit of
 *   the edited file takes it out of the changelists, the selection goes, and
 *   the editor with it. The log and the branch tree do not touch the editor and
 *   refresh anyway; the state follows when the editor closes, or on focus.
 *
 * The TCC loop described in `App.tsx` cannot be fed from here: a permission
 * prompt does not write into the git directory, and the refresh's own writes
 * (`index`, `changelists.json`) are excluded by the watcher. The two refreshes
 * also share one clock (`msSinceRefresh`), so a watcher refresh whose `git`
 * raised a prompt does not have the returning focus run git again at once.
 */

const FOREGROUND_GAP_MS = 1500;
const BACKGROUND_GAP_MS = 5000;

/** An external change the state has not been re-read for. */
let dirtyState = false;
/** An external change the log and the branch tree have not been re-read for. */
let dirtyHistory = false;
let inFlight = false;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastRefreshAt = 0;

/** Time since the last refresh this module or the focus handler started. */
export const msSinceRefresh = (): number => Date.now() - lastRefreshAt;

const due = (): boolean => dirtyHistory || (dirtyState && !editorOpen());

/** The window is hidden or minimized — nobody sees it. Shared with the
 *  background fetch, which holds off for the same reason. */
export async function windowHidden(): Promise<boolean> {
  if (document.visibilityState === "hidden") return true;
  try {
    return await getCurrentWindow().isMinimized();
  } catch {
    return false;
  }
}

function schedule(): void {
  if (timer !== undefined || inFlight || busy() || !due()) return;
  if (document.visibilityState === "hidden") return;
  const gap = document.hasFocus() ? FOREGROUND_GAP_MS : BACKGROUND_GAP_MS;
  const wait = Math.max(0, lastRefreshAt + gap - Date.now());
  timer = setTimeout(() => {
    timer = undefined;
    void flush();
  }, wait);
}

async function flush(): Promise<void> {
  if (inFlight || busy() || !due()) return;
  if (await windowHidden()) return; // focus or visibility brings it back
  // Past an await: read everything again.
  if (inFlight || busy() || !due()) return;
  const doState = dirtyState && !editorOpen();
  const doHistory = dirtyHistory;
  if (doState) dirtyState = false;
  if (doHistory) dirtyHistory = false;
  inFlight = true;
  lastRefreshAt = Date.now();
  try {
    if (doState) await refreshKeepingError(); // the banner stays until read
    if (doHistory) afterRepoChange({ log: !doState });
  } finally {
    inFlight = false;
  }
  schedule(); // what arrived meanwhile
}

/**
 * The focus refresh, on the shared clock. The caller keeps its own cooldown
 * (see `App.tsx`); this only adds what a pending external change needs on top
 * of the plain state refresh the focus has always done.
 */
export async function refreshOnFocus(): Promise<void> {
  if (inFlight) return; // a watcher refresh is reading right now
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
  const history = dirtyHistory;
  dirtyState = false;
  dirtyHistory = false;
  lastRefreshAt = Date.now();
  await refresh();
  if (history) afterRepoChange({ log: false }); // the fresh state moves the log
}

/** Act on a pending external change if nothing holds it back — for the focus
 *  handler inside its cooldown. Does nothing when no change is pending, so it
 *  cannot run git in answer to focus alone. */
export const nudgeRepoWatch = (): void => schedule();

/** Start listening. Call once, from inside a component's synchronous setup. */
export function startRepoWatch(): () => void {
  let stopped = false;
  const unlisten = onRepoExternalChange(({ repoPath }) => {
    // A late event of a repository the window has already left.
    if (repoPath !== state()?.repoPath) return;
    dirtyState = true;
    dirtyHistory = true;
    schedule();
  });
  const onVisibility = () => schedule();
  document.addEventListener("visibilitychange", onVisibility);
  // Whatever was held back for a mutation or an open editor goes once they end.
  const disposeRoot = createRoot((dispose) => {
    createEffect(on([busy, editorOpen], () => schedule(), { defer: true }));
    return dispose;
  });
  return () => {
    if (stopped) return;
    stopped = true;
    void unlisten.then((f) => f());
    document.removeEventListener("visibilitychange", onVisibility);
    disposeRoot();
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
}
