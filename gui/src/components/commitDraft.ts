import { createSignal } from "solid-js";
import { state } from "../store";
import type { Person } from "./coAuthorRules";

/**
 * The commit panel's draft: the typed message, "amend" and the chosen co-authors.
 *
 * Module state, not component state: `CommitPanel` lives only in the Changes
 * mode, and `Cmd+2` / `Cmd+1` unmount it — a draft held by the component was
 * gone after a look at the log.
 *
 * Kept **per repository** (keyed by `RepoState.repoPath`), not reset on a switch:
 * a message written for one repository must not be committed into another, and
 * a reset would throw away text the user typed the moment they opened a second
 * repository to look something up. Keyed, it neither leaks nor is lost, and it
 * comes back when they return. It lives as long as the window, like the other
 * module signals; the lasting draft of a changelist is its comment, not this.
 */
export interface CommitDraft {
  message: string;
  amend: boolean;
  coAuthors: Person[];
}

const EMPTY: CommitDraft = { message: "", amend: false, coAuthors: [] };

const [drafts, setDrafts] = createSignal<Record<string, CommitDraft>>({});

/** The repository the panel works on now; the key of its draft. */
export const draftRepo = (): string => state()?.repoPath ?? "";

/** The draft of the open repository. */
export const commitDraft = (): CommitDraft => drafts()[draftRepo()] ?? EMPTY;

/** Change part of a draft — the open repository's unless `repo` names another. */
export function patchCommitDraft(patch: Partial<CommitDraft>, repo = draftRepo()): void {
  setDrafts((all) => ({ ...all, [repo]: { ...(all[repo] ?? EMPTY), ...patch } }));
}

/** Forget a draft after its commit went through. `repo` is the repository the
 * commit was made in, taken before the await: the user may have switched since. */
export function clearCommitDraft(repo: string): void {
  setDrafts((all) => {
    if (!(repo in all)) return all;
    const rest = { ...all };
    delete rest[repo];
    return rest;
  });
}
