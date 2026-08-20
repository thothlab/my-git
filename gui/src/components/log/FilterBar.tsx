import { open } from "@tauri-apps/plugin-dialog";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  type JSX,
} from "solid-js";
import { branchList, logAuthors, type LogFilter } from "../../api";
import { d, locale } from "../../i18n";
import { registerHotkey } from "../../hotkeys";
import {
  applyFilter,
  dim,
  filter,
  jumpToMatch,
  note,
  setDim,
  search,
  setBranchScope,
  setSearch,
} from "../../logStore";
import { state } from "../../store";
import { setSelectedBranch } from "./branchSelection";
import { DAY, asInputDate, dayEnd, dayStart, relativeToRepo, startOfToday } from "./filterValues";
import { searchPattern } from "./searchMatch";

/**
 * The row above the log: one search field and four filters.
 *
 * The distinction the whole file is built around — **search and filters are
 * different mechanisms**:
 *
 *  - the search field highlights and moves between matches (`setSearch`,
 *    `jumpToMatch`) and never narrows the set of rows;
 *  - the four filters narrow it (`applyFilter`, `setBranchScope`), each one
 *    removable on its own;
 *  - dimming non-matching rows is a third thing again, and belongs to the row
 *    renderer — see `searchMatch.ts`.
 *
 * Two consequences that look like omissions and are not:
 *
 *  1. **`.*` and `Cc` never touch `LogFilter.regex` / `LogFilter.matchCase`.**
 *     Those flags also decide how git matches `--author` (`engine/log.rs`), so
 *     driving them from the search field would change which rows exist from a
 *     control that claims to change only highlighting. The author filter owns
 *     them, and this bar sets them nowhere else.
 *  2. **No guard against stale answers here.** The store carries a monotonic
 *     `seq` and drops an answer older than the last request; a second guard in
 *     the bar would only be a second thing to get wrong. Typing is debounced
 *     so a fast typist does not issue the requests in the first place.
 */

/** Idle time before a keystroke becomes a search (R20i.2). */
const TYPE_DELAY_MS = 250;

export default function FilterBar() {
  const [text, setText] = createSignal(search().text);
  const [openMenu, setOpenMenu] = createSignal<"branch" | "author" | "date" | "paths" | null>(null);
  /** A jump is running. One owner, and it is the store: `jumpToMatch` raises
   * this note while it loads pages towards a match and the list already renders
   * it, so a second flag here would let the buttons and the text disagree about
   * whether anything is happening. */
  const searching = () => note() === "searching";
  const [pathNote, setPathNote] = createSignal("");
  let input: HTMLInputElement | undefined;
  let timer: number | undefined;

  const repoPath = () => state()?.repoPath ?? "";

  // ── Search ────────────────────────────────────────────────────────────────

  const commit = (t: string) => setSearch({ ...search(), text: t });

  const flush = () => {
    if (timer === undefined) return;
    clearTimeout(timer);
    timer = undefined;
    commit(text());
  };
  onCleanup(() => timer !== undefined && clearTimeout(timer));

  const onType = (t: string) => {
    setText(t);
    if (timer !== undefined) clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = undefined;
      commit(t);
    }, TYPE_DELAY_MS);
  };

  // A toggle is a decision, not typing: it applies at once, and it applies to
  // the text already on screen rather than to the one the timer is holding.
  const toggle = (key: "regex" | "matchCase") => {
    flush();
    setSearch({ ...search(), text: text(), [key]: !search()[key] });
  };

  const patternError = () => {
    const p = searchPattern();
    return p.kind === "error" ? p.message : "";
  };

  /** A jump is a loop over `loadMore`, so it is gated: a held Cmd+G would
   * otherwise stack concurrent searches over a moving list. The gate is the
   * store's own note, which is raised before every page it waits for. */
  const jump = async (dir: 1 | -1) => {
    if (searching()) return;
    flush();
    if (!search().text || patternError()) return;
    await jumpToMatch(dir);
  };

  // Registered in the component body: `<Show when={isLog()}>` unmounts the panel
  // on every mode switch, and `registerHotkey` throws on a duplicate — the
  // `onCleanup` inside it is what makes a remount safe.
  registerHotkey("KeyF", () => {
    input?.focus();
    input?.select();
  });
  registerHotkey("KeyG", () => void jump(1));
  registerHotkey("KeyG", () => void jump(-1), { shift: true });

  // ── Filters ───────────────────────────────────────────────────────────────

  const set = (patch: Partial<LogFilter>) => applyFilter({ ...filter(), ...patch });

  /** The branch scope is shared with the tree, so both are written: the store
   * ignores a scope equal to the current one, which keeps the tree's own effect
   * from reloading the log a second time. */
  const setBranch = (b: string | null) => {
    setBranchScope(b);
    setSelectedBranch(b);
  };

  const dateLabel = () => {
    const f = filter();
    const fmt = (t: number) => new Date(t * 1000).toLocaleDateString(locale());
    if (f.since && f.until) return d().fltDateRange(fmt(f.since), fmt(f.until));
    if (f.since) return d().fltDateSince(fmt(f.since));
    if (f.until) return d().fltDateUntil(fmt(f.until));
    return "";
  };

  const chips = createMemo(() => {
    const f = filter();
    const out: { key: string; label: string; clear: () => void }[] = [];
    if (f.branch) out.push({ key: "branch", label: f.branch, clear: () => setBranch(null) });
    if (f.author) out.push({ key: "author", label: f.author, clear: () => set({ author: null }) });
    if (f.since || f.until)
      out.push({ key: "date", label: dateLabel(), clear: () => set({ since: null, until: null }) });
    for (const p of f.paths)
      out.push({
        key: `path:${p}`,
        label: p,
        clear: () => set({ paths: filter().paths.filter((x) => x !== p) }),
      });
    return out;
  });

  const clearAll = () => {
    setSelectedBranch(null);
    // One reload, not four: the whole filter is replaced in a single call.
    applyFilter({ ...filter(), branch: null, author: null, since: null, until: null, paths: [] });
  };

  return (
    <div class="sticky top-0 z-10 shrink-0 border-b border-border bg-bg-subtle">
      <div class="flex flex-wrap items-center gap-1 px-2 py-1 text-xs">
        <div
          class="flex items-center gap-0.5 rounded border bg-bg px-1"
          classList={{ "border-danger": !!patternError(), "border-border": !patternError() }}
        >
          <span class="text-fg-muted">⌕</span>
          <input
            ref={input}
            class="w-48 bg-transparent py-0.5 text-xs outline-none placeholder:text-fg-subtle"
            placeholder={d().fltSearchPlaceholder()}
            title={d().fltSearchTip()}
            value={text()}
            onInput={(e) => onType(e.currentTarget.value)}
            onKeyDown={(e) => {
              // Local to the field: the keyboard layer never takes bare keys
              // from a text input, so this is the only place Escape can act.
              if (e.key === "Escape") {
                onType("");
                flush();
              }
              if (e.key === "Enter") void jump(e.shiftKey ? -1 : 1);
            }}
          />
          <ToggleBtn label=".*" tip={d().fltRegexTip()} on={search().regex} onClick={() => toggle("regex")} />
          <ToggleBtn
            label="Cc"
            tip={d().fltCaseTip()}
            on={search().matchCase}
            onClick={() => toggle("matchCase")}
          />
        </div>

        <BarBtn
          label="‹"
          tip={d().fltPrevMatch()}
          disabled={!text() || !!patternError() || searching()}
          onClick={() => void jump(-1)}
        />
        <BarBtn
          label="›"
          tip={d().fltNextMatch()}
          disabled={!text() || !!patternError() || searching()}
          onClick={() => void jump(1)}
        />
        {/* Same flag as the buttons above, so "the search is running" cannot be
            on in one place and off in the other. The wording stays with the
            list, which already renders the store's note — this is the mark, not
            a second copy of the sentence. */}
        <Show when={searching()}>
          <span class="animate-pulse text-fg-muted" title={d().searchingNote()} aria-busy="true">
            ⋯
          </span>
        </Show>

        {/* The third mode of the search: non-matching rows read muted. Still not
            a filter — nothing leaves the list — which is why it sits here beside
            the pattern toggles and not among the four narrowing menus. */}
        <ToggleBtn
          label={d().fltDimLabel()}
          tip={d().fltDimTip()}
          on={dim()}
          onClick={() => setDim(!dim())}
        />

        <span class="mx-1 h-4 w-px bg-border" />

        <Menu
          id="branch"
          label={d().fltBranch()}
          value={filter().branch}
          open={openMenu()}
          setOpen={setOpenMenu}
        >
          <BranchMenu
            current={filter().branch ?? null}
            onPick={(b) => {
              setBranch(b);
              setOpenMenu(null);
            }}
          />
        </Menu>
        <Menu
          id="author"
          label={d().fltAuthor()}
          value={filter().author}
          open={openMenu()}
          setOpen={setOpenMenu}
        >
          <AuthorMenu
            repo={repoPath()}
            current={filter().author ?? null}
            onPick={(a) => {
              set({ author: a });
              setOpenMenu(null);
            }}
          />
        </Menu>
        <Menu
          id="date"
          label={d().fltDate()}
          value={dateLabel() || null}
          tip={d().fltDateTip()}
          open={openMenu()}
          setOpen={setOpenMenu}
        >
          <DateMenu
            since={filter().since ?? null}
            until={filter().until ?? null}
            onPick={(since, until) => {
              set({ since, until });
              setOpenMenu(null);
            }}
          />
        </Menu>
        <Menu
          id="paths"
          label={d().fltPaths()}
          value={filter().paths.length ? d().fltPathsChip(filter().paths.length) : null}
          open={openMenu()}
          setOpen={setOpenMenu}
        >
          <PathsMenu
            repo={repoPath()}
            paths={filter().paths}
            note={pathNote()}
            setNote={setPathNote}
            onChange={(paths) => set({ paths })}
          />
        </Menu>

        <Show when={chips().length > 0}>
          <button
            class="ml-auto rounded border border-border px-1.5 py-0.5 text-fg-muted hover:bg-bg"
            onClick={clearAll}
          >
            {d().fltClearAll()}
          </button>
        </Show>
      </div>

      <Show when={patternError()}>
        <div class="px-2 pb-1 text-[11px] text-danger">{d().fltRegexInvalid(patternError())}</div>
      </Show>

      <Show when={chips().length > 0}>
        <div class="flex flex-wrap items-center gap-1 px-2 pb-1">
          <For each={chips()}>
            {(c) => (
              <span class="flex items-center gap-1 rounded border border-accent bg-accent/10 px-1.5 py-0.5 text-[11px] text-accent">
                <span class="max-w-48 truncate">{c.label}</span>
                <button class="text-fg-muted hover:text-danger" title={d().fltRemove()} onClick={c.clear}>
                  ×
                </button>
              </span>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

// ── Small parts ──────────────────────────────────────────────────────────────

function ToggleBtn(props: { label: string; tip: string; on: boolean; onClick: () => void }) {
  return (
    <button
      class="rounded px-1 py-0.5 text-[11px] leading-none"
      classList={{
        "bg-accent/20 text-accent": props.on,
        "text-fg-muted hover:bg-bg-muted": !props.on,
      }}
      title={props.tip}
      aria-pressed={props.on}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

function BarBtn(props: { label: string; tip: string; disabled?: boolean; onClick: () => void }) {
  return (
    <button
      class="rounded border border-transparent px-1.5 py-0.5 text-fg-muted hover:border-border hover:bg-bg disabled:cursor-not-allowed disabled:opacity-40"
      title={props.tip}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

/** One filter: a button showing its value, and a popover with the choices. */
function Menu(props: {
  id: "branch" | "author" | "date" | "paths";
  label: string;
  value: string | null | undefined;
  tip?: string;
  open: string | null;
  setOpen: (v: "branch" | "author" | "date" | "paths" | null) => void;
  children: JSX.Element;
}) {
  const isOpen = () => props.open === props.id;
  return (
    <div class="relative">
      <button
        class="flex max-w-56 items-center gap-1 rounded border px-1.5 py-0.5 hover:bg-bg"
        classList={{
          "border-accent text-accent": !!props.value,
          "border-border text-fg-muted": !props.value,
        }}
        title={props.tip ?? props.label}
        onClick={() => props.setOpen(isOpen() ? null : props.id)}
      >
        <span class="shrink-0">{props.label}:</span>
        <span class="truncate">{props.value || "—"}</span>
        <span class="shrink-0 text-fg-subtle">▾</span>
      </button>
      <Show when={isOpen()}>
        <div
          class="absolute left-0 top-6 z-30 w-72 rounded border border-border bg-bg p-1 shadow-lg"
          onMouseLeave={() => props.setOpen(null)}
        >
          {props.children}
        </div>
      </Show>
    </div>
  );
}

function Item(props: { label: string; on?: boolean; onClick: () => void }) {
  return (
    <button
      class="flex w-full items-center gap-2 rounded px-2 py-1 text-left text-xs hover:bg-bg-muted"
      classList={{ "text-accent": props.on, "text-fg": !props.on }}
      onClick={props.onClick}
    >
      <span class="w-3 shrink-0">{props.on ? "✓" : ""}</span>
      <span class="truncate">{props.label}</span>
    </button>
  );
}

// ── Branch ───────────────────────────────────────────────────────────────────

/**
 * `null` is not "no branch" but "whatever HEAD points at" — the same meaning the
 * branch tree's top row carries, so the two controls agree on what an empty
 * scope is instead of one of them showing "all branches" that git never had.
 */
function BranchMenu(props: { current: string | null; onPick: (b: string | null) => void }) {
  const [q, setQ] = createSignal("");
  const [branches] = createResource(() => state()?.repoPath ?? "", async (repo) =>
    repo ? await branchList() : [],
  );
  // Local first, then remote — the tree's order, and the same `refname:short`
  // names it writes into the shared selection, so the two never disagree about
  // which branch is meant.
  const shown = () =>
    (branches() ?? [])
      .filter((b) => b.name.toLowerCase().includes(q().toLowerCase()))
      .sort((a, b) => Number(a.isRemote) - Number(b.isRemote) || a.name.localeCompare(b.name));
  return (
    <div class="max-h-72 overflow-auto">
      <input
        class="mb-1 w-full rounded border border-border bg-bg px-1.5 py-0.5 text-xs outline-none"
        placeholder={d().filterBranches()}
        value={q()}
        onInput={(e) => setQ(e.currentTarget.value)}
      />
      <Item
        label={d().fltHeadScope()}
        on={props.current === null}
        onClick={() => props.onPick(null)}
      />
      <For each={shown()}>
        {(b) => (
          <Item
            label={b.name}
            on={props.current === b.name}
            onClick={() => props.onPick(b.name)}
          />
        )}
      </For>
    </div>
  );
}

// ── Author ───────────────────────────────────────────────────────────────────

/** Authors of this repository's history, read once per repository per session:
 * the list walks the whole log and does not change while the panel is open. */
function AuthorMenu(props: { repo: string; current: string | null; onPick: (a: string | null) => void }) {
  const [q, setQ] = createSignal("");
  const [authors] = createResource(
    () => props.repo,
    async (repo) => (repo ? await logAuthors() : []),
  );
  const shown = () =>
    (authors() ?? []).filter((a) => a.toLowerCase().includes(q().toLowerCase()));
  return (
    <div class="max-h-72 overflow-auto">
      <input
        class="mb-1 w-full rounded border border-border bg-bg px-1.5 py-0.5 text-xs outline-none"
        placeholder={d().fltAuthorFilter()}
        value={q()}
        onInput={(e) => setQ(e.currentTarget.value)}
      />
      <Item label={d().fltAnyAuthor()} on={!props.current} onClick={() => props.onPick(null)} />
      <Show when={!authors.loading} fallback={<div class="px-2 py-1 text-fg-muted">{d().fltAuthorsLoading()}</div>}>
        <Show when={shown().length > 0} fallback={<div class="px-2 py-1 text-fg-muted">{d().fltAuthorsEmpty()}</div>}>
          <For each={shown()}>
            {(a) => <Item label={a} on={props.current === a} onClick={() => props.onPick(a)} />}
          </For>
        </Show>
      </Show>
    </div>
  );
}

// ── Date ─────────────────────────────────────────────────────────────────────

function DateMenu(props: {
  since: number | null;
  until: number | null;
  onPick: (since: number | null, until: number | null) => void;
}) {
  const [from, setFrom] = createSignal(asInputDate(props.since));
  const [to, setTo] = createSignal(asInputDate(props.until));
  createEffect(() => setFrom(asInputDate(props.since)));
  createEffect(() => setTo(asInputDate(props.until)));
  const now = () => Math.floor(Date.now() / 1000);
  return (
    <div>
      <div class="px-2 py-1 text-[11px] text-fg-muted">{d().fltDateTip()}</div>
      <Item
        label={d().fltDateAny()}
        on={props.since === null && props.until === null}
        onClick={() => props.onPick(null, null)}
      />
      <Item label={d().fltDateToday()} onClick={() => props.onPick(startOfToday(), null)} />
      <Item label={d().fltDateWeek()} onClick={() => props.onPick(now() - 7 * DAY, null)} />
      <Item label={d().fltDateMonth()} onClick={() => props.onPick(now() - 30 * DAY, null)} />
      <Item label={d().fltDateYear()} onClick={() => props.onPick(now() - 365 * DAY, null)} />
      <div class="mt-1 border-t border-border px-2 pt-1">
        <div class="mb-1 text-[11px] text-fg-muted">{d().fltDateCustom()}</div>
        <label class="mb-1 flex items-center gap-1">
          <span class="w-8 text-fg-muted">{d().fltDateFrom()}</span>
          <input
            type="date"
            class="flex-1 rounded border border-border bg-bg px-1 py-0.5 text-xs outline-none"
            value={from()}
            onInput={(e) => setFrom(e.currentTarget.value)}
          />
        </label>
        <label class="mb-1 flex items-center gap-1">
          <span class="w-8 text-fg-muted">{d().fltDateTo()}</span>
          <input
            type="date"
            class="flex-1 rounded border border-border bg-bg px-1 py-0.5 text-xs outline-none"
            value={to()}
            onInput={(e) => setTo(e.currentTarget.value)}
          />
        </label>
        <button
          class="w-full rounded border border-border px-1.5 py-0.5 text-xs hover:bg-bg-muted"
          onClick={() => props.onPick(from() ? dayStart(from()) : null, to() ? dayEnd(to()) : null)}
        >
          {d().fltDateApply()}
        </button>
      </div>
    </div>
  );
}

// ── Paths ────────────────────────────────────────────────────────────────────

function PathsMenu(props: {
  repo: string;
  paths: string[];
  note: string;
  setNote: (v: string) => void;
  onChange: (paths: string[]) => void;
}) {
  const [draft, setDraft] = createSignal("");

  const add = (list: string[]) => {
    const next = props.paths.slice();
    for (const p of list) if (p && !next.includes(p)) next.push(p);
    if (next.length !== props.paths.length) props.onChange(next);
  };

  const pick = async (directory: boolean) => {
    // Read before the await: everything past it is outside the tracked scope.
    const repo = props.repo;
    props.setNote("");
    if (!repo) {
      props.setNote(d().fltNoRepo());
      return;
    }
    const picked = await open({ directory, multiple: true, defaultPath: repo });
    if (!picked) return;
    const abs = Array.isArray(picked) ? picked : [picked];
    const rel = abs.map((a) => relativeToRepo(repo, a));
    if (rel.some((r) => r === null)) props.setNote(d().fltPathOutside());
    add(rel.filter((r): r is string => r !== null));
  };

  return (
    <div>
      <For each={props.paths}>
        {(p) => (
          <div class="flex items-center gap-1 px-2 py-0.5 text-xs">
            <span class="flex-1 truncate" title={p}>
              {p}
            </span>
            <button
              class="text-fg-muted hover:text-danger"
              title={d().fltRemove()}
              onClick={() => props.onChange(props.paths.filter((x) => x !== p))}
            >
              ×
            </button>
          </div>
        )}
      </For>
      <div class="mt-1 flex items-center gap-1 border-t border-border px-2 pt-1">
        <input
          class="min-w-0 flex-1 rounded border border-border bg-bg px-1.5 py-0.5 text-xs outline-none"
          placeholder={d().fltPathPlaceholder()}
          value={draft()}
          onInput={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            add([draft().trim()]);
            setDraft("");
          }}
        />
        <button
          class="rounded border border-border px-1.5 py-0.5 text-xs hover:bg-bg-muted"
          onClick={() => {
            add([draft().trim()]);
            setDraft("");
          }}
        >
          {d().fltPathAdd()}
        </button>
      </div>
      <div class="flex items-center gap-1 px-2 py-1">
        <button
          class="flex-1 rounded border border-border px-1.5 py-0.5 text-xs hover:bg-bg-muted"
          onClick={() => void pick(false)}
        >
          {d().fltPathChooseFile()}
        </button>
        <button
          class="flex-1 rounded border border-border px-1.5 py-0.5 text-xs hover:bg-bg-muted"
          onClick={() => void pick(true)}
        >
          {d().fltPathChooseDir()}
        </button>
      </div>
      <Show when={props.note}>
        <div class="px-2 pb-1 text-[11px] text-warn">{props.note}</div>
      </Show>
    </div>
  );
}
