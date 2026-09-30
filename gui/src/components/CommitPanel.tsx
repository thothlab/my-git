import { For, Show, createEffect, createResource, createSignal, on } from "solid-js";
import { commitList, errText, logCoAuthors, push, type CoAuthor } from "../api";
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
import { DISABLED_CLASS } from "./IconButton";
import { pickable, trailerLine, trailersToAdd, withCoAuthors, type Person } from "./coAuthorRules";

export default function CommitPanel() {
  const [message, setMessage] = createSignal("");
  const [amend, setAmend] = createSignal(false);
  const [coAuthors, setCoAuthors] = createSignal<Person[]>([]);

  const list = () => state()?.changelists.find((c) => c.id === selectedListId());

  // number of files a commit would include: the marked subset, else the whole list
  const count = () => (checked().size > 0 ? checked().size : list()?.files.length ?? 0);
  const subset = () => checked().size > 0;

  // seed the message from the list's draft comment when switching lists
  createEffect(() => {
    const l = list();
    if (l && !message().trim() && l.comment) setMessage(l.comment);
  });

  // Another repository has other people.
  createEffect(on(() => state()?.repoPath, () => setCoAuthors([]), { defer: true }));

  const disabled = () =>
    busy() ||
    count() === 0 ||
    !message().trim() ||
    (!!list()?.isUnversioned && !subset());

  const runCommit = async (): Promise<boolean> => {
    // The trailers go in here, not into the field: the field stays what the user
    // typed, and removing a chip never has to find its line again.
    const text = withCoAuthors(message(), coAuthors());
    const args = subset()
      ? { paths: [...checked()], message: text, amend: amend() }
      : { id: selectedListId(), message: text, amend: amend() };
    await run(commitList(args));
    return !error();
  };
  const clear = () => {
    setMessage("");
    setAmend(false);
    setCoAuthors([]);
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

      <CoAuthorsField chosen={coAuthors()} onChange={setCoAuthors} message={message()} />

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
            class={`bg-accent px-3 py-1 text-sm font-medium text-white ${DISABLED_CLASS}`}
            disabled={disabled()}
            onClick={() => void doCommit()}
            title={list()?.isUnversioned && !subset() ? d().untrackedSelectTip() : ""}
          >
            Commit
          </button>
          <button
            class={`border-l border-white/20 bg-accent px-2 py-1 text-sm text-white ${DISABLED_CLASS}`}
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

/**
 * Co-authors of the next commit, picked from the people of this history — never
 * typed free-hand, so every trailer names an address the forge has seen. Each one
 * becomes a `Co-authored-by:` line at commit time (`withCoAuthors`), shown below
 * exactly as it will be written.
 *
 * The history is read on the first focus of the field, not when the panel mounts:
 * the Changes mode does not walk the log until someone asks for something from it.
 * The list opens upwards — the panel sits at the bottom of the window.
 */
function CoAuthorsField(props: {
  chosen: Person[];
  onChange: (next: Person[]) => void;
  /** The field's text: who it credits already is left out of the preview. */
  message: string;
}) {
  const [wanted, setWanted] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [open, setOpen] = createSignal(false);
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;

  const [people] = createResource(
    () => (wanted() ? state()?.repoPath : undefined),
    () => logCoAuthors(),
  );
  const loaded = (): CoAuthor[] | undefined => (people.state === "ready" ? people() : undefined);
  const matches = () =>
    open() ? pickable(loaded() ?? [], query(), props.chosen, state()?.userEmail) : [];

  const pick = (p: CoAuthor | undefined) => {
    if (!p) return;
    props.onChange([...props.chosen, { name: p.name, email: p.email }]);
    setQuery("");
    setActive(0);
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const list = matches();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setOpen(true);
      if (list.length) setActive((i) => (i + (e.key === "ArrowDown" ? 1 : -1) + list.length) % list.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      pick(list[Math.min(active(), list.length - 1)]);
    } else if (e.key === "Escape" && open()) {
      e.preventDefault();
      setOpen(false);
    } else if (e.key === "Backspace" && !query() && props.chosen.length) {
      props.onChange(props.chosen.slice(0, -1));
    }
  };

  // What the commit will really add: a person the message credits already is not.
  const preview = () => trailersToAdd(props.message, props.chosen);

  const hint = () => {
    if (!open()) return null;
    if (people.state === "errored") return errText(people.error);
    if (people.loading) return d().coAuthorsLoading();
    const all = loaded();
    if (!all || matches().length) return null;
    if (!query().trim()) {
      // Nobody left to offer at all — only the reader, or everyone already picked.
      return pickable(all, "", [], state()?.userEmail).length === 0 ? d().coAuthorsNone() : null;
    }
    return d().coAuthorsNoMatch();
  };

  return (
    <div class="flex flex-col gap-1">
      <div class="flex flex-wrap items-center gap-1 text-xs text-fg-muted">
        <span>{d().coAuthorsColon()}</span>
        <For each={props.chosen}>
          {(p) => (
            <span
              class="flex items-center gap-1 rounded border border-border bg-bg px-1.5 py-px text-fg"
              title={trailerLine(p)}
            >
              {p.name}
              <button
                type="button"
                class="text-fg-muted hover:text-danger"
                aria-label={d().coAuthorRemove(p.name)}
                title={d().coAuthorRemove(p.name)}
                onClick={() => props.onChange(props.chosen.filter((x) => x !== p))}
              >
                ×
              </button>
            </span>
          )}
        </For>
        <div class="relative min-w-32 flex-1">
          <input
            ref={input}
            class="w-full rounded border border-border bg-bg px-1.5 py-px text-xs text-fg outline-none focus:border-accent"
            placeholder={d().coAuthorAdd()}
            value={query()}
            autocomplete="off"
            spellcheck={false}
            role="combobox"
            aria-expanded={matches().length > 0}
            onFocus={() => {
              setWanted(true);
              setOpen(true);
            }}
            onBlur={() => setOpen(false)}
            onInput={(e) => {
              setQuery(e.currentTarget.value);
              setActive(0);
              setOpen(true);
            }}
            onKeyDown={onKeyDown}
          />
          <Show when={matches().length > 0}>
            <ul
              role="listbox"
              class="absolute bottom-full right-0 z-30 mb-1 max-h-60 w-full min-w-64 overflow-auto rounded border border-border bg-bg py-0.5 shadow-lg"
            >
              <For each={matches()}>
                {(p, i) => (
                  <li
                    role="option"
                    aria-selected={i() === active()}
                    class={`flex cursor-default items-baseline gap-2 px-2 py-0.5 ${
                      i() === active() ? "bg-accent/15" : ""
                    }`}
                    // mousedown, not click: the field's blur would close the list first
                    onMouseDown={(e) => {
                      e.preventDefault();
                      pick(p);
                      input?.focus();
                    }}
                    onMouseEnter={() => setActive(i())}
                  >
                    <span class="truncate text-fg">{p.name}</span>
                    <span class="truncate text-fg-muted">{p.email}</span>
                    <span class="ml-auto shrink-0 text-fg-subtle">{d().coAuthorCommits(p.commits)}</span>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </div>
      </div>
      <Show when={hint()}>
        {(text) => <div class="text-xs text-fg-muted">{text()}</div>}
      </Show>
      <Show when={preview().length > 0}>
        <div class="flex flex-col text-xs text-fg-muted">
          <span>{d().coAuthorsPreview()}</span>
          <For each={preview()}>{(line) => <code class="truncate font-mono">{line}</code>}</For>
        </div>
      </Show>
    </div>
  );
}
