import { createSignal } from "solid-js";
import { repoLocalChanges, type RepoState } from "../../../api";
import { error, run, state } from "../../../store";
import { loaded, refreshLog } from "../../../logStore";

/**
 * What every action does after the repository changed.
 *
 * `run()` installs the fresh `RepoState`, and that is enough for anything read
 * off it — the operation state, the current branch, the counters in the status
 * bar. It is not enough for the two panels that read git through their own
 * requests: the branch tree's resource is keyed on the repository path, which a
 * delete or a rename does not move, and the log's pages were cut before the
 * history changed. A `reset --hard` moves the tip *backwards*, so the log's own
 * "are there newer commits" probe cannot notice it either.
 *
 * Hence one revision counter the tree keys on as well, bumped here, and an
 * explicit reload of the log — never a hope that some other effect will see it.
 */
const [revision, setRevision] = createSignal(0);

/** Bumped whenever an action changed refs; the branch tree re-reads on it. */
export const repoRevision = revision;

export function bumpRepoRevision(): void {
  setRevision((n) => n + 1);
}

/**
 * Run a mutation and report what git said, rather than only pushing it into the
 * error banner: a dialog that must stay open needs the text itself.
 *
 * The message is `run()`'s own — `errText` joins the message with git's whole
 * output — so the banner and the dialog say exactly the same thing, verbatim
 * (PRD История 31).
 */
export async function runResult(p: Promise<RepoState>, label = ""): Promise<string | null> {
  await run(p, label);
  return error() || null;
}

/** After a change to refs or history: re-read the branch tree and the log. */
export function afterRepoChange(opts: { log?: boolean; tree?: boolean } = {}): void {
  if (opts.tree !== false) bumpRepoRevision();
  if (opts.log !== false && loaded()) void refreshLog();
}

/** Is a merge / rebase / cherry-pick / revert unfinished right now? */
export const operationActive = (): boolean => {
  const k = state()?.operation?.kind;
  return !!k && k !== "none";
};

/**
 * Is there anything uncommitted right now?
 *
 * One question, one answer: the `repo_local_changes` command. The obvious
 * shortcut — deriving it from the changelists the panel already holds — gives a
 * *different* answer, because that view leaves untracked files out; the checkout
 * dialog and the hard-reset dialog would then disagree about the same working
 * tree, and only one of them could be right.
 */
export const localChangesNow = (): Promise<boolean> => repoLocalChanges();
