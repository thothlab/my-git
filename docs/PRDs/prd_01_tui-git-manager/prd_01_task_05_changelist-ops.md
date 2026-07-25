---
title: "Task 05 — changelist operations UI: create/rename/delete, multi-select, move, active"
project: my-git
type: plan
status: todo
tags: [my-git, task, changelist, ui, multi-select, move]
updated: 2026-07-25
---

# Task 05 — Changelist operations UI

## Goal

Wire the interactive changelist operations onto the store: create / rename / delete a list,
set the active list, multi-select files, and move files between lists (including into a
"Not for commit" list). Each change persists via Task 02's store.

## Scope

- Create / rename / delete list; set-active; "Not for commit" is an ordinary list the user creates.
- Multi-select files (mark/unmark) and act on marked files.
- Move file(s) to a chosen list via a changelist-picker popup.
- Out of scope: commit (Task 06); the store invariants themselves live in Task 02.

## Subtasks

1. New-list flow (name input) → `create`; enforce unique-name feedback.
2. Rename / delete flows; block Default deletion; on delete, files fall back to Default (via store).
3. Set-active-list action; reflect active list in the status bar.
4. Multi-select: mark/unmark file, track a selection set, clear on action.
5. Move flow: `m` → changelist-picker popup → `move(markedOrCurrent, targetId)`; persist.
6. Refresh the grouped view after each operation.

## Deliverables

- Interactive create/rename/delete/set-active flows.
- Multi-select + move-to-list popup, persisting to `.git/changelists.json`.

## Definition of Done

- [ ] User can create, rename, and delete non-default lists; Default deletion is blocked with feedback.
- [ ] Deleting a non-default list moves its files to Default (visible immediately).
- [ ] Active list is selectable and shown in the status bar.
- [ ] User can mark multiple files and move them to another list in one action.
- [ ] Every operation persists and survives restart (AC#2 path).

## Tests

- **`changelists` → "Move reassigns exclusively".**
- **`changelists` → "Move persists across restart and is visible to the GUI" (AC#2)** —
  create "Not for commit", move a file in by key, restart, confirm assignment on disk + in view.
- **`changelists` → "Default cannot be deleted"** (UI rejects with feedback).
- **`changelists` → "Files survive list deletion"** (UI path).
- **`changelists` → "Duplicate name rejected"** (UI feedback).

## Dependencies

Task 02 (store operations), Task 03 (popups/keymap), Task 04 (grouped view to act on).
