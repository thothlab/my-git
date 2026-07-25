---
title: "Reference — gwm-cli (Rust/ratatui git TUI) reuse map"
project: my-git
type: reference
status: active
tags: [my-git, reference, rust, ratatui, gwm-cli, architecture, theme, keymap]
updated: 2026-07-25
---

# Reference — gwm-cli reuse map

Source: **https://github.com/kbrdn1/gwm-cli** — MIT, Rust, ratatui, libgit2 (git2),
109★, actively maintained (updated 2026-07-24).

## Verdict

gwm-cli is a **git worktree manager**, not a changelist tool — so **no product features
transfer** (worktrees, GitHub issue linking, JSON-RPC daemon, fleet exec, bootstrap hooks,
stack presets, AI-session detection are all out of our ТЗ scope; copying them = scope
creep). But it is the **same stack we chose** (Rust + ratatui + a git library, single
cross-platform binary) and is unusually well-architected. **Take it as an architecture
reference, and — under MIT with attribution — selectively adapt its theme and keymap
layers.** It does not change our PRD scope; it improves *how* we build several tasks.

## Concrete reuse map (gwm module → our task)

| gwm module | Pattern | Our task |
|---|---|---|
| `src/tui/theme.rs` | **Role-based theme**: semantic roles (`focus`, `accent`, `clean`, `dirty`, `staged`, `modified`, `untracked`, `muted`, `selection_bg`…) instead of hardcoded colours; 3 layers default→preset→per-role override; colour parser accepts named / 256-index / hex. | **Task 03** §7 palette — our Pane tokens (`accent`/`success`/`warn`/`danger`/`fg`/`bg`) *are* the roles; near drop-in for truecolor + 256 degradation. |
| `src/tui/keymap.rs`, `modal_keymap.rs` | **Declarative keymap**: `define_actions!` macro → `Action` enum + slug + table from one list; layers defaults→override→hardcoded escape hatch (`Ctrl+C`); chord-prefix ambiguity is a **load-time hard error** (pure state machine, no Vim timeout). | **Task 03** central key dispatch by context. |
| `src/tui/state/mod.rs` + `state/*` | **State decomposition**: App is an orchestrator; each overlay/modal is one module owning one concern with a **pure-state API** (no I/O); App owns side effects. Explicitly refactored away from a 1300-line god struct. | **Task 03/05/06/07** — structure `App` this way from day one. |
| `src/tui/state/confirm.rs` | **Destructive-confirm modal**: pure state; default focus = **Cancel** (a stray Enter cancels, not deletes); optional safety countdown before firing. | **Task 07** confirm for `reset --hard` / file rollback; **Task 08** force-push confirm. |
| `src/tui/state/create_form.rs` | Text-input form (issue/type/slug) as pure state. | **Task 05** new/rename changelist name input; **Task 06** commit message editor. |
| `src/tui/state/spinner.rs`, `async_task.rs` | Spinner + **generic off-thread spine** (coalescing + late-drop) for slow ops. | **Task 08** push/fetch "in-progress" state; **Task 09** rebase progress. |
| `src/tui/commit_graph.rs` | Commit-line rendering + cache. | **Task 07** mini-log panel. |
| `src/contract.rs` + `tests/contract_tests.rs` | **Frozen versioned contract**: `SCHEMA_VERSION` bumped only on backward-incompatible change; additive fields don't bump; `serde(deny_unknown_fields)` + **freeze tests** fail CI on shape drift. | **Task 02** — add a freeze test on `changelists.json` to guarantee GUI wire-compat (AC#2). |

## git library data point (does NOT change our decision)

gwm uses **git2/libgit2 (vendored, no shell-outs)**; we chose **gix + shell-out for
rebase** (ТЗ §2.1). gwm proves the git2 single-binary path is mature and cross-platform —
**but it does no rebase** (worktree ops are well-covered by libgit2), so it is *not*
evidence that native rebase is easy. Our hybrid stands. Keep **gix** (pure-Rust, lighter,
easier cross-compile; validated at ~1.9 MB RSS). Fallback: if gix API gaps bite during
Tasks 04–08, `git2` is the proven alternative for the simple-ops side.

## License / attribution

MIT. Pattern-level learning needs no attribution. If we adapt code substantially
(realistically: `theme.rs` role model, the `define_actions!` keymap macro), keep the MIT
copyright + permission notice in that file and note the origin. Do not vendor whole modules
we don't need.
