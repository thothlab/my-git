---
title: "Task 02 — changelists model + store + sync"
project: my-git
type: plan
status: todo
tags: [my-git, task, changelist, storage, sync]
updated: 2026-07-25
---

# Task 02 — Changelists model + store + sync

## Goal

Implement the changelist metadata layer: the in-memory model, the
`.git/changelists.json` store in the shared schema, the startup/refresh sync against real
git status, and atomic conflict-tolerant persistence. This is the product's core and the
GUI-compatibility contract.

## Scope

- `ChangelistStore` (version, activeChangelistId, ordered changelists) and `Changelist`
  (id, name, comment, isDefault, files) — serde types matching ТЗ §6.1 **exactly**.
- Load (create default store if file missing), sync (ТЗ §6.2 rules 1–4), persist
  (atomic temp+rename, last-write-wins on conflict — rules 5).
- Invariants: single non-deletable Default; one file in at most one list; unassigned →
  active/Default; unique names.
- Out of scope: UI for list operations (Task 05) and commit removal (Task 06) — expose the
  operations, wire UX later.

## Subtasks

1. Define serde models; round-trip test against the exact §6.1 JSON sample (incl. a GUI-authored fixture).
2. Implement `load()` with missing-file → fresh Default store.
3. Implement `sync(changedFiles)`: prune vanished, assign unassigned → active (or Default), enforce one-list membership.
4. Implement `persist()` atomically (temp file + rename); implement re-read + last-write-wins on write conflict.
5. Implement `create`/`rename`/`delete`/`setActive`/`move` with invariants (Default protection, unique names, delete→reassign to Default).
6. Normalise paths to repo-relative, `/`-separated on all platforms.

## Deliverables

- `changelists` module with store load/sync/persist and mutation operations.
- Test fixtures: a GUI-authored `changelists.json` and the §6.1 canonical sample.
- Unit + integration tests for sync and atomic write.

## Definition of Done

- [ ] Serde types serialize to the exact §6.1 schema (`version: 1`, field names, order).
- [ ] A GUI-authored fixture loads without loss and re-serializes byte-compatibly.
- [ ] `sync` prunes vanished files and routes unassigned changed files to active/Default.
- [ ] Default list cannot be deleted; deleting a non-default list reassigns files to Default.
- [ ] Duplicate list names are rejected on create/rename.
- [ ] `persist` never leaves a partial file (temp+rename); concurrent-write test yields valid JSON.
- [ ] Paths stored repo-relative with `/` separators on macOS/Linux/Windows.

## Tests

- **`changelists` → "Store written in the shared schema":** persist then assert JSON shape.
- **`changelists` → "GUI-written file is read without loss":** load GUI fixture → unchanged.
- **`changelists` → "First run without a store file":** no file → fresh active Default.
- **`changelists` → "Active list captures unassigned changed files".**
- **`changelists` → "A file belongs to at most one list" / "Move reassigns exclusively".**
- **`changelists` → "Default cannot be deleted" / "Files survive list deletion".**
- **`changelists` → "Vanished file is pruned".**
- **`changelists` → "Concurrent write does not corrupt the store".**
- **`changelists` → "Duplicate name rejected".**

## Dependencies

Task 01 (crate + engine `status()` for sync input).
