# MVP acceptance — my-git GUI (prd_01)

Статус критериев приёмки §7 ТЗ. Разделено честно: что проверено **автоматически
здесь** (тесты бэкенда на реальных git-репозиториях + сборка) и что требует
**живого прогона человеком** (интерактив GUI, замер ресурсов, реальный remote).

Прогнать тесты: `cargo test --manifest-path gui/src-tauri/Cargo.toml` (17 тестов).

| §7 | Критерий | Проверено автоматически | Требует живого прогона |
|----|----------|--------------------------|------------------------|
| 1 | Открыть репо → изменения по changelist'ам | `snapshot_reports_branch_and_file_states`; группировка в `build_state` + синк | Клик/вид дерева в окне |
| 2 | "Not for commit", DnD 2 файла, персист + рестарт | `tracked_goes_to_default...`, `delete_reassigns...`, `save_load_roundtrip...`, `byte_compat_with_tui_fixture` | Сам жест drag-and-drop |
| 3 | Коммит Default не трогает "Not for commit" | **`commit_isolates_to_given_paths` (AC#3)** ✓ | — |
| 4 | side-by-side diff, stage hunk, revert hunk | **`hunk_stage_and_revert_are_independent` (AC#4, движок)** ✓ | Рендер side-by-side, кнопки в окне |
| 5 | Ветка → коммит → push новой ветки с upstream | **`push_sets_upstream...` (AC#5, bare-origin)**, `branches_lists...`, `commit_*` ✓ | Push на реальный сервер |
| 6 | Update ветки (pull) + ahead/behind | **`push_sets_upstream_and_fetch_sees_remote_advance` (AC#6)** ✓ | Pull с реального сервера |
| 7 | Idle RAM < 250 МБ, старт < 1.5 с | Косвенно: релизный бинарник **2.3 МБ arm64**, `.app` 2.3 МБ, без JVM; WKWebView системный | **Замер RAM/старта на запущенном окне — не сделан** |

## Сборка (проверено)

- `npm run build` → фронт `dist/` (68 КБ).
- `cargo build --release` → бинарник **2.3 МБ** (LTO+strip, arm64).
- `npm run tauri build` → **`my-git GUI.app` (2.3 МБ)** и **`.dmg` (1.2 МБ)** собраны.

## Что должен прогнать человек (шаги)

1. `cd gui && npm install && npm run tauri dev` — откроется окно на репозитории
   текущего каталога.
2. **AC#1/#2:** увидеть изменения по спискам; создать "Not for commit"; перетащить
   2 файла; перезапустить (`Ctrl-R`/переоткрыть) — назначение сохранилось
   в `.git/changelists.json`.
3. **AC#4:** выбрать изменённый файл → side-by-side → Stage одного hunk'а, Revert
   другого.
4. **AC#5/#6:** создать ветку, закоммитить, Push (ставит upstream); на ветке с
   upstream — Pull, увидеть индикатор ↑/↓.
5. **AC#7:** замерить idle RAM (Activity Monitor) и время холодного старта.
6. **byte-compat GUI↔TUI:** поработать над одним репо GUI и
   [terminal](../terminal/)-версией вперемешку; убедиться, что
   `.git/changelists.json` читается обоими без потерь (схема покрыта тестом
   `byte_compat_with_tui_fixture`, но живой прогон двух приложений — за человеком).

## Известные упрощения MVP (в следующие PRD)

- Токен-подсветка синтаксиса в diff — сейчас раскраска diff-строк; token-highlight позже.
- Прогресс долгих операций — busy-бар (не потоковый прогресс push/pull).
- Частичный коммит только застейдженных hunk'ов — коммит берёт полное содержимое
  выбранных файлов (hunk-staging живёт в индексе, но commit_list коммитит файлы целиком).
- Rename в коммите: стейджится новый путь; удаление старого — общий случай, не спец-обработка.
- Picker "открыть репозиторий" — сейчас открывается репо текущего каталога.
