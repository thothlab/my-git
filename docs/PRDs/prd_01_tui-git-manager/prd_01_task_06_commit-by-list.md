---
title: "Task 06 — commit by changelist + amend + message editor"
project: my-git
type: plan
status: todo
tags: [my-git, task, commit, amend, changelist]
updated: 2026-07-25
---

# Task 06 — Commit by changelist (+ amend + message editor)

## Goal

Commit a selected changelist (or its marked files) by staging only those paths, entering a
message, creating the commit, and removing committed files from the list. Support amend.
Guarantee other lists — especially "Not for commit" — are never included.

## Scope

- Commit selected list or marked subset: `git add -- <files>` → `git commit -m <msg>`.
- Commit message entry: in-TUI mini-editor or `$EDITOR`; reject empty messages.
- Remove committed files from the list after a successful commit (store update).
- Amend last commit (separate key).
- Out of scope: revert/reset (Task 07).

## Subtasks

1. Commit action: resolve target files (marked subset or whole list), stage exactly those.
2. Message editor (mini-editor + `$EDITOR` fallback); block empty message.
3. Create the commit via engine; on success, remove committed files from the list and refresh.
4. Ensure files in other lists (incl. "Not for commit") are never staged.
5. Amend flow targeting HEAD (files + edited message), no new commit hash on top.

## Deliverables

- Commit-by-list flow with message entry.
- Amend flow.
- Post-commit store update removing committed files.

## Definition of Done

- [ ] Committing a list stages and commits exactly that list's files (or marked subset).
- [ ] Files in other lists, including "Not for commit", are never staged or committed.
- [ ] Empty commit message is rejected; no commit created.
- [ ] Committed files are removed from the list after commit and the view refreshes.
- [ ] Amend updates HEAD's commit (files + message) without adding a new commit on top.

## Tests

- **`commit` → "Committing Default excludes other lists" (AC#3).**
- **`commit` → "Marked-subset commit".**
- **`commit` → "Not-for-commit is immune to a Default commit".**
- **`commit` → "Empty message blocks the commit".**
- **`commit` → "Amend updates HEAD".**

## Dependencies

Task 02 (store + post-commit removal), Task 05 (marked selection), Task 01 (engine commit/amend).
