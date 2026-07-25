---
title: "Task 07 — mini-log + revert + reset + file rollback"
project: my-git
type: plan
status: todo
tags: [my-git, task, log, revert, reset, rollback]
updated: 2026-07-25
---

# Task 07 — Mini-log + revert + reset + file rollback

## Goal

Provide the compact commit log and the rollback actions it feeds: revert a commit, reset to
a commit (soft/mixed/hard with confirmation for hard), and roll a file back to HEAD with
confirmation.

## Scope

- Mini-log panel: recent commits (hash, summary, author, refs) of the current branch.
- Revert selected commit (inverse commit).
- Reset to selected commit in soft/mixed/hard; `--hard` requires a confirm dialog naming lost data.
- Rollback a changed file to HEAD (checkout file) with confirmation.
- Out of scope: rebase (Task 08/09 — the log is a source, rebase-onto UI is Task 09).

## Subtasks

1. Render the mini-log from `log(limit)` in the Diff/Log panel.
2. Revert action on the selected commit.
3. Reset action with mode picker (soft/mixed/hard); wire the destructive-confirm dialog for hard.
4. File-rollback action (`checkoutFile`) with confirmation; refresh working-tree view after.
5. Cancellation paths leave state unchanged.

## Deliverables

- Mini-log panel.
- Revert, reset (with hard-confirm), and file-rollback (with confirm) actions.

## Definition of Done

- [ ] Mini-log lists recent commits with hash, summary, author.
- [ ] Revert creates an inverse commit and preserves the reverted commit in history.
- [ ] Soft/mixed reset moves HEAD; hard reset requires explicit confirmation and cancel is a no-op.
- [ ] File rollback restores a file to HEAD only after confirmation; cancel keeps edits.

## Tests

- **`history` → "Log lists recent commits".**
- **`history` → "Revert adds an inverse commit" (AC#5).**
- **`history` → "Soft/mixed reset moves the branch" (AC#5).**
- **`history` → "Hard reset requires confirmation".**
- **`history` → "File rollback discards local edits after confirm" / "Cancelling rollback keeps edits".**

## Dependencies

Task 01 (engine log/revert/reset/checkoutFile), Task 03 (confirm dialog + Diff/Log panel).
