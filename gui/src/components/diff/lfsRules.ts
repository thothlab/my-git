/**
 * Pure rules of the Git LFS card in the diff panel: what the change did to the
 * stored file, and sizes as a person reads them. Recognising a pointer is the
 * backend's (`engine::lfs`) — this module only words what it found.
 *
 * Imports nothing, on purpose: `scripts/check-log-filters.mjs` transpiles it
 * rather than bundling it, and one import would fail module resolution there.
 */

/** The shape of `api.LfsDiff` this module reads, restated structurally. */
export interface LfsSides {
  old: { oid: string; size: number } | null;
  new: { oid: string; size: number } | null;
}

export type LfsChange = "added" | "removed" | "replaced" | "unchanged";

/** What the change did to the file kept in LFS. */
export function lfsChange(l: LfsSides): LfsChange {
  if (!l.old) return "added";
  if (!l.new) return "removed";
  return l.old.oid === l.new.oid ? "unchanged" : "replaced";
}

export type SizeUnit = "B" | "KB" | "MB" | "GB" | "TB";

/**
 * 1536 → `{ value: 1.5, unit: "KB" }`. Powers of 1024, as git-lfs counts. One
 * decimal below ten, whole numbers above; bytes are always whole. `null` for
 * something that is not a size, so the caller shows nothing rather than "NaN".
 */
export function scaleBytes(bytes: number): { value: number; unit: SizeUnit } | null {
  if (!Number.isFinite(bytes) || bytes < 0) return null;
  if (bytes < 1024) return { value: Math.round(bytes), unit: "B" };
  const units: SizeUnit[] = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  // Rounding can carry into the next unit: 1023.96 KB is "1 MB", not "1024 KB".
  let rounded = value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
  if (rounded >= 1024 && i < units.length - 1) {
    rounded = 1;
    i++;
  }
  return { value: rounded, unit: units[i] };
}

/** The first twelve digits of an oid — enough to tell objects apart on screen. */
export function shortOid(oid: string): string {
  return oid.slice(0, 12);
}
