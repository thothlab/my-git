import { createEffect, createResource } from "solid-js";
import { undoState, undoStep, type UndoDirection, type UndoSide } from "../api";
import { busy, confirmAction, reportError, setNotice, state } from "../store";
import { registerHotkey } from "../hotkeys";
import { d } from "../i18n";
import { editorOpen } from "./diff/editState";
import { IconButton, IconRedo, IconUndo } from "./IconButton";
import { afterRepoChange, runResult } from "./log/actions/repoRefresh";

/**
 * Undo / Redo of Graft's own git actions (`src-tauri/src/engine/undo.rs`): two
 * toolbar buttons and Cmd/Ctrl+Z, Cmd/Ctrl+Shift+Z.
 *
 * - **Availability is re-read on every fresh `RepoState`** — after an action, a
 *   focus refresh or the git-dir watcher's. That read is also where the backend
 *   notices a change made outside Graft and ends the chain, so the button's
 *   tooltip says why instead of the next click failing.
 * - **A click reads it once more** and runs the step id of *that* read: the one on
 *   screen may be a moment old, and the id is what keeps a confirmation shown for
 *   one step from running another.
 * - **The shortcuts stand down in text fields** (`typing: false` — a commit
 *   message's Cmd+Z is the field's own) **and while the file editor is open**:
 *   unregistered, not refused inside the handler, because `hotkeys.ts` swallows
 *   the key on a match before the handler could decline it. The conflict editor
 *   is a modal and answers its own Cmd+Z; `hotkeys.ts` stands down under it.
 */
export default function UndoButtons() {
  const [info, { refetch }] = createResource(
    () => state(),
    () => undoState().catch(() => null),
  );

  // `latest`: the previous answer stays on screen while the next one is read.
  const side = (dir: UndoDirection): UndoSide | null => info.latest?.[dir] ?? null;
  const what = (s: UndoSide) => d().undoWhat(s.action ?? "", s.detail);
  const why = (s: UndoSide | null) =>
    s?.reason ? d().undoReason(s.reason.code, s.reason.action ?? null) : d().undoReason("empty", null);
  const tip = (dir: UndoDirection) => {
    const s = side(dir);
    if (s?.id != null) return dir === "undo" ? d().undoTip(what(s)) : d().redoTip(what(s));
    return dir === "undo" ? d().undoUnavailable(why(s)) : d().redoUnavailable(why(s));
  };

  const go = async (dir: UndoDirection) => {
    if (busy()) return;
    const repo = state()?.repoPath ?? "";
    let now;
    try {
      now = await undoState();
    } catch (e) {
      reportError(e);
      return;
    }
    const s = now[dir];
    if (s.id == null) {
      void refetch();
      setNotice({
        repo,
        text: () => (dir === "undo" ? d().undoUnavailable(why(s)) : d().redoUnavailable(why(s))),
      });
      return;
    }
    if (s.destructive && !(await confirmAction(d().undoConfirmHard(dir === "redo", what(s), s.lostCommits)))) {
      return;
    }
    const failed = await runResult(undoStep(dir, s.id), dir === "undo" ? d().busyUndo() : d().busyRedo());
    // Whatever a notice offered ("Rolled back N · Restore") was about the state
    // before this step.
    if (!failed) setNotice(null);
    afterRepoChange();
  };

  createEffect(() => {
    if (editorOpen()) return;
    registerHotkey("KeyZ", () => void go("undo"), { typing: false });
    registerHotkey("KeyZ", () => void go("redo"), { shift: true, typing: false });
  });

  return (
    <div class="flex items-center">
      <IconButton
        tip={tip("undo")}
        disabled={busy() || side("undo")?.id == null}
        onClick={() => void go("undo")}
      >
        <IconUndo />
      </IconButton>
      <IconButton
        tip={tip("redo")}
        disabled={busy() || side("redo")?.id == null}
        onClick={() => void go("redo")}
      >
        <IconRedo />
      </IconButton>
    </div>
  );
}
