---
title: "Task 01 — project scaffold + git-engine abstraction"
project: my-git
type: plan
status: todo
tags: [my-git, task, rust, scaffold, git-engine]
updated: 2026-07-25
---

# Task 01 — Project scaffold + git-engine abstraction

## Goal

Stand up the Rust project (binary crate, `ratatui`/`crossterm`), the git-engine trait that
abstracts simple ops (via `gix`/`git2`) from shell-out ops (rebase/merge), and the
repository precondition. Establish the lightness baseline.

## Scope

- Cargo binary crate `mygit`; minimal dependency set (`ratatui`, `crossterm`, one git lib,
  `serde`/`serde_json`, an error crate). No feature creep.
- `GitEngine` trait covering the operation surface in the PRD "API list"; a default impl
  using `gix`/`git2` for simple ops and **shell-out to system `git`** for rebase/merge/
  conflict ops (ТЗ §2.1). Engine boundary only — not the UI or command wiring.
- Repository detection + non-repo error path.
- Out of scope: any panel rendering, changelist logic, git action UX (later tasks).

## Subtasks

1. `cargo init` the binary crate; pin the minimal dependency set; set up lint/format (rustfmt, clippy) and CI-less local checks.
2. Define the `GitEngine` trait (status, diff, stage, commit, log, revert, reset,
   checkoutFile, branchState, checkout/create branch, push, fetch/pull, rebase*).
3. Implement simple ops via `gix`/`git2`; implement `rebaseOnto`/`rebase*` via shell-out to `git`.
4. Implement repository detection; on non-repo, print a clear error + hint and exit non-zero.
5. Add a smoke entrypoint that opens/closes an empty ratatui screen to prove the render loop.
6. Capture the lightness baseline (RSS + startup time).

## Deliverables

- Compiling `mygit` binary that detects a repo and renders/exits a blank ratatui screen.
- `GitEngine` trait + default implementation with the CLI/shell-out boundary documented.
- A short `docs/` note on engine boundary and the measured lightness baseline.

## Definition of Done

- [ ] `cargo build --release` succeeds; `cargo clippy` clean; `cargo fmt --check` clean.
- [ ] Running inside a git repo opens and cleanly exits the alt-screen TUI.
- [ ] Running outside a git repo prints a clear error + hint and exits non-zero (no TUI).
- [ ] `GitEngine` exposes every operation in the PRD API list; rebase/merge go through shell-out.
- [ ] Lightness gate (AC#7): release binary RSS is single-to-tens-of-MB and startup is
      effectively instant, measured with `/usr/bin/time -l ./target/release/mygit` (and a
      startup timing) — numbers recorded in the docs note.

## Tests

- **`tui-shell` → "Non-repo exits with guidance" (AC#1):** run the binary in a non-repo
  temp dir → asserts clear error + hint, non-zero exit, no TUI.
- Manual: run in a repo → blank screen opens and quits with the quit key.
- Lightness measurement recorded (AC#7 DoD gate).

## Dependencies

None (foundational).
