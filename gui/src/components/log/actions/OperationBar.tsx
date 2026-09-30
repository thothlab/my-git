import { For, Show } from "solid-js";
import { opAbort, opBisectReset, opSkip, type BisectMark, type BisectState, type OperationState } from "../../../api";
import { d } from "../../../i18n";
import { busy, confirmAction, run, state } from "../../../store";
import { afterRepoChange } from "./repoRefresh";
import { continueOperation, operationWord } from "./operation";
import { DISABLED_CLASS } from "../../IconButton";
import { openConflict } from "../../conflicts/ConflictPanel";
import { CONFLICT_CODES } from "../../conflicts/conflictRules";
import { bisectPhase, customTerm } from "../bisectMarks";
import { markBisect } from "./commitActions";
import { showCommitInLog } from "./showInLog";

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
 *
 * A rebase stopped on an `edit` step (`OperationState.editStop`) says so and
 * what to do — amend the commit, then Continue — instead of "no conflicts".
 *
 * Each conflicted path carries git's two letters for its kind (`UU`, `DU`, …)
 * and opens the conflict editor (R05e); the editor itself offers "Continue"
 * once the last one is resolved, through the same `continueOperation`.
 *
 * A bisect has a strip of its own ([`BisectStrip`]): it is not continued or
 * skipped but answered, and it can sit *under* another operation — a
 * cherry-pick stopped inside a bisect shows both strips, the inner one to finish
 * first.
 */
export default function OperationBar() {
  const op = (): OperationState | null => {
    const o = state()?.operation;
    return o && o.kind !== "none" && o.kind !== "bisect" ? o : null;
  };
  const bisect = (): BisectState | null => state()?.operation?.bisect ?? null;

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

  const kindWord = (o: OperationState) => operationWord(o.kind);

  const doContinue = () => continueOperation();
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
    <>
    <Show when={bisect()}>{(b) => <BisectStrip b={b()} inner={op() !== null} />}</Show>
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
            fallback={
              <div class="mt-0.5 text-fg-subtle">
                {o().editStop
                  ? d().opEditStop((o().editStop ?? "").slice(0, 7))
                  : d().opNoConflicts()}
              </div>
            }
          >
            <div class="mt-0.5 text-fg-muted">{d().opConflicts(o().conflicted.length)}</div>
            <ul class="mt-0.5 max-h-24 overflow-auto">
              <For each={o().conflicted}>
                {(c) => (
                  <li class="flex items-center gap-2 font-mono text-[0.6875rem]">
                    <span class="shrink-0 font-bold text-danger" title={d().conflictKind(c.kind)}>
                      {CONFLICT_CODES[c.kind]}
                    </span>
                    <span class="min-w-0 flex-1 truncate text-danger" title={c.path}>
                      {c.path}
                    </span>
                    <button
                      class="shrink-0 rounded border border-border px-1.5 font-sans text-fg hover:bg-bg-muted"
                      title={d().conflictResolveTip(c.path)}
                      onClick={() => openConflict(c.path)}
                    >
                      {d().conflictResolveBtn()}
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <div class="mt-0.5 text-[0.625rem] text-fg-subtle">{d().opBlocksActions()}</div>
        </div>
      )}
    </Show>
    </>
  );
}

/**
 * The bisect strip: what the search is waiting for, the commit under test with
 * git's own estimate of the steps left, and the answers — in the UI's words, or
 * in the repository's own terms when it was started with `--term-new/--term-old`.
 * Once git names the first bad commit the answers give way to that commit, which
 * opens in the log; "Finish" (`git bisect reset`) goes back to where it started.
 *
 * `inner` — another operation stopped inside the bisect: it is finished first,
 * and the answers wait with a reason. A state that did not parse (`problem`)
 * offers only "Finish", which needs nothing but git's `BISECT_START`.
 */
function BisectStrip(props: { b: BisectState; inner: boolean }) {
  const short = (h: string | null) => (h ?? "").slice(0, 7);
  const phase = () => bisectPhase(props.b);
  const over = () => phase() === "found" || phase() === "ambiguous";
  const where = () =>
    props.b.startBranch ??
    (props.b.startCommit ? d().bisectReturnCommit(short(props.b.startCommit)) : d().bisectReturnUnknown());

  const label = (mark: BisectMark) => {
    if (mark === "skip") return d().bisectBtnSkip();
    const term = mark === "bad" ? customTerm(props.b.termBad, "bad") : customTerm(props.b.termGood, "good");
    if (term !== null) return d().bisectBtnTerm(term);
    return mark === "bad" ? d().bisectBtnBad() : d().bisectBtnGood();
  };
  const why = () =>
    props.inner
      ? d().whyOperationRunning()
      : props.b.problem
        ? d().whyBisectBroken()
        : props.b.current
          ? undefined
          : d().whyBisectNoCommit();

  const doFinish = async () => {
    if (!over() && !(await confirmAction(d().confirmBisectFinish(where()), false))) return;
    await run(opBisectReset(), d().phaseBisect());
    afterRepoChange();
  };

  const detail = () => {
    const b = props.b;
    switch (phase()) {
      case "broken":
        return d().bisectBroken(b.problem ?? "");
      case "waitBoth":
        return d().bisectWaitBoth();
      case "waitBad":
        return d().bisectWaitBad();
      case "waitGood":
        return d().bisectWaitGood();
      case "ambiguous":
        return d().bisectCandidates(b.candidates.length);
      default:
        return "";
    }
  };

  return (
    <div class="shrink-0 border-b border-accent/50 bg-accent/10 px-2 py-1 text-xs">
      <div class="flex items-center gap-2">
        <span class="min-w-0 truncate" title={props.b.currentSubject ?? undefined}>
          <span class="font-semibold text-accent">{d().bisectTitle()}</span>
          <Show when={phase() === "testing" && props.b.current}>
            <span class="text-fg">
              {": "}
              {d().bisectTesting(short(props.b.current), props.b.currentSubject ?? "", props.b.steps)}
            </span>
          </Show>
        </span>
        <div class="ml-auto flex shrink-0 items-center gap-1">
          <Show when={!over()}>
            <For each={["bad", "good", "skip"] as BisectMark[]}>
              {(m) => (
                <BarBtn
                  label={label(m)}
                  disabled={busy() || !!why()}
                  reason={why() ?? d().bisectMarkTip(label(m), short(props.b.current))}
                  onClick={() => void markBisect(m, null)}
                />
              )}
            </For>
          </Show>
          <BarBtn
            label={d().bisectBtnFinish()}
            disabled={busy() || props.inner}
            reason={props.inner ? d().whyOperationRunning() : d().bisectFinishTip(where())}
            onClick={() => void doFinish()}
          />
        </div>
      </div>
      <Show when={props.b.firstBad}>
        {(h) => (
          <div class="mt-0.5 flex items-center gap-2">
            <span class="min-w-0 truncate font-semibold text-danger" title={props.b.firstBadSubject ?? undefined}>
              {d().bisectFound(customTerm(props.b.termBad, "bad"), short(h()), props.b.firstBadSubject ?? "")}
            </span>
            <button
              class="shrink-0 rounded border border-border px-1.5 text-fg hover:bg-bg-muted"
              onClick={() => void showCommitInLog(h(), short(h()))}
            >
              {d().bisectShowInLog()}
            </button>
          </div>
        )}
      </Show>
      <Show when={detail()}>
        <div class="mt-0.5 text-fg-subtle" classList={{ "text-danger": phase() === "broken" }}>
          {detail()}
        </div>
      </Show>
      <Show when={phase() === "ambiguous"}>
        <div class="mt-0.5 flex flex-wrap gap-1 font-mono text-[0.6875rem]">
          <For each={props.b.candidates}>
            {(h) => (
              <button
                class="rounded border border-warn px-1 text-warn hover:bg-warn/10"
                title={d().bisectShowInLog()}
                onClick={() => void showCommitInLog(h, short(h))}
              >
                {short(h)}
              </button>
            )}
          </For>
        </div>
      </Show>
    </div>
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
