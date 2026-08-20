import { createSignal } from "solid-js";

/**
 * The comparison the panel is showing, if any — the seam between the log's
 * "Compare" actions (this task, the only writer) and the layout that renders
 * the file list and the diff for it (task 14, the reader).
 *
 * `from` and `to` are what `commitsCompare(from, to)` takes: `from` is the older
 * side. `api.WORKING_TREE` (the empty string) on the `to` side means the working
 * tree, which is why the labels travel with the pair — a side named "" cannot be
 * shown to anyone, and the reader must not have to reconstruct the wording.
 *
 * `null` means "no comparison": the details panel shows the selected commit.
 */
export interface CompareTarget {
  from: string;
  to: string;
  fromLabel: string;
  toLabel: string;
}

const [compareTarget, setCompareTarget] = createSignal<CompareTarget | null>(null);

export { compareTarget, setCompareTarget };

/** Leave comparison mode (a plain commit selection replaces it). */
export const clearCompare = () => setCompareTarget(null);
