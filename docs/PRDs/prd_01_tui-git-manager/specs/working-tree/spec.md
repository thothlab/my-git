---
title: "Spec delta — working-tree (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, working-tree, status, diff]
updated: 2026-07-25
---

# working-tree — spec delta (prd_01)

Observable behaviour of the changed-files view and unified diff. Seeds the domain.

## ADDED Requirements

### Requirement: Changed files grouped by changelist
The system MUST display the current working tree's changed files grouped under their
changelist headings, each list header showing its name and file count.

#### Scenario: Grouped display in a repo (AC#1)
- GIVEN a repository with `src/main.rs` in Default and `config/local.xml` in "Not for commit"
- WHEN the app opens
- THEN the Changes view shows a "Default" heading over `src/main.rs` and a
  "Not for commit" heading over `config/local.xml`

### Requirement: Per-file status indicator
The system MUST mark each changed file with its status — Modified, Added, Deleted, Renamed,
Untracked, or Conflicted — using a status letter and a colour.

#### Scenario: Statuses are distinguishable
- GIVEN one added file, one deleted file, and one conflicted file are changed
- WHEN the Changes view renders
- THEN each shows a distinct status letter/colour identifying Added, Deleted, and Conflicted

### Requirement: Empty state when nothing is changed
The system MUST show an explicit empty state with a hint when the working tree has no
changes, rather than a blank panel.

#### Scenario: Clean tree shows guidance
- GIVEN a repository with no changed files
- WHEN the app opens
- THEN the Changes view shows an empty-state message instead of file rows

### Requirement: Unified diff of the selected file
The system MUST show a unified diff of the file selected in the Changes view. Per-hunk
partial staging is out of scope for this PRD.

#### Scenario: Selecting a file shows its diff
- GIVEN `src/main.rs` is modified and selected
- WHEN focus is on the diff panel
- THEN a unified diff of `src/main.rs` (hunk headers, `-`/`+` lines) is displayed
