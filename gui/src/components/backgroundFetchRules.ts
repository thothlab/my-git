/**
 * Pure rules of the scheduled background fetch (`src/backgroundFetch.ts`): which
 * intervals exist, when a fetch is due, and what holds a due one back.
 *
 * Imports nothing, on purpose: `scripts/check-log-filters.mjs` transpiles it
 * rather than bundling it, and one import would fail module resolution there.
 */

/** Minutes between fetches; 0 is Off, the default. */
export const FETCH_INTERVALS = [0, 5, 15, 30, 60] as const;

/** A repository that became current (or scheduling that was switched on) is
 *  first fetched this long after, not at once: opening a repository is not a
 *  request to go to the network in the same second. */
export const FIRST_FETCH_DELAY_MS = 15_000;

/**
 * Quiet time after the person's last action before a background fetch starts.
 *
 * Longer than the watcher's own grace (`watch::OWN_GRACE`, 1 s) on purpose: the
 * background fetch is not the person's action, and it relies on the git-dir
 * watcher to report what it moved. Events arriving within the grace after an
 * action are taken for that action's own and dropped — a fetch finishing then
 * would move `refs/remotes/` without the window ever re-reading.
 */
export const QUIET_AFTER_BUSY_MS = 3_000;

/** A stored value, read back: anything that is not one of the intervals is Off. */
export function normalizeInterval(value: unknown): number {
  const n = typeof value === "string" ? Number(value) : value;
  return (FETCH_INTERVALS as readonly unknown[]).includes(n) ? (n as number) : 0;
}

export interface FetchClock {
  now: number;
  /** Minutes, one of `FETCH_INTERVALS`. */
  interval: number;
  /** The last attempt on this repository in this session — success or failure. */
  lastAttempt: number | null;
  /** When this repository became current, or scheduling was switched on, which
   *  ever is later. */
  since: number;
}

/**
 * When the next fetch of this repository is due, or `null` when scheduling is
 * off. A failed attempt counts as an attempt: offline or refused credentials
 * wait for the next interval instead of retrying in a loop.
 */
export function dueAt(c: FetchClock): number | null {
  if (!c.interval) return null;
  if (c.lastAttempt === null) return c.since + FIRST_FETCH_DELAY_MS;
  // A shorter interval chosen after the last attempt applies at once; the first
  // fetch after switching on still waits its delay.
  return Math.max(c.lastAttempt + c.interval * 60_000, c.since + FIRST_FETCH_DELAY_MS);
}

export interface FetchConditions {
  now: number;
  /** A repository is open. */
  repo: boolean;
  /** The window is shown (not hidden, not minimized). */
  visible: boolean;
  /** A mutation of the person runs (`store.busy()`). */
  busy: boolean;
  /** When `busy` last went false, if it ever did. */
  busyEndedAt: number | null;
  /** A merge / rebase / cherry-pick / revert / bisect is unfinished. */
  operation: boolean;
  /** A background fetch is already running. */
  inFlight: boolean;
}

export type FetchHold = "no-repo" | "hidden" | "busy" | "settling" | "operation" | "in-flight";

/** What holds a due fetch back, or `null` when nothing does. A hold does not
 *  count as an attempt: the fetch runs as soon as it lifts. */
export function fetchHold(c: FetchConditions): FetchHold | null {
  if (!c.repo) return "no-repo";
  if (c.inFlight) return "in-flight";
  if (!c.visible) return "hidden";
  if (c.busy) return "busy";
  if (c.busyEndedAt !== null && c.now - c.busyEndedAt < QUIET_AFTER_BUSY_MS) return "settling";
  if (c.operation) return "operation";
  return null;
}

/** Start a fetch now? */
export function shouldFetch(clock: FetchClock, cond: FetchConditions): boolean {
  const due = dueAt(clock);
  return due !== null && clock.now >= due && fetchHold(cond) === null;
}

/** What the backend answered, or `error` for a refusal. */
export type FetchOutcome = "fetched" | "busy" | "operation" | "no-remotes" | "error";

/**
 * Does this outcome use up the interval? A fetch that ran — or failed, or found
 * no remote to talk to — does; one that gave way to something else running does
 * not, and is tried again on the next tick.
 */
export function countsAsAttempt(o: FetchOutcome): boolean {
  return o !== "busy" && o !== "operation";
}

/**
 * The one line of a failed fetch the status bar shows: the first line git wrote
 * to stderr (`fatal: … does not appear to be a git repository`, `Could not
 * resolve host`), else the first line of the message. The message itself opens
 * with the whole command line (`git fetch --quiet --all … failed:`), which is
 * the least useful part in a line this short.
 */
export function errorSummary(stderr: string | null | undefined, message: string): string {
  const first = (t: string) =>
    t
      .split("\n")
      .map((l) => l.trim())
      .find((l) => l !== "") ?? "";
  return first(stderr ?? "") || first(message);
}
