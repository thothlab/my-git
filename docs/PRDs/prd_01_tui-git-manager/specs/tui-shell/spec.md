---
title: "Spec delta — tui-shell (prd_01)"
project: my-git
type: spec
status: active
tags: [my-git, spec, tui, ratatui, keymap, states, theme]
updated: 2026-07-25
---

# tui-shell — spec delta (prd_01)

Observable behaviour of the application shell: repository precondition, panels/focus, status
bar, keymap, help, and named UI states. Exact colours, ASCII wireframes, and the final
Pane→ANSI mapping (ТЗ §7) are a **design deliverable** of Task 03, not scenarios; only the
observable behaviours below are testable here.

## ADDED Requirements

### Requirement: Refuse to run outside a git repository
The system MUST NOT open the TUI when the working directory is not inside a git repository;
it MUST instead exit with a clear error message and a hint.

#### Scenario: Non-repo exits with guidance (AC#1)
- GIVEN the current directory is not inside a git repository
- WHEN the app is launched
- THEN it does not open the TUI and prints a clear error naming the problem with a hint

### Requirement: Panelled full-screen layout with focus switching
The system MUST present a full-screen layout with a Changes panel and a Diff/Log panel, and
MUST let the user switch focus between panels; the focused panel MUST be visually indicated.

#### Scenario: Focus switch is visible
- GIVEN focus is on the Changes panel
- WHEN the user switches focus
- THEN the Diff/Log panel becomes focused and the focused panel is visually distinguished

### Requirement: Status bar with branch and context
The system MUST show a status bar reporting the current branch, ahead/behind, the active
changelist, and key hints.

#### Scenario: Status bar reflects context
- GIVEN branch `feature/x` is active, 2 ahead / 0 behind, active list "Default"
- WHEN the app renders
- THEN the status bar shows the branch, `↑2 ↓0`, and `active: Default`

### Requirement: Keyboard map drives actions
The system MUST bind keys to the core actions (navigate, switch focus, mark, new/rename/move
list, commit, amend, rollback, push, fetch, branches, log, rebase, help, quit) so each key
triggers its action from the appropriate context.

#### Scenario: A key triggers its action
- GIVEN the Changes panel is focused with a file selected
- WHEN the user presses the "mark" key
- THEN the file's marked state toggles

### Requirement: Help overlay
The system MUST provide a help overlay listing key bindings grouped by context.

#### Scenario: Help lists bindings
- GIVEN the app is running
- WHEN the user opens help
- THEN an overlay lists available key bindings grouped by context and dismisses on request

### Requirement: Named UI states
The system MUST render distinct states for empty (no changes), rebase-in-progress, conflict,
push-in-progress, and detached HEAD, so the user can tell the current mode.

#### Scenario: Detached HEAD is indicated
- GIVEN the repository is in a detached HEAD state
- WHEN the app renders
- THEN the status area indicates detached HEAD rather than a branch name
