---
title: "Spec delta — history (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, log, revert, reset, rollback]
updated: 2026-07-25
---

# history — spec delta (prd_01)

Observable behaviour of the mini-log and commit/file rollback. Seeds the domain.

## ADDED Requirements

### Requirement: Mini-log of recent commits
The system MUST show a compact list of the current branch's recent commits, each row
exposing short hash, summary, author, and any ref labels, usable as the source for revert,
reset, and rebase-onto.

#### Scenario: Log lists recent commits
- GIVEN the current branch has at least three commits
- WHEN the user opens the mini-log
- THEN the most recent commits are listed with hash, summary, and author

### Requirement: Revert a commit
The system MUST create an inverse commit for a commit selected in the mini-log, leaving
prior history intact.

#### Scenario: Revert adds an inverse commit (AC#5)
- GIVEN a commit `C` is selected in the mini-log
- WHEN the user reverts it
- THEN a new commit is created on top that undoes `C`'s changes and `C` still exists in history

### Requirement: Reset to a commit with mode choice
The system MUST reset the current branch to a commit selected in the mini-log in one of
soft, mixed, or hard modes. A hard reset MUST require explicit confirmation with a warning
that names the data that will be lost.

#### Scenario: Soft/mixed reset moves the branch (AC#5)
- GIVEN commit `C` is selected and the user chooses a mixed reset
- WHEN the reset runs
- THEN the branch HEAD points at `C` and later changes appear as uncommitted changes

#### Scenario: Hard reset requires confirmation
- GIVEN the user chooses `reset --hard` to an earlier commit
- WHEN the confirmation dialog appears
- THEN the reset proceeds only after explicit confirmation, and cancelling leaves the branch
  and working tree unchanged

### Requirement: Roll a file back to HEAD
The system MUST restore a selected changed file to its HEAD content on request, after an
explicit confirmation because local edits are discarded.

#### Scenario: File rollback discards local edits after confirm
- GIVEN `src/main.rs` has uncommitted edits
- WHEN the user rolls it back to HEAD and confirms
- THEN `src/main.rs` matches HEAD and is no longer listed as changed

#### Scenario: Cancelling rollback keeps edits
- GIVEN `src/main.rs` has uncommitted edits
- WHEN the user starts a rollback and cancels at the confirmation
- THEN `src/main.rs` keeps its uncommitted edits
