import { For, Show } from "solid-js";
import { opAbort, opContinue, opSkip, type OperationState } from "../../../api";
import { d } from "../../../i18n";
import { busy, confirmAction, run, state } from "../../../store";
import { afterRepoChange } from "./repoRefresh";
import { DISABLED_CLASS } from "../../IconButton";

/**
 * The strip that shows an unfinished merge, rebase, cherry-pick or revert and
 * drives it.
 *
 * It reads `RepoState.operation`, which every mutation brings back with it —
 * there is no separate poll, so the strip appears in the same update that
 * created the state rather than after the next navigation (PRD История 30,
 * `history/spec.md` "State is announced on entry").
 *
 * "Skip" is disabled for a merge before it is pressed: a merge has no step to
 * skip and the engine refuses the call by rule. The spec promises continue /
 * skip / abort, not a refusal after the click.
 *
 * "Abort" is destructive, so it goes through the shared confirmation — the one
 * whose focus sits on Cancel and which Enter therefore does not accept.
 */
export default function OperationBar() {
  const op = (): OperationState | null => {
    const o = state()?.operation;
    return o && o.kind !== "none" ? o : null;
  };

  const title = (o: OperationState) => {
    switch (o.kind) {
      case "merge":
        return d().opMergeTitle();
      case "rebase":
        return o.current !== null && o.total !== null
          ? d().opRebaseTitle(String(o.current), String(o.total))
          : d().opRebaseTitlePlain();
      case "cherryPick":
        return d().opCherryPickTitle();
      case "revert":
        return d().opRevertTitle();
      default:
        return "";
    }
  };

  const kindWord = (o: OperationState) =>
    o.kind === "merge"
      ? d().phaseMerge()
      : o.kind === "rebase"
        ? d().phaseRebase()
        : o.kind === "cherryPick"
          ? d().phaseCherryPick()
          : d().phaseRevert();

  const doContinue = async () => {
    await run(opContinue(), d().phaseOpContinue());
    afterRepoChange();
  };
  const doSkip = async () => {
    await run(opSkip(), d().phaseOpSkip());
    afterRepoChange();
  };
  const doAbort = async (o: OperationState) => {
    if (!(await confirmAction(d().confirmAbortOperation(kindWord(o)), true))) return;
    await run(opAbort(), d().phaseOpAbort());
    afterRepoChange();
  };

  return (
    <Show when={op()}>
      {(o) => (
        <div class="shrink-0 border-b border-warn/50 bg-warn/10 px-2 py-1 text-xs">
          <div class="flex items-center gap-2">
            <span class="font-semibold text-warn">{title(o())}</span>
            <div class="ml-auto flex items-center gap-1">
              <BarBtn label={d().opContinue()} disabled={busy()} onClick={() => void doContinue()} />
              <BarBtn
                label={d().opSkip()}
                disabled={busy() || o().kind === "merge"}
                reason={o().kind === "merge" ? d().whyMergeHasNoSkip() : undefined}
                onClick={() => void doSkip()}
              />
              <BarBtn
                label={d().opAbort()}
                danger
                disabled={busy()}
                onClick={() => void doAbort(o())}
              />
            </div>
          </div>
          <Show
            when={o().conflicted.length > 0}
            fallback={<div class="mt-0.5 text-fg-subtle">{d().opNoConflicts()}</div>}
          >
            <div class="mt-0.5 text-fg-muted">{d().opConflicts(o().conflicted.length)}</div>
            <ul class="mt-0.5 max-h-24 overflow-auto">
              <For each={o().conflicted}>
                {(p) => (
                  <li class="truncate font-mono text-[11px] text-danger" title={p}>
                    {p}
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <div class="mt-0.5 text-[10px] text-fg-subtle">{d().opBlocksActions()}</div>
        </div>
      )}
    </Show>
  );
}

function BarBtn(props: {
  label: string;
  disabled?: boolean;
  danger?: boolean;
  reason?: string;
  onClick: () => void;
}) {
  return (
    <button
      class={`rounded border px-1.5 py-0.5 text-xs ${DISABLED_CLASS}`}
      classList={{
        "border-danger text-danger hover:bg-danger/10": !!props.danger,
        "border-border text-fg hover:bg-bg-muted": !props.danger,
      }}
      disabled={props.disabled}
      title={props.reason ?? props.label}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}
