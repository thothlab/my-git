import { focusPanel } from "../../../hotkeys";
import { d } from "../../../i18n";
import { revealCommit } from "../../../logStore";
import { setNotice, setViewMode, state } from "../../../store";
import { clearCompare } from "./compareSelection";

/**
 * Take the reader to one commit in the Log mode — the one way an overlay (the
 * file history, the blame) hands a commit to the log. The overlay closes itself
 * first; this switches the mode, selects the commit through the log's own
 * selection (`revealCommit`) and focuses the commit list, or says out loud that
 * the log does not reach the commit.
 *
 * A comparison on screen would keep the details pane on it: the commit being
 * revealed is a plain selection, like a click on its row.
 */
export async function showCommitInLog(hash: string, shortHash: string): Promise<void> {
  const repo = state()?.repoPath;
  if (!repo) return;
  clearCompare();
  setViewMode("log");
  const found = await revealCommit(hash);
  if (found) focusPanel("commits");
  else setNotice({ repo, text: () => d().commitNotInLog(shortHash) });
}
