# Graft

**Graft** — нативное десктоп-приложение git-менеджера в стиле git-панели JetBrains/Android
Studio, с ядром в виде **именованных changelist'ов**. Делит формат
`<repo>/.git/changelists.json` с [terminal-версией](../terminal/) (byte-compat),
поэтому оба инструмента работают на одном репозитории вперемешку.

Стек: **Tauri 2** (нативное окно, Rust-бэкенд) + **SolidJS/TypeScript/Vite/Tailwind**.
git-движок — **CLI-first** (shell-out на системный `git`) за trait'ом `GitEngine`
(ТЗ §2.1). Полное ТЗ и PRD — в Obsidian vault, `Projects/my-git/gui/`.

## Требования

- Rust (собрано на 1.94), `cargo`
- Node 20+ и npm
- Tauri CLI: `cargo install tauri-cli` (или `cargo tauri` уже в PATH)
- Системный `git` в PATH
- macOS использует встроенный WKWebView; Linux — `webkit2gtk`, Windows — WebView2

## Разработка

```sh
cd gui
npm install
npm run tauri dev      # поднимает Vite и открывает нативное окно
```

Приложение открывает git-репозиторий, содержащий текущий рабочий каталог
(`git rev-parse --show-toplevel`).

> ⚠️ **Не гоняйте `npm run tauri dev` на самом репозитории `my-git`, переключая ветки
> прочь от рабочей.** Исходники приложения (`gui/`) лежат только на ветке разработки;
> `git checkout main` из приложения удалит их с диска, dev-сервер Vite потеряет
> фронтенд и окно закроется (данные при этом целы — они в коммитах ветки). Для проверки
> берите **отдельный тестовый репозиторий** или запускайте собранный бинарник (в нём
> фронтенд вшит и от файлов `gui/` при работе не зависит).

## Сборка

```sh
cd gui
npm install
npm run build                                   # фронт → dist/
cargo build --manifest-path src-tauri/Cargo.toml # бэкенд (встраивает dist/)
# или релизный бандл (.app/.dmg на macOS):
npm run tauri build
```

## Тесты бэкенда

```sh
cargo test --manifest-path gui/src-tauri/Cargo.toml
```

Тесты гоняют реальные git-операции на временных репозиториях (`tempfile` +
`git init`): синк changelist'ов и byte-compat с TUI, изоляция коммита по списку,
stage/revert по hunk'ам, push/fetch/ahead-behind на локальном bare-origin.

## Что умеет (MVP, P0 по §7 ТЗ)

- Изменения по changelist'ам: дерево, статусы, DnD-перенос, CRUD списков, откат.
- Side-by-side / unified diff со stage/revert по hunk'ам.
- Коммит по списку/выбранным файлам, amend, Commit / Commit and Push.
- Ветки: список/создание/checkout (со stash при грязном дереве), push (-u/force-with-lease),
  fetch, pull, индикатор ahead/behind.
- Тема Pane (dark/light/auto), ресинк при фокусе окна.

Лог-граф, rebase/merge, разрешение конфликтов, stash-менеджер, blame — следующие
PRD (P1/P2), в MVP не входят.
