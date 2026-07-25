---
title: "Spec delta — commit (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, commit, changelist, amend]
updated: 2026-07-25
---

# commit — spec delta (prd_01)

Observable behaviour of committing per changelist. Seeds the domain.

## ADDED Requirements

### Requirement: Commit a single changelist
The system MUST commit exactly the changed files of the selected changelist (or the marked
files within it) by staging only those paths and creating one commit, then MUST remove the
committed files from the list.

#### Scenario: Committing Default excludes other lists (AC#3)
- GIVEN Default contains `src/main.rs` and "Not for commit" contains `config/local.xml`
- WHEN the user commits Default with a message
- THEN a commit is created containing `src/main.rs` only, `config/local.xml` remains a
  changed file under "Not for commit", and `src/main.rs` is removed from Default

#### Scenario: Marked-subset commit
- GIVEN a list contains `a.rs` and `b.rs` and the user marks only `a.rs`
- WHEN the user commits the marked files
- THEN the commit contains `a.rs` only and `b.rs` stays changed in the list

### Requirement: "Not for commit" never commits
The system MUST NOT stage or commit files in a list designated "Not for commit" as part of
any other list's commit. "Not for commit" is an ordinary user-named list — its behaviour is
guaranteed purely by commit isolation (a commit stages only its own list's files), NOT by
any schema flag. No `isNotForCommit`-style field is added to `changelists.json`, to preserve
byte-compatibility with the GUI version (AC#2).

#### Scenario: Not-for-commit is immune to a Default commit
- GIVEN `config/local.xml` is in "Not for commit"
- WHEN any other list is committed
- THEN `config/local.xml` is neither staged nor committed

### Requirement: Commit message entry
The system MUST let the user enter a commit message before creating the commit, via an
in-TUI editor or the configured `$EDITOR`, and MUST NOT create a commit with an empty message.

#### Scenario: Empty message blocks the commit
- GIVEN the user triggers a commit and leaves the message empty
- WHEN the user confirms
- THEN no commit is created and the user is prompted for a message

### Requirement: Amend the last commit
The system SHOULD allow amending the most recent commit, incorporating the selected files
and an edited message into the existing HEAD commit rather than creating a new one.

#### Scenario: Amend updates HEAD
- GIVEN a commit exists at HEAD and `src/main.rs` is changed
- WHEN the user amends with `src/main.rs` and a revised message
- THEN HEAD's commit now includes `src/main.rs` and shows the revised message, and no new
  commit hash is added on top
