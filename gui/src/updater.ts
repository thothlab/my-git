/**
 * Self-update: the plumbing behind the "Check for updates" button.
 *
 * The endpoint and the public key live in `src-tauri/tauri.conf.json` under
 * `plugins.updater` — a single `latest.json` attached to the GitHub release
 * marked `releases/latest`. Until a release carries that manifest the endpoint
 * answers 404, and a check has to say so out loud rather than look like a
 * successful "you are up to date": see `checkForUpdatesNow`, which returns the
 * outcome instead of reporting it, so the caller decides what to say.
 *
 * Desktop only. In a plain browser (`npm run dev` without Tauri) the plugin
 * imports would throw, so every entry point here no-ops — and a button that
 * silently reports nothing is worse than no button at all, which is why the UI
 * gates on `updatesSupported` instead of trusting the no-op.
 *
 * The plugin modules are pulled in with a dynamic `import()` for the same
 * reason: the bundle must load in a browser, and a static import of
 * `@tauri-apps/plugin-updater` runs IPC code at module scope.
 */

import { createSignal } from "solid-js";

/** Narrow view of the plugin's `Update`. The plugin object never leaves here. */
type UpdateHandle = {
  version: string;
  downloadAndInstall: () => Promise<void>;
};

/**
 * `__TAURI_INTERNALS__`, not `__TAURI__`: the latter exists only when
 * `withGlobalTauri` is on, and it is not.
 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** True in the desktop app only — the UI hides the update controls otherwise. */
export const updatesSupported = isTauri();

const [pending, setPending] = createSignal<UpdateHandle | null>(null);
const [checking, setChecking] = createSignal(false);
const [lastCheckedAt, setLastCheckedAt] = createSignal<Date | null>(null);

export const pendingUpdate = pending;
export const isCheckingForUpdates = checking;
export const updaterLastCheckedAt = lastCheckedAt;

let bootChecked = false;

export type CheckOutcome = { found: boolean; error?: unknown };

/**
 * The check in flight, if any. A second caller joins it instead of starting
 * its own: the hourly tick and the focus handler fire without asking whether
 * the About button is mid-check, and two overlapping runs make the second
 * `finally` clear `checking` while the first is still going — the button greys
 * out and un-greys for no reason the reader can see. Joining also keeps the
 * manual click honest: it reports the shared outcome instead of an invented
 * "up to date".
 */
let inFlight: Promise<CheckOutcome> | null = null;

function runCheck(): Promise<CheckOutcome> {
  if (!isTauri()) return Promise.resolve({ found: false });
  if (inFlight) return inFlight;
  inFlight = doCheck().finally(() => {
    inFlight = null;
  });
  return inFlight;
}

async function doCheck(): Promise<CheckOutcome> {
  setChecking(true);
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    // Only a check that reached the server counts as a check: a 404 or a dead
    // network must not refresh "last checked", or the timestamp would claim a
    // freshness nobody verified.
    setLastCheckedAt(new Date());
    if (!update) {
      setPending(null);
      return { found: false };
    }
    setPending({
      version: update.version,
      downloadAndInstall: () => update.downloadAndInstall(),
    });
    return { found: true };
  } catch (e) {
    console.debug("[updater] check failed", e);
    return { found: false, error: e };
  } finally {
    setChecking(false);
  }
}

/** Silent check at boot. Idempotent: repeated mounts do not re-check. */
export async function checkForUpdatesOnStartup(): Promise<void> {
  if (bootChecked || !isTauri()) return;
  bootChecked = true;
  await runCheck();
}

/**
 * A check whose outcome goes back to the caller: the button reports all three
 * endings (found / up to date / unreachable), the hourly timer and the focus
 * handler report none.
 */
export function checkForUpdatesNow(): Promise<CheckOutcome> {
  return runCheck();
}

/**
 * Download and install the pending bundle, then restart into it.
 *
 * Resolves to an error message on failure, `null` on success — the caller shows
 * it. Nothing is thrown and nothing is `alert()`ed: this WebView draws no native
 * dialog, so a failed install used to look exactly like a hung one.
 */
export async function installPendingUpdate(): Promise<string | null> {
  const update = pending();
  if (!update) return null;
  try {
    await update.downloadAndInstall();
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
    return null;
  } catch (e) {
    console.error("[updater] install failed", e);
    return (e as Error)?.message ?? String(e);
  }
}
