[Русский](README.md) · **English**

# my-git

Keyboard-driven git tooling built around **named changelists** (like the JetBrains git
panel): changed files are grouped into named lists, commits are made per list, and one list
can be held as "not for commit".

- **[`terminal/`](terminal/)** — the TUI (Rust + [ratatui](https://ratatui.rs)).
  Status: **MVP** — all acceptance criteria met (grouped changes, changelists, commit-by-list,
  revert/reset, push/rebase, rebase + force-push a branch with a conflict preflight).
  Lightweight: ~1 MB binary, single-digit-MB RAM, instant start.
- **[`gui/`](gui/)** — desktop GUI (Tauri 2 + Rust + SolidJS). Status: **MVP** —
  changelists (byte-compatible with the TUI), side-by-side/unified diff with hunk
  stage/revert, per-list commit, branches and push/pull, rollbacks, dark/light theme.
  Native and light: release binary ~2.3 MB, no JVM. See [`gui/README.md`](gui/README.md).

Both tools operate on one repository and share the same
`<repo>/.git/changelists.json` format, so they see the same lists.

## Install — terminal (TUI)

**Quick (no Gatekeeper / `xattr`):**

```sh
curl -fsSL https://raw.githubusercontent.com/thothlab/my-git/main/install.sh | sh
```

Installs into `/usr/local/bin` (may prompt for `sudo`). For a different location use `--dir`:

```sh
curl -fsSL https://raw.githubusercontent.com/thothlab/my-git/main/install.sh | sh -s -- --dir ~/bin
```

The installer downloads via `curl`, so macOS does **not** quarantine the binary — no `xattr`
needed. Options: `--dir <path>` (default `/usr/local/bin`), `--version vX.Y.Z` (default: latest).

**Manual:** download a prebuilt archive from **[Releases](../../releases)** for your platform,
extract, and put `mygit` on your `PATH`. Then run `mygit` inside any git repository.

- **macOS** (arm64 / x86_64):
  `tar -xzf mygit-*-macos-*.tar.gz && sudo mv mygit /usr/local/bin/`
  (if Gatekeeper blocks it: `xattr -d com.apple.quarantine /usr/local/bin/mygit`)
- **Linux** (x86_64 / arm64):
  `tar -xzf mygit-*-linux-*.tar.gz && sudo mv mygit /usr/local/bin/`
- **Windows** (x86_64): unzip and put `mygit.exe` on your `PATH`.

Prebuilt binaries are produced by CI (`.github/workflows/release.yml`) on every `v*` tag;
a manual run also uploads them as downloadable workflow artifacts.

## Build from source

```sh
cd terminal
cargo build --release
./target/release/mygit      # run inside a git repo
```

Requires a recent Rust toolchain (built and tested on 1.94).

## Changelists

Changed (tracked) files land in the **Default** list. Brand-new (untracked) files are
collected automatically into an **Unversioned Files** list that appears and disappears on
its own. Move files between lists with `m`; commits are made per list.

## Keys (TUI)

`j/k` navigate · `Tab` switch panel · `space` mark · `n/r/d` new/rename/delete list ·
`m` move files · `c` commit · `A` amend · `u` rollback file ·
`P` push · `F` fetch · `B` branches · `L` log (→ `v` revert / `x` reset) · `R` rebase ·
`U` rebase + push a branch · `Ctrl-R` refresh · `?` help · `q` quit.

**Rebase (`R`) with a conflict preflight and Abort.** Before rebasing, mygit
previews it (commit by commit, in-memory `git merge-tree`) and, if conflicts are
predicted, warns and asks for confirmation — without it the rebase won't start. If
the rebase does stop on a conflict, the **Continue / Skip / Abort** picker opens
right away (`Abort` cancels the rebase and restores the branch). The same picker is
available via `R` while a rebase is in progress.

### `U` — rebase a branch onto a base and force-push (like `rebase_and_push.sh`)

Pick the branch and the base (`origin/...`) and the push mode (`--force-with-lease`
by default, or `--force`). Then mygit: `fetch` → **previews the rebase** (commit by
commit, in-memory `git merge-tree` — touches nothing) and, if conflicts are
predicted, warns and asks whether to continue → stashes uncommitted changes →
switches to the branch → rebases onto the base → force-pushes → switches back →
restores the stash. If the rebase still stops on a conflict, **Continue** after
resolving by hand, or **Abort**, which rolls the whole operation back (returns to
the original branch and restores the stash).

## Docs

PRD, living specs and reports live in the team Obsidian vault under
`Projects/my-git/terminal/`.
