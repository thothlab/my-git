/**
 * The pure rules behind the interactive rebase dialog and the log menu's squash
 * (`RebasePanel.tsx`, `log/actions/commitActions.ts`).
 *
 * The backend (`engine::rebase`) validates the plan again and decides where git
 * receives each message; this module decides what the dialog shows and what it
 * sends — which rows carry a message field, whether the plan can start, what the
 * history will look like afterwards, and whether a selection of log rows is one
 * unbroken run that can be squashed.
 *
 * **No imports, and none may be added**: `scripts/check-log-filters.mjs`
 * transpiles this file on its own, and an import would fail module resolution
 * there. The shapes below repeat the few fields of `api.ts` they need.
 *
 * Message rules, the same as the backend's:
 *
 *  - a chain is a kept commit plus the `squash` / `fixup` rows right after it
 *    (`drop` rows do not split a chain — git ignores where they stand);
 *  - a `reword` row always has its own field; the text is what the commit — or the
 *    chain it heads — will say;
 *  - a chain with a `squash` whose head is not a reword gets one "combined message"
 *    field, on its last row, prefilled with what git would write by default (the
 *    texts of the head and of the squashed rows; fixups' texts are dropped). Left as
 *    prefilled it is not sent, and git writes its own default;
 *  - a chain of fixups keeps the head's message; to change it, reword the head.
 */

export type Action = "pick" | "reword" | "edit" | "squash" | "fixup" | "drop";

/** In the order the dialog lists them. */
export const ACTIONS: Action[] = ["pick", "reword", "edit", "squash", "fixup", "drop"];

/** Physical key (`KeyboardEvent.code`) → action, while a row has the focus. */
export const ACTION_KEYS: Record<string, Action> = {
  KeyP: "pick",
  KeyR: "reword",
  KeyE: "edit",
  KeyS: "squash",
  KeyF: "fixup",
  KeyD: "drop",
};

export interface PlanEntry {
  hash: string;
  shortHash: string;
  subject: string;
  /** The commit's own whole message. */
  original: string;
  action: Action;
  /** The text typed into this row's message field; `null` — not touched, the
   *  field shows its prefill. */
  text: string | null;
}

/** A commit as the backend lists it (`RebaseCommit`). */
export interface CommitLike {
  hash: string;
  shortHash: string;
  subject: string;
  message: string;
}

export const melds = (a: Action): boolean => a === "squash" || a === "fixup";

/** Every commit picked, in the order given (oldest first). */
export function fromCommits(commits: CommitLike[]): PlanEntry[] {
  return commits.map((c) => ({
    hash: c.hash,
    shortHash: c.shortHash,
    subject: c.subject,
    original: c.message,
    action: "pick",
    text: null,
  }));
}

/** `list` with the element at `from` moved to `to`; out of range — unchanged. */
export function moveEntry<T>(list: T[], from: number, to: number): T[] {
  if (from < 0 || from >= list.length || to < 0 || to >= list.length || from === to) return list;
  const next = list.slice();
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}

/**
 * Chains as lists of entry indices, head first. A melding row with no kept row
 * before it opens no chain of its own — that is the `firstMelds` problem.
 */
export function chainsOf(entries: PlanEntry[]): number[][] {
  const chains: number[][] = [];
  entries.forEach((e, i) => {
    if (e.action === "drop") return;
    if (melds(e.action) && chains.length > 0) chains[chains.length - 1].push(i);
    else chains.push([i]);
  });
  return chains;
}

function chainOf(entries: PlanEntry[], i: number): number[] | null {
  return chainsOf(entries).find((c) => c.includes(i)) ?? null;
}

const hasSquash = (entries: PlanEntry[], chain: number[]) =>
  chain.slice(1).some((i) => entries[i].action === "squash");

/** What git writes for a squash chain by default: the head's and the squashed
 *  rows' texts, blank-line separated; fixups' texts are left out. */
export function combinedDefault(entries: PlanEntry[], chain: number[]): string {
  return chain
    .filter((i, k) => k === 0 || entries[i].action === "squash")
    .map((i) => entries[i].original.trim())
    .filter((t) => t !== "")
    .join("\n\n");
}

export type MessageSlot = "reword" | "combined";

/** Which message field, if any, row `i` shows. */
export function messageSlot(entries: PlanEntry[], i: number): MessageSlot | null {
  const e = entries[i];
  if (!e || e.action === "drop") return null;
  if (e.action === "reword") return "reword";
  const chain = chainOf(entries, i);
  if (!chain || chain.length < 2 || chain[chain.length - 1] !== i) return null;
  if (!hasSquash(entries, chain)) return null;
  return entries[chain[0]].action === "reword" ? null : "combined";
}

/** The text row `i`'s field shows: what was typed, or the prefill. */
export function slotText(entries: PlanEntry[], i: number): string {
  const e = entries[i];
  if (e.text !== null) return e.text;
  if (messageSlot(entries, i) === "combined") {
    return combinedDefault(entries, chainOf(entries, i) ?? [i]);
  }
  return e.original;
}

export type Problem = "noneKept" | "firstMelds" | "emptyMessage";

/** Why the plan cannot start, or null. */
export function planProblem(entries: PlanEntry[]): Problem | null {
  const kept = entries.filter((e) => e.action !== "drop");
  if (kept.length === 0) return "noneKept";
  if (melds(kept[0].action)) return "firstMelds";
  const empty = entries.some(
    (_, i) => messageSlot(entries, i) !== null && slotText(entries, i).trim() === "",
  );
  return empty ? "emptyMessage" : null;
}

export interface Step {
  hash: string;
  action: Action;
  message?: string;
}

/** The plan as the backend takes it. A combined message left as prefilled is not
 *  sent, so git writes its own default. */
export function toSteps(entries: PlanEntry[]): Step[] {
  return entries.map((e, i) => {
    const slot = messageSlot(entries, i);
    const step: Step = { hash: e.hash, action: e.action };
    if (slot === "reword") step.message = slotText(entries, i);
    if (slot === "combined" && e.text !== null) {
      const chain = chainOf(entries, i) ?? [i];
      if (e.text.trim() !== combinedDefault(entries, chain).trim()) step.message = e.text;
    }
    return step;
  });
}

export interface PreviewCommit {
  /** First line of the message the commit will have. */
  subject: string;
  /** Short hashes of the commits melded into it, head first. */
  from: string[];
  /** The rebase stops on it (`edit`). */
  stops: boolean;
  /** Its message differs from the head's own. */
  reworded: boolean;
}

/** The commits the plan produces, oldest first. */
export function preview(entries: PlanEntry[]): PreviewCommit[] {
  return chainsOf(entries).map((chain) => {
    const head = entries[chain[0]];
    const last = chain[chain.length - 1];
    let text = head.original;
    if (head.action === "reword") text = slotText(entries, chain[0]);
    else if (messageSlot(entries, last) === "combined") text = slotText(entries, last);
    const subject = text.trim().split("\n")[0] ?? "";
    return {
      subject,
      from: chain.map((i) => entries[i].shortHash),
      stops: head.action === "edit",
      reworded: text.trim() !== head.original.trim(),
    };
  });
}

export function summary(entries: PlanEntry[]): { kept: number; melded: number; dropped: number } {
  let kept = 0;
  let melded = 0;
  let dropped = 0;
  for (const e of entries) {
    if (e.action === "drop") dropped++;
    else if (melds(e.action)) melded++;
    else kept++;
  }
  return { kept, melded, dropped };
}

// ── squash from the log's selection ─────────────────────────────────────────

/** A log row as far as squashing cares. */
export interface Linked {
  hash: string;
  parents: string[];
}

export type SquashCheck =
  | { ok: true; oldestFirst: string[] }
  | { ok: false; reason: "tooFew" | "merge" | "gap" };

/**
 * Is the selection one unbroken run of commits, each the first parent of the
 * next? Judged by the parent links, not by adjacency on screen: the log with all
 * branches interleaves rows of different lines, and two neighbouring rows may
 * belong to different branches. Whether the run lies on the current branch is the
 * range's question (`runOpensRange`).
 */
export function squashRun(selected: Linked[]): SquashCheck {
  if (selected.length < 2) return { ok: false, reason: "tooFew" };
  if (selected.some((c) => c.parents.length > 1)) return { ok: false, reason: "merge" };
  const byHash = new Map(selected.map((c) => [c.hash, c]));
  const isParent = new Set(selected.map((c) => c.parents[0]).filter((p) => p !== undefined));
  const newest = selected.filter((c) => !isParent.has(c.hash));
  if (newest.length !== 1) return { ok: false, reason: "gap" };
  const chain: string[] = [];
  let at: Linked | undefined = newest[0];
  while (at) {
    chain.push(at.hash);
    const parent: string | undefined = at.parents[0];
    at = parent !== undefined ? byHash.get(parent) : undefined;
  }
  if (chain.length !== selected.length) return { ok: false, reason: "gap" };
  return { ok: true, oldestFirst: chain.reverse() };
}

/** The run is the start of the range read from its oldest commit — that is, it
 *  lies on HEAD's line with nothing else between. */
export function runOpensRange(run: string[], rangeOldestFirst: string[]): boolean {
  return run.length <= rangeOldestFirst.length && run.every((h, i) => rangeOldestFirst[i] === h);
}

/** The prefill of a squash: every message of the run, oldest first. */
export function squashMessage(messagesOldestFirst: string[]): string {
  return messagesOldestFirst
    .map((m) => m.trim())
    .filter((m) => m !== "")
    .join("\n\n");
}
