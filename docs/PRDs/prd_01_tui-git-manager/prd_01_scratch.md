---
title: "prd_01 scratch — TUI git manager with changelists"
project: my-git
type: idea
status: draft
tags: [my-git, prd, terminal, tui, rust, ratatui, changelist, scratch]
updated: 2026-07-25
---

# prd_01 — Scratchpad

Analysis of `Projects/my-git/terminal/Техническое задание.md` before drafting the PRD.

## Problem & target users

Developers who want JetBrains-style **named changelists** while working from the
terminal. Native git has no changelists — the value is grouping changed files into
named lists and committing per-list, plus the everyday loop (commit, revert/reset,
push/update, rebase) in one keyboard-driven full-screen TUI. Target: fast, low-RAM,
instant-start alternative to a git GUI for a *fixed* set of tasks.

## Scope

**In (P0 unless noted):**
- Display changed files of the current working tree, grouped by changelist, with
  status letter + colour (M/A/D/R/?/conflict).
- Changelists: create / rename / delete; always-present non-deletable **Default**;
  one **active** list; move files between lists (key + multi-select); a
  "Not for commit" list that never commits.
- Commit a selected changelist (or marked files in it); message editor / `$EDITOR`;
  amend last commit.
- Revert commit; reset to a commit (soft/mixed/hard) with explicit confirmation for
  `--hard`; rollback a file to HEAD (checkout file) with confirmation.
- Branches: push new (`-u`), update existing (`--force-with-lease` hint; force only
  with explicit confirm); fetch/pull; ahead/behind indicator; checkout; create.
- Rebase current branch onto another (shell-out); rebase-in-progress state (N of M);
  continue / abort / skip; conflict resolution — P0 continue/abort, P1 convenient flow.
- Mini-log of recent commits (source for reset/revert/rebase-onto).
- Storage of changelists in `<repo>/.git/changelists.json` — **contract shared with the
  GUI version** (byte-compatible; both apps operate on the same repo).

**Out (explicitly):**
- IDE-grade side-by-side diff editor with hunk staging (unified-diff view is enough;
  partial per-hunk stage is P2, not MVP).
- Full branch graph, blame, stash manager, merge strategies — those belong to the GUI.
- Implementing the GUI version. Only format compatibility is in scope.

## Domain model — key entities

- **ChangelistStore** — the `changelists.json` document: `version`, `activeChangelistId`,
  ordered `changelists[]`.
- **Changelist** — `{ id, name, comment, isDefault, files[] }`. Exactly one `isDefault`
  (non-deletable, catch-all). `files[]` are repo-relative `/`-separated paths of
  **changed** files explicitly assigned.
- **ChangedFile** — a working-tree entry: path + status (Modified/Added/Deleted/
  Renamed/Untracked/Conflicted) + which changelist it currently sits in.
- **Commit** (mini-log row) — hash, summary, author, refs.
- **BranchState** — current branch, upstream, ahead/behind, rebase-in-progress info.

## Git engine (approach note, NOT a spec requirement)

Per ТЗ §2.1: simple ops (status, diff, stage, commit, branch, push, fetch, checkout,
reset) via `gix`/`git2`; **rebase / merge / conflict resolution via shell-out to system
`git`**. Abstract behind a trait; some ops pass through to CLI. This is *implementation
approach* → lives in Task 01 scope, **not** in a behavioural spec.

## Lifecycle / status model

- **File status:** Modified | Added | Deleted | Renamed | Untracked | Conflicted.
- **Changelist assignment:** a changed file is in exactly one list; unassigned changed
  files fall into the active list (or Default). On commit, committed files leave the list.
- **Rebase state:** none | in-progress(N of M) → continue / skip / abort → none.
- **Branch vs upstream:** ahead A / behind B; new branch has no upstream until pushed `-u`.

## Sync rules (contract, identical to GUI — ТЗ §6.2)

1. On start: compute changed files via `git status`.
2. Drop file entries from `changelists.json` that are no longer changed.
3. Changed files not assigned anywhere → active changelist (or Default if none active).
4. One file in at most one list.
5. Writes atomic (temp + rename); on concurrent conflict → re-read, **last write wins for
   file location**.
6. Commit list = `git add -- <files>` → `git commit` → remove committed files from list.

## Acceptance criteria (canonical — ТЗ §8, become spec scenarios)

1. In a repo: changed files shown grouped by changelist; outside a repo: clear error.
2. Create "Not for commit", move a file into it by key; assignment persists in
   `.git/changelists.json`, survives restart, and is visible to the GUI version.
3. Commit "Default"; "Not for commit" files stay changed and out of the commit.
4. Push new branch with `-u`; update existing; ahead/behind indicator.
5. From the mini-log: revert a commit and (with confirmation) reset.
6. Rebase current branch onto another, with continue/abort.
7. RAM in single-to-tens-of-MB; instant start. *(DoD check, not a scenario.)*

## Affected domains (delta specs this PRD seeds)

| Domain | Covers |
|--------|--------|
| `changelists` | store format contract, sync rules, active list, not-for-commit, move, atomic write |
| `working-tree` | changed-files display grouped by list, statuses, unified diff view |
| `commit` | commit-by-list, exclusion of other lists, amend, message input |
| `history` | mini-log, revert, reset (soft/mixed/hard) + confirm, rollback file to HEAD |
| `branches` | push new `-u` / update, force-with-lease + confirm, fetch/pull, ahead/behind, checkout/create |
| `rebase` | rebase onto, in-progress state, continue/abort/skip, conflict resolution |
| `tui-shell` | full-screen layout, panels/focus, status bar, keymap, help, states, non-repo error, theme (design deliverable) |

All are new — this PRD seeds the first living spec for each domain at archive time.

## Notes / risks flagged for the PRD

- Format compatibility with GUI is the hardest correctness constraint (AC#2) — it is a
  contract, verify against the exact schema in ТЗ §6.1.
- Concurrency with GUI writing the same file → atomic temp+rename + last-write-wins.
- Rebase/merge correctness depends on shell-out git; keep engine boundary clean.
- §7 theme/wireframe is the designer's output — treat as a design deliverable, not
  testable scenarios; only observable UI behaviour (non-repo error, empty state, focus
  switch, key→effect, destructive confirm) becomes scenarios.
