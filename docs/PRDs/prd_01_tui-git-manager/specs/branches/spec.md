---
title: "Spec delta — branches (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, branch, push, fetch, upstream]
updated: 2026-07-25
---

# branches — spec delta (prd_01)

Observable behaviour of push/update, fetch/pull, ahead-behind, and branch switching. Seeds
the domain.

## ADDED Requirements

### Requirement: Push a new branch with upstream
The system MUST push a branch that has no upstream by setting its upstream
(`push -u origin <branch>`), after which the branch tracks that remote ref.

#### Scenario: First push sets upstream (AC#4)
- GIVEN the current branch has no upstream
- WHEN the user pushes it
- THEN the branch is pushed with upstream set and subsequently reports a tracking remote

### Requirement: Update an existing branch
The system MUST push an existing tracked branch. When local and remote have diverged, the
system MUST suggest `--force-with-lease` and MUST perform a force push only after explicit
confirmation.

#### Scenario: Diverged branch offers force-with-lease
- GIVEN the current branch has diverged from its upstream
- WHEN the user pushes
- THEN the system warns of divergence and offers `--force-with-lease`, force-pushing only on
  explicit confirmation

### Requirement: Ahead/behind indicator
The system MUST show the current branch's ahead/behind counts relative to its upstream.

#### Scenario: Indicator reflects divergence (AC#4)
- GIVEN the branch is 2 commits ahead and 1 behind its upstream
- WHEN the status is displayed
- THEN an indicator shows ahead 2 / behind 1

### Requirement: Fetch and pull
The system MUST let the user fetch and pull to integrate incoming remote changes.

#### Scenario: Fetch updates ahead/behind
- GIVEN the upstream has new commits
- WHEN the user fetches
- THEN the behind count reflects the newly fetched commits

### Requirement: Checkout and create branches
The system MUST let the user switch to another branch and create a new branch from the
current one.

#### Scenario: Create branch from current
- GIVEN the user is on branch `main`
- WHEN the user creates branch `feature/x` from the current branch
- THEN `feature/x` exists starting at `main`'s current commit and becomes the checked-out branch
