---
title: "Task 04 — working-tree view: grouped changes + diff + empty state"
project: my-git
type: plan
status: todo
tags: [my-git, task, working-tree, status, diff]
updated: 2026-07-25
---

# Task 04 — Working-tree view (grouped changes / diff / empty state)

## Goal

Render changed files grouped under their changelist headings with per-file status
indicators, show the unified diff of the selected file, and provide the empty state.

## Scope

- Changes panel: changelist headings (name + count) over their files; status letter + colour
  per file (Modified/Added/Deleted/Renamed/Untracked/Conflicted).
- Diff panel: unified diff of the selected file (view only; no hunk staging).
- Empty state when nothing is changed.
- Out of scope: list mutation and move (Task 05), commit (Task 06).

## Subtasks

1. Join `status()` output with the changelist store to build the grouped model.
2. Render list headings with file counts and files with status letter/colour.
3. Wire selection in the Changes panel to a `diff(path)` unified-diff render in the Diff panel.
4. Render the empty state with a hint when there are no changes.
5. Handle scrolling for long file lists and long diffs.

## Deliverables

- Changes panel grouped by changelist with status indicators.
- Diff panel showing the selected file's unified diff.
- Empty state.

## Definition of Done

- [ ] Changed files appear under the correct changelist headings with counts.
- [ ] Each status (M/A/D/R/?/conflict) has a distinct letter + colour.
- [ ] Selecting a file shows its unified diff (hunk headers, +/- lines).
- [ ] Clean working tree shows the empty-state hint, not a blank panel.

## Tests

- **`working-tree` → "Grouped display in a repo" (AC#1).**
- **`working-tree` → "Per-file status indicator" / "Statuses are distinguishable".**
- **`working-tree` → "Empty state when nothing is changed".**
- **`working-tree` → "Unified diff of the selected file".**

## Dependencies

Task 02 (store/model), Task 03 (panels/focus).
