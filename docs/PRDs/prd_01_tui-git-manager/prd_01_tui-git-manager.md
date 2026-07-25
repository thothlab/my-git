---
title: "PRD 01 — TUI git manager with changelists (my-git terminal)"
project: my-git
type: report
status: active
tags: [my-git, prd, terminal, tui, rust, ratatui, git, changelist, rebase, push]
updated: 2026-07-25
---

# PRD 01 — TUI git manager with changelists

Source: `Projects/my-git/terminal/Техническое задание.md` (ТЗ, draft, 2026-07-24).
Scope of this PRD = **exactly** the ТЗ feature list (§3), no expansion toward the GUI
version. Delta specs seed seven domains; ten tasks decompose delivery.

## Objective

Ship a keyboard-driven, full-screen terminal application (Rust + `ratatui`/`crossterm`)
that lets a developer do the everyday git loop **organised around named changelists**:

- see the current working tree's changed files grouped into named lists;
- move files between lists, hold a "Not for commit" list, and **commit per list**;
- revert / reset commits and roll a file back to HEAD;
- push new branches (`-u`) and update existing ones, with ahead/behind visibility;
- rebase the current branch onto another, including continue/abort.

Changelists are a metadata layer stored in `<repo>/.git/changelists.json`, **byte-format
compatible with the my-git GUI version** so both tools operate on one repository and see
the same lists. The product's defining constraints are *lightness* (single-to-tens-of-MB
RAM, instant start) and *operating on real git* (no mocks).

## Non-objectives

- **Not** a general git GUI. No branch graph, blame, stash manager, or merge strategies —
  those live in the GUI version.
- **Not** an IDE diff editor. Unified-diff *viewing* only; per-hunk partial staging is P2
  and out of MVP.
- **Not** building the GUI version — only wire-compatibility with its `changelists.json`.
- **Not** re-implementing rebase/merge in-process — those shell out to system `git`.
- No network features beyond git remote operations (push/fetch/pull) the user invokes.
- No configuration/plugin system, no telemetry, no auto-update.

## Data model

### ChangelistStore — `<repo_root>/.git/changelists.json`

The on-disk contract, **identical to the GUI version** (ТЗ §6.1). Lives inside `.git/`
so it is outside version control and outside `git status`.

| Field | Type | Notes |
|-------|------|-------|
| `version` | integer | Schema version; `1` for this PRD. |
| `activeChangelistId` | string | `id` of the active list; new/unassigned changed files land here. |
| `changelists` | array\<Changelist\> | Ordered; rendered top-to-bottom. |

### Changelist

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Stable identifier, referenced by `activeChangelistId`. |
| `name` | string | Display name. Unique among lists (case-sensitive). |
| `comment` | string | Optional description; may be empty. |
| `isDefault` | boolean | **Exactly one** list has `true`; that list is non-deletable and is the catch-all. |
| `files` | array\<string\> | Repo-relative, `/`-separated paths of **changed** files explicitly assigned to this list. |

Path rule: repo-root-relative, `/` separator on all platforms (including Windows).

### ChangedFile (in-memory, derived)

`{ path, status, changelistId }` where `status ∈ {Modified, Added, Deleted, Renamed,
Untracked, Conflicted}`. Derived each session from `git status` + the store; never persisted
except via its `changelistId` membership in `files[]`.

### Commit (mini-log row, in-memory)

`{ hash, summary, author, refs }` for the last N commits of the current branch.

### BranchState (in-memory, derived)

`{ currentBranch, upstream?, ahead, behind, rebase: none | { current, total } , detached }`.

## API list

There is no network API. The contract surface is (a) the internal **git-engine operation
set** behind a trait, (b) the **changelist store** operations, and (c) optional **CLI
subcommands**. Listed with inputs → outputs/effects; behaviour is specified in the delta
specs.

### Git-engine operations (trait surface)

| Operation | Input | Output / effect |
|-----------|-------|-----------------|
| `status()` | — | list of `ChangedFile` (path, status) for the working tree |
| `diff(path)` | file path | unified diff text for that file |
| `stage(paths)` | paths | files staged (`git add -- <paths>`) |
| `commit(paths, message, amend?)` | paths, message, amend flag | new commit created; returns new HEAD hash |
| `log(limit)` | count | list of `Commit` for current branch |
| `revert(hash)` | commit hash | inverse commit created |
| `reset(hash, mode)` | hash, `soft\|mixed\|hard` | branch pointer (and tree, per mode) moved |
| `checkoutFile(path)` | path | file content restored to HEAD |
| `branchState()` | — | `BranchState` (current, upstream, ahead/behind, rebase, detached) |
| `checkoutBranch(name)` | branch name | working tree switched |
| `createBranch(name, from)` | name, start point | branch created |
| `push(branch, opts)` | branch, `{ setUpstream, force, forceWithLease }` | remote ref updated |
| `fetch()` / `pull()` | — | remote refs fetched / integrated |
| `rebaseOnto(target)` | branch/ref | rebase started (shell-out); may enter in-progress |
| `rebaseContinue()` / `rebaseSkip()` / `rebaseAbort()` | — | advance / skip / cancel rebase |

Simple operations use `gix`/`git2`; **`rebaseOnto`, `rebase*`, and conflict handling
shell out to system `git`** (ТЗ §2.1). The trait boundary is an implementation detail, not
a behavioural requirement.

### Changelist store operations

| Operation | Input | Output / effect |
|-----------|-------|-----------------|
| `load()` | — | `ChangelistStore`; creates a default store if the file is missing |
| `sync(changedFiles)` | current `git status` | prunes vanished files, assigns unassigned → active/Default (ТЗ §6.2) |
| `create(name)` / `rename(id, name)` / `delete(id)` | — | mutate lists (Default non-deletable) |
| `setActive(id)` | list id | set `activeChangelistId` |
| `move(paths, targetId)` | file paths, target list | reassign files to one list only |
| `persist()` | — | atomic write (temp + rename); last-write-wins on conflict |

### CLI subcommands (optional, secondary — ТЗ §1.1)

`mygit` (no args) → launches the TUI. Optional non-interactive helpers, additive only:
`mygit commit <list> -m <msg>`, `mygit hide <path>` (move to "Not for commit"). CLI is a
convenience layer; the TUI is the core.

## Validation & state transitions

**Repository precondition.** The app MUST refuse to run outside a git repository, printing
a clear error and a hint; it never opens the TUI in that case.

**Changelist invariants.**
- Exactly one list has `isDefault: true`; that list cannot be deleted or renamed away from
  its default role. Unassigned changed files fall into the active list, or Default if none
  is active.
- A changed file belongs to **at most one** list. Moving a file removes it from its prior list.
- Deleting a non-default list reassigns its files to Default.
- List names are unique; creating/renaming to a duplicate name is rejected.

**Sync (on start and on external change).** Recompute `git status`; drop entries for files
no longer changed; place unassigned changed files into the active list (or Default). Writes
are atomic (temp + rename); on a concurrent write conflict, re-read and apply
last-write-wins for a file's location, preserving compatibility with a GUI writing the same
file.

**Commit.** Committing a list stages only that list's files (`git add -- <files>`), creates
the commit, and removes the committed files from the list. Files in other lists (including
"Not for commit") are never staged or committed. Amend targets the last commit.

**Destructive actions require explicit confirmation:** `reset --hard`, force push, and
rollback of a file to HEAD each prompt with a warning naming the data that will be lost;
the action proceeds only on confirm.

**Branch / upstream transitions.** A new branch has no upstream; the first push uses
`-u origin <branch>` and establishes it. Updating an existing branch pushes; when local and
remote diverge, the UI suggests `--force-with-lease` and performs a force push only on
explicit confirmation. Ahead/behind is shown relative to upstream.

**Rebase state machine.** `none → in-progress(current of total)`; from in-progress the user
may `continue` (after resolving), `skip`, or `abort` (returns to pre-rebase state). Conflict
files are listed; the user opens them in `$EDITOR`/mergetool, marks resolved, and continues.
MVP guarantees continue/abort; a convenient conflict flow is P1.

## Risks & mitigations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Format drift from GUV `changelists.json` breaks interop | Lists diverge across tools (AC#2 fails) | Freeze schema to ТЗ §6.1 exactly; a compat scenario asserts a GUI-written file round-trips unchanged; version field gates future changes. |
| Concurrent GUI + TUI writes corrupt the file | Lost/garbled assignments | Atomic temp+rename; re-read + last-write-wins on conflict; never partial-write. |
| `gix`/`git2` gaps in rebase/merge | Broken or half-done rebase | Shell out to system `git` for rebase/merge/conflicts behind the engine trait; surface in-progress state (N of M) and continue/abort/skip. |
| Destructive ops (`reset --hard`, force push, file rollback) cause data loss | Irreversible user harm | Mandatory confirm dialog naming what is lost; force push defaults to `--force-with-lease`. |
| External git changes mid-session (branch switch, commits) | Stale UI, wrong assignments | Re-sync store against fresh `git status` on (re)start and on refresh; tolerate missing file on first run. |
| Heavy dependencies erode the "lightness" advantage | Loses core differentiator | Keep deps minimal (`ratatui`, `crossterm`, one git lib, serde); track RAM/startup as a DoD gate (AC#7). |
| Terminal colour/theme variance (truecolor vs 256) | Unreadable UI | Map Pane tokens to truecolor with graceful degradation to 256; respect the user's terminal background. |

## Acceptance criteria

Canonical criteria from ТЗ §8. Each maps to a spec scenario and is referenced by a task's
Tests section. #1–#6 are automated/observable scenarios; #7 is a DoD performance gate.

1. **Grouped display / non-repo error** — In a git repo the app shows changed files grouped
   by changelist; outside a repo it exits with a clear error + hint.
   → `working-tree`, `tui-shell`.
2. **Move persists & interops** — Create "Not for commit", move a file into it by key; the
   assignment is written to `.git/changelists.json`, survives restart, and is readable by
   the GUI version (schema-compatible). → `changelists`.
3. **Commit-by-list isolation** — Commit "Default"; "Not for commit" files remain changed
   and are absent from the commit. → `commit`, `changelists`.
4. **Push new & update** — Push a new branch with `-u`; update an existing branch; the UI
   shows an ahead/behind indicator. → `branches`.
5. **Revert & reset from log** — From the mini-log, revert a commit and (with confirmation)
   reset to a commit. → `history`.
6. **Rebase with continue/abort** — Rebase the current branch onto another; drive
   continue/abort through in-progress state. → `rebase`.
7. **Lightness** — Resident memory in single-to-tens-of-MB and effectively instant start.
   → DoD gate on Task 01 and Task 10 (`/usr/bin/time -l`, startup timing).
