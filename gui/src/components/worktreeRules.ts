/**
 * Pure rules of the Worktrees dialog: what each row may do and why not, which
 * branches can go into a new worktree, what the create form still lacks.
 *
 * No imports, on purpose: `scripts/check-log-filters.mjs` transpiles this file on
 * its own, and one import from `../api` would turn into a module-resolution failure
 * that reads as an unrelated breakage. The shape it works on is the subset of
 * `WorktreeInfo` it reads. Reasons are codes; the dialog words them (`i18n.ts`).
 */

export interface WorktreeLike {
  path: string;
  branch: string | null;
  bare: boolean;
  locked: boolean;
  prunable: boolean;
  isMain: boolean;
  isCurrent: boolean;
}

/** Why "Open" is unavailable for a row, or null. */
export type OpenBlock = "current" | "prunable" | "bare";
export function openBlock(w: WorktreeLike): OpenBlock | null {
  if (w.isCurrent) return "current";
  if (w.prunable) return "prunable";
  if (w.bare) return "bare";
  return null;
}

/** Why "Remove…" is unavailable, or null. The backend refuses the same cases; the
 *  dialog names them before the click. */
export type RemoveBlock = "main" | "current" | "locked" | "prunable";
export function removeBlock(w: WorktreeLike): RemoveBlock | null {
  if (w.isMain) return "main";
  if (w.isCurrent) return "current";
  if (w.locked) return "locked";
  if (w.prunable) return "prunable";
  return null;
}

/** Why "Lock" / "Unlock" is unavailable: git refuses both on the main worktree. */
export const lockBlock = (w: WorktreeLike): "main" | null => (w.isMain ? "main" : null);

/** The worktree that has `branch` checked out, if any. */
export function checkedOutIn<W extends WorktreeLike>(branch: string, all: readonly W[]): W | null {
  return all.find((w) => w.branch === branch) ?? null;
}

/** Local branches no worktree has checked out — what "existing branch" can offer. */
export function freeBranches(local: readonly string[], all: readonly WorktreeLike[]): string[] {
  const taken = new Set(all.map((w) => w.branch).filter((b): b is string => !!b));
  return local.filter((b) => !taken.has(b));
}

export const hasPrunable = (all: readonly WorktreeLike[]): boolean => all.some((w) => w.prunable);

/** Last path component — the row's title. */
export const folderOf = (path: string): string =>
  path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;

/** Whether `path` is absolute, on either platform — the backend refuses anything else. */
export const isAbsolutePath = (path: string): boolean =>
  path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");

export interface CreateInput {
  create: boolean;
  /** New branch name, or the chosen existing branch. */
  branch: string;
  path: string;
  /** Where each local branch is checked out (from the list). */
  worktrees: readonly WorktreeLike[];
  /** Names of local branches. */
  local: readonly string[];
}

/** What the create form still lacks, as a code, or null when it can be sent. Only
 *  what the client knows for sure: branch names are judged by git on the backend. */
export type CreateBlock = "no-branch" | "exists" | "taken" | "no-path" | "relative-path";
export function createBlock(i: CreateInput): CreateBlock | null {
  const branch = i.branch.trim();
  if (!branch) return "no-branch";
  if (i.create && i.local.includes(branch)) return "exists";
  if (!i.create && checkedOutIn(branch, i.worktrees)) return "taken";
  const path = i.path.trim();
  if (!path) return "no-path";
  if (!isAbsolutePath(path)) return "relative-path";
  return null;
}
