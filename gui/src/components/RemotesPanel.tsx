import { For, Show, createEffect, createSignal } from "solid-js";
import {
  errText,
  remoteAdd,
  remoteList,
  remoteRemove,
  remoteRename,
  remoteSetUrl,
  type RemoteInfo,
  type RepoState,
} from "../api";
import { d } from "../i18n";
import { busy, confirmAction, registerModalSource, state } from "../store";
import { afterRepoChange, runResult } from "./log/actions/repoRefresh";
import { httpLogin, isPlainHttp } from "./cloneRules";
import { DISABLED_CLASS } from "./IconButton";

/**
 * The remotes of the open repository: the list with its addresses, and add /
 * rename / change address / remove.
 *
 * Mounted by the window, next to the stash manager: it is opened from the
 * repository menu, which both modes show.
 *
 * Seams worth naming:
 *
 *  - **The form is part of this dialog, not `openDialog`.** The action dialog host
 *    lives in the Log panels only; opened from the Changes mode, a form there
 *    would render nowhere and leave `modalOpen()` true with the keyboard dead.
 *    The form stays open on a refusal with what was typed — the backend's reason
 *    (a token in the address, a name git refuses) is shown under it.
 *  - **`run()` refreshes `RepoState`, which is not this list.** Every change
 *    re-reads it, and `afterRepoChange()` re-reads the branch tree and the log:
 *    a removed or renamed remote takes its remote-tracking branches with it.
 *  - **Addresses come masked** when they carry a password or token; such a row is
 *    flagged, and changing its address starts from an empty field rather than
 *    from `***`, which the backend would refuse anyway.
 *  - **`z-40`, below the store's modals (`z-50`)**: the remove confirmation must
 *    render over it.
 */

type FormKind = "add" | "rename" | "url" | "push";
interface Form {
  kind: FormKind;
  /** The remote acted on; empty for "add". */
  target: string;
}

const [open, setOpen] = createSignal(false);
const [remotes, setRemotes] = createSignal<RemoteInfo[]>([]);
const [listError, setListError] = createSignal("");
const [selectedName, setSelectedName] = createSignal<string | null>(null);

registerModalSource(open);

async function reload(): Promise<void> {
  if (!state()) {
    setRemotes([]);
    return;
  }
  try {
    const list = await remoteList();
    setRemotes(list);
    setListError("");
    if (!list.some((r) => r.name === selectedName())) setSelectedName(list[0]?.name ?? null);
  } catch (e) {
    setListError(errText(e));
  }
}

export function openRemotesPanel(): void {
  if (!state()) return;
  setOpen(true);
  void reload();
}

export default function RemotesPanel() {
  return (
    <Show when={open()}>
      <RemotesView />
    </Show>
  );
}

function RemotesView() {
  let box: HTMLDivElement | undefined;
  let firstInput: HTMLInputElement | undefined;
  const [form, setForm] = createSignal<Form | null>(null);
  const [name, setName] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [formError, setFormError] = createSignal("");
  const [saving, setSaving] = createSignal(false);

  const selected = () => remotes().find((r) => r.name === selectedName()) ?? null;
  const close = () => setOpen(false);
  const refocus = () => queueMicrotask(() => box?.focus());

  createEffect(() => {
    if (open()) refocus();
  });

  const openForm = (kind: FormKind) => {
    const r = selected();
    if (kind !== "add" && !r) return;
    setForm({ kind, target: r?.name ?? "" });
    setFormError("");
    setName(kind === "rename" && r ? r.name : "");
    // A masked address is not offered for editing: `***` is not the real one.
    const current =
      !r || r.hasCredentials
        ? ""
        : kind === "url"
          ? (r.fetchUrls[0] ?? "")
          : kind === "push"
            ? (r.pushUrls[0] ?? "")
            : "";
    setUrl(current);
    queueMicrotask(() => firstInput?.focus());
  };
  const closeForm = () => {
    setForm(null);
    setFormError("");
    refocus();
  };

  /** After any change: this list, then the branch tree and the log. */
  const apply = async (p: Promise<RepoState>, phase: string): Promise<string | null> => {
    const err = await runResult(p, phase);
    await reload();
    afterRepoChange();
    return err;
  };

  const submit = async () => {
    const f = form();
    if (!f || saving()) return;
    const n = name().trim();
    const u = url().trim();
    let p: Promise<RepoState>;
    let phase: string;
    switch (f.kind) {
      case "add":
        if (!n || !u) return;
        p = remoteAdd(n, u);
        phase = d().phaseRemoteAdd();
        break;
      case "rename":
        if (!n) return;
        p = remoteRename(f.target, n);
        phase = d().phaseRemoteRename();
        break;
      case "url":
        if (!u) return;
        p = remoteSetUrl(f.target, u, false);
        phase = d().phaseRemoteSetUrl();
        break;
      case "push":
        p = remoteSetUrl(f.target, u, true);
        phase = d().phaseRemoteSetUrl();
        break;
    }
    setSaving(true);
    const err = await apply(p, phase);
    setSaving(false);
    if (err) {
      setFormError(err); // stays open, keeping what was typed
      return;
    }
    if (f.kind === "add" || f.kind === "rename") setSelectedName(n);
    closeForm();
  };

  const doRemove = async () => {
    const r = selected();
    if (!r || busy()) return;
    if (!(await confirmAction(d().confirmRemoteRemove(r.name, r.branches), true))) {
      refocus();
      return;
    }
    await apply(remoteRemove(r.name), d().phaseRemoteRemove());
    refocus();
  };

  const move = (delta: number) => {
    const list = remotes();
    if (list.length === 0) return;
    const at = list.findIndex((r) => r.name === selectedName());
    const next = Math.min(list.length - 1, Math.max(0, (at < 0 ? 0 : at) + delta));
    setSelectedName(list[next].name);
  };

  const canSubmit = () => {
    const f = form();
    if (!f || saving() || busy()) return false;
    if (f.kind === "add") return name().trim() !== "" && url().trim() !== "";
    if (f.kind === "rename") return name().trim() !== "" && name().trim() !== f.target;
    if (f.kind === "url") return url().trim() !== "";
    return true;
  };

  // The application layer does not know about this dialog; keys that start inside
  // it stop here. Escape is not taken in the capture phase: while the remove
  // confirmation is up, that modal owns it.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (form()) {
      if (e.code === "Escape") {
        e.preventDefault();
        closeForm();
      } else if (e.code === "Enter" || e.code === "NumpadEnter") {
        e.preventDefault();
        void submit();
      }
      return;
    }
    switch (e.code) {
      case "Escape":
        e.preventDefault();
        close();
        break;
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
        e.preventDefault();
        move(-remotes().length);
        break;
      case "End":
        e.preventDefault();
        move(remotes().length);
        break;
      case "Enter":
      case "NumpadEnter":
        e.preventDefault();
        openForm("url");
        break;
      case "Delete":
      case "Backspace":
        e.preventDefault();
        void doRemove();
        break;
    }
  };

  const formTitle = (f: Form) =>
    f.kind === "add"
      ? d().remoteFormAdd()
      : f.kind === "rename"
        ? d().remoteFormRename(f.target)
        : f.kind === "url"
          ? d().remoteFormUrl(f.target)
          : d().remoteFormPushUrl(f.target);

  const btn = `rounded border border-border px-2.5 py-1 text-xs hover:bg-bg-muted ${DISABLED_CLASS}`;

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        role="dialog"
        aria-label={d().remotesTitle()}
        class="flex max-h-[84vh] w-[min(46rem,92vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="text-sm font-semibold">{d().remotesTitle()}</span>
          <span class="text-xs text-fg-muted">({remotes().length})</span>
          <button
            class="ml-auto rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>

        <Show when={listError()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {listError()}
          </pre>
        </Show>

        <div class="min-h-[8rem] flex-1 overflow-auto py-1">
          <Show
            when={remotes().length > 0}
            fallback={<div class="p-4 text-center text-xs text-fg-muted">{d().remotesEmpty()}</div>}
          >
            <For each={remotes()}>
              {(r) => (
                <button
                  class="flex w-full flex-col items-start gap-0.5 px-3 py-1.5 text-left text-xs hover:bg-bg-muted"
                  classList={{ "bg-accent/10": r.name === selectedName() }}
                  onClick={() => setSelectedName(r.name)}
                  onDblClick={() => {
                    setSelectedName(r.name);
                    openForm("url");
                  }}
                >
                  <span class="flex w-full items-center gap-2">
                    <span class="truncate font-medium">{r.name}</span>
                    <span class="shrink-0 text-[0.6875rem] text-fg-muted">
                      {d().remoteBranches(r.branches)}
                    </span>
                    <Show when={r.hasCredentials}>
                      <span
                        class="shrink-0 rounded bg-warn/20 px-1 text-[0.625rem] text-warn"
                        title={d().remoteHasCredentials()}
                      >
                        ⚠
                      </span>
                    </Show>
                  </span>
                  <UrlLine label={d().remoteFetch()} urls={r.fetchUrls} fallback={d().remoteNoUrl()} />
                  <UrlLine label={d().remotePush()} urls={r.pushUrls} fallback={d().remotePushSame()} />
                </button>
              )}
            </For>
          </Show>
        </div>

        <Show when={form()} keyed>
          {(f) => (
            <div class="border-t border-border bg-bg-subtle px-4 py-3">
              <div class="mb-2 text-xs font-semibold">{formTitle(f)}</div>
              <Show when={f.kind === "add" || f.kind === "rename"}>
                <label class="mb-2 block text-xs text-fg-muted">
                  {d().remoteNameLabel()}
                  <input
                    ref={(el) => (firstInput = el)}
                    class="mt-1 w-full rounded border border-border bg-bg-muted px-2 py-1 text-sm text-fg outline-none focus:border-accent"
                    value={name()}
                    placeholder="origin"
                    onInput={(e) => setName(e.currentTarget.value)}
                  />
                </label>
              </Show>
              <Show when={f.kind !== "rename"}>
                <label class="mb-1 block text-xs text-fg-muted">
                  {d().remoteUrlLabel()}
                  <input
                    ref={(el) => {
                      if (f.kind !== "add") firstInput = el;
                    }}
                    class="mt-1 w-full rounded border border-border bg-bg-muted px-2 py-1 font-mono text-xs text-fg outline-none focus:border-accent"
                    value={url()}
                    placeholder={d().remoteUrlPlaceholder()}
                    spellcheck={false}
                    onInput={(e) => setUrl(e.currentTarget.value)}
                  />
                </label>
                <div class="mb-2 text-[0.6875rem] text-fg-subtle">
                  {f.kind === "push" ? d().remotePushUrlNote() : d().remoteUrlNote()}
                </div>
                <Show when={isPlainHttp(url())}>
                  <div class="mb-2 text-[0.6875rem] text-warn">{d().remoteHttpWarn()}</div>
                </Show>
                <Show when={httpLogin(url())}>
                  {(user) => (
                    <div class="mb-2 text-[0.6875rem] text-warn">{d().remoteLoginNote(user())}</div>
                  )}
                </Show>
              </Show>
              <Show when={formError()}>
                <pre class="mb-2 max-h-28 overflow-auto whitespace-pre-wrap rounded border border-danger/40 bg-danger/10 p-2 font-mono text-[0.6875rem] text-danger">
                  {formError()}
                </pre>
              </Show>
              <div class="flex justify-end gap-2">
                <button class={btn} onClick={closeForm}>
                  {d().cancel()}
                </button>
                <button
                  class={`rounded bg-accent px-3 py-1 text-xs text-white ${DISABLED_CLASS}`}
                  disabled={!canSubmit()}
                  onClick={() => void submit()}
                >
                  {d().remoteSave()}
                </button>
              </div>
            </div>
          )}
        </Show>

        <Show when={!form()}>
          {/* Wraps: the Russian labels are much longer, and the window may be 720px. */}
          <div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2">
            <button class={btn} disabled={busy()} onClick={() => openForm("add")}>
              {d().remoteAddBtn()}
            </button>
            <span class="text-[0.6875rem] text-fg-subtle">{d().remotesKeys()}</span>
            <span class="ml-auto" />
            <button
              class={btn}
              disabled={busy() || !selected()}
              title={selected() ? undefined : d().remoteSelectOne()}
              onClick={() => openForm("rename")}
            >
              {d().remoteRenameBtn()}
            </button>
            <button
              class={btn}
              disabled={busy() || !selected()}
              title={selected() ? undefined : d().remoteSelectOne()}
              onClick={() => openForm("url")}
            >
              {d().remoteUrlBtn()}
            </button>
            <button
              class={btn}
              disabled={busy() || !selected()}
              title={selected() ? undefined : d().remoteSelectOne()}
              onClick={() => openForm("push")}
            >
              {d().remotePushUrlBtn()}
            </button>
            <button
              class={`rounded bg-danger px-2.5 py-1 text-xs text-white ${DISABLED_CLASS}`}
              disabled={busy() || !selected()}
              title={selected() ? undefined : d().remoteSelectOne()}
              onClick={() => void doRemove()}
            >
              {d().remoteRemoveBtn()}
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
}

function UrlLine(props: { label: string; urls: string[]; fallback: string }) {
  return (
    <span class="flex w-full items-baseline gap-2 text-[0.6875rem]">
      <span class="w-10 shrink-0 text-fg-subtle">{props.label}</span>
      <Show
        when={props.urls.length > 0}
        fallback={<span class="italic text-fg-muted">{props.fallback}</span>}
      >
        <span class="min-w-0 truncate font-mono text-fg-muted" title={props.urls.join("\n")}>
          {props.urls.join(" · ")}
        </span>
      </Show>
    </span>
  );
}
