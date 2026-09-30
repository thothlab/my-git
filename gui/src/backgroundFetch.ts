import { createEffect, createMemo, createRoot, createSignal, on, onCleanup } from "solid-js";
import { errText, repoFetchBackground } from "./api";
import { busy, state } from "./store";
import { windowHidden } from "./repoWatch";
import {
  countsAsAttempt,
  errorSummary,
  normalizeInterval,
  shouldFetch,
  type FetchOutcome,
} from "./components/backgroundFetchRules";

/**
 * The scheduled background fetch, off by default (Settings → Network).
 *
 * The interval lives in `localStorage` (`backgroundFetchMinutes`): how often this
 * machine talks to the network on its own is a fact about the machine and the
 * person, not about a repository — the same choice for every repository opened.
 *
 * Only the open repository, only while the window is shown, not while a mutation
 * runs or just after one (`QUIET_AFTER_BUSY_MS`), not during an unfinished
 * operation; the rule itself is `backgroundFetchRules.ts`, under the harness.
 *
 * **Not through `run()`** — the second exception to the mutation funnel, after
 * `fileWrite`. `run()` would flash busy in the toolbar every few minutes and put
 * an offline laptop's failure in the banner on every tick. A failure is kept
 * instead, per repository, as a line in the status bar (`backgroundFetchError`);
 * the catch lives here, not in a component.
 *
 * **No refresh on the answer.** The backend journals the fetch as background
 * work and does not count it as Graft's own action, so what it moved in
 * `refs/remotes/` is reported by the git-dir watcher and refreshed once by
 * `repoWatch.ts` (↑/↓, branch tree). Refreshing here as well would read
 * everything twice; a fetch that brought nothing moves nothing the watcher sees
 * and costs no refresh at all.
 */

const KEY = "backgroundFetchMinutes";
/** How often the rule is asked. Cheap: no git runs unless a fetch is due. */
const TICK_MS = 5_000;

function stored(): number {
  try {
    return normalizeInterval(localStorage.getItem(KEY));
  } catch {
    return 0;
  }
}

const [minutes, setMinutes] = createSignal(stored());
/** When scheduling was last switched on; the first fetch waits its delay from here. */
let enabledAt = Date.now();

export const backgroundFetchMinutes = minutes;

export function setBackgroundFetchMinutes(m: number): void {
  const next = normalizeInterval(m);
  if (next && !minutes()) enabledAt = Date.now();
  setMinutes(next);
  try {
    localStorage.setItem(KEY, String(next));
  } catch {
    // Not persisted; the choice still holds for this session.
  }
}

/** Last attempt per repository path, this session. */
const lastAttempt = new Map<string, number>();
/** When each repository became current. */
const since = new Map<string, number>();
/** A signal, not a flag: the status bar says "fetching in background" while it
 *  runs, because a person's action started meanwhile waits for it on the backend
 *  (`Undo::perform_background`) and would otherwise be slow for no visible reason. */
const [inFlight, setInFlight] = createSignal(false);
export const backgroundFetchRunning = inFlight;
let busyEndedAt: number | null = null;

const [lastError, setLastError] = createSignal<{
  repo: string;
  /** The one line shown (`errorSummary`). */
  line: string;
  /** Everything, for the tooltip. */
  text: string;
  at: number;
} | null>(null);

/** The last background fetch of the open repository failed: its message. Cleared
 *  by the next one that succeeds. */
export const backgroundFetchError = () => {
  const e = lastError();
  return e && e.repo === state()?.repoPath ? e : null;
};

async function tick(): Promise<void> {
  // Everything read before the first await: tracking and freshness end there.
  const s = state();
  if (!s) return;
  const repo = s.repoPath;
  const now = Date.now();
  const clock = {
    now,
    interval: minutes(),
    lastAttempt: lastAttempt.get(repo) ?? null,
    since: Math.max(since.get(repo) ?? now, enabledAt),
  };
  const cond = {
    now,
    repo: true,
    visible: document.visibilityState !== "hidden",
    busy: busy(),
    busyEndedAt,
    operation: s.operation.kind !== "none",
    inFlight: inFlight(),
  };
  if (!shouldFetch(clock, cond)) return;
  setInFlight(true);
  try {
    if (await windowHidden()) return;
    // Past an await: the person may have started something, or left the repository.
    if (busy() || state()?.repoPath !== repo) return;
    let outcome: FetchOutcome;
    try {
      outcome = await repoFetchBackground();
      if (outcome === "fetched" && lastError()?.repo === repo) setLastError(null);
    } catch (e) {
      outcome = "error";
      const be = e as { stderr?: string | null; message?: string } | undefined;
      const text = errText(e);
      setLastError({ repo, line: errorSummary(be?.stderr, be?.message ?? text), text, at: Date.now() });
    }
    if (countsAsAttempt(outcome)) lastAttempt.set(repo, Date.now());
  } finally {
    setInFlight(false);
  }
}

/** Start the scheduler. Call once, from inside a component's synchronous setup. */
export function startBackgroundFetch(): () => void {
  return createRoot((dispose) => {
    createEffect(
      on(
        busy,
        (b) => {
          if (!b) busyEndedAt = Date.now();
        },
        { defer: true },
      ),
    );
    // The path, not the state: every refresh installs a new state object, and
    // re-arming the timer on each would keep pushing the next tick away.
    const repoPath = createMemo(() => state()?.repoPath ?? null);
    createEffect(() => {
      const repo = repoPath();
      if (repo && !since.has(repo)) since.set(repo, Date.now());
    });
    // A timer only while there is something to do: switched on, a repository open.
    createEffect(() => {
      if (!minutes() || !repoPath()) return;
      const t = setInterval(() => void tick(), TICK_MS);
      onCleanup(() => clearInterval(t));
    });
    return dispose;
  });
}
