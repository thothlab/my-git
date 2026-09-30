import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import {
  branchTree,
  errText,
  worktreeAdd,
  worktreeDirty,
  worktreeList,
  worktreeLock,
  worktreePrune,
  worktreeRemove,
  worktreeSuggestPath,
  worktreeUnlock,
  type RepoState,
  type WorktreeInfo,
} from "../api";
import { d } from "../i18n";
import { busy, confirmAction, openRepoAt, promptText, registerModalSource, state } from "../store";
import { afterRepoChange, runResult } from "./log/actions/repoRefresh";
import { DISABLED_CLASS } from "./IconButton";
import {
  checkedOutIn,
  createBlock,
  folderOf,
  freeBranches,
  hasPrunable,
  lockBlock,
  openBlock,
  removeBlock,
  type CreateBlock,
  type OpenBlock,
  type RemoveBlock,
} from "./worktreeRules";

/**
 * The worktrees of the open repository: the list with its marks (open, main,
 * branch or detached, locked, folder missing) and open / create / remove / lock /
 * clean up missing.
 *
 * Mounted by the window, next to the remotes dialog: it is opened from the
 * repository menu, which both modes show, and from the branch tree's context menu
 * ("Open in new worktree…" — `openWorktreeCreate`).
 *
 * Seams worth naming:
 *
 *  - **"Open" switches the window**, through `openRepoAt` — the path "Open…" and
 *    the recent list take, so the worktree lands in `recentRepos` too. Graft has
 *    one window and one open repository; there are no tabs to open it in.
 *  - **The create form is part of this dialog, not `openDialog`**: the action
 *    dialog host lives in the Log panels only, and this dialog is reachable from
 *    the Changes mode as well (same reason as `RemotesPanel`). A refusal stays in
 *    the form with what was typed.
 *  - **Remove asks twice when it would lose work**: `worktreeDirty` is asked after
 *    the first confirmation, and only a second one, naming `--force`, sends it
 *    with force. Without force git refuses a dirty worktree anyway.
 *  - **`run()` refreshes `RepoState`, which is not this list**: every change
 *    re-reads it, and `afterRepoChange()` re-reads the branch tree (a new branch).
 *  - **`z-40`, below the store's modals (`z-50`)**: confirmations render over it.
 */

/** What the branch tree hands the form: an existing local branch, or a new branch
 *  started at a revision (a remote branch — its local name at its full ref). */
export interface WorktreePreset {
  create: boolean;
  branch: string;
  start: string | null;
}

const [open, setOpen] = createSignal(false);
const [worktrees, setWorktrees] = createSignal<WorktreeInfo[]>([]);
const [localBranches, setLocalBranches] = createSignal<string[]>([]);
const [listError, setListError] = createSignal("");
const [selectedPath, setSelectedPath] = createSignal<string | null>(null);
/** The create form's starting values; null — the form is closed. */
const [preset, setPreset] = createSignal<WorktreePreset | null>(null);

registerModalSource(open);

async function reload(): Promise<void> {
  if (!state()) {
    setWorktrees([]);
    return;
  }
  try {
    const [list, tree] = await Promise.all([worktreeList(), branchTree()]);
    setWorktrees(list);
    setLocalBranches(tree.filter((b) => !b.isRemote).map((b) => b.name));
    setListError("");
    if (!list.some((w) => w.path === selectedPath())) {
      setSelectedPath((list.find((w) => w.isCurrent) ?? list[0])?.path ?? null);
    }
  } catch (e) {
    setListError(errText(e));
  }
}

export function openWorktreesPanel(): void {
  if (!state()) return;
  setPreset(null);
  setOpen(true);
  void reload();
}

/** "Open in new worktree…" from the branch tree: the dialog with the form filled. */
export function openWorktreeCreate(p: WorktreePreset): void {
  if (!state()) return;
  setPreset(p);
  setOpen(true);
  void reload();
}

export default function WorktreesPanel() {
  return (
    <Show when={open()}>
      <WorktreesView />
    </Show>
  );
}

function WorktreesView() {
  let box: HTMLDivElement | undefined;
  const selected = () => worktrees().find((w) => w.path === selectedPath()) ?? null;
  const close = () => setOpen(false);
  const refocus = () => queueMicrotask(() => box?.focus());

  createEffect(() => {
    if (open() && !preset()) refocus();
  });

  /** After remove / lock / unlock / prune: this list and the branch tree (none of
   *  them moves a ref, so the log stays). A refusal is shown here too — the banner
   *  is behind this dialog. */
  const apply = async (p: Promise<RepoState>, phase: string): Promise<string | null> => {
    const err = await runResult(p, phase);
    await reload();
    afterRepoChange({ log: false });
    if (err) setListError(err);
    return err;
  };

  const openBlockText = (b: OpenBlock) =>
    b === "current"
      ? d().whyWorktreeOpenCurrent()
      : b === "prunable"
        ? d().whyWorktreeOpenPrunable()
        : d().whyWorktreeOpenBare();
  const removeBlockText = (b: RemoveBlock) =>
    b === "main"
      ? d().whyWorktreeRemoveMain()
      : b === "current"
        ? d().whyWorktreeRemoveCurrent()
        : b === "locked"
          ? d().whyWorktreeRemoveLocked()
          : d().whyWorktreeRemovePrunable();

  const doOpen = async () => {
    const w = selected();
    if (!w || openBlock(w) || busy()) return;
    close();
    await openRepoAt(w.path);
  };

  const doRemove = async () => {
    const w = selected();
    if (!w || removeBlock(w) || busy()) return;
    if (!(await confirmAction(d().confirmWorktreeRemove(w.path, w.branch), true))) {
      refocus();
      return;
    }
    let dirty: boolean;
    try {
      dirty = await worktreeDirty(w.path);
    } catch (e) {
      setListError(errText(e));
      refocus();
      return;
    }
    if (dirty && !(await confirmAction(d().confirmWorktreeRemoveForce(w.path), true))) {
      refocus();
      return;
    }
    await apply(worktreeRemove(w.path, dirty), d().phaseWorktreeRemove());
    refocus();
  };

  const doLock = async () => {
    const w = selected();
    if (!w || lockBlock(w) || busy()) return;
    if (w.locked) {
      await apply(worktreeUnlock(w.path), d().phaseWorktreeUnlock());
    } else {
      const reason = await promptText(d().worktreeLockPrompt());
      if (reason === null) {
        refocus();
        return;
      }
      await apply(worktreeLock(w.path, reason.trim() || null), d().phaseWorktreeLock());
    }
    refocus();
  };

  const doPrune = async () => {
    if (!hasPrunable(worktrees()) || busy()) return;
    await apply(worktreePrune(), d().phaseWorktreePrune());
    refocus();
  };

  const move = (delta: number) => {
    const list = worktrees();
    if (list.length === 0) return;
    const at = list.findIndex((w) => w.path === selectedPath());
    const next = Math.min(list.length - 1, Math.max(0, (at < 0 ? 0 : at) + delta));
    setSelectedPath(list[next].path);
  };

  // Keys that start inside the dialog stop here; the form handles its own. Escape
  // is not taken in the capture phase: a confirmation above owns it then.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (preset()) return;
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
        move(-worktrees().length);
        break;
      case "End":
        e.preventDefault();
        move(worktrees().length);
        break;
      case "Enter":
      case "NumpadEnter":
        e.preventDefault();
        void doOpen();
        break;
      case "Delete":
      case "Backspace":
        e.preventDefault();
        void doRemove();
        break;
    }
  };

  const btn = `rounded border border-border px-2.5 py-1 text-xs hover:bg-bg-muted ${DISABLED_CLASS}`;
  const sel = selected;
  const openWhy = () => {
    const w = sel();
    if (!w) return d().worktreeSelectOne();
    const b = openBlock(w);
    return b ? openBlockText(b) : undefined;
  };
  const removeWhy = () => {
    const w = sel();
    if (!w) return d().worktreeSelectOne();
    const b = removeBlock(w);
    return b ? removeBlockText(b) : undefined;
  };
  const lockWhy = () => {
    const w = sel();
    if (!w) return d().worktreeSelectOne();
    return lockBlock(w) ? d().whyWorktreeLockMain() : undefined;
  };
  const pruneWhy = () => (hasPrunable(worktrees()) ? undefined : d().whyWorktreeNoPrunable());

  return (
    <div class="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
      <div
        ref={box}
        tabindex={-1}
        role="dialog"
        aria-label={d().worktreesTitle()}
        class="flex max-h-[84vh] w-[min(48rem,92vw)] flex-col rounded-lg border border-border bg-bg text-fg shadow-xl outline-none"
        onKeyDown={onKeyDown}
      >
        <div class="flex items-center gap-2 border-b border-border px-4 py-2">
          <span class="text-sm font-semibold">{d().worktreesTitle()}</span>
          <span class="text-xs text-fg-muted">({worktrees().length})</span>
          <button
            class="ml-auto rounded border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
            onClick={close}
          >
            {d().close()}
          </button>
        </div>
        <div class="border-b border-border px-4 py-1.5 text-[0.6875rem] text-fg-subtle">
          {d().worktreesIntro()}
        </div>

        <Show when={listError()}>
          <pre class="max-h-24 overflow-auto whitespace-pre-wrap border-b border-border bg-danger/10 px-3 py-2 font-mono text-[0.6875rem] text-danger">
            {listError()}
          </pre>
        </Show>

        <div class="min-h-[8rem] flex-1 overflow-auto py-1">
          <For each={worktrees()}>
            {(w) => (
              <button
                class="flex w-full flex-col items-start gap-0.5 px-3 py-1.5 text-left text-xs hover:bg-bg-muted"
                classList={{ "bg-accent/10": w.path === selectedPath() }}
                onClick={() => setSelectedPath(w.path)}
                onDblClick={() => {
                  setSelectedPath(w.path);
                  void doOpen();
                }}
              >
                <span class="flex w-full flex-wrap items-center gap-1.5">
                  <span class="truncate font-medium">{folderOf(w.path)}</span>
                  <Show when={w.isCurrent}>
                    <Badge tone="accent">{d().worktreeBadgeCurrent()}</Badge>
                  </Show>
                  <Show when={w.isMain}>
                    <Badge tone="muted">{d().worktreeBadgeMain()}</Badge>
                  </Show>
                  <Show when={w.locked}>
                    <Badge tone="warn" title={d().worktreeLockedBy(w.lockReason)}>
                      {d().worktreeBadgeLocked()}
                    </Badge>
                  </Show>
                  <Show when={w.prunable}>
                    <Badge tone="danger" title={d().worktreeMissing(w.prunableReason)}>
                      {d().worktreeBadgePrunable()}
                    </Badge>
                  </Show>
                  <span class="ml-auto shrink-0 font-mono text-[0.6875rem] text-fg-muted">
                    {w.bare
                      ? d().worktreeBare()
                      : w.branch
                        ? w.branch
                        : w.detached
                          ? d().worktreeDetached(w.head?.slice(0, 7) ?? "")
                          : ""}
                    <Show when={!w.bare && !w.head}> · {d().worktreeUnborn()}</Show>
                    <Show when={w.branch && w.head}> · {w.head!.slice(0, 7)}</Show>
                  </span>
                </span>
                <span class="w-full truncate font-mono text-[0.6875rem] text-fg-subtle" title={w.path}>
                  {w.path}
                </span>
                <Show when={w.locked && w.lockReason}>
                  <span class="text-[0.6875rem] text-warn">{d().worktreeLockedBy(w.lockReason)}</span>
                </Show>
                <Show when={w.prunable}>
                  <span class="text-[0.6875rem] text-danger">{d().worktreeMissing(w.prunableReason)}</span>
                </Show>
              </button>
            )}
          </For>
        </div>

        <Show when={preset()} keyed>
          {(p) => (
            <CreateForm
              preset={p}
              onDone={(path, openAfter) => {
                setPreset(null);
                if (openAfter) {
                  close();
                  void openRepoAt(path);
                } else {
                  refocus();
                }
              }}
              onCancel={() => {
                setPreset(null);
                refocus();
              }}
            />
          )}
        </Show>

        <Show when={!preset()}>
          {/* Wraps: the Russian labels are much longer, and the window may be 720px. */}
          <div class="flex flex-wrap items-center gap-2 border-t border-border px-3 py-2">
            <button
              class={btn}
              disabled={busy()}
              onClick={() => setPreset({ create: true, branch: "", start: null })}
            >
              {d().worktreeCreateBtn()}
            </button>
            <button class={btn} disabled={busy() || !!pruneWhy()} title={pruneWhy()} onClick={() => void doPrune()}>
              {d().worktreePruneBtn()}
            </button>
            <span class="text-[0.6875rem] text-fg-subtle">{d().worktreesKeys()}</span>
            <span class="ml-auto" />
            <button class={btn} disabled={busy() || !!lockWhy()} title={lockWhy()} onClick={() => void doLock()}>
              {sel()?.locked ? d().worktreeUnlockBtn() : d().worktreeLockBtn()}
            </button>
            <button
              class={`rounded bg-danger px-2.5 py-1 text-xs text-white ${DISABLED_CLASS}`}
              disabled={busy() || !!removeWhy()}
              title={removeWhy()}
              onClick={() => void doRemove()}
            >
              {d().worktreeRemoveBtn()}
            </button>
            <button
              class={`rounded bg-accent px-3 py-1 text-xs text-white ${DISABLED_CLASS}`}
              disabled={busy() || !!openWhy()}
              title={openWhy()}
              onClick={() => void doOpen()}
            >
              {d().worktreeOpenBtn()}
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
}

function Badge(props: { tone: "accent" | "muted" | "warn" | "danger"; title?: string; children: string }) {
  const tone = () =>
    props.tone === "accent"
      ? "bg-accent/15 text-accent"
      : props.tone === "warn"
        ? "bg-warn/20 text-warn"
        : props.tone === "danger"
          ? "bg-danger/15 text-danger"
          : "bg-bg-muted text-fg-muted";
  return (
    <span class={`shrink-0 rounded px-1 text-[0.625rem] ${tone()}`} title={props.title}>
      {props.children}
    </span>
  );
}

/** How long typing a branch name waits before the suggested folder is asked for. */
const SUGGEST_DELAY_MS = 250;

function CreateForm(props: {
  preset: WorktreePreset;
  onDone: (path: string, openAfter: boolean) => void;
  onCancel: () => void;
}) {
  let firstInput: HTMLInputElement | HTMLSelectElement | undefined;
  const [create, setCreate] = createSignal(props.preset.create);
  const [name, setName] = createSignal(props.preset.create ? props.preset.branch : "");
  const [existing, setExisting] = createSignal(props.preset.create ? "" : props.preset.branch);
  const [start, setStart] = createSignal(props.preset.start ?? "HEAD");
  const [path, setPath] = createSignal("");
  /** The user edited the folder: suggestions stop overwriting it. */
  const [pathTouched, setPathTouched] = createSignal(false);
  const [openAfter, setOpenAfter] = createSignal(true);
  const [formError, setFormError] = createSignal("");
  const [saving, setSaving] = createSignal(false);

  const branch = () => (create() ? name().trim() : existing());
  const free = () => freeBranches(localBranches(), worktrees());
  /** The preset's branch stays in the choice even when it is taken, so the note
   *  below can say where — a silently different selection would hide why. */
  const choices = () => {
    const f = free();
    const want = existing();
    return want && !f.includes(want) ? [want, ...f] : f;
  };
  const taken = () => (create() ? null : checkedOutIn(existing(), worktrees()));

  createEffect(() => {
    if (!create() && !existing() && free()[0]) setExisting(free()[0]);
  });

  // The suggested folder follows the branch until the user edits the field. A
  // sequence number drops an answer that arrives after a newer question.
  let seq = 0;
  createEffect(() => {
    const b = branch();
    if (pathTouched() || !b) return;
    const mine = ++seq;
    const timer = setTimeout(() => {
      worktreeSuggestPath(b)
        .then((p) => {
          if (mine === seq && !pathTouched()) setPath(p);
        })
        .catch((e) => {
          if (mine === seq) setFormError(errText(e));
        });
    }, SUGGEST_DELAY_MS);
    onCleanup(() => clearTimeout(timer));
  });

  queueMicrotask(() => firstInput?.focus());

  const block = (): CreateBlock | null =>
    createBlock({
      create: create(),
      branch: branch(),
      path: path(),
      worktrees: worktrees(),
      local: localBranches(),
    });
  const blockText = (b: CreateBlock) =>
    b === "no-branch"
      ? d().whyWorktreeNoBranch()
      : b === "exists"
        ? d().whyWorktreeExists()
        : b === "taken"
          ? d().whyWorktreeTaken()
          : b === "no-path"
            ? d().whyWorktreeNoPath()
            : d().whyWorktreeRelativePath();

  const chooseFolder = async () => {
    const dir = await openFolderDialog({ directory: true, title: d().worktreeFolderDialog() });
    if (typeof dir === "string") {
      setPathTouched(true);
      setPath(dir);
    }
  };

  const submit = async () => {
    if (saving() || busy() || block()) return;
    const target = path().trim();
    setSaving(true);
    const err = await runResult(
      worktreeAdd(target, branch(), create(), create() ? start().trim() || null : null),
      d().phaseWorktreeAdd(),
    );
    await reload();
    // A new branch is a new ref: the tree and the log both show it.
    afterRepoChange();
    setSaving(false);
    if (err) {
      setFormError(err); // stays open, keeping what was typed
      return;
    }
    props.onDone(target, openAfter());
  };

  // Stops here: the list's handler above would read Escape as "close the dialog"
  // once the form has unmounted itself, and the keys are not the application's.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      props.onCancel();
    } else if (
      (e.code === "Enter" || e.code === "NumpadEnter") &&
      !(e.target instanceof HTMLSelectElement) &&
      !(e.target instanceof HTMLButtonElement)
    ) {
      e.preventDefault();
      void submit();
    }
  };

  const seg = (on: boolean) =>
    `px-2.5 py-0.5 text-xs ${on ? "bg-accent text-white" : "bg-bg hover:bg-bg-muted"}`;
  const input =
    "mt-1 w-full rounded border border-border bg-bg-muted px-2 py-1 text-sm text-fg outline-none focus:border-accent";

  return (
    <div class="border-t border-border bg-bg-subtle px-4 py-3" onKeyDown={onKeyDown}>
      <div class="mb-2 flex items-center gap-3">
        <span class="text-xs font-semibold">{d().worktreeFormTitle()}</span>
        <div class="flex overflow-hidden rounded border border-border" role="group">
          <button type="button" class={seg(create())} aria-pressed={create()} onClick={() => setCreate(true)}>
            {d().worktreeModeNew()}
          </button>
          <button type="button" class={seg(!create())} aria-pressed={!create()} onClick={() => setCreate(false)}>
            {d().worktreeModeExisting()}
          </button>
        </div>
      </div>

      <Show
        when={create()}
        fallback={
          <label class="mb-2 block text-xs text-fg-muted">
            {d().worktreeExistingLabel()}
            <Show
              when={choices().length > 0}
              fallback={<div class="mt-1 text-xs italic text-fg-muted">{d().worktreeNoFreeBranch()}</div>}
            >
              <select
                ref={(el) => (firstInput = el)}
                class={input}
                value={existing()}
                onChange={(e) => setExisting(e.currentTarget.value)}
              >
                <For each={choices()}>{(b) => <option value={b}>{b}</option>}</For>
              </select>
            </Show>
          </label>
        }
      >
        <div class="mb-2 flex gap-2">
          <label class="block flex-1 text-xs text-fg-muted">
            {d().worktreeBranchLabel()}
            <input
              ref={(el) => (firstInput = el)}
              class={input}
              value={name()}
              placeholder="hotfix/login"
              spellcheck={false}
              onInput={(e) => setName(e.currentTarget.value)}
            />
          </label>
          <label class="block flex-1 text-xs text-fg-muted">
            {d().worktreeStartLabel()}
            <input
              class={`${input} font-mono text-xs`}
              value={start()}
              spellcheck={false}
              onInput={(e) => setStart(e.currentTarget.value)}
            />
          </label>
        </div>
      </Show>

      <Show when={taken()}>
        {(w) => <div class="mb-2 text-[0.6875rem] text-warn">{d().worktreeTakenNote(existing(), w().path)}</div>}
      </Show>

      <label class="mb-1 block text-xs text-fg-muted">
        {d().worktreePathLabel()}
        <div class="flex gap-2">
          <input
            class={`${input} font-mono text-xs`}
            value={path()}
            spellcheck={false}
            onInput={(e) => {
              setPathTouched(true);
              setPath(e.currentTarget.value);
            }}
          />
          <button type="button" class={`mt-1 shrink-0 rounded border border-border px-2 text-xs hover:bg-bg-muted`} onClick={() => void chooseFolder()}>
            {d().worktreeChooseFolder()}
          </button>
        </div>
      </label>
      <div class="mb-2 text-[0.6875rem] text-fg-subtle">{d().worktreePathNote()}</div>

      <label class="mb-2 flex items-center gap-1.5 text-xs">
        <input type="checkbox" checked={openAfter()} onChange={(e) => setOpenAfter(e.currentTarget.checked)} />
        {d().worktreeOpenAfter()}
      </label>

      <Show when={formError()}>
        <pre class="mb-2 max-h-28 overflow-auto whitespace-pre-wrap rounded border border-danger/40 bg-danger/10 p-2 font-mono text-[0.6875rem] text-danger">
          {formError()}
        </pre>
      </Show>
      <div class="flex items-center justify-end gap-2">
        <Show when={block()}>
          {(b) => <span class="mr-auto text-[0.6875rem] text-fg-subtle">{blockText(b())}</span>}
        </Show>
        <button class={`rounded border border-border px-2.5 py-1 text-xs hover:bg-bg-muted`} onClick={props.onCancel}>
          {d().cancel()}
        </button>
        <button
          class={`rounded bg-accent px-3 py-1 text-xs text-white ${DISABLED_CLASS}`}
          disabled={saving() || busy() || !!block()}
          title={block() ? blockText(block()!) : undefined}
          onClick={() => void submit()}
        >
          {d().worktreeCreateSubmit()}
        </button>
      </div>
    </div>
  );
}
