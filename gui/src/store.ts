import { createSignal } from "solid-js";
import {
  errText,
  repoState as apiRepoState,
  type FileState,
  type RepoState,
} from "./api";

// Single-window app → module-level signals are the store. Every command that
// mutates the repo returns a fresh RepoState; run() funnels them through one place
// so busy/error handling is uniform and the UI never blocks.

export const [state, setState] = createSignal<RepoState | null>(null);
export const [error, setError] = createSignal("");
export const [busy, setBusy] = createSignal(false);

/** path of the file whose diff is shown on the right */
export const [selectedPath, setSelectedPath] = createSignal<string | null>(null);
/** list highlighted for list-level actions / commit (defaults to the active list) */
export const [selectedListId, setSelectedListId] = createSignal<string>("default");
/** multi-select for move/commit-subset */
export const [checked, setChecked] = createSignal<Set<string>>(new Set());

export async function run(p: Promise<RepoState>): Promise<void> {
  setBusy(true);
  try {
    const s = await p;
    setState(s);
    setError("");
    // keep selection valid after the tree changes
    const paths = new Set(s.changelists.flatMap((c) => c.files.map((f) => f.path)));
    if (selectedPath() && !paths.has(selectedPath()!)) setSelectedPath(null);
    setChecked((prev) => new Set([...prev].filter((x) => paths.has(x))));
    if (!s.changelists.some((c) => c.id === selectedListId())) {
      setSelectedListId(s.activeChangelistId);
    }
  } catch (e) {
    setError(errText(e));
  } finally {
    setBusy(false);
  }
}

export const refresh = () => run(apiRepoState());

export function toggleChecked(path: string) {
  setChecked((prev) => {
    const n = new Set(prev);
    n.has(path) ? n.delete(path) : n.add(path);
    return n;
  });
}
export const isChecked = (path: string) => checked().has(path);

// ── In-app modals (styled, not native alert/confirm) ─────────────────────────

export const [confirmState, setConfirmState] = createSignal<{
  message: string;
  danger: boolean;
  resolve: (ok: boolean) => void;
} | null>(null);

export function confirmAction(message: string, danger = true): Promise<boolean> {
  return new Promise((resolve) => setConfirmState({ message, danger, resolve }));
}

export const [promptState, setPromptState] = createSignal<{
  title: string;
  value: string;
  resolve: (v: string | null) => void;
} | null>(null);

export function promptText(title: string, value = ""): Promise<string | null> {
  return new Promise((resolve) => setPromptState({ title, value, resolve }));
}

export type ChoiceOption = { key: string; label: string; danger?: boolean };
export const [chooseState, setChooseState] = createSignal<{
  message: string;
  options: ChoiceOption[];
  resolve: (key: string | null) => void;
} | null>(null);

export function chooseOption(
  message: string,
  options: ChoiceOption[],
): Promise<string | null> {
  return new Promise((resolve) => setChooseState({ message, options, resolve }));
}

/** Status → { letter, colour class }. Mirrors the TUI palette mapping. */
export function statusMeta(s: FileState): { letter: string; cls: string } {
  switch (s) {
    case "modified":
      return { letter: "M", cls: "text-warn" };
    case "added":
      return { letter: "A", cls: "text-success" };
    case "deleted":
      return { letter: "D", cls: "text-danger" };
    case "renamed":
      return { letter: "R", cls: "text-accent" };
    case "conflicted":
      return { letter: "C", cls: "text-danger" };
    case "untracked":
      return { letter: "?", cls: "text-fg-muted" };
  }
}
