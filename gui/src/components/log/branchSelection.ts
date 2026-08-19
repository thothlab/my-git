import { createSignal } from "solid-js";

/**
 * Which branch the log is scoped to — the seam between the branch tree and the
 * log store (task 08 reads it into `LogFilter.branch`).
 *
 * `null` means "current HEAD": the tree's top row and the detached-HEAD node
 * both select it, so the log falls back to the history of whatever HEAD points
 * at instead of naming a branch that may not exist.
 *
 * Module level on purpose: `<Show when={isLog()}>` unmounts the whole panel on
 * every mode switch, and a signal owned by the component would silently reset
 * the log's scope each time the user looks at Changes.
 */
const [selectedBranch, setSelectedBranch] = createSignal<string | null>(null);

export { selectedBranch, setSelectedBranch };
