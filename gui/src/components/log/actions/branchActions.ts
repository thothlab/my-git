import {
  branchCheckout,
  branchCreate,
  branchDelete,
  branchDeleteMany,
  branchMerge,
  branchRebaseOnto,
  branchRename,
  branchUnmergedCount,
  branchUpdate,
  fetchRemote,
  push,
  type BranchNode,
} from "../../../api";
import { d } from "../../../i18n";
import { chooseOption, confirmAction, reportError, run, setError, state } from "../../../store";
import { openStashPanel } from "../../StashPanel";
import { openWorktreeCreate } from "../../WorktreesPanel";
import { selectedBranch, setSelectedBranch } from "../branchSelection";
import { groupDeleteBlock } from "../branchMarks";
import { copyText } from "./clipboard";
import type { MenuEntry } from "./ContextMenu";
import { openDialog } from "./dialogs";
import {
  afterRepoChange,
  localChangesNow,
  operationActive,
  operationReason,
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
    // errText (inside reportError), not the message alone: git's whole output is
    // the half that names the file or the ref, and dropping it is what "операция
    // не удалась" is.
    reportError(e);
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

/**
 * Delete a group from the tree's multi-selection — all local or all remote
 * (`groupDeleteBlock` keeps a mixed group, and one holding the current branch,
 * from getting here).
 *
 * The confirmation lists every branch, and each local one with the commits only
 * it holds, counted before the dialog exactly as for one branch; `force` is
 * passed only when that list named some. One backend call deletes them all or —
 * local ones — none (`branches::delete_many` checks every name first), and it is
 * one Undo step, not one per branch.
 */
export async function deleteBranches(nodes: BranchNode[]): Promise<void> {
  if (nodes.length === 0) return;
  const remote = nodes[0].isRemote;
  const names = nodes.map((n) => n.name);
  let counts = names.map(() => 0);
  if (!remote) {
    try {
      counts = await Promise.all(names.map((n) => branchUnmergedCount(n)));
    } catch (e) {
      reportError(e);
      return;
    }
  }
  const lossy = counts.some((n) => n > 0);
  const ok = await confirmAction(
    remote
      ? d().confirmDeleteRemoteBranches(names)
      : d().confirmDeleteBranches(names.map((name, i) => ({ name, unmerged: counts[i] }))),
    remote || lossy,
  );
  if (!ok) return;
  await run(branchDeleteMany(names, remote, lossy), d().phaseDeleteBranches());
  const scoped = selectedBranch();
  if (scoped !== null && names.includes(scoped)) setSelectedBranch(null);
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

/**
 * The one Push of the application — the menu item, the toolbar button and the
 * Changes mode all come here.
 *
 * There is no second "force push" item any more. Two of them offered only
 * `--force-with-lease`, left a bare `--force` unreachable, and asked the reader
 * to guess *before* the push which of the two the situation needed. So the plain
 * push is attempted first and the choice is offered on the refusal — the moment
 * git has already said why, and the only moment at which the answer is knowable.
 *
 * `runResult`, not `run`: the branch is taken on git's own text, and `run` would
 * only put it in the banner. A successful forced push clears that banner itself,
 * because `run` clears the error on success.
 */
export async function pushCurrent(): Promise<void> {
  const first = state()?.upstream ? "normal" : "upstream";
  const err = await runResult(push(first), d().phasePush());
  afterRepoChange();
  if (!err) return;
  // A branch with no upstream has nothing to force *over*: `-u` failed for some
  // other reason, and neither force answers it.
  if (first === "upstream") return;

  const branch = state()?.branch ?? "";
  const remote = currentRemote() ?? "";
  const pick = await chooseOption(d().pushRejected(branch, remote, err), [
    { key: "lease", label: d().pushForceLease() },
    { key: "force", label: d().pushForceHard(), danger: true },
    { key: "cancel", label: d().cancel() },
  ]);
  if (!pick || pick === "cancel") return;
  const hard = pick === "force";
  await run(push(hard ? "force-hard" : "force"), hard ? d().phaseForcePushHard() : d().phaseForcePush());
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
  const opReason = operationReason();
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
    // Not held back by an unfinished operation: the new worktree is another
    // folder, and nothing here is touched. A branch checked out in another
    // worktree is only known to the dialog, which says where.
    items.push({
      label: d().menuOpenInWorktree(),
      disabled: !node.isRemote && node.isCurrent,
      reason: !node.isRemote && node.isCurrent ? d().whyWorktreeHere() : undefined,
      run: () =>
        openWorktreeCreate(
          node.isRemote
            ? { create: true, branch: localNameOf(node.name), start: node.fullRef }
            : { create: false, branch: node.name, start: null },
        ),
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
  // One item: forcing is offered by `pushCurrent` on a refusal, not chosen here
  // in advance (see its docblock).
  const pushReason = busyOp ? opReason : detached ? d().whyDetachedHead() : undefined;
  items.push({
    label: d().menuPush(),
    disabled: !!pushReason,
    reason: pushReason,
    run: () => after(pushCurrent()),
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

/**
 * Items of the tree's context menu for a group: more than one branch marked, and
 * the menu opened on one of them (`branchMarks.menuTargets`). Only what means the
 * same for many branches at once; checkout, merge and the rest stay single.
 *
 * Favourites are the tree's own (`.git/graft-ui.json`), so the tree passes how to
 * read and write them.
 */
export function branchGroupMenuItems(
  nodes: BranchNode[],
  favorites: { has: (n: BranchNode) => boolean; set: (nodes: BranchNode[], on: boolean) => void },
  refreshTree: () => void,
): MenuEntry[] {
  const busyOp = operationActive();
  const block = groupDeleteBlock(nodes);
  const deleteReason = busyOp
    ? operationReason()
    : block?.code === "mixed"
      ? d().whyMixedDelete()
      : block?.code === "current"
        ? d().whyCurrentSelected(block.name)
        : undefined;
  const remote = nodes.every((n) => n.isRemote);
  const items: MenuEntry[] = [
    {
      label: remote ? d().menuDeleteRemoteBranches(nodes.length) : d().menuDeleteBranches(nodes.length),
      danger: true,
      disabled: busyOp || block !== null,
      reason: deleteReason,
      run: () => void deleteBranches(nodes).then(refreshTree),
    },
    { kind: "sep" },
  ];
  const plain = nodes.filter((n) => !favorites.has(n));
  const starred = nodes.filter((n) => favorites.has(n));
  if (plain.length > 0) items.push({ label: d().menuFavoriteAdd(), run: () => favorites.set(plain, true) });
  if (starred.length > 0) {
    items.push({ label: d().menuFavoriteRemove(), run: () => favorites.set(starred, false) });
  }
  items.push({
    label: d().menuCopyBranchNames(nodes.length),
    run: () => void copyOrReport(nodes.map((n) => n.name).join("\n")),
  });
  return items;
}

/** Copy, and say so when the platform refused — a silent failure looks like success. */
export async function copyOrReport(text: string): Promise<void> {
  if (!(await copyText(text))) setError(d().copyFailed());
}
