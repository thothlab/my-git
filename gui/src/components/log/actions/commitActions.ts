import {
  commitCheckout,
  commitCherryPick,
  commitDetails,
  commitReword,
  commitsSquash,
  commitReset,
  commitResetLostCount,
  commitRevert,
  opBisectMark,
  opBisectStart,
  tagCreate,
  WORKING_TREE,
  type BisectMark,
  type LogCommit,
  type RebaseRange,
  type RepoState,
  type ResetMode,
} from "../../../api";
import { d } from "../../../i18n";
import { chooseOption, confirmAction, reportError, run, state } from "../../../store";
import { copyOrReport, newBranchFrom } from "./branchActions";
import type { MenuEntry } from "./ContextMenu";
import { setCompareTarget } from "./compareSelection";
import { openDialog } from "./dialogs";
import { openRebasePlan } from "../../rebase/RebasePanel";
import { runOpensRange, squashMessage, squashRun } from "../../rebase/rebaseRules";
import { afterRepoChange, localChangesNow, operationActive, operationReason, runResult } from "./repoRefresh";
import { customTerm } from "../bisectMarks";

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
      reportError(e); // git's own output, whole, with its journal link
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

// ── Bisect: the search for the commit that brought a bug in ──────────────────

/**
 * Start a bisect from the log. One commit: it has the bug, and the search then
 * waits for a commit without it (the strip says so, the menu offers the mark).
 * Two commits: the newer one in the log's order has the bug, the older one does
 * not — git checks out the first commit to test straight away. A pair the other
 * way round is git's to refuse, word for word.
 */
export async function startBisect(targets: LogCommit[]): Promise<void> {
  if (targets.length !== 1 && targets.length !== 2) return;
  const [bad, good] = targets; // display order is newest first
  const ok = await confirmAction(d().confirmBisectStart(bad.shortHash, good ? good.shortHash : null), false);
  if (!ok) return;
  await run(opBisectStart(bad.hash, good ? [good.hash] : []), d().phaseBisect());
  afterRepoChange();
}

/** Answer for one commit; `null` — the commit under test. The checkout that
 *  follows moves HEAD, so the log and the tree are re-read. */
export async function markBisect(mark: BisectMark, hash: string | null): Promise<void> {
  await run(opBisectMark(mark, hash), d().phaseBisect());
  afterRepoChange();
}

/** The menu's bisect items: start one, or — while one runs — answer for a commit. */
function bisectItems(targets: LogCommit[], one: LogCommit | null): MenuEntry[] {
  const op = state()?.operation;
  const b = op?.bisect ?? null;
  if (!b) {
    const reason = operationReason();
    return [
      {
        label: d().menuBisectStartBad(),
        disabled: !!reason || !one,
        reason: reason ?? (one ? undefined : d().whyOneCommitOnly()),
        run: () => void startBisect(targets),
      },
      {
        label: d().menuBisectBetween(),
        disabled: !!reason || targets.length !== 2,
        reason: reason ?? (targets.length === 2 ? undefined : d().whyBisectNeedsTwo()),
        run: () => void startBisect(targets),
      },
    ];
  }
  const why =
    op?.kind !== "bisect"
      ? d().whyOperationRunning()
      : b.problem
        ? d().whyBisectBroken()
        : one
          ? undefined
          : d().whyOneCommitOnly();
  const item = (mark: BisectMark): MenuEntry => ({
    label: d().menuBisectMark(
      mark,
      mark === "bad" ? customTerm(b.termBad, "bad") : mark === "good" ? customTerm(b.termGood, "good") : null,
    ),
    disabled: !!why,
    reason: why,
    run: () => {
      if (one) void markBisect(mark, one.hash);
    },
  });
  return [item("bad"), item("good"), item("skip")];
}

// ── Rewriting history: reword, squash, interactive rebase ────────────────────

/**
 * What the menu knows about rewriting from the commit it was opened on: the
 * answer of `op_rebase_range`, asked when the menu opens (like `commit_contains`),
 * for the target or — for a squash — for the oldest commit of the selected run.
 * `null` when the question does not apply to this menu.
 */
export type RangeAnswer =
  | { status: "checking" }
  | { status: "failed" }
  | { status: "ok"; range: RebaseRange };

/**
 * Rewritten commits the upstream already has: say so and ask. Rewriting them is
 * legal, but the result only goes back with a force push.
 */
async function confirmPublished(range: RebaseRange): Promise<boolean> {
  if (range.published === 0) return true;
  return confirmAction(d().confirmRewritePublished(range.published), true);
}

/** After a rewrite: close on success or on a stop (conflict, `edit` — the strip
 *  drives it now); keep the dialog open only for a refusal that changed nothing. */
async function rewriteResult(p: Promise<RepoState>, label: string) {
  const err = await runResult(p, label);
  if (err && !operationActive()) return err;
  afterRepoChange();
  return null;
}

/**
 * New message for one commit. HEAD is amended with `--only` — what is staged
 * stays out of it; an older commit is rewritten by a rebase with one `reword`.
 * The dialog is prefilled with the whole message, read from the range or, for a
 * HEAD the range does not list (a merge), from the commit itself.
 */
export async function rewordCommit(commit: LogCommit, range: RebaseRange): Promise<void> {
  const isHead = range.head === commit.hash;
  let message = range.commits[0]?.hash === commit.hash ? range.commits[0].message : null;
  if (message === null) {
    try {
      const c = await commitDetails(commit.hash);
      message = c.body.trim() === "" ? c.subject : `${c.subject}\n\n${c.body.trim()}`;
    } catch (e) {
      reportError(e);
      return;
    }
  }
  if (!(await confirmPublished(range))) return;
  await openDialog({
    title: d().dlgRewordTitle(commit.shortHash),
    note: isHead ? d().dlgRewordNoteHead() : d().dlgRewordNoteDeep(range.commits.length - 1),
    fields: [{ key: "message", label: d().dlgMessage(), value: message, multiline: true }],
    submitLabel: d().dlgRewordSubmit(),
    submit: (v) => rewriteResult(commitReword(commit.hash, v.message), d().phaseReword()),
  });
}

/** Meld the selected run into one commit, prefilled with every message of it. */
export async function squashCommits(targets: LogCommit[], range: RebaseRange): Promise<void> {
  const run = squashRun(targets);
  if (!run.ok) return;
  const n = run.oldestFirst.length;
  const messages = range.commits.slice(0, n).map((c) => c.message);
  if (!(await confirmPublished(range))) return;
  await openDialog({
    title: d().dlgSquashTitle(n),
    note: d().dlgSquashNote(range.commits.length - n),
    fields: [
      { key: "message", label: d().dlgSquashMessage(), value: squashMessage(messages), multiline: true },
    ],
    submitLabel: d().dlgSquashSubmit(),
    submit: (v) => rewriteResult(commitsSquash(run.oldestFirst, v.message), d().phaseSquash()),
  });
}

/** The plan dialog for everything from this commit up to HEAD. */
export async function rebaseFromCommit(commit: LogCommit, range: RebaseRange): Promise<void> {
  if (!(await confirmPublished(range))) return;
  openRebasePlan({ hash: commit.hash, shortHash: commit.shortHash, range });
}

/** Why a rewrite over `answer` cannot run, or undefined. `head` — rewording HEAD
 *  by amend, which neither merges nor a dirty tree stop. */
function rangeReason(answer: RangeAnswer | null, head?: LogCommit): string | undefined {
  if (!answer || answer.status === "checking") return d().whyChecking();
  if (answer.status === "failed") return d().whyRebaseUnknown();
  const r = answer.range;
  if (head && r.head === head.hash) return undefined;
  if (r.blocked) return d().whyRebaseBlocked(r.blocked);
  if (r.dirty) return d().whyDirtyTree();
  return undefined;
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
  rewrite: RangeAnswer | null = null,
): MenuEntry[] {
  const n = targets.length;
  const one = n === 1 ? targets[0] : null;
  const busyOp = operationActive();
  const opReason = operationReason();
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

  // Rewriting history. Squash is judged by first-parent links (the all-branches
  // log interleaves lines), and then against the range read from the run's
  // oldest commit: the run has to open it, i.e. lie on HEAD's line.
  items.push({ kind: "sep" });
  const range = rewrite?.status === "ok" ? rewrite.range : null;
  const rewordReason = singleReason ?? (one ? rangeReason(rewrite, one) : undefined);
  items.push({
    label: d().menuReword(),
    disabled: !!rewordReason,
    reason: rewordReason,
    run: () => {
      if (one && range) void rewordCommit(one, range);
    },
  });
  const squash = squashRun(targets);
  let squashReason: string | undefined = opReason;
  if (!squashReason && n < 2) squashReason = d().whyNeedTwoToSquash();
  if (!squashReason && !squash.ok) squashReason = d().whySquashRun(squash.reason);
  if (!squashReason) {
    const r = rangeReason(rewrite);
    if (r) squashReason = r;
    else if (range && squash.ok && !runOpensRange(squash.oldestFirst, range.commits.map((c) => c.hash)))
      squashReason = d().whySquashOffBranch();
  }
  items.push({
    label: d().menuSquash(n),
    disabled: !!squashReason,
    reason: squashReason,
    run: () => {
      if (range) void squashCommits(targets, range);
    },
  });
  const rebaseReason = singleReason ?? rangeReason(rewrite);
  items.push({
    label: d().menuRebaseFrom(),
    disabled: !!rebaseReason,
    reason: rebaseReason,
    run: () => {
      if (one && range) void rebaseFromCommit(one, range);
    },
  });

  items.push({ kind: "sep" });
  items.push(...bisectItems(targets, one));

  return items;
}

/**
 * The commit whose range the menu should read for `targets`: the target itself,
 * or the oldest commit of a squashable run; null when nothing needs it.
 */
export function rewriteAnchor(targets: LogCommit[]): string | null {
  if (targets.length === 1) return targets[0].hash;
  const run = squashRun(targets);
  return run.ok ? run.oldestFirst[0] : null;
}

/** "New branch from this commit" — the branch dialog, anchored on a hash. */
const newBranchFromCommit = (commit: LogCommit) =>
  newBranchFrom(commit.hash, commit.shortHash);
