---
title: "Task 09 — rebase onto + in-progress + continue/abort/skip + conflict flow"
project: my-git
type: plan
status: todo
tags: [my-git, task, rebase, conflict, shell-out]
updated: 2026-07-25
---

# Task 09 — Rebase (onto / in-progress / continue-abort-skip / conflict flow)

## Goal

Implement rebasing the current branch onto a target via shell-out to system `git`, surface
the in-progress state (N of M), drive continue/abort/skip, and list conflicted files with an
entry point to resolve them.

## Scope

- Rebase onto a chosen branch/ref (from branch picker or mini-log).
- In-progress rendering: "N of M" position + continue/abort/skip actions.
- Conflict listing; open a conflicted file in `$EDITOR`/mergetool; mark resolved; continue.
- MVP guarantees continue/abort; a streamlined conflict flow is P1.
- Out of scope: interactive-rebase reordering/squash UI (not in ТЗ scope).

## Subtasks

1. Rebase-onto action wired to `rebaseOnto(target)` (shell-out).
2. Detect and render rebase-in-progress state (current/total) from `branchState()`.
3. Continue / abort / skip actions (`rebaseContinue`/`rebaseAbort`/`rebaseSkip`).
4. List conflicted files; open in `$EDITOR`/mergetool; mark resolved.
5. Abort returns the branch to its pre-rebase commit.

## Deliverables

- Rebase-onto flow, in-progress UI, continue/abort/skip, conflict listing + resolution entry.

## Definition of Done

- [ ] A conflict-free rebase onto a target completes and replays commits on top.
- [ ] In-progress state shows "N of M" with continue/abort/skip available.
- [ ] Abort restores the branch to its pre-rebase state.
- [ ] Conflicted files are listed with an action to open them for resolution; marking resolved + continue advances.

## Tests

- **`rebase` → "Clean rebase completes" (AC#6).**
- **`rebase` → "Abort restores pre-rebase state" (AC#6).**
- **`rebase` → "Continue advances after resolution".**
- **`rebase` → "Conflicted files are shown".**

## Dependencies

Task 01 (shell-out rebase* + branchState rebase info), Task 03 (in-progress/conflict states + keymap).
