import { Show, createEffect, createSignal } from "solid-js";
import { commitList, push } from "../api";
import { d } from "../i18n";
import {
  checked,
  error,
  busy,
  run,
  selectedListId,
  setChecked,
  state,
} from "../store";

export default function CommitPanel() {
  const [message, setMessage] = createSignal("");
  const [amend, setAmend] = createSignal(false);

  const list = () => state()?.changelists.find((c) => c.id === selectedListId());

  // number of files a commit would include: the marked subset, else the whole list
  const count = () => (checked().size > 0 ? checked().size : list()?.files.length ?? 0);
  const subset = () => checked().size > 0;

  // seed the message from the list's draft comment when switching lists
  createEffect(() => {
    const l = list();
    if (l && !message().trim() && l.comment) setMessage(l.comment);
  });

  const disabled = () =>
    busy() ||
    count() === 0 ||
    !message().trim() ||
    (!!list()?.isUnversioned && !subset());

  const runCommit = async (): Promise<boolean> => {
    const args = subset()
      ? { paths: [...checked()], message: message(), amend: amend() }
      : { id: selectedListId(), message: message(), amend: amend() };
    await run(commitList(args));
    return !error();
  };
  const clear = () => {
    setMessage("");
    setAmend(false);
    setChecked(new Set<string>());
  };
  const doCommit = async () => {
    if (await runCommit()) clear();
  };
  const doCommitAndPush = async () => {
    if (!(await runCommit())) return;
    clear();
    const s = state();
    await run(push(s?.upstream ? "normal" : "upstream"));
  };

  return (
    <div class="flex flex-col gap-1.5 px-3 py-2">
      <div class="flex items-center gap-2 text-xs text-fg-muted">
        <span>
          {d().commitColon()}&nbsp;
          <span class="font-semibold text-fg">{list()?.name ?? "—"}</span>
          <Show when={subset()}>
            <span>{d().selectedCount(checked().size)}</span>
          </Show>
        </span>
        <span class="ml-auto">{d().filesCount(count())}</span>
      </div>

      <textarea
        class="h-16 w-full resize-none rounded border border-border bg-bg px-2 py-1 text-sm outline-none focus:border-accent"
        placeholder={d().commitMessage()}
        value={message()}
        onInput={(e) => setMessage(e.currentTarget.value)}
      />

      <div class="flex items-center gap-3">
        <label class="flex items-center gap-1.5 text-xs text-fg-muted">
          <input
            type="checkbox"
            class="accent-accent"
            checked={amend()}
            onChange={(e) => setAmend(e.currentTarget.checked)}
          />
          {d().amendLast()}
        </label>

        <div class="ml-auto flex overflow-hidden rounded">
          <button
            class="bg-accent px-3 py-1 text-sm font-medium text-white disabled:opacity-40"
            disabled={disabled()}
            onClick={() => void doCommit()}
            title={list()?.isUnversioned && !subset() ? d().untrackedSelectTip() : ""}
          >
            Commit
          </button>
          <button
            class="border-l border-white/20 bg-accent px-2 py-1 text-sm text-white disabled:opacity-40"
            disabled={disabled()}
            onClick={() => void doCommitAndPush()}
            title={d().commitAndPushTip()}
          >
            + Push
          </button>
        </div>
      </div>
    </div>
  );
}
