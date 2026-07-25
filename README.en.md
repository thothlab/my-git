[Русский](README.md) · **English**

# my-git

Keyboard-driven git tooling built around **named changelists** (like the JetBrains git
panel): changed files are grouped into named lists, commits are made per list, and one list
can be held as "not for commit".

- **[`terminal/`](terminal/)** — the TUI (Rust + [ratatui](https://ratatui.rs)).
  Status: **MVP** — all acceptance criteria met (grouped changes, changelists, commit-by-list,
  revert/reset, push/rebase). Lightweight: ~1 MB binary, single-digit-MB RAM, instant start.
- **`gui/`** — desktop GUI (planned). Will share the changelist format with the TUI.

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

## Keys (TUI)

`j/k` navigate · `Tab` switch panel · `space` mark · `n/r/d` new/rename/delete list ·
`s` set active · `m` move files · `c` commit · `A` amend · `u` rollback file ·
`P` push · `F` fetch · `B` branches · `L` log (→ `v` revert / `x` reset) · `R` rebase ·
`?` help · `q` quit.

## Docs

PRD, living specs and reports live in the team Obsidian vault under
`Projects/my-git/terminal/` (see `terminal/docs/README.md`).
