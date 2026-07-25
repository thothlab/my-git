---
title: "Spec delta — rebase (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, rebase, conflict]
updated: 2026-07-25
---

# rebase — spec delta (prd_01)

Observable behaviour of rebasing the current branch onto another and driving the
in-progress state. Seeds the domain.

## ADDED Requirements

### Requirement: Rebase current branch onto a target
The system MUST rebase the current branch onto a target branch/ref chosen by the user.

#### Scenario: Clean rebase completes (AC#6)
- GIVEN the current branch can rebase onto `main` without conflicts
- WHEN the user rebases onto `main`
- THEN the branch's commits are replayed on top of `main` and the rebase completes

### Requirement: Rebase-in-progress state with continue/abort/skip
The system MUST show that a rebase is in progress, including position as "N of M", and MUST
offer continue, abort, and skip. Abort MUST return the branch to its pre-rebase state.

#### Scenario: Abort restores pre-rebase state (AC#6)
- GIVEN a rebase onto `main` has stopped in progress
- WHEN the user aborts
- THEN the branch returns to the commit it was on before the rebase started

#### Scenario: Continue advances after resolution
- GIVEN a rebase has stopped at step "2 of 4" on a conflict that the user has resolved
- WHEN the user continues
- THEN the rebase advances past step 2 toward completion

### Requirement: Conflict listing and resolution entry
The system MUST list conflicted files during a stopped rebase and MUST let the user open a
file in `$EDITOR` or an external mergetool and mark it resolved before continuing. (MVP
guarantees continue/abort; a streamlined conflict flow is P1.)

#### Scenario: Conflicted files are shown
- GIVEN a rebase stops with a conflict in `src/main.rs`
- WHEN the in-progress state is displayed
- THEN `src/main.rs` is listed as conflicted with an action to open it for resolution
