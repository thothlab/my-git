import { createSignal } from "solid-js";
import type { FileState } from "../../api";

/**
 * The file picked inside the commit-details panel — the seam between that panel
 * (task 09, the only writer) and the diff panel (task 10, the only reader).
 *
 * `hash` travels with the path on purpose: the reader needs it for
 * `commitFileDiff(hash, path, ws)` and it lets a reader notice that the
 * selection belongs to a commit other than the one it is showing. The selection
 * is cleared whenever the selected commit changes — no file is auto-selected,
 * so the diff panel keeps its own empty state until the user asks for a file.
 */
export interface CommitFileSelection {
  hash: string;
  path: string;
  /** Previous path of a renamed file, `null` otherwise. */
  oldPath: string | null;
  status: FileState;
}

const [selection, setSelection] = createSignal<CommitFileSelection | null>(null);

export const selectedCommitFile = selection;
export const setSelectedCommitFile = setSelection;
