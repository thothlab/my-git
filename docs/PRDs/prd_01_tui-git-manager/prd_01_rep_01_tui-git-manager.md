---
title: "Planning report — prd_01 TUI git manager"
project: my-git
type: report
status: done
tags: [my-git, report, prd, planning, terminal, tui]
updated: 2026-07-25
---

# prd_01 — Planning report

## What was done

Converted `Projects/my-git/terminal/Техническое задание.md` into a full PRD delivery
package: a scratchpad analysis, one PRD with the ТЗ feature list scoped exactly (no
expansion toward the GUI version), seven delta specs seeding new domains, and ten task files
each tied to spec scenarios. The GUI ТЗ's §6 `changelists.json` contract was cross-checked —
it is identical to the terminal ТЗ's, and the shared-format compatibility (AC#2) is captured
as an explicit spec scenario. Steps 1–7 of the PRD workflow are complete; task execution and
archive (Step 8) are later, user-triggered phases.

## Files produced

- `prd_01_scratch.md` — problem, scope, domain model, sync rules, AC map, affected domains.
- `prd_01_tui-git-manager.md` — PRD (objective, non-objectives, data model, API list,
  validation & state transitions, risks, acceptance criteria).
- Delta specs (`specs/<domain>/spec.md`): `changelists`, `working-tree`, `commit`,
  `history`, `branches`, `rebase`, `tui-shell`.
- Tasks:
  - `task_01_scaffold-engine` — crate + `GitEngine` trait (gix/git2 + shell-out) + repo precondition + lightness baseline.
  - `task_02_changelists-store` — model + `.git/changelists.json` (shared schema) + sync + atomic write.
  - `task_03_tui-shell` — layout/focus/status bar/keymap/help/states + design deliverable (§7 theme/wireframes).
  - `task_04_working-tree-view` — grouped changes + status indicators + unified diff + empty state.
  - `task_05_changelist-ops` — create/rename/delete, multi-select, move, active list.
  - `task_06_commit-by-list` — commit-by-list + isolation + amend + message editor.
  - `task_07_log-revert-reset` — mini-log + revert + reset (hard-confirm) + file rollback.
  - `task_08_branches-push` — push new/update, force-with-lease, fetch/pull, ahead/behind, checkout/create.
  - `task_09_rebase` — rebase onto + in-progress + continue/abort/skip + conflict flow.
  - `task_10_mvp-acceptance` — end-to-end verification of AC#1–#7.
- `prd_01_rep_01_tui-git-manager.md` — this report.

## Affected domains

Seven living specs will be seeded at archive time (all new): `changelists`, `working-tree`,
`commit`, `history`, `branches`, `rebase`, `tui-shell`.

## Deviations

- **Engine abstraction (ТЗ §2.1)** and **theme/wireframes (ТЗ §7)** were deliberately kept
  out of behavioural specs (they are implementation/design), placed in Task 01 and Task 03
  respectively per the spec quality gate. Only observable UI behaviour became scenarios.
- **AC#7 (lightness)** is a DoD performance gate on Tasks 01 and 10, not a Given/When/Then
  scenario, since RAM/startup is a measurement not a behaviour.
- **CLI subcommands (ТЗ §1.1)** are documented as an optional secondary layer in the PRD;
  the TUI is the core. No separate task — additive, low-risk.
- Scope of this invocation is the planning package only (Steps 1–7). No Rust code written;
  implementation is the next, user-triggered phase.

## Next step

Begin **Task 01 — project scaffold + git-engine abstraction** (foundational; unblocks
Tasks 02–09). It establishes the crate, the `GitEngine` trait boundary (gix/git2 vs
shell-out), the non-repo precondition (AC#1), and the lightness baseline (AC#7).
