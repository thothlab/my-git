---
title: "Task 01 note — engine boundary & lightness baseline"
project: my-git
type: report
status: active
tags: [my-git, task, engine, lightness, baseline]
updated: 2026-07-25
---

# Task 01 — engine boundary & lightness baseline

## Engine boundary (ТЗ §2.1)

`GitEngine` (`src/engine.rs`) is the single trait through which the UI reaches git.
The `gix`-backed `GixEngine` services simple ops (status, diff, stage, commit, log,
branch, push, fetch, checkout, reset); `rebase`/`merge`/conflict handling will shell out
to the system `git` CLI behind the same trait. The UI depends on observable behaviour, not
on which backend answers a call.

Status of the first slice: repository discovery + the non-repo precondition (AC#1) and the
render loop are implemented. The remaining operations return an explicit "not yet
implemented" error and are filled in over the rest of Task 01 and Tasks 04–09.

## Lightness baseline (AC#7)

Measured on the release binary (`opt-level="z"`, `lto=true`, `strip=true`), fast path
(non-repo discover + exit) via `/usr/bin/time -l`:

| Metric | Value |
|--------|-------|
| Binary size | ~965 KB |
| Peak resident set size | ~1.9 MB |
| CPU (user+sys) | ~0.00 s (work is process spawn + gix discover) |

Well within "single-to-tens-of-MB RAM, instant start." Full resident-RSS of the running
TUI is re-measured at MVP sign-off (Task 10).

## Verified so far

- AC#1 non-repo path: clear error + hint, exit code 1, no TUI opened.
- Debug + release builds compile; render loop enters/exits the alt-screen cleanly (manual).
