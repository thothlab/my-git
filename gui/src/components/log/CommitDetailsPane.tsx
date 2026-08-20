import { openUrl } from "@tauri-apps/plugin-opener";
import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
} from "solid-js";
import {
  commitDetails,
  commitFiles,
  commitsCompare,
  errText,
  type CommitDetails,
  type CommitFileEntry,
} from "../../api";
import { focusPanel } from "../../hotkeys";
import { d, fmtDateTime } from "../../i18n";
import { setSelectedPath, setViewMode, state, statusMeta } from "../../store";
import type { CompareTarget } from "./actions/compareSelection";
import { PanelBtn, PanelChrome, PanelNote } from "./PanelChrome";
import { setSelectedCommitFile, selectedCommitFile } from "./commitFileSelection";
import {
  baseName,
  buildFileTree,
  countFiles,
  treeDirPaths,
  type FileTreeNode,
} from "../pathTree";

/**
 * Commit card + the files the commit changed.
 *
 * Both halves come from one resource keyed on the commit hash: two resources on
 * the same hash resolve at different moments and the card would show commit A
 * beside commit B's files. One request also means one stale-response guard —
 * Solid drops the answer of a superseded fetch, so clicking quickly through the
 * log can never leave an older commit's details on screen.
 */
const BRANCH_PREVIEW = 6;

type Row =
  | { kind: "dir"; key: string; name: string; count: number; depth: number }
  | { kind: "file"; key: string; file: CommitFileEntry; name: string; depth: number };

export default function CommitDetailsPane(props: {
  selected: () => string | null;
  /** A comparison of two revisions replaces the selected commit as the subject
   * of this panel: the card gives way to the pair of side labels and the file
   * list is the one *between* the revisions (R39i). */
  compare?: () => CompareTarget | null;
}) {
  const [grouped, setGrouped] = createSignal(true);
  const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());
  const [allBranches, setAllBranches] = createSignal(false);
  const [cursor, setCursor] = createSignal<string | null>(null);

  /** What the panel is showing. One key for both modes: a comparison and a
   * commit must never be in flight at once and settle in the wrong order. */
  const rawSubject = (): { cmp: CompareTarget } | { hash: string } | null => {
    const cmp = props.compare?.() ?? null;
    if (cmp) return { cmp };
    const hash = props.selected();
    return hash ? { hash } : null;
  };

  /**
   * The subject the reader stopped on, not every subject they passed over.
   *
   * Reading a commit costs two git processes (`show` and the file list) and the
   * memory to hold both answers, and holding the arrow key down walks the cursor
   * over hundreds of commits a second. Fetched per step, that was the single
   * largest source of the log panel's memory growth: a keyboard pass down a long
   * history asked git for hundreds of commits nobody ever saw, and the browser
   * heap grew to hold every answer. So the request waits for the cursor to
   * settle; what stands still on screen is what gets read.
   *
   * The wait applies to *arriving* at a subject, never to leaving one: clearing
   * is immediate, so the panel does not show the previous commit's card while
   * the selection is empty.
   */
  const SETTLE_MS = 120;
  const [settled, setSettled] = createSignal<{ cmp: CompareTarget } | { hash: string } | null>(
    rawSubject(),
  );
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    const next = rawSubject();
    clearTimeout(settleTimer);
    if (next === null) {
      setSettled(null);
      return;
    }
    settleTimer = setTimeout(() => setSettled(next), SETTLE_MS);
  });
  onCleanup(() => clearTimeout(settleTimer));

  const subject = settled;

  const [data] = createResource(subject, async (s) => {
    if ("cmp" in s) {
      const files = await commitsCompare(s.cmp.from, s.cmp.to);
      // The right-hand revision is what a file of a comparison belongs to; the
      // diff panel takes the pair from `compareTarget` and only the path from
      // here, so this hash is the file's identity and not the diff's base.
      return { hash: s.cmp.to || s.cmp.from, details: null, files, cmp: s.cmp };
    }
    const [details, files] = await Promise.all([commitDetails(s.hash), commitFiles(s.hash)]);
    return { hash: s.hash, details, files, cmp: null };
  });

  // A new commit resets everything that belonged to the old one. The condition
  // is spelled out rather than inferred from what the effect happens to read:
  // the day this effect tracks a second signal, "the hash changed" must still
  // be what triggers the reset, not "something in here changed".
  //
  // Selection goes to null rather than to the first file: picking a file is what
  // asks for a diff, and no user asked for one yet.
  let shownHash: string | null = null;
  createEffect(() => {
    const s = subject();
    const hash = s === null ? null : "cmp" in s ? `${s.cmp.from}..${s.cmp.to}` : s.hash;
    if (hash === shownHash) return;
    shownHash = hash;
    setCollapsed(new Set<string>());
    setAllBranches(false);
    setCursor(null);
    setSelectedCommitFile(null);
  });

  // `data()` rethrows on a failed request, and the keyboard handlers below run
  // outside the JSX guard — so every non-ready state reads as "no rows".
  const ready = () => (data.state === "ready" ? data() : undefined);
  const files = () => ready()?.files ?? [];
  const tree = createMemo(() => buildFileTree(files()));

  const rows = createMemo<Row[]>(() => {
    if (!grouped()) {
      return files().map((f) => ({
        kind: "file" as const,
        key: `f:${f.path}`,
        file: f,
        name: f.path,
        depth: 0,
      }));
    }
    const out: Row[] = [];
    const walk = (node: FileTreeNode<CommitFileEntry>, depth: number) => {
      for (const dir of node.dirs) {
        out.push({
          kind: "dir",
          key: `d:${dir.path}`,
          name: dir.name,
          count: countFiles(dir),
          depth,
        });
        if (!collapsed().has(dir.path)) walk(dir, depth + 1);
      }
      for (const f of node.files) {
        out.push({ kind: "file", key: `f:${f.path}`, file: f, name: baseName(f.path), depth });
      }
    };
    walk(tree(), 0);
    return out;
  });

  // The panel body is scrolled by PanelChrome's wrapper, so a cursor moved by
  // the keyboard has to pull its own row into view — same anchor map as the
  // difference-to-difference navigation in DiffView. "nearest" scrolls only when
  // the row is actually outside, so arrowing through visible rows stays still.
  const anchors = new Map<string, HTMLElement>();
  const anchor = (key: string) => (el: HTMLElement) => {
    anchors.set(key, el);
    onCleanup(() => {
      if (anchors.get(key) === el) anchors.delete(key);
    });
  };
  const goToRow = (row: Row) => {
    setCursor(row.key);
    if (row.kind === "file") pick(row.file);
    anchors.get(row.key)?.scrollIntoView({ block: "nearest" });
  };

  const pick = (f: CommitFileEntry) => {
    const hash = ready()?.hash;
    if (!hash) return;
    setCursor(`f:${f.path}`);
    setSelectedCommitFile({ hash, path: f.path, oldPath: f.oldPath, status: f.status });
  };

  const moveCursor = (delta: number) => {
    const list = rows();
    if (list.length === 0) return;
    const at = list.findIndex((r) => r.key === cursor());
    goToRow(list[Math.max(0, Math.min(at < 0 ? 0 : at + delta, list.length - 1))]);
  };

  const toggleDir = (path: string, force?: boolean) =>
    setCollapsed((prev) => {
      const n = new Set(prev);
      const shouldCollapse = force ?? !n.has(path);
      shouldCollapse ? n.add(path) : n.delete(path);
      return n;
    });

  const currentRow = () => rows().find((r) => r.key === cursor());

  /**
   * A02 / История 78: from a file of a commit to that file as it is on disk.
   *
   * "Its current version" is the Changes panel's version, and that panel only
   * has a row for a file with uncommitted changes — so the action is offered
   * only for such a file and says why when it is not, instead of switching modes
   * to an empty selection.
   */
  const uncommitted = () =>
    new Set((state()?.changelists ?? []).flatMap((cl) => cl.files.map((f) => f.path)));
  const pickedPath = () => {
    const row = currentRow();
    return row?.kind === "file" ? row.file.path : (selectedCommitFile()?.path ?? null);
  };
  const canOpenCurrent = () => {
    const p = pickedPath();
    return !!p && uncommitted().has(p);
  };
  const openCurrent = () => {
    const p = pickedPath();
    if (!p || !uncommitted().has(p)) return;
    setSelectedPath(p);
    setViewMode("changes");
  };

  const onKey = (e: KeyboardEvent) => {
    const row = currentRow();
    if (!row || row.kind !== "dir") return false;
    const path = row.key.slice(2);
    if (e.code === "ArrowRight") {
      toggleDir(path, false);
      anchors.get(row.key)?.scrollIntoView({ block: "nearest" });
      return true;
    }
    if (e.code === "ArrowLeft") {
      toggleDir(path, true);
      anchors.get(row.key)?.scrollIntoView({ block: "nearest" });
      return true;
    }
    return false;
  };

  const activate = () => {
    const row = currentRow();
    if (!row) return;
    if (row.kind === "dir") toggleDir(row.key.slice(2));
    else pick(row.file);
  };

  return (
    <PanelChrome
      id="details"
      title={d().commitDetailsTitle()}
      handlers={{
        moveSelection: moveCursor,
        moveToEdge: (edge) => {
          const list = rows();
          if (list.length === 0) return;
          goToRow(edge === -1 ? list[0] : list[list.length - 1]);
        },
        activate,
        onKey,
      }}
      toolbar={
        <Show when={ready()}>
          <PanelBtn
            label="⇱"
            tip={d().openCurrentVersionTip()}
            disabled={!canOpenCurrent()}
            disabledTip={
              pickedPath() ? d().openCurrentUnchanged() : d().openCurrentVersionTip()
            }
            onClick={openCurrent}
          />
          <PanelBtn
            label={grouped() ? "▤" : "⊞"}
            tip={d().groupByDirTip()}
            onClick={() => setGrouped((g) => !g)}
          />
          <PanelBtn
            label="+"
            tip={d().expandAllTip()}
            disabled={!grouped()}
            disabledTip={d().treeOnlyTip()}
            onClick={() => setCollapsed(new Set<string>())}
          />
          <PanelBtn
            label="−"
            tip={d().collapseAllTip()}
            disabled={!grouped()}
            disabledTip={d().treeOnlyTip()}
            onClick={() => setCollapsed(new Set(treeDirPaths(tree())))}
          />
        </Show>
      }
    >
      <Show when={subject()} fallback={<PanelNote title={d().selectCommitHint()} />}>
        {/* Order matters: reading `data()` on a failed resource rethrows, so the
            error branch is taken before the accessor is ever touched. */}
        <Show when={!data.error} fallback={<PanelNote title={errText(data.error)} />}>
          <Show
            when={data.state === "ready" && data()}
            fallback={<PanelNote title={d().loadingCommitDetails()} />}
          >
            {(loaded) => (
              <div class="flex min-h-0 flex-col">
                <Show when={loaded().details} fallback={<CompareCard cmp={loaded().cmp!} />}>
                  {(details) => (
                    <CommitCard
                      details={details()}
                      allBranches={allBranches()}
                      onToggleBranches={() => setAllBranches((v) => !v)}
                    />
                  )}
                </Show>
                <div class="border-t border-border px-2 py-1 text-[10px] uppercase tracking-wide text-fg-subtle">
                  {loaded().cmp ? d().compareFilesTitle() : d().changedFiles()} ·{" "}
                  {d().filesCount(loaded().files.length)}
                </div>
                <Show
                  when={loaded().files.length > 0}
                  fallback={
                    <div class="px-2 py-2 text-xs text-fg-subtle">{d().noChangedFiles()}</div>
                  }
                >
                  <div class="pb-2">
                    <For each={rows()}>
                      {(row) =>
                        row.kind === "file" ? (
                          <FileRow
                            ref={anchor(row.key)}
                            row={row}
                            selected={selectedCommitFile()?.path === row.file.path}
                            cursor={cursor() === row.key}
                            onPick={() => pick(row.file)}
                          />
                        ) : (
                          <DirRow
                            ref={anchor(row.key)}
                            row={row}
                            collapsed={collapsed().has(row.key.slice(2))}
                            cursor={cursor() === row.key}
                            onToggle={() => {
                              setCursor(row.key);
                              toggleDir(row.key.slice(2));
                            }}
                          />
                        )
                      }
                    </For>
                  </div>
                </Show>
              </div>
            )}
          </Show>
        </Show>
      </Show>
    </PanelChrome>
  );
}

/**
 * The header of a comparison: which two revisions it is between. It stands in
 * the commit card's place because a comparison has no single commit to describe
 * — naming the pair is the whole of what can honestly be said about it.
 */
function CompareCard(props: { cmp: CompareTarget }) {
  return (
    <div class="select-text px-2 py-2 text-xs" onPointerDown={(e) => e.stopPropagation()}>
      <div class="font-mono">
        {d().compareSides(props.cmp.fromLabel, props.cmp.toLabel)}
      </div>
    </div>
  );
}

/** Subject, body, hash and authorship. Everything here is selectable text. */
function CommitCard(props: {
  details: CommitDetails;
  allBranches: boolean;
  onToggleBranches: () => void;
}) {
  const c = () => props.details;
  // Author and committer are the same entry only when the person *and* the
  // moment match: an amended or rebased commit keeps the author and moves the
  // committer date, and hiding that would lose the fact the amend happened.
  const sameAuthor = () =>
    c().author === c().committer &&
    c().authorEmail === c().committerEmail &&
    c().authorAt === c().committerAt;

  const branches = () => c().branches;
  const shown = () => (props.allBranches ? branches() : branches().slice(0, BRANCH_PREVIEW));

  return (
    // The window forbids text selection globally (styles.css `user-select:none`),
    // so the card opts back in. PanelChrome focuses the panel on every
    // pointerdown; that call lands mid-drag and kills the selection, hence the
    // stopPropagation — focus is handed to the panel on pointerup instead, once
    // the drag is over and only when it did not select anything.
    <div
      class="select-text px-2 py-2 text-xs"
      onPointerDown={(e) => e.stopPropagation()}
      onPointerUp={() => {
        if (!window.getSelection()?.toString()) focusPanel("details");
      }}
    >
      <div class="whitespace-pre-wrap break-words font-medium">{c().subject}</div>
      <Show when={c().body.trim()}>
        <div class="mt-1 whitespace-pre-wrap break-words text-fg-muted">{c().body.trim()}</div>
      </Show>

      <div class="mt-2 font-mono text-[11px] text-fg-muted">{c().hash}</div>

      <div class="mt-1 text-fg-muted">
        {d().authorLabel()}: {c().author} <MailLink email={c().authorEmail} /> ·{" "}
        {fmtTime(c().authorAt)}
      </div>
      <Show when={!sameAuthor()}>
        <div class="text-fg-muted">
          {d().committerLabel()}: {c().committer} <MailLink email={c().committerEmail} /> ·{" "}
          {fmtTime(c().committerAt)}
        </div>
      </Show>

      <div class="mt-2 text-fg-subtle">
        <Show when={branches().length > 0} fallback={<span>{d().noContainingBranches()}</span>}>
          <span>{d().inBranches(branches().length)}: </span>
          <span class="font-mono">{shown().join(", ")}</span>
          <Show when={branches().length > BRANCH_PREVIEW}>
            {" "}
            <button
              class="text-accent underline decoration-dotted"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={props.onToggleBranches}
            >
              {props.allBranches
                ? d().showFewerBranches()
                : d().showAllBranches(branches().length)}
            </button>
          </Show>
          <Show when={c().branchesTruncated}>
            <div class="text-warn">{d().branchesCapped()}</div>
          </Show>
        </Show>
      </div>
    </div>
  );
}

/**
 * The e-mail address as a real link. `mailto:` inside a WKWebView does nothing
 * on its own, so it goes through the opener plugin, like the footer's links.
 */
function MailLink(props: { email: string }) {
  return (
    <Show when={props.email} fallback={null}>
      <button
        class="text-accent hover:underline"
        title={props.email}
        onPointerDown={(e) => e.stopPropagation()}
        // `opener:default` permits mailto: (allow-default-urls); a refusal still must
        // not escape as an unhandled rejection.
        onClick={() => void openUrl(`mailto:${props.email}`).catch(() => {})}
      >
        &lt;{props.email}&gt;
      </button>
    </Show>
  );
}

function DirRow(props: {
  ref: (el: HTMLElement) => void;
  row: Extract<Row, { kind: "dir" }>;
  collapsed: boolean;
  cursor: boolean;
  onToggle: () => void;
}) {
  return (
    <div
      ref={props.ref}
      class="flex cursor-default items-center gap-1 py-0.5 pr-2 text-xs hover:bg-bg-muted"
      classList={{ "bg-bg-muted": props.cursor }}
      style={{ "padding-left": `${8 + props.row.depth * 16}px` }}
      onClick={props.onToggle}
    >
      <Chevron collapsed={props.collapsed} />
      <span class="truncate text-fg-subtle" title={props.row.name}>
        {props.row.name}
      </span>
      <span class="shrink-0 text-fg-subtle">({props.row.count})</span>
    </div>
  );
}

function FileRow(props: {
  ref: (el: HTMLElement) => void;
  row: Extract<Row, { kind: "file" }>;
  selected: boolean;
  cursor: boolean;
  onPick: () => void;
}) {
  const f = () => props.row.file;
  const m = () => statusMeta(f().status);
  return (
    <div
      ref={props.ref}
      class="flex cursor-default items-center gap-1 py-0.5 pr-2 font-mono text-xs hover:bg-bg-muted"
      classList={{ "bg-accent/15": props.selected, "ring-1 ring-inset ring-accent/40": props.cursor }}
      style={{ "padding-left": `${8 + props.row.depth * 16 + 16}px` }}
      onClick={props.onPick}
      title={f().oldPath ? `${f().oldPath} → ${f().path}` : f().path}
    >
      <span class={`w-3 shrink-0 text-center font-bold ${m().cls}`} title={f().status}>
        {m().letter}
      </span>
      <span class="truncate">{props.row.name}</span>
      {/* A rename is only readable with both names present; the old one follows
          the new one so the column of current names stays aligned. */}
      <Show when={f().oldPath}>
        <span class="truncate text-fg-subtle">← {f().oldPath}</span>
      </Show>
    </div>
  );
}

function Chevron(props: { collapsed: boolean }) {
  return (
    <span class="flex h-4 w-4 shrink-0 items-center justify-center text-fg-subtle">
      <svg
        width="13"
        height="13"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        style={{
          transform: props.collapsed ? "none" : "rotate(90deg)",
          transition: "transform 0.12s",
        }}
      >
        <path d="M6 4l4 4-4 4" />
      </svg>
    </span>
  );
}

/**
 * Unix seconds from git (`%at` / `%ct`) in the window's one date format
 * (`i18n.fmtDateTime`) and the user's zone.
 */
const fmtTime = (unixSeconds: number) => fmtDateTime(unixSeconds);
