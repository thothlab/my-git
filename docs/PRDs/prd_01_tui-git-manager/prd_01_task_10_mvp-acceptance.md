---
title: "Task 10 — MVP acceptance verification (ТЗ §8)"
project: my-git
type: plan
status: todo
tags: [my-git, task, acceptance, verification, mvp]
updated: 2026-07-25
---

# Task 10 — MVP acceptance verification (ТЗ §8)

## Goal

Verify end-to-end that all seven ТЗ §8 acceptance criteria hold on a real repository, and
that the lightness gate (AC#7) is met. This is the MVP sign-off task.

## Scope

- An end-to-end integration pass over a scratch git repo covering AC#1–#6.
- The lightness measurement for AC#7.
- Cross-tool compatibility check for AC#2 against the GUI schema fixture.
- Out of scope: new features; only verification and any defect fixes surfaced.

## Subtasks

1. Scripted integration harness that provisions a scratch repo + remote and drives each AC scenario.
2. AC#1 grouped display / non-repo error. AC#2 move persists + GUI-readable. AC#3 commit-by-list isolation.
3. AC#4 push new/update + ahead/behind. AC#5 revert + reset. AC#6 rebase continue/abort.
4. AC#7 lightness: `/usr/bin/time -l` RSS + startup timing on the release binary.
5. Record results; file defects against the owning task if any AC fails.

## Deliverables

- Integration test suite / harness covering AC#1–#6.
- A recorded lightness measurement (AC#7).
- An MVP acceptance report (pass/fail per AC).

## Definition of Done

- [ ] AC#1 verified: grouped display in-repo; clear error out-of-repo.
- [ ] AC#2 verified: move persists to `.git/changelists.json`, survives restart, GUI-schema-compatible.
- [ ] AC#3 verified: commit Default; "Not for commit" untouched.
- [ ] AC#4 verified: push new `-u`; update existing; ahead/behind shown.
- [ ] AC#5 verified: revert + (confirmed) reset from mini-log.
- [ ] AC#6 verified: rebase onto with continue/abort.
- [ ] AC#7 verified: RSS single-to-tens-of-MB and effectively instant start, numbers recorded.

## Tests

Exercises every scenario referenced by AC#1–#6 across the `changelists`, `working-tree`,
`commit`, `history`, `branches`, and `rebase` specs end-to-end, plus the AC#7 lightness gate.

## Dependencies

Tasks 02–09 (all feature work complete).
