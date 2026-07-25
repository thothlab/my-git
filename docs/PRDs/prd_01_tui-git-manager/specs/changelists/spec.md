---
title: "Spec delta — changelists (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, changelist, storage, sync]
updated: 2026-07-25
---

# changelists — spec delta (prd_01)

Observable behaviour of the named-changelist metadata layer and its on-disk store. This
PRD seeds the domain (no prior living spec).

## ADDED Requirements

### Requirement: Changelist store location and format
The system MUST persist changelists in a single JSON document at
`<repo_root>/.git/changelists.json`, using schema `version: 1` with fields
`activeChangelistId` and an ordered `changelists` array whose items expose
`id`, `name`, `comment`, `isDefault`, and `files`. File paths in `files` MUST be
repo-root-relative and `/`-separated on every platform. The format MUST be byte-compatible
with the my-git GUI version so both tools read and write the same document.

#### Scenario: Store written in the shared schema
- GIVEN a repository with a changed file `src/main.rs` assigned to the Default list
- WHEN the store is persisted
- THEN `.git/changelists.json` contains `version: 1`, an `activeChangelistId`, and a
  `changelists` entry with `isDefault: true` whose `files` includes `src/main.rs`

#### Scenario: GUI-written file is read without loss
- GIVEN a `.git/changelists.json` produced by the GUI version with a list "Not for commit"
  containing `config/local.xml`
- WHEN the terminal app loads the store
- THEN the "Not for commit" list and its `config/local.xml` assignment are shown unchanged

### Requirement: Exactly one non-deletable Default list
The system MUST always present exactly one list with `isDefault: true`. That list MUST NOT
be deletable and MUST act as the catch-all for changed files not assigned elsewhere.

#### Scenario: Default cannot be deleted
- GIVEN the Default list exists
- WHEN the user attempts to delete the Default list
- THEN the deletion is rejected and the Default list remains

#### Scenario: First run without a store file
- GIVEN a repository with no `.git/changelists.json`
- WHEN the app starts
- THEN a store is created containing a single Default list marked active, and the app does
  not error

### Requirement: Active list captures unassigned changed files
The system MUST place any changed file not explicitly assigned to a list into the active
list identified by `activeChangelistId`, or into Default when no active list is set.

#### Scenario: New change lands in the active list
- GIVEN list "WIP" is active and file `src/ui.rs` becomes modified and is unassigned
- WHEN the working tree is synced
- THEN `src/ui.rs` appears under "WIP"

### Requirement: A file belongs to at most one list
The system MUST ensure a changed file is a member of at most one changelist. Moving a file
to another list MUST remove it from its previous list.

#### Scenario: Move reassigns exclusively
- GIVEN `config/local.xml` is in Default
- WHEN the user moves it to "Not for commit"
- THEN `config/local.xml` is listed only under "Not for commit" and no longer under Default

#### Scenario: Move persists across restart and is visible to the GUI (AC#2)
- GIVEN the user creates "Not for commit" and moves `config/local.xml` into it
- WHEN the app is restarted and the same `.git/changelists.json` is opened by the GUI version
- THEN both tools show `config/local.xml` under "Not for commit"

### Requirement: Deleting a list reassigns its files to Default
The system MUST, when a non-default list is deleted, move that list's changed files back to
the Default list rather than dropping their changes.

#### Scenario: Files survive list deletion
- GIVEN list "WIP" contains `src/ui.rs`
- WHEN the user deletes "WIP"
- THEN `src/ui.rs` appears under Default and remains a changed file

### Requirement: Startup sync against real git status
The system MUST, on startup and on refresh, recompute the working tree via git status,
remove store entries for files that are no longer changed, and assign newly changed
unassigned files to the active list (or Default).

#### Scenario: Vanished file is pruned
- GIVEN `.git/changelists.json` lists `old.txt` under Default but `old.txt` is no longer changed
- WHEN the app syncs on startup
- THEN `old.txt` is removed from the store

### Requirement: Atomic, conflict-tolerant writes
The system MUST write the store atomically (write to a temporary file, then rename) so a
concurrent GUI writer never observes a partial file. On a detected write conflict the system
MUST re-read and resolve with last-write-wins for a file's location.

#### Scenario: Concurrent write does not corrupt the store
- GIVEN the GUI version writes `changelists.json` while the terminal app also saves
- WHEN both writes complete
- THEN the file remains valid JSON parseable by both tools with no partial content

### Requirement: Unique list names
The system MUST reject creating or renaming a list to a name already used by another list.

#### Scenario: Duplicate name rejected
- GIVEN a list named "WIP" exists
- WHEN the user creates or renames another list to "WIP"
- THEN the operation is rejected and the user is informed
