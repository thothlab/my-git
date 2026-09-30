import { opContinue, type OperationKind } from "../../../api";
import { d } from "../../../i18n";
import { run } from "../../../store";
import { afterRepoChange } from "./repoRefresh";

/**
 * Driving the unfinished operation, shared by the operation strip and the
 * conflict editor — the editor offers "Continue" once the last conflict is
 * resolved, and a second copy of this would be a second place to forget
 * `afterRepoChange()` in.
 */

/** The operation's name as a word inside a sentence ("Continue the merge?"). */
export function operationWord(kind: OperationKind): string {
  return kind === "merge"
    ? d().phaseMerge()
    : kind === "rebase"
      ? d().phaseRebase()
      : kind === "cherryPick"
        ? d().phaseCherryPick()
        : kind === "bisect"
          ? d().phaseBisect()
          : d().phaseRevert();
}

/** `git <op> --continue`, then the refs and the log are re-read. */
export async function continueOperation(): Promise<void> {
  await run(opContinue(), d().phaseOpContinue());
  afterRepoChange();
}
