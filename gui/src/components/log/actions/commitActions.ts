import {
  commitCheckout,
  commitCherryPick,
  commitReset,
  commitResetLostCount,
  commitRevert,
  errText,
  tagCreate,
  WORKING_TREE,
  type LogCommit,
  type ResetMode,
} from "../../../api";
import { d } from "../../../i18n";
import { chooseOption, confirmAction, run, setError, state } from "../../../store";
import { copyOrReport, newBranchFrom } from "./branchActions";
import type { MenuEntry } from "./ContextMenu";
import { setCompareTarget } from "./compareSelection";
import { openDialog } from "./dialogs";
import { afterRepoChange, localChangesNow, operationActive, runResult } from "./repoRefresh";

/**
 * Actions over commits and the log's context menu.
 *
 * The menu takes its targets from the selection, not from the row under the
 * pointer: a right-click inside the selection acts on the whole selection and
 * says how big it is, a right-click outside it acts on that row alone and
 * leaves the selection where it was (PRD История 51). Deciding that is the
 * caller's job — the panel knows what was clicked; this module is given the
 * targets already resolved.
 *
 * Everything a confirmation has to state is asked for before the dialog is
 * shown: how many commits a reset discards, whether the tree is dirty, whether
 * a commit is already on the current branch. None of it is extracted from the
 * text of a refusal.
 */

/** The commits a menu acts on, in the log's display order (newest first). */
export interface CommitTargets {
  commits: LogCommit[];
}

// ── Actions ──────────────────────────────────────────────────────────────────

export const copyHashes = (targets: LogCommit[]) =>
  copyOrReport(targets.map((c) => c.hash).join("\n"));

/**
 * Compare two commits. Which one is the "from" side is decided by their order
 * in the log, not by the order they happened to be selected in: the older one
 * is the base, so the diff reads as "what changed on the way to the newer one".
 */
export function compareCommits(targets: LogCommit[]): void {
  if (targets.length !== 2) return;
  const [newer, older] = targets; // display order is newest first
  setCompareTarget({
    from: older.hash,
    to: newer.hash,
    fromLabel: older.shortHash,
    toLabel: newer.shortHash,
  });
}

/** Compare one commit with what is on disk right now (История 77). */
export function compareWithWorkingTree(commit: LogCommit): void {
  setCompareTarget({
    from: commit.hash,
    to: WORKING_TREE,
    fromLabel: commit.shortHash,
    toLabel: d().workingTreeSide(),
  });
}

export async function tagCommit(commit: LogCommit): Promise<void> {
  await openDialog({
    title: d().dlgTagTitle(commit.shortHash),
    fields: [
      { key: "name", label: d().dlgTagName() },
      { key: "message", label: d().dlgTagMessage(), optional: true },
    ],
    submitLabel: d().dlgCreate(),
    submit: async (v) => {
      const msg = v.message.trim();
      const err = await runResult(
        tagCreate(commit.hash, v.name.trim(), msg === "" ? undefined : msg),
        d().phaseTag(),
      );
      if (err) return err;
      afterRepoChange();
      return null;
    },
  });
}

/** Check out a revision — the confirmation names the detached HEAD it causes. */
export async function checkoutRevision(commit: LogCommit): Promise<void> {
  if (!(await confirmAction(d().confirmCheckoutRevision(commit.shortHash), false))) return;
  await run(commitCheckout(commit.hash), d().phaseCheckout());
  afterRepoChange();
}

export async function revertCommit(commit: LogCommit): Promise<void> {
  await run(commitRevert(commit.hash), d().phaseRevert());
  afterRepoChange();
}

export async function cherryPickCommit(commit: LogCommit): Promise<void> {
  await run(commitCherryPick(commit.hash), d().phaseCherryPick());
  afterRepoChange();
}

/**
 * Reset the current branch to a commit.
 *
 * Two steps on purpose. The mode is a choice among four equal options, so it is
 * a chooser; `hard` then goes through a confirmation, which is the dialog whose
 * default is refusal and which Enter does not accept. Folding the two together
 * would put "discard everything" one keystroke away from "which mode again?".
 */
export async function resetToCommit(commit: LogCommit): Promise<void> {
  const branch = state()?.branch ?? "";
  const mode = (await chooseOption(d().resetChooseTitle(branch, commit.shortHash), [
    { key: "soft", label: d().resetSoft() },
    { key: "mixed", label: d().resetMixed() },
    { key: "keep", label: d().resetKeep() },
    { key: "hard", label: d().resetHard(), danger: true },
    { key: "cancel", label: d().cancel() },
  ])) as ResetMode | null;
  if (!mode || (mode as string) === "cancel") return;

  if (mode === "hard") {
    // Both facts are asked for before the question is put, so the dialog names
    // what is lost instead of describing it in the abstract.
    let lost = 0;
    let dirty = false;
    try {
      [lost, dirty] = await Promise.all([commitResetLostCount(commit.hash), localChangesNow()]);
    } catch (e) {
      setError(errText(e)); // git's own output, whole
      return;
    }
    const ok = await confirmAction(
      d().confirmHardReset(branch, commit.shortHash, lost, dirty),
      true,
    );
    if (!ok) return;
  }
  await run(commitReset(commit.hash, mode), d().phaseReset());
  afterRepoChange();
}

// ── Menu ─────────────────────────────────────────────────────────────────────

/**
 * Items of the log's context menu.
 *
 * `contains` is the answer of `commit_contains` for a single target: `null`
 * means the question is still in flight, and cherry-pick stays disabled with
 * "checking…" meanwhile. An item that is enabled and turns grey a moment later
 * is worse than one that starts out grey.
 */
export function commitMenuItems(
  targets: LogCommit[],
  contains: boolean | null,
): MenuEntry[] {
  const n = targets.length;
  const one = n === 1 ? targets[0] : null;
  const busyOp = operationActive();
  const opReason = busyOp ? d().whyOperationRunning() : undefined;
  const detached = !!state()?.detached;
  /** Reason a single-commit action cannot run right now, or undefined. */
  const singleReason = busyOp ? opReason : one ? undefined : d().whyOneCommitOnly();

  const items: MenuEntry[] = [];
  if (n === 0) return items;

  items.push({ label: d().menuCopyHash(n), run: () => void copyHashes(targets) });

  items.push({
    label: d().menuCompare(n),
    disabled: n !== 2,
    reason: n !== 2 ? d().whyNeedTwoCommits() : undefined,
    run: () => compareCommits(targets),
  });
  items.push({
    label: d().menuCompareWorktree(),
    disabled: !one,
    reason: one ? undefined : d().whyOneCommitOnly(),
    run: () => {
      if (one) compareWithWorkingTree(one);
    },
  });

  items.push({ kind: "sep" });
  items.push({
    label: d().menuBranchFromCommit(),
    disabled: !!singleReason,
    reason: singleReason,
    run: () => {
      if (one) void newBranchFromCommit(one);
    },
  });
  items.push({
    label: d().menuTagCommit(),
    disabled: !!singleReason,
    reason: singleReason,
    run: () => {
      if (one) void tagCommit(one);
    },
  });
  items.push({
    label: d().menuCheckoutRevision(),
    disabled: !!singleReason,
    reason: singleReason,
    run: () => {
      if (one) void checkoutRevision(one);
    },
  });

  items.push({ kind: "sep" });
  items.push({
    label: d().menuRevertCommit(),
    disabled: !!singleReason,
    reason: singleReason,
    run: () => {
      if (one) void revertCommit(one);
    },
  });
  const resetReason = singleReason ?? (detached ? d().whyDetachedHead() : undefined);
  items.push({
    label: d().menuResetHere(),
    danger: true,
    disabled: !!resetReason,
    reason: resetReason,
    run: () => {
      if (one) void resetToCommit(one);
    },
  });
  const pickReason =
    singleReason ?? (contains === null ? d().whyChecking() : contains ? d().whyAlreadyContained() : undefined);
  items.push({
    label: d().menuCherryPick(),
    disabled: !!pickReason,
    reason: pickReason,
    run: () => {
      if (one) void cherryPickCommit(one);
    },
  });

  return items;
}

/** "New branch from this commit" — the branch dialog, anchored on a hash. */
const newBranchFromCommit = (commit: LogCommit) =>
  newBranchFrom(commit.hash, commit.shortHash);
