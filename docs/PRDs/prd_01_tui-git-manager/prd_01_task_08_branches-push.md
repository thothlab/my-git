---
title: "Task 08 — branches: push new/update, fetch/pull, ahead-behind, checkout/create"
project: my-git
type: plan
status: todo
tags: [my-git, task, branch, push, fetch, upstream]
updated: 2026-07-25
---

# Task 08 — Branches (push / update / fetch / pull / ahead-behind / checkout / create)

## Goal

Implement branch operations: push a new branch with upstream, update an existing branch
(with `--force-with-lease` on divergence, force only on confirm), fetch/pull, the
ahead/behind indicator, and branch checkout/create.

## Scope

- Push new (`-u origin <branch>`) vs update existing (detected by upstream presence).
- Divergence handling: suggest `--force-with-lease`; force push only after explicit confirm.
- Fetch and pull.
- Ahead/behind indicator (status bar, fed by `branchState()`).
- Branch checkout and create-from-current.
- Out of scope: rebase (Task 09).

## Subtasks

1. Push action: branch has no upstream → `-u`; else update push.
2. Divergence path: detect diverged, warn, offer `--force-with-lease`; confirm gate for force.
3. Fetch / pull actions; refresh ahead/behind afterward.
4. Ahead/behind indicator wired into the status bar.
5. Branch picker: checkout an existing branch; create a new branch from current.

## Deliverables

- Push (new + update), fetch/pull, ahead/behind indicator, branch checkout/create.

## Definition of Done

- [ ] Pushing a branch with no upstream sets upstream (`-u`) and it then tracks the remote.
- [ ] Updating an existing branch pushes; divergence offers `--force-with-lease`, force only on confirm.
- [ ] Ahead/behind counts display and update after fetch.
- [ ] User can checkout an existing branch and create a new branch from the current one.

## Tests

- **`branches` → "First push sets upstream" (AC#4).**
- **`branches` → "Diverged branch offers force-with-lease".**
- **`branches` → "Indicator reflects divergence" (AC#4).**
- **`branches` → "Fetch updates ahead/behind".**
- **`branches` → "Create branch from current".**

## Dependencies

Task 01 (engine push/fetch/pull/branchState/checkout/create), Task 03 (status bar + branch picker + confirm dialog).
