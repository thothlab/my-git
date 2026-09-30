/**
 * Pure rules of the branch tree's multi-selection: which rows a Shift range
 * takes, which marks count, what a context menu acts on, and whether a group
 * can be deleted — and why not.
 *
 * No imports, on purpose: `scripts/check-log-filters.mjs` transpiles this file on
 * its own, and one import from `../../api` would turn into a module-resolution
 * failure that reads as an unrelated breakage. The shapes it works on are the
 * subsets of the tree's `Row` and of `BranchNode` it reads. Reasons are codes;
 * the menu words them (`i18n.ts`).
 *
 * The one rule the rest follows: **what is acted on is what is drawn marked.**
 * The set of marks may hold rows the filter, a folded folder or "favourites
 * only" hides at the moment — they are kept (clearing the filter shows them
 * marked again), but never counted and never deleted while they are out of
 * sight. A group delete of branches the reader cannot see is exactly the
 * mistake a confirmation list is too long to catch.
 */

/** A drawn row of the tree: its key and kind. Only a branch row can be marked. */
export interface MarkRow {
  key: string;
  kind: "head" | "folder" | "branch";
}

const isBranch = (r: MarkRow) => r.kind === "branch";

/**
 * Branch rows from `from` to `to`, both included, in drawn order — what a
 * Shift+click or Shift+arrow marks. An anchor that is no longer drawn (or none
 * yet) starts the range at `to` itself. Folders and HEAD inside the range are
 * stepped over: they are not branches, so there is nothing to mark.
 */
export function rangeKeys(rows: readonly MarkRow[], from: string | null, to: string): string[] {
  const j = rows.findIndex((r) => r.key === to);
  if (j < 0) return [];
  const i = from === null ? -1 : rows.findIndex((r) => r.key === from);
  const [lo, hi] = i < 0 ? [j, j] : i <= j ? [i, j] : [j, i];
  return rows
    .slice(lo, hi + 1)
    .filter(isBranch)
    .map((r) => r.key);
}

/** `marks` with `key` added, or taken out if it was there. A new set: signals compare by identity. */
export function toggled(marks: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(marks);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

/** The marked rows the reader can see, in drawn order — the only ones that count. */
export function drawnMarks<R extends MarkRow>(rows: readonly R[], marks: ReadonlySet<string>): R[] {
  return rows.filter((r) => isBranch(r) && marks.has(r.key));
}

/**
 * What a context menu opened on `row` acts on: the whole drawn selection when
 * the row is part of one (more than one row), else that row alone — the log's
 * rule (`LogTable.openMenuAt`): a right-click outside the selection neither
 * acts on it nor changes it.
 */
export function menuTargets<R extends MarkRow>(drawn: readonly R[], row: R): R[] {
  return drawn.length > 1 && drawn.some((r) => r.key === row.key) ? [...drawn] : [row];
}

/** The subset of `BranchNode` a group delete is judged on. */
export interface BranchLike {
  name: string;
  isRemote: boolean;
  isCurrent: boolean;
}

/**
 * Why "Delete N branches" is unavailable for a group, or null.
 *
 * - `mixed` — local and remote branches together. Deleting a remote branch is
 *   another action: it goes to the server, it is gone for everyone, and Undo
 *   cannot take it back. It gets its own item and its own confirmation, never
 *   a line inside a local one.
 * - `current` — the current branch is among them (named, so the reader knows
 *   which mark to take off). git refuses it; so does the backend, before it
 *   deletes anything else.
 */
export type GroupDeleteBlock = { code: "mixed" } | { code: "current"; name: string };

export function groupDeleteBlock(nodes: readonly BranchLike[]): GroupDeleteBlock | null {
  const remote = nodes.filter((n) => n.isRemote).length;
  if (remote > 0 && remote < nodes.length) return { code: "mixed" };
  const current = nodes.find((n) => !n.isRemote && n.isCurrent);
  return current ? { code: "current", name: current.name } : null;
}
