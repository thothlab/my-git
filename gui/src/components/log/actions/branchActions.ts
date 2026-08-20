import {
  branchCheckout,
  branchCreate,
  branchDelete,
  branchMerge,
  branchRebaseOnto,
  branchRename,
  branchUnmergedCount,
  branchUpdate,
  errText,
  fetchRemote,
  push,
  type BranchNode,
} from "../../../api";
import { d } from "../../../i18n";
import { chooseOption, confirmAction, run, setError, state } from "../../../store";
import { openStashPanel } from "../../StashPanel";
import { selectedBranch, setSelectedBranch } from "../branchSelection";
import { copyText } from "./clipboard";
import type { MenuEntry } from "./ContextMenu";
import { openDialog } from "./dialogs";
import {
  afterRepoChange,
  localChangesNow,
  operationActive,
  runResult,
} from "./repoRefresh";

/**
 * Every action the branch tree offers, and the menu that offers them.
 *
 * The rules the whole file is built around:
 *
 *  - **What a dangerous confirmation costs is asked before it is shown.**
 *    `branchUnmergedCount` runs before the delete, so the dialog can name the
 *    number of commits; the alternative — deleting, failing and reading the
 *    number out of git's refusal — states the cost only once it is too late to
 *    matter, and depends on the wording of a message we do not own.
 *  - **A disabled item names its reason** (PRD История 59); the menu never
 *    offers something that will be refused after the click.
 *  - **An unfinished merge / rebase disables everything that mutates**, and
 *    nothing that only reads: copying a branch name during a conflict is fine.
 */

const remoteOf = (fullRemoteBranch: string) => fullRemoteBranch.split("/")[0];
const localNameOf = (remoteBranch: string) => remoteBranch.split("/").slice(1).join("/");

/** Upstream remote of the current branch, or null when it tracks nothing. */
const currentRemote = (): string | null => {
  const up = state()?.upstream;
  return up ? remoteOf(up) : null;
};

// ── Actions ──────────────────────────────────────────────────────────────────

/**
 * Check out a branch. A remote branch is checked out by its local name, which is
 * what makes git create the tracking branch (`branches/spec.md`, "Remote branch
 * checkout"); a dirty tree asks first, and the stash it may create shows up in
 * the stash panel marked as the application's own.
 */
export async function checkoutBranch(node: BranchNode): Promise<void> {
  const target = node.isRemote ? localNameOf(node.name) : node.name;
  let stash = false;
  if (await localChangesNow()) {
    const k = await chooseOption(d().switchDirty(target), [
      { key: "stash", label: d().stashAndSwitch() },
      { key: "switch", label: d().switchAsIs() },
      { key: "cancel", label: d().cancel() },
    ]);
    if (!k || k === "cancel") return;
    stash = k === "stash";
  }
  await run(branchCheckout(target, stash), d().phaseCheckout());
  afterRepoChange();
}

/**
 * New branch from `from` (a branch name or a commit hash).
 *
 * The engine creates with `checkout -b`, so the branch is always switched to.
 * Leaving the box unticked therefore means "switch back afterwards" rather than
 * "do not switch", which is honest about what happens and keeps the option the
 * story asks for without reaching into the engine's zone.
 */
export async function newBranchFrom(from: string, fromLabel: string): Promise<void> {
  const previous = state()?.detached ? null : (state()?.branch ?? null);
  await openDialog({
    title: d().dlgNewBranchTitle(fromLabel),
    note: previous ? d().dlgNewBranchNote(previous) : undefined,
    fields: [{ key: "name", label: d().dlgBranchName() }],
    checkbox: previous ? { label: d().dlgCheckoutNew(), checked: true } : undefined,
    submitLabel: d().dlgCreate(),
    submit: async (v, checked) => {
      const err = await runResult(branchCreate(v.name.trim(), from), d().phaseCreateBranch());
      if (err) return err;
      if (previous && !checked) {
        const back = await runResult(branchCheckout(previous, false), d().phaseCheckout());
        if (back) return back;
      }
      afterRepoChange();
      return null;
    },
  });
}

/** Rename a local branch. A duplicate keeps the dialog open with the name in it. */
export async function renameBranch(node: BranchNode): Promise<void> {
  await openDialog({
    title: d().dlgRenameBranchTitle(node.name),
    fields: [{ key: "name", label: d().dlgNewName(), value: node.name }],
    submitLabel: d().dlgRename(),
    submit: async (v) => {
      const to = v.name.trim();
      const err = await runResult(branchRename(node.name, to), d().phaseRenameBranch());
      if (err) return err;
      if (selectedBranch() === node.name) setSelectedBranch(to);
      afterRepoChange();
      return null;
    },
  });
}

/**
 * Delete a local branch. The number of commits no other branch holds is asked
 * for first, so the confirmation states it; `force` is passed only after that
 * confirmation, never as a way of getting past the engine's own refusal.
 */
export async function deleteBranch(node: BranchNode): Promise<void> {
  let unmerged = 0;
  try {
    unmerged = await branchUnmergedCount(node.name);
  } catch (e) {
    // errText, not the message alone: git's whole output is the half that names
    // the file or the ref, and dropping it is what "операция не удалась" is.
    setError(errText(e));
    return;
  }
  const ok = await confirmAction(
    unmerged > 0
      ? d().confirmDeleteUnmerged(node.name, unmerged)
      : d().confirmDeleteBranch(node.name),
    unmerged > 0,
  );
  if (!ok) return;
  await run(branchDelete(node.name, false, unmerged > 0), d().phaseDeleteBranch());
  if (selectedBranch() === node.name) setSelectedBranch(null);
  afterRepoChange();
}

/** Delete a branch on the remote — its own item behind its own confirmation. */
export async function deleteRemoteBranch(node: BranchNode): Promise<void> {
  if (!(await confirmAction(d().confirmDeleteRemote(node.name), true))) return;
  await run(branchDelete(node.name, true, false), d().phaseDeleteBranch());
  if (selectedBranch() === node.name) setSelectedBranch(null);
  afterRepoChange();
}

/** Merge the selected branch into the current one; a conflict lands in the bar. */
export async function mergeBranch(node: BranchNode): Promise<void> {
  await run(branchMerge(node.name), d().phaseMerge());
  afterRepoChange();
}

/** Rebase the current branch onto the selected one. */
export async function rebaseOntoBranch(node: BranchNode): Promise<void> {
  await run(branchRebaseOnto(node.name), d().phaseRebase());
  afterRepoChange();
}

export async function pushCurrent(): Promise<void> {
  await run(push(state()?.upstream ? "normal" : "upstream"), d().phasePush());
  afterRepoChange();
}

/** Force push, behind a confirmation that names the branch and the remote. */
export async function forcePushCurrent(): Promise<void> {
  const branch = state()?.branch ?? "";
  const remote = currentRemote() ?? "";
  if (!(await confirmAction(d().confirmForcePush(branch, remote), true))) return;
  await run(push("force"), d().phaseForcePush());
  afterRepoChange();
}

export async function fetchAll(): Promise<void> {
  await run(fetchRemote(), d().fetching());
  afterRepoChange();
}

/**
 * Bring a branch up to date with its upstream (История 21c).
 *
 * The engine decides what "update" means: a pull for the current branch, a
 * fast-forward in place for any other. A branch that has commits its upstream
 * does not is refused there, verbatim — this side only offers the action for a
 * branch that has an upstream at all, so the common "nothing to update from"
 * case is a disabled item with a reason rather than a refusal after the click.
 */
export async function updateBranch(node: BranchNode): Promise<void> {
  await run(branchUpdate(node.name), d().phaseBranchUpdate());
  afterRepoChange();
}

// ── Menu ─────────────────────────────────────────────────────────────────────

/**
 * Items of the branch tree's context menu.
 *
 * `node` is null for the HEAD row, which owns no branch of its own: only the
 * repository-wide half of the menu applies to it.
 */
export function branchMenuItems(node: BranchNode | null, refreshTree: () => void): MenuEntry[] {
  const busyOp = operationActive();
  const opReason = busyOp ? d().whyOperationRunning() : undefined;
  const detached = !!state()?.detached;
  const currentBranch = state()?.branch ?? "";
  const after = (p: Promise<void>) => void p.then(refreshTree);

  const items: MenuEntry[] = [];

  if (node) {
    items.push({
      label: node.isRemote ? d().menuCheckoutTracking(localNameOf(node.name)) : d().menuCheckout(),
      disabled: busyOp || node.isCurrent,
      reason: busyOp ? opReason : node.isCurrent ? d().whyCurrentBranch() : undefined,
      run: () => after(checkoutBranch(node)),
    });
    items.push({
      label: d().menuNewBranchHere(),
      disabled: busyOp,
      reason: opReason,
      run: () => after(newBranchFrom(node.name, node.name)),
    });
    items.push({
      label: d().menuRenameBranch(),
      disabled: busyOp || node.isRemote,
      reason: busyOp ? opReason : node.isRemote ? d().whyRemoteBranch() : undefined,
      run: () => after(renameBranch(node)),
    });
    if (node.isRemote) {
      items.push({
        label: d().menuDeleteRemoteBranch(),
        danger: true,
        disabled: busyOp,
        reason: opReason,
        run: () => after(deleteRemoteBranch(node)),
      });
    } else {
      items.push({
        label: d().menuDeleteBranch(),
        danger: true,
        disabled: busyOp || node.isCurrent,
        reason: busyOp ? opReason : node.isCurrent ? d().whyCurrentBranch() : undefined,
        run: () => after(deleteBranch(node)),
      });
    }

    items.push({ kind: "sep" });
    const sameAsCurrent = node.isCurrent;
    const mergeReason = busyOp
      ? opReason
      : detached
        ? d().whyDetachedHead()
        : sameAsCurrent
          ? d().whyCurrentBranch()
          : undefined;
    items.push({
      label: d().menuMergeInto(currentBranch),
      disabled: !!mergeReason,
      reason: mergeReason,
      run: () => after(mergeBranch(node)),
    });
    items.push({
      label: d().menuRebaseOnto(node.name),
      disabled: !!mergeReason,
      reason: mergeReason,
      run: () => after(rebaseOntoBranch(node)),
    });
    const updateReason = busyOp
      ? opReason
      : node.isRemote
        ? d().whyRemoteBranch()
        : node.upstream
          ? undefined
          : d().whyNoUpstreamUpdate();
    items.push({
      label: d().menuUpdateBranch(node.name),
      disabled: !!updateReason,
      reason: updateReason,
      run: () => after(updateBranch(node)),
    });
    items.push({ kind: "sep" });
    items.push({
      label: d().menuCopyBranchName(),
      run: () => void copyOrReport(node.name),
    });
  }

  items.push({ kind: "sep" });
  const pushReason = busyOp ? opReason : detached ? d().whyDetachedHead() : undefined;
  items.push({
    label: d().menuPush(),
    disabled: !!pushReason,
    reason: pushReason,
    run: () => after(pushCurrent()),
  });
  const forceReason = pushReason ?? (state()?.upstream ? undefined : d().whyNoUpstream());
  items.push({
    label: d().menuForcePush(),
    danger: true,
    disabled: !!forceReason,
    reason: forceReason,
    run: () => after(forcePushCurrent()),
  });
  items.push({
    label: d().menuFetch(),
    disabled: busyOp,
    reason: opReason,
    run: () => after(fetchAll()),
  });
  items.push({
    label: d().menuStashes(),
    run: () => openStashPanel(),
  });

  return items;
}

/** Copy, and say so when the platform refused — a silent failure looks like success. */
export async function copyOrReport(text: string): Promise<void> {
  if (!(await copyText(text))) setError(d().copyFailed());
}
