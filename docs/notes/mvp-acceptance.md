---
title: "MVP acceptance report (ТЗ §8 / AC#1–#7)"
project: my-git
type: report
status: done
tags: [my-git, acceptance, mvp, verification]
updated: 2026-07-25
---

# MVP acceptance — ТЗ §8

All seven acceptance criteria are met and verified. Behavioural criteria (AC#1–#6) each
have an automated test exercising **real git** on a scratch repository (no mocks); AC#7 is
a measurement on the release binary.

| AC | Criterion | Verified by | Result |
|----|-----------|-------------|--------|
| 1 | Grouped display in a repo; clear error outside a repo | `engine::tests` non-repo path + `tui::tests::renders_frame_with_panels_and_content` / `renders_empty_state_when_no_changes` | ✅ |
| 2 | Move file → persists in `.git/changelists.json`, survives restart, GUI-compatible | `changelists::tests` (schema freeze, GUI-fixture round-trip, first-run) + `tui::tests::changelist_ops_create_move_and_persist` + `engine::tests::…startup_pipeline_persists` | ✅ |
| 3 | Commit Default; "Not for commit" stays uncommitted | `tui::tests::commit_default_excludes_not_for_commit`, `commit_requires_nonempty_message` | ✅ |
| 4 | Push new `-u`; update; ahead/behind indicator | `tui::tests::push_new_branch_sets_upstream_and_create_checkout` (bare remote) | ✅ |
| 5 | Revert + (confirmed) reset from mini-log | `tui::tests::revert_and_reset_from_log`, `rollback_confirm_defaults_to_cancel` | ✅ |
| 6 | Rebase onto with continue/abort | `tui::tests::rebase_onto_completes_cleanly`, `rebase_conflict_then_abort_restores` | ✅ |
| 7 | Single-to-tens-of-MB RAM, instant start | `/usr/bin/time -l` on release binary | ✅ |

## AC#7 measurement (release binary)

| Metric | Value |
|--------|-------|
| Binary size | 1.1 MB (`opt-level="z"`, `lto`, `strip`) |
| Peak resident set size | ~1.93 MB |
| CPU (user+sys) | ~0.00 s (work is process spawn + gix discover) |
| Startup | effectively instant |

## Test suite

`cargo test` → **29 passed, 0 failed**. `cargo clippy` → clean. Real-repo integration tests
provision scratch repos (and, for AC#4, a bare remote) and drive each scenario end-to-end.

## Smoke

`printf 'q' | script -q /dev/null ./target/release/mygit` in a real repo → the full panelled
UI renders and quits cleanly (exit 0).

## Notes / deferred (as planned)

- Conflict resolution in rebase: continue/abort/skip + conflict listing are done (P0);
  opening a conflicted file in `$EDITOR`/mergetool inline is P1 (ТЗ §3.6).
- `pull` exists in the engine API; the UI wires `fetch` (safe incremental) — `pull` binding
  is a follow-up.
- Per-hunk partial staging is explicitly out of MVP scope (ТЗ §3.7, P2).
- Theme default uses named ANSI colours (honours the terminal); the truecolor Pane preset
  awaits exact hex from `pane-app` `tokens.css`.
