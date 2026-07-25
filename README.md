**Русский** · [English](README.en.md)

# my-git

Клавиатурный инструментарий для git вокруг **именованных changelist'ов** (как git-панель в
JetBrains): изменённые файлы группируются в именованные списки, коммит делается отдельно по
каждому списку, один список можно держать как «не для коммита».

- **[`terminal/`](terminal/)** — TUI (Rust + [ratatui](https://ratatui.rs)).
  Статус: **MVP** — все критерии приёмки закрыты (группировка изменений, changelist'ы,
  коммит по списку, revert/reset, push/rebase). Лёгкий: бинарник ~1 МБ, единицы МБ RAM,
  мгновенный старт.
- **`gui/`** — десктопный GUI (планируется). Будет использовать тот же формат
  changelist'ов, что и TUI.

Оба инструмента работают на одном репозитории и делят формат
`<repo>/.git/changelists.json`, поэтому видят одни и те же списки.

## Установка — terminal (TUI)

**Быстро (без Gatekeeper / `xattr`):**

```sh
curl -fsSL https://raw.githubusercontent.com/thothlab/my-git/main/install.sh | sh
```

Ставит в `/usr/local/bin` (может запросить `sudo`). Свой путь — флагом `--dir`:

```sh
curl -fsSL https://raw.githubusercontent.com/thothlab/my-git/main/install.sh | sh -s -- --dir ~/bin
```

Установщик качает бинарник через `curl`, поэтому macOS **не** ставит карантин — команда
`xattr` не нужна. Опции: `--dir <путь>` (по умолчанию `/usr/local/bin`),
`--version vX.Y.Z` (по умолчанию последний релиз).

**Вручную:** скачайте архив со страницы **[Releases](../../releases)** под вашу платформу,
распакуйте и положите `mygit` в `PATH`. Затем запустите `mygit` внутри любого
git-репозитория.

- **macOS** (arm64 / x86_64):
  `tar -xzf mygit-*-macos-*.tar.gz && sudo mv mygit /usr/local/bin/`
  (если блокирует Gatekeeper: `xattr -d com.apple.quarantine /usr/local/bin/mygit`)
- **Linux** (x86_64 / arm64):
  `tar -xzf mygit-*-linux-*.tar.gz && sudo mv mygit /usr/local/bin/`
- **Windows** (x86_64): распакуйте и положите `mygit.exe` в `PATH`.

Готовые бинарники собирает CI (`.github/workflows/release.yml`) на каждый тег `v*`; ручной
запуск также выкладывает их как artifacts.

## Сборка из исходников

```sh
cd terminal
cargo build --release
./target/release/mygit      # запускать внутри git-репозитория
```

Нужен свежий тулчейн Rust (собрано и протестировано на 1.94).

## Списки изменений

Изменённые (отслеживаемые) файлы попадают в список **Default**. Совсем новые
(неотслеживаемые) файлы автоматически собираются в список **Unversioned Files**, который
появляется и исчезает сам. Файлы можно переносить между списками (`m`); коммит делается
отдельно по каждому списку.

## Клавиши (TUI)

`j/k` навигация · `Tab` переключить панель · `space` отметить · `n/r/d`
новый/переименовать/удалить список · `m` перенести файлы · `c` коммит · `A` amend ·
`u` откатить файл к HEAD · `P` push · `F` fetch · `B` ветки ·
`L` лог (→ `v` revert / `x` reset) · `R` rebase · `Ctrl-R` обновить · `?` помощь · `q` выход.

## Документация

PRD, living-спеки и отчёты хранятся в командном Obsidian vault в
`Projects/my-git/terminal/` (см. `terminal/docs/README.md`).
