---
title: "Task 03 — TUI shell: layout, focus, status bar, keymap, help, states, theme"
project: my-git
type: plan
status: todo
tags: [my-git, task, tui, ratatui, keymap, theme, states]
updated: 2026-07-25
---

# Task 03 — TUI shell (layout / focus / status bar / keymap / help / states / theme)

## Goal

Build the application shell: full-screen panelled layout, focus switching, status bar,
central keymap dispatch, help overlay, and the named UI states. Produce the **design
deliverable** (ТЗ §7): wireframes, the Pane→ANSI/truecolor colour map, the finalised key
map, and the state specification.

**Reference:** adapt the architecture from `gwm-cli` (MIT) — role-based theme
(`tui/theme.rs`), the `define_actions!` keymap (`tui/keymap.rs`), the App-orchestrator +
pure-state overlay slices (`tui/state/*`), and the default-to-Cancel confirm modal. See
[reference note](../../notes/reference_gwm-cli.md) for the module→task map and attribution.

## Scope

- Two-panel layout (Changes | Diff/Log) + status bar; focus model with visible focus.
- Central key-dispatch that routes keys to actions per focus/context (actions themselves
  land in Tasks 04–09; this task wires the dispatch and stubs where needed).
- Help overlay grouped by context; state rendering for empty / rebase-in-progress /
  conflict / push-in-progress / detached HEAD.
- **Design deliverable (§7):** ASCII wireframes (main, changelist-picker popup, destructive
  confirm dialog, help), Pane-token→ANSI + truecolor-hex table with 256-colour degradation,
  finalised keymap grouped by context, and the state spec. Respect the terminal's default
  background; colour only content.
- Out of scope: the actual git actions behind each key (later tasks).

## Subtasks

1. Compose the ratatui layout: status bar + two panels; implement focus state + visible focus indicator.
2. Implement the status bar (branch, ahead/behind, active changelist, key hints) fed by `branchState()`.
3. Build central key dispatch mapping keys → action enum by context.
4. Implement the help overlay (grouped bindings) and its dismissal.
5. Implement state rendering: empty, rebase-in-progress, conflict, push-in-progress, detached HEAD.
6. Implement the theme: map Pane semantic tokens (accent/success/warn/danger/fg/bg) to
   truecolor with 256-colour fallback; centralise as a palette.
7. Author the design deliverable (wireframes + colour table + keymap + state spec) in `docs/`.

## Deliverables

- Running shell with two panels, focus switching, status bar, help overlay, and state rendering.
- `docs/design/` design deliverable: wireframes, Pane→ANSI/truecolor table, keymap, state spec.
- Central keymap/action dispatch module.

## Definition of Done

- [ ] Layout renders status bar + Changes + Diff/Log; focused panel is visually distinct.
- [ ] Status bar shows branch, ahead/behind, and active changelist.
- [ ] Help overlay lists bindings grouped by context and dismisses on request.
- [ ] Each named state (empty, rebase-in-progress, conflict, push-in-progress, detached HEAD) renders distinctly.
- [ ] Palette maps every Pane token to truecolor + a 256-colour fallback; terminal default background respected.
- [ ] Design deliverable committed under `docs/design/`.

## Tests

- **`tui-shell` → "Panelled full-screen layout with focus switching".**
- **`tui-shell` → "Status bar reflects context".**
- **`tui-shell` → "A key triggers its action"** (dispatch-level, e.g. mark toggle).
- **`tui-shell` → "Help lists bindings".**
- **`tui-shell` → "Detached HEAD is indicated".**
- Manual: verify colours degrade to 256-colour on a non-truecolor `TERM`.

## Dependencies

Task 01 (crate, `branchState()`). Coordinates with Task 04 for the Changes/Diff panels.
