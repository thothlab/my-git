<!-- prd:start -->
# my-git / Graft

Клавиатурный инструментарий для git вокруг именованных changelist'ов. Два независимых
инструмента в одном репозитории, для одного и того же пользователя — разработчика, который
не хочет уходить из клавиатуры в мышь:

- `terminal/` — TUI `mygit` (Rust + ratatui + crossterm, движок на `gix`);
- `gui/` — десктопное приложение Graft (Tauri 2 + Rust + SolidJS 1.9 + TypeScript 5.6 +
  Tailwind 3 + Vite 6), движок — шелл-аут в системный `git`.

Общее у них одно: формат `<repo>/.git/changelists.json`. Он байт-совместим, совместимость
держится тестом `byte_compat_with_tui_fixture` в `gui/src-tauri/src/changelists.rs`.
Кода они не делят — это две отдельные реализации.

## Команды

Репозиторий — один cargo-workspace (корневой `Cargo.toml`, члены `terminal` и
`gui/src-tauri`). Cargo-команды работают из корня без `--manifest-path`, `target/` и
`Cargo.lock` — общие, корневые. Профиль `[profile.release]` задаётся **только** в корневом
манифесте: в манифестах членов cargo его игнорирует.

| Команда | Что делает |
|---------|------------|
| `cd gui && npm install` | Зависимости GUI |
| `cd gui && npm run tauri dev` | Запустить Graft локально (нужен дисплей) |
| `cd gui && npm run build` | Сборка фронта (vite, ~1 с) |
| `cd gui && npx tsc --noEmit` | Проверка типов |
| `cd gui && node scripts/check-log-filters.mjs` | Харнесс чистых функций (фильтры лога, `pathTree`, `editRules`, `lineSelection`, `blameRules`, `conflictRules`, `rebaseRules`, `bisectMarks`, разбор и печать команды консоли), 309 утверждений |
| `cargo test` | Оба крейта разом: 332 теста GUI + 73 TUI |
| `cargo test -p graft` | Только Rust-сторона GUI, 332 теста |
| `cargo test -p mygit` | Только тесты TUI, 73 теста |
| `cargo build -p mygit --release` | Собрать TUI (`target/release/mygit`) |
| `cargo clean` | Один общий `target/` на оба крейта |

**`cargo test` из корня требует собранного `gui/dist`:** `tauri-build` проверяет
`frontendDist` из `tauri.conf.json`, и без `cd gui && npm run build` крейт `graft` не
соберётся. Перед коммитом в `gui/` зелёными должны быть `npm run build`, `npx tsc --noEmit`
и `cargo test`. Запускать по очереди: `cargo` и `vite` на одном дереве дерутся за
блокировки, а `target/` теперь один на оба крейта.

CI (`.github/workflows/ci.yml`) гоняет только `terminal` (через `-p mygit`, чтобы не тянуть
Tauri) — fmt / clippy / test. GUI в CI не
проверяется ничем, локальный прогон — единственные ворота. Релизы: тег `v*` собирает TUI
(`release.yml`), тег `gui-v*` — инсталляторы Graft (`release-gui.yml`), версию бампить
одновременно в **пяти** местах, а не в трёх: `gui/package.json`,
`gui/src-tauri/Cargo.toml`, `gui/src-tauri/tauri.conf.json` — и следом два файла блокировок,
`gui/package-lock.json` (два вхождения) и `Cargo.lock` (пакет `graft`). Локи перепишут
`npm install --package-lock-only` и `cargo build`, но если оставить их отставшими, релизная
сборка стартует с грязным деревом. Комментарий в `.github/workflows/release-gui.yml` до сих
пор называет только первые три — не он источник правды.

## Карта каталогов

```
terminal/src/       TUI: engine.rs (gix), changelists.rs, tui/ (mod.rs — экран и цикл, logview.rs, keymap.rs, theme.rs, ui.rs)
gui/src-tauri/src/  бэк GUI
  commands.rs       все #[tauri::command]; AppState держит корень открытого репозитория
  model.rs          типы границы Tauri (serde camelCase)
  error.rs          Error и его сериализация
  changelists.rs    хранилище .git/changelists.json
  uistate.rs        хранилище .git/graft-ui.json
  watch.rs          наблюдатель за git-dir (крейт notify): событие repo-external-change
  engine/exec.rs    единственный запуск процесса git, журнал команд, маскировка учётных данных
  engine/cli.rs     CliEngine — snapshot, диффы, стейджинг, ветки, push/pull + общие парсеры
  engine/log.rs     история и раскладка лейнов графа
  engine/file_history.rs история одного файла (`log --follow`), закреплённая на коммите
  engine/blame.rs   blame файла (`--line-porcelain`) в ревизии или рабочем дереве, «blame до изменения»
  engine/commit.rs  один коммит и сравнение двух ревизий
  engine/branches.rs дерево веток и операции над ветками
  engine/ops.rs     операции, переписывающие историю, и распознавание незавершённой
  engine/bisect.rs  git bisect: чтение BISECT_START / BISECT_TERMS / BISECT_LOG / refs/bisect,
                    старт, ответ good/bad/skip, reset
  engine/conflict.rs конфликтные файлы: стороны из индекса (стадии 1/2/3), вид конфликта,
                    запись разрешения + `git add`, сторона целиком / удаление (`git rm`)
  engine/discard.rs резервная копия перед откатом (refs/graft/discard) и её восстановление
  engine/patch.rs   патч по выбранным строкам диффа (чистый, без git) + единая нумерация строк хунков
  engine/rebase.rs  интерактивный rebase по утверждённому плану, reword, squash; файлы плана
                    в каталоге данных приложения
  engine/undo.rs    Undo / Redo своих действий: снимок + отпечаток до и после каждой мутации,
                    цепочка шагов на репозиторий (память + каталог данных приложения)
gui/src/            фронт
  api.ts            зеркала всех команд в camelCase + типы
  store.ts          глобальное состояние окна, run(), модалки
  logStore.ts       состояние панели лога
  repoWatch.ts      когда отвечать на repo-external-change: refreshKeepingError() + дерево веток
  hotkeys.ts        клавиатурный слой
  i18n.ts           словари en/ru
  components/       Changes-режим (ChangesView, DiffView, CommitPanel, Toolbar, ...);
                    DiscardPanel — runDiscard, уведомление «Откатано N · Вернуть», диалог копий;
                    FileHistoryPanel — оверлей «История файла» (список коммитов + DiffView)
                    UndoButtons — Undo/Redo в тулбаре и Cmd/Ctrl+Z, Cmd/Ctrl+Shift+Z
  components/blame/ BlamePanel — оверлей blame (строки + коммит строки + DiffView, стек
                    «blame до изменения»); blameRules.ts — чистые правила, без единого импорта
  components/conflicts/ ConflictPanel — оверлей редактора конфликта (блоки ours · base ·
                    theirs + редактируемый результат, свой undo); conflictRules.ts — разбор
                    маркеров, сборка результата из решений, история undo, без единого импорта
  components/rebase/ RebasePanel — оверлей плана интерактивного rebase (строки с действием,
                    перестановка, поля сообщений, предпросмотр); rebaseRules.ts — цепочки,
                    где поле сообщения, что уходит на бэк, предпросмотр, `squashRun` по
                    первым родителям, без единого импорта
  components/log/   панель Git: BranchTree, LogTable, LogGraph, CommitDetailsPane, FilterBar, LogView, PanelChrome + чистые модули;
                    bisectMarks.ts — метка bisect у строки лога и фаза поиска, без единого импорта
  components/log/actions/  действия над коммитами и ветками, контекстное меню, диалоги;
                    operation.ts — `continueOperation`, `operationWord` (полоса операции и
                    редактор конфликта зовут одно и то же)
  components/diff/  DiffPanel — обёртка DiffView под панель лога; model.ts — раскладка diff;
                    editRules.ts — чистые правила правки (доступность, редьюсер черновика,
                    замеры текста), без единого импорта; editState.ts — черновик, отложенная
                    запись, отпечаток, диалог при внешнем изменении; lineSelection.ts —
                    чистые правила выбора строк (щелчок, диапазон, шаг, привязка к digest)
gui/scripts/        check-log-filters.mjs
```

## Архитектура GUI и границы модулей

Окно одно, режима два: `changes` и `log` (`store.viewMode`, `Cmd/Ctrl+1` / `Cmd/Ctrl+2`).
Переключение размонтирует панели, состояние живёт в модульных сигналах и переживает это.
История не читается, пока пользователь не открыл режим Log.

Внешние изменения (коммит, checkout, fetch, stash из терминала) окно видит двумя путями:
наблюдатель за git-dir (`watch.rs` → событие `repo-external-change` → `repoWatch.ts`:
`refreshKeepingError()` + `afterRepoChange({ log: false })`) и `refresh()` на возврате фокуса
(`App.tsx`). Фокус
остаётся единственным, кто видит внешний `git add`: `index` наблюдатель не слушает.

### Rust

Git вызывается только как внешний процесс. `gix` / `git2` в `gui/` не заводить — запрет
зафиксирован комментарием в `gui/src-tauri/Cargo.toml`; в `terminal/` `gix` наоборот основной
движок, это не общий запрет на репозиторий.

**`tauri-plugin-fs` тоже не заводить.** Чтение и запись файла рабочего дерева живут в
`engine::cli` на голом `std::fs` — так решено prd_03: плагин стоил бы крейта и записи в
`capabilities/default.json`, а запись файла вынес бы из движка, которому она по правилам
этого репозитория принадлежит. Запрет касается движка git, а не файловой системы: `std::fs`
в `engine::cli` был и до того (`metadata`, `remove_file` в `rollback`). Так же на `std::fs`
`engine::discard` читает файлы для копии и пишет их обратно при восстановлении.

Кто чем владеет:

- `engine::exec` — **единственное место, где стартует процесс git**: `git(dir, args)` →
  `.env()` / `.env_remove()` / `.input(bytes)` → `.run() -> Output` (сырые байты, код выхода,
  id записи журнала; ненулевой код здесь не ошибка). Что считать отказом, решает вызывающая
  обёртка: `Output::checked` (stderr — конвенция cli/commit/log), `checked_both` /
  `fail_both` (оба потока — branches/ops), трёхзначный код (`show-ref`,
  `merge-base --is-ancestor`) разбирается на месте. Плюс журнал (`journal_list`,
  `journal_output`; два кольца на процесс — `USER_CAP = 1000` действий пользователя и
  `BACKGROUND_CAP = 2000` фоновых чтений, чтобы частый фон не вытеснял «Мои»; каждый поток
  обрезан до `STREAM_CAP = 256 KiB` у действий пользователя и упавших фоновых запусков и до
  `QUIET_STREAM_CAP = 16 KiB` у успешных фоновых), `as_user(action, || …)` и
  `mask_credentials`. `as_user` ведёт ещё и `OWN_ACTIONS: Activity` — счётчик действий на
  весь процесс и время конца последнего (`within(grace)`): thread-local `ACTION` отвечает
  только своему потоку, а наблюдатель за git-dir живёт на своём.
- `engine::cli` — рабочее дерево, стейджинг, diff файла, коммит, базовые операции с ветками,
  push/fetch/pull. Плюс чтение и запись текстового файла рабочего дерева:
  `read_text_file(rel) -> TextFile`, `write_text_file(rel, text, eol, expect) -> новый
  отпечаток`, `EDIT_SIZE_CEILING = 2 MiB` (потолок ремесла: textarea в webview выше него не
  успевает, и каждая автозапись гоняет весь текст через границу Tauri). Коммит списка —
  `commit_paths(paths, message, amend)`: «индекс главнее», собирается во временном индексе
  (подробно — в «Что уже кусало»). Плюс `TempIndex` — единственный временный индекс крейта
  (им же пользуется `engine::discard`). Построчные действия: `diff_file` отдаёт `digest`,
  `selection_patch(path, against, picks, digest, context, reverse)` перечитывает дифф
  (приватный `raw_diff` — тот же, что рисуется), сверяет отпечаток и строит патч через
  `engine::patch`; `apply_patch(&[u8], cached, reverse)` его прикладывает. `raw_diff`
  прибит флагами `--no-ext-diff --no-textconv --no-color --src-prefix=a/ --dst-prefix=b/`:
  внешний и textconv-дифф не применяются, `diff.noprefix` сдвинул бы путь под `-p1`. Плюс общие
  `pub(crate)`-функции: `parse_diff`, `parse_refs`, `whitespace_args`, `context_arg`,
  `git_paths`, `user_email`, `fnv1a`, `literal`, `check_branch_name`, `check_tag_name`
  (новое имя ветки или тега проверяется ими **до** мутации и отвергается `Error::Rule`:
  `check-ref-format --branch` / `refs/tags/<имя>`, ведущий `-`, голый `@`, раскрытие
  `@{-1}`) и метод `CliEngine::worktree_path` (резолв пути клиента с проверкой симлинков —
  им же `engine::conflict` читает файл для сверки отпечатка). **Своих копий не писать:** разбор diff, словарь режимов
  пробелов, разбор меток `%D`, резолв путей внутри git-dir и отпечаток FNV-1a живут здесь по
  одному разу — тем же `fnv1a` `engine::log` отпечатывает аргументы фильтра, и вторая копия
  цикла была бы вторым шансом перепутать константы.
- `engine::log` — `page(&Path, &LogFilter, Option<&LogCursor>, u32)`, `authors(&Path)`.
  Прячет формат `git log`, устройство курсора и потоковый алгоритм лейнов. Плюс
  `pub(crate)` `remotes` и `short` — ими же пользуется `engine::file_history`.
- `engine::file_history` — `page(&Path, path, rev, Option<&FileHistoryCursor>, u32)`:
  `log --follow -M --name-status -z` по одному пути (`literal()`), строка — поля строки лога
  без графа плюс `path` / `old_path` / `status` файла **в этом коммите**. Разбор статуса —
  общий `commit::parse_name_status`; запись не с одним файлом — `Error::Parse`. Курсор —
  `{ skip, anchor: "@<хэш>:<fnv1a(path)>" }`: первая страница резолвит `rev` (нет — `HEAD`)
  в хэш, дальше обход всегда от него (почему не `--skip` — в «Что уже кусало»). Пустая
  история (файл не в коммитах, нерождённый `HEAD`) — пустая страница; `rev`, не называющий
  коммит, — `Error::Rule`.
- `engine::blame` — `file(&Path, rev: Option<&str>, path) -> Blame` (`None` — рабочее
  дерево, незакоммиченные строки под нулевым хэшем, `uncommitted: true`),
  `before(&Path, hash, path, line, prev_hash, prev_path) -> BlameBefore` и чистый
  `pub(crate) map_line_back(&[patch::HunkText], line)`. Строки ссылаются на таблицу
  `origins` по индексу (коммит один раз, с `parents` из одного `rev-list --stdin`,
  `previous`, `boundary`). Непригодный файл — `blocked` (`binary`, `too-large`, `missing`,
  `untracked`), как у `TextFile`, и судится **до** blame по блобу / файлу на диске:
  `BLAME_SIZE_CEILING = 4 MiB`, `BLAME_LINE_CEILING = 50 000`. Ревизия резолвится общим
  `file_history::resolve` (`pub(crate)`, своей копии не писать). «Blame до изменения»
  отображает `orig_line` через дифф блоба против блоба (`-U0`), а не через дифф коммита
  с первым родителем: `previous` у merge может быть не первым родителем.
- `engine::commit` — `details`, `files`, `file_diff` (с `old_path: Option<&str>` —
  источник переименования, если вызывающий его знает; `None` — поиск в `files`),
  `compare`, `compare_diff`,
  `unreachable_from_head`; общий `pub(crate) parse_name_status` (разбор `--name-status -z`,
  им же `commit_paths` читает, что подготовлено у списка — своей копии не писать). Всё
  сравнивается с первым родителем; корневой коммит читается через `diff-tree --root`.
- `engine::branches` — `tree`, `rename`, `delete`, `merge`, `rebase_onto`, `unmerged_count`,
  `update_from_upstream`, константа `DETACHED_REF = "HEAD"`.
- `engine::ops` — `detect_state`, `detect_kind`, `revert`, `reset`, `cherry_pick`, `checkout_rev`,
  `tag_create`, `op_continue`, `op_abort`, `op_skip`, `stash_list_app`, `stash_restore`,
  `stash_list`, `stash_apply`, `stash_pop`, `stash_drop`, `stash_files`, `stash_push`,
  `contains_commit`, `commits_after`, `has_local_changes`, `reset_mode_flag`, константа
  `APP_STASH_TAG`. Список конфликтов `detect_state` берёт у `engine::conflict::list`.
  `op_continue` / `op_skip` / `op_abort` принимают вторым параметром каталог данных
  приложения (`Option<&Path>`): идущему rebase из плана Graft `drive` ставит редакторы плана
  вместо `GIT_EDITOR=true`, после — `rebase::sweep`. `OperationState.edit_stop` — хэш
  коммита, на котором rebase встал по `edit` (последняя команда `rebase-merge/done`, не
  маркер `amend`: его git пишет и для упавшего squash). `detect_kind` — вид по одним
  маркерам, без чтения файлов состояния: им пользуются снимки Undo, драйверы `op_*` и
  `rebase::ensure_calm`. Приоритет вида: merge > rebase > cherry-pick > revert > **bisect**;
  `OperationState.bisect` заполняется при наличии `BISECT_START` независимо от `kind`
  (cherry-pick, вставший на конфликте посреди bisect, — это `kind: cherryPick` плюс поиск).
  Для bisect: `op_abort` = `git bisect reset`, `op_skip` = `git bisect skip`, `op_continue` —
  `Error::Rule` (продолжать нечего, нужен ответ).
- `engine::bisect` — `active`, `read` (строго), `state` (для `detect_state`), `start(&Path,
  bad: Option, good: &[String])`, `mark(&Path, "bad"|"good"|"skip", hash: Option)`, `reset`,
  `pub(crate) parse_log`. Состояние — только из файлов git (`BISECT_START`, `BISECT_TERMS`,
  `BISECT_LOG`, `BISECT_HEAD` через `git_paths`) и `refs/bisect/*`, никогда из фраз `git
  bisect`. Списки good/bad/skip — из ссылок (по ним работает алгоритм git), ответ поиска
  (`# first <bad> commit: [oid] subject`) и кандидаты при одних пропущенных (`# possible
  first …`) — из лога; следующая отметка в логе «переоткрывает» поиск. Оценка шагов —
  `rev-list --bisect-vars` (те же числа, что печатает git; пропуски не вычитаются, как и у
  него). Роли `bad`/`good` на входе `mark` пишутся словами репозитория (`--term-new/--term-old`).
- `engine::rebase` — `range(&Path, hash) -> RebaseRange` (от коммита **включительно** до
  HEAD, старые первыми; `blocked`: `notOnBranch` | `merge` | `tooMany`; `dirty` — только
  отслеживаемые; `published` — сколько из них уже в `@{upstream}`), `start(repo, data_dir,
  hash, steps)`, `reword(repo, data_dir, hash, message)` (HEAD — `commit --amend --only`,
  глубже — план с одним `reword`), `squash(repo, data_dir, hashes, message)` (старший —
  `merge-base --octopus`, остальные `fixup`), `sweep(data_dir, repo)`, `pub(crate)`
  `compile` (план → todo + сообщения по хэшам), `comment_char`, `resume`, `Plan::env`.
  Константы `MAX_STEPS = 1000`, `MAX_MESSAGE = 100_000`. Как устроено — докблок модуля.
- `engine::conflict` — `list(&Path) -> Vec<ConflictEntry>` (все unmerged-пути с видом),
  `read(&Path, path) -> ConflictFile` (стороны из индекса, рабочий файл через
  `read_text_file`, `conflict-marker-size`, `wholeOnly`), `resolve(&Path, path, text, eol,
  expect)` (текст — через `write_text_file` со `Stale`, потом `add -A -- :(literal)`),
  `take(&Path, path, "ours"|"theirs")` (`checkout --ours|--theirs` + `add`; стороны нет —
  `git rm`), `pub(crate) kind_of` — вид по набору стадий, таблица самого git (1 DD, 2 AU,
  3 UD, 4 UA, 5 DU, 6 AA, 7 UU). Маркеры **не** разбирает: парсер живёт на клиенте
  (`conflictRules.ts`), потому что редактор перечитывает результат на каждое нажатие.
  Стороны читаются по object id (`cat-file blob <oid>`), не как `:N:path`: это синтаксис
  ревизии, `literal()` там не место, а `git show` запустил бы textconv.
- `engine::patch` — чистый (без git, по байтам) `hunks(raw)` и `build(raw, picks, reverse)`.
  `hunks` — **единственная нумерация строк хунков**: `parse_diff` строит `Hunk.lines` из неё
  же, иначе индекс выбора указал бы на соседнюю строку. Правило зеркальное: stage (дифф
  worktree↔index, `apply --cached`) — невыбранное удаление в контекст, невыбранное добавление
  выбросить; unstage (`diff --cached`, `apply --cached -R`) и revert (дифф worktree↔index,
  `apply -R` по рабочему дереву) — наоборот. Сопоставляемая сторона выходит ровно как её
  напечатал git (счётчики сверяются с заголовком — несовпадение `Error::Parse`), пишущая
  сдвигается на накопленную дельту. Новый файл остаётся созданием, только если выбрано всё,
  удаление — так же; иначе это правка существующего. `diff --cc` (конфликт) рисуется, но
  `build` отказывает `Error::Rule`.
- `engine::discard` — `with_backup(repo, kind, paths, discard)`, `list`, `stale_paths`,
  `restore(repo, id, force)`, `patch_paths` (пути патча по `apply --numstat -z`), константы
  `DISCARD_REF = "refs/graft/discard"`, `CHAIN_LIMIT = 200`. **Любое действие, уничтожающее
  содержимое рабочего дерева, оборачивается в `with_backup`**: снимок до, действие, снимок
  после. Не удался снимок — действие не выполняется. Один откат — два коммита (`before`,
  `after`, родитель `after` — всегда его `before`); решение начать цепочку заново
  принимается только для `before`. Машиночитаемы только трейлеры `Graft-Discard` /
  `Graft-Kind`; список путей — объединение деревьев пары (`diff-tree before after`), тело
  сообщения — для людей. Байты пишутся и читаются как на диске (`hash-object
  --no-filters` + своя запись tmp + rename, не `git restore`): смешать фильтрованный хэш
  с нефильтрованной записью значит испортить CRLF/LFS-файлы. Дерево собирается во
  временном индексе (`cli::TempIndex`: `GIT_INDEX_FILE`, путь через `git_paths`, файла до
  первого вызова быть не должно — нулевой файл git считает битым индексом). Восстановление трогает только
  рабочее дерево; изменённые после отката пути — `Error::Stale`, с `force` — сначала копия
  текущего (каждое восстановление само записывается как `kind: restore`).
- `engine::undo` — `Undo` (`perform`, `state`, `step`), `Hint { args, lists }`, `DEPTH = 100`.
  **Каждая мутация команды Tauri идёт через `commands::undoable(&state, "<имя команды>",
  hint, || …)`, а не голый `exec::as_user`** — он внутри: действие, которого журнал не видел,
  для него «изменение вне Graft» и рвёт цепочку зря. `perform` снимает `Snapshot` до и после
  (`status --porcelain=v2 -uall`, свои ссылки без `refs/remotes/`, `refs/graft/`, `refs/stash`,
  `ls-files --stage`, список стешей, вид операции, байты перечисленных статусом файлов через
  `discard::describe`; отпечаток — `fnv1a`) и классифицирует по имени команды: шаг с обратной
  (`Soft` — ссылки + записи индекса, дерево не тронуто: коммит в т.ч. changelist'а, amend и
  первый, reword HEAD, reset soft/mixed, stage/unstage строк; `Hard` — `reset --hard`, только
  без отслеживаемых изменений до и после: merge, cherry-pick, revert, reset hard; `Refs` —
  checkout + ссылки: переключение, создание/удаление ветки (с upstream), тег, переключение со
  стешем; `Rename`; `StashPush` / `StashRestore` / `StashDrop` — только верхний стеш и
  восстановление только на чистое дерево; `Discard` — `discard::restore` копии отката и
  копии самого restore, плюс записи индекса), «ничего» (отпечаток не изменился) или разрыв
  цепочки с `UndoReasonCode` (push, pull, fetch с новыми тегами, rebase/squash/reword старого,
  консоль, незавершённая операция, упавшее действие, bisect — свой код `bisect` для
  `op_bisect_*` и для любого действия, до или после которого шёл bisect; проверяется раньше
  общего `operation`). Undo и Redo выполняются, только если
  текущий отпечаток равен ожидаемому, и только для шага с тем `id`, что показали клиенту.
  Два действия разом над одним репозиторием — второе не записывается, цепочка рвётся
  (`concurrent`). Своего «второго механизма» отката файлов нет: Undo отката — это копия
  `refs/graft/discard`.
- `uistate` — `get`, `set`, `state_path`. Атомарная запись через уникальный tmp + rename.
- `watch` — `start(repo, own, report) -> RepoWatcher` (drop — остановка), `GitDirs::resolve`
  (git-dir и common-dir через `git_paths`, канонизированные), чистые `relevant` /
  `relevant_common` (фильтр путей), константы `DEBOUNCE = 300 мс`, `MAX_BATCH = 2 с`,
  `OWN_GRACE = 1 с`. Крейт `notify` 8, пауза своя, а не `notify-debouncer-*`: каждое сырое
  событие помечается «своё / чужое» в момент прихода. Сам git на событиях не запускает —
  один `rev-parse` при создании. Слушается allowlist от корня git-dir, по компонентам:
  `HEAD`, `ORIG_HEAD`, `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_START`,
  `BISECT_LOG`, `BISECT_TERMS`, `BISECT_HEAD`, `packed-refs`,
  `refs/`, `logs/HEAD`, `logs/refs/`, `rebase-merge/`, `rebase-apply/`, `sequencer/`; явно
  **нет**: `index`, `FETCH_HEAD`, `*.lock`, `refs/graft/` и `logs/refs/graft/`,
  `changelists.json*`, `graft-ui.json*`. В common-dir linked worktree — только общее (`refs/`,
  `packed-refs`, `logs/refs/`), `worktrees/<чужой>/` — никогда. Свои действия Graft
  подавляет `exec::OWN_ACTIONS` (счётчик областей `as_user` на процесс + `OWN_GRACE` после
  конца).

Соглашения слоя:

- **git запускается только через `engine::exec`.** `Command::new("git")` вне `exec.rs`
  допустим лишь в тестовом коде. Своя обёртка в модуле — тонкий вызов `exec::git(...)`,
  сохраняющий свою семантику ошибок; env (`GIT_EDITOR` и прочее) задаётся на месте вызова и
  не унифицируется: `exec_raw` консоли убирает ASKPASS, а push/pull движка — нет.
- **Происхождение записи журнала объявляется, а не угадывается.** Команда Tauri, которая
  меняет репозиторий по воле пользователя, оборачивает вызов движка в
  `undoable(&state, "<имя команды>", hint, || …)` — тот зовёт `exec::as_user` с тем же
  именем и пишет шаг Undo; всё прочее — `background` по умолчанию. Имя команды — ещё и ключ
  классификации в `engine::undo::classify`: новая мутация без своей ветки там рвёт цепочку
  как `unsupported`, а не молча записывается.
  `build_state` стоит **снаружи** замыкания, так что снимок после мутации — фоновый.
  Замыкание, а не guard: `.await` внутри области не скомпилируется, и thread-local не утечёт
  в чужую задачу на том же воркере. Предварительные чтения мутации (`show-ref`,
  `check-ref-format`, `detect_state`) попадают в «Мои» вместе с ней — это сознательно.
- **Учётные данные в URL маскируются на выходе, а не в движке**: `Display` / `Serialize`
  у `Error` и запись в журнал прогоняют текст через `mask_credentials`
  (`https://user:token@host` → `https://***@host`). Поля `Error::Git` и возвращаемые движком
  значения остаются дословными — разбор remote'ов и тем коммитов маскировка сломала бы.
- `Error::Git` несёт `journal: Option<u64>` — id записи упавшего запуска; клиент получает его
  как `journalId` и даёт в баннере «Показать вывод».
- Функции движка принимают корень репозитория первым параметром `&Path`. `CliEngine` —
  единственный, кто держит корень в себе.
- **Существующее имя или ревизия идёт в git после `--end-of-options`** (git ≥ 2.24): ветка
  `-x` существует — `update-ref` её создаст, хоть `git branch` и откажет, — и без флага
  становится опцией. Опции ставятся до него: после `--end-of-options` даже `--not` —
  ревизия, поэтому исключения пишутся как `^rev`. У `checkout` ветки и ревизии ещё и
  хвостовой `--`, иначе имя, совпавшее с файлом, читается как путь. `--contains=<rev>`
  клеится через `=` (аргумент опционален). У `rev-parse` без `--verify` флаг
  `--end-of-options` печатается в stdout строкой вывода — только в паре с `--verify`.
  Исключения: `tag` и `branch -m` уже разделены `--`, а `stash@{N}` строит
  `stash_entry_ref`, и с `-` он не начинается.
- **Разбор вывода git — по NUL-разделителям**, не по пробелам: поля `%x00`, записи `%x01`.
  Тема и тело коммита содержат что угодно, включая пробелы и переводы строк.
- **Неразобранная запись — ошибка, а не молчаливое усечение списка.** Обрезанный поток даёт
  более короткий список, неотличимый от полного, и читается как «файл пропал из коммита».
- Типы ошибок: `Error::Git { command, stderr }` — отказ git, `Error::Parse` — «git сказал
  то, чего я не понимаю», `Error::Rule` — «так делать нельзя» (доменный запрет),
  `Error::Io`, `Error::Stale` — «файл на диске уже не тот, что читали». Ошибка git не
  схлопывается в литерал и доезжает до UI дословно. `Stale` — отдельный вариант, а не `Rule`
  с узнаваемым началом: клиент на нём **ветвится** (перечитать | перезаписать), а матч по
  прозе ломается на первой же переформулировке. Различитель — сериализованный `kind`.
- **`Error::Git.stderr` несёт оба потока** там, где git печатает диагностику в stdout:
  `merge`, `rebase`, `cherry-pick`, `revert` пишут `CONFLICT (content): …` именно туда, и
  выброшенный stdout стоил бы имени конфликтного файла. Так делают `branches::git` и
  `ops::git`; поле называется `stderr` по историческим причинам.
- **Путь файла в позиции pathspec — только через `literal()`** (`:(literal)<путь>`). Голый
  путь после `--` для git всё ещё шаблон: `x[ab].txt` совпадает и с `xa.txt`, откат одного
  откатывал другой, `rm -f` нового файла удалял с диска чужой отслеживаемый, а `add` при
  коммите тащил файл из чужого changelist'а. Роуты Next.js (`app/[id]/…`) — обычный случай,
  не экзотика. Исключения: `diff --no-index` (там пути файловой системы, не pathspec),
  `git blame` (путь читается буквально, префикс он искал бы как часть имени) и
  `git check-attr` (принимает pathnames, а не pathspec: с префиксом искал бы атрибуты файла
  с именем `:(literal)…`). Фильтр
  `Paths` в `engine::log` тоже идёт через `literal()` (каталог в нём по-прежнему покрывает
  файлы под ним), и `--follow` истории файла — тоже: `:(literal)` он принимает и держит
  буквальным и после переключения на старое имя.
- Обнаружение переименований (`-M`) включается одинаково на парных вызовах: если список
  файлов сообщил о переименовании, diff того же файла обязан показать переименование.
- Маркеры незавершённой операции ищутся через `git rev-parse --git-path`, а не склейкой
  `.git` с корнем дерева — иначе linked worktree не распознаётся.
- **Коммит списка не делается `git add` + `git commit` всего индекса.** Он собирается во
  временном индексе (`cli::TempIndex`, `GIT_INDEX_FILE`), реальный индекс трогается только
  после состоявшегося коммита и только по путям списка. Подготовленное у файла списка
  главнее рабочего дерева; подготовленное вне списка в коммит не идёт никогда. Исключение —
  только при наличии `MERGE_HEAD`, `CHERRY_PICK_HEAD` или `REVERT_HEAD`: там коммит всего
  индекса. Остановка rebase (`edit`/`break`/`exec`, конфликт без этих маркеров) идёт общим
  путём. Отказ `git commit` несёт оба потока: «nothing to commit» git печатает в stdout.
- Мутация состояния changelist'ов идёт через `commands::mutate` (загрузить → изменить →
  сохранить → пересобрать состояние). Все команды `async`, чтобы git не блокировал UI-поток.
- Путь от клиента резолвится в `worktree_path` дважды: лексически **и** через
  `canonicalize` — иначе симлинк внутри репозитория уводит запись наружу. Сравниваются две
  канонизированные стороны: на macOS временный каталог живёт в `/var/...`, чей реальный путь
  `/private/var/...`, и сравнение разных написаний отвергало бы обычные пути. Отсутствующий
  хвост судится по глубочайшему существующему предку — несуществующая компонента симлинком
  быть не может. Наружу отдаётся **резолвнутый** путь: `rename` симлинк не следует, и
  переименование на путь ссылки заменило бы её обычным файлом. Для операций над самой
  записью (копия отката читает и пишет симлинк как ссылку) — `worktree_entry`: то же
  правило к **родительскому** каталогу, последняя компонента не разыменовывается. Ссылка
  наружу копируется текстом цели и не читается насквозь; отказ — для `linkdir/file`, где
  каталог ведёт наружу.
- **Непригодность файла к правке — это ключ `blocked` с `text: null`, а не ошибка**
  (`binary`, `too-large`, `mixed-eol`, `missing`): причину нужно показать *до* того, как
  пользователь потянется к кнопке. Ошибка — только путь, который репозитория не касается.
- Порядок отказов записи: `stale` → `rule` → `io`. Свежесть первая, потому что только на ней
  клиенту есть что выбрать; сообщи он другую причину — внешнее изменение осталось бы
  необъявленным.
- **`expect: ""` означает «файла здесь быть не должно»** и создаёт его: так ветка
  «перезаписать» воссоздаёт удалённый под редактором файл. Однозначно — существующий файл
  никогда не отпечатывается в пустую строку.
- Переводы строк нормализуются **на входе записи** (и `\r\n`, и одиночный `\r`), наружу идёт
  заданный `eol`. Инвариант: что приложение записало, оно же прочитает обратно с
  `blocked: null`; одиночный `\r` из вставки, записанный дословно, запер бы пользователя в
  `mixed-eol` собственного файла. Хвост текста пишется дословно: терминатор не дописывается
  и не срезается, `finalNewline` — справочное поле для UI.
- Запись — уникальный tmp рядом с целью плюс `rename`, как у `changelists.json`. Права цели
  переносятся на tmp: иначе 755-скрипт получил бы режим временного файла, и «изменена одна
  строка» стало бы в git сменой режима.

### Граница Tauri

- **Мутация возвращает целиком `RepoState`.** Отдельной команды опроса незавершённой
  операции нет: `RepoState.operation` — единственный источник правды о ней. Почта
  пользователя — `RepoState.userEmail`. Единственное исключение — `file_write`: см. ниже.
  Мутация, которой есть что сказать сверх состояния, возвращает `{ state, … }` и идёт через
  `runWithOutput`: `git_exec` (`GitExecResult`) и откаты — `file_rollback`, `list_rollback`,
  `lines_revert`, `discard_restore` возвращают `DiscardOutcome { state, backup }`, где
  `backup` — снятая копия (`null`, если на диске ничего не изменилось). На фронте все
  четыре вызываются только через `runDiscard` из `DiscardPanel.tsx`.
- Копии отката: `discard_list(limit)`, `discard_check(id) -> string[]` (read-only: пути,
  изменённые после отката), `discard_restore(id, force)`. Клиент спрашивает `discard_check`
  **до** восстановления и подтверждает с перечнем путей: `run()` схлопывает ошибку в текст, и
  ветвиться на `stale` после было бы не на чем. Бэк всё равно отказывает, если между
  вопросом и восстановлением что-то изменилось.
- **Клиент не шлёт текст патча.** `lines_stage` / `lines_unstage` / `lines_revert` (путь,
  `picks: HunkPick[]` — `{ hunk, lines: number[] | "all" }`, `digest`, `context`): индексы —
  в `FileDiff.hunks` / `Hunk.lines` показанного диффа, `digest` — `FileDiff.digest` того же
  ответа (FNV-1a байтов git; у диффов ревизий пустой), `context` — с которым он запрошен.
  Бэк перечитывает дифф (worktree — для stage и revert, index — для unstage) с тем же
  контекстом и **всегда** без игнорирования пробелов; другой отпечаток — `Error::Stale`.
  Ханковые кнопки — тот же механизм с `"all"`; `Hunk.patch` больше нет.
- История файла: `file_history(path, rev, cursor, limit) -> FileHistoryPage` (read-only;
  `rev: null` — `HEAD`, курсор возвращать дословно). Дифф строки —
  `commit_file_diff(hash, path, whitespace, context, oldPath)` с путём и старым путём
  **из строки**, а не с путём, по которому историю открыли; `oldPath: null` — прежний поиск
  переименования на бэке. В `DiffSource` вида `commit` это необязательное поле `oldPath`.
- Blame: `file_blame(path, rev) -> Blame` (`rev: null` — рабочее дерево) и
  `file_blame_before(hash, path, line, prevHash, prevPath) -> BlameBefore` — оба read-only.
  В `before` уходят `origin.hash` / `origin.path`, **`line.origLine`** (номер строки в коммите
  строки, а не в показанном файле) и `origin.previous`. Ответ — blame старой версии плюс
  `from..to` / `exact`: куда строка легла. `exact: false` — строку внёс коммит, подсвечено
  то, что она заменила, или строка, после которой её вставили.
- `file_read(path) -> TextFile` — read-only, через `createResource` + `refetch`.
  `file_write(path, text, eol, expect) -> FileWritten` — **единственная мутация проекта, не
  возвращающая `RepoState` и не идущая через `run()`**: она срабатывает на каждой паузе в
  наборе, а публикация глобального состояния так часто мигала бы busy в тулбаре и
  переразмечала панель под кареткой. Отказ всё равно доезжает в общий баннер через
  `setError`.
- **Отпечаток из ответа `file_write` клиент обязан положить вместо прежнего** — иначе
  следующая автозапись сравнится с отпечатком, который её же предшественница сделала
  протухшим.
- Read-only команда со своим типом идёт через `createResource` + `refetch`.
- Журнал: `journal_list(mine, after) -> JournalSummary[]` (без вывода, только записи с id
  больше `after`; `mine` — только кольцо пользователя, иначе слияние двух колец по id, то есть
  по времени записи) и `journal_output(id) -> JournalOutput | null` (оба потока по
  требованию, с `limitBytes` — под каким потолком запись хранится; сотни мегабайт вывода
  целиком через границу не возят). Слияние по id, а не по `startedAt`: долгая команда,
  начавшаяся раньше, иначе встала бы позади записей, которые клиент уже догрузил. Ни то,
  ни другое не запускает git и не требует открытого репозитория. Панель консоли опрашивает
  `journal_list` раз в секунду, **только пока открыта** — событие на каждый процесс git стоило
  бы сериализации ради почти всегда закрытой панели. `git_exec` возвращает `journalId`
  своей записи.
- Конфликты (R05e): `conflict_read(path) -> ConflictFile` (read-only),
  `conflict_resolve(path, text, eol, expect) -> RepoState` (`text` — записать и `git add`;
  `null` — взять файл как лежит; `expect` — отпечаток, `null` — без проверки, для бинарного)
  и `conflict_take(path, "ours"|"theirs") -> RepoState`. `RepoState.operation.conflicted` —
  `ConflictEntry[]` (`{ path, kind }`), а не строки: вид конфликта (`UU`, `DU`, …) едет с
  тем же `ls-files -u`, и второй команды за ним нет. Конфликт без операции (`stash pop`)
  виден только в Changes — туда же пункт «Разрешить конфликт…».
- Имена команд: `log_*`, `commit_*`, `commits_*` (`commits_compare`, `commits_squash`, …), `branch_*`, `op_*` (в т.ч. `op_rebase_range`, `op_rebase_start`, `op_bisect_start`, `op_bisect_mark`, `op_bisect_reset`), `ui_state_*`, `journal_*`, `undo_*`, `discard_*`, `lines_*`, `conflict_*`, `file_*` (`file_read`, `file_write`, `file_rollback`, `file_history`, `file_blame`, `file_blame_before`). Имя `commit_list`
  занято операцией «закоммитить changelist» и переиспользовано быть не может.
- Полный список зарегистрированных команд — `invoke_handler` в `gui/src-tauri/src/lib.rs`;
  он же роспись того, что вообще доступно фронту.
- **Параметр `ui_state_set` называется `ui`, а не `state`** — `state` занято managed-состоянием
  Tauri.
- `WORKING_TREE = ""` в `api.ts` — сентинел «сравнить с рабочим деревом» для `commits_compare`
  и `commits_compare_diff`. Пустая строка означает именно это, а не отсутствие значения.
- Признак «коммит недостижим от HEAD» — не поле строки лога, а отдельная команда
  `commits_unreachable(hashes)`, которую клиент зовёт раз на страницу.
- Undo / Redo: `undo_state() -> UndoState { undo, redo }` (read-only для репозитория, но
  разрывает цепочку, увидев внешнее изменение — поэтому клиент перечитывает его на каждом
  свежем `RepoState`) и `undo_step(direction, id) -> RepoState` через `run()` + `afterRepoChange()`.
  `UndoSide.reason` — код (`UndoReasonCode`, kebab-case), слова — в `i18n.ts`
  (`undoReason`, `undoWhat`). `destructive` (reset --hard) — сначала `confirmAction` с
  `lostCommits`. Отменённый коммит changelist'а возвращает файлы в их списки
  (`Step.lists`, `move_files` в `undo_step`), удалённый с тех пор список — в Default.

## Конвенции фронта

- **Клавиши регистрируются только через `hotkeys.ts`.** Свои `keydown` на `window` не вешать.
  `registerHotkey(code, run, opts)` — всегда Cmd/Ctrl + физический код (`"KeyF"`, `"Digit1"`);
  голые буквы приложению не принадлежат. По умолчанию сочетание срабатывает и в текстовом
  поле (Cmd+1/2, Cmd+S редактора); `typing: false` ставит его ниже отсечки «пользователь
  набирает текст» — там оно не матчится вовсе и поле получает нажатие как обычно. `usePanel(id, handlers)` подключает панель к циклу
  фокуса; `PanelId` — закрытый союз, новая панель добавляется в него первой.
- **Панель оборачивается в `PanelChrome`**, а не рисует свою рамку: он же регистрирует
  `PanelId`, поэтому `DiffPanel` существует отдельно от `DiffView`. Оверлеи, открываемые из
  обоих режимов (`StashPanel`, `GitConsolePanel`, `FileHistoryPanel`, `BlamePanel`), — не панели: они
  монтируются в `App`, регистрируются через `registerModalSource` и сами разбирают свои
  клавиши в `onKeyDown` (не в capture — Esc остаётся у store-модалки поверх). `PanelId` им
  не заводить: он поставил бы панели под оверлеем в цикл Tab, а `DiffPanel` в
  `FileHistoryPanel` зарегистрировал бы `"diff"` второй раз — там голый `DiffView` с `source`.
- Перейти к коммиту в логе извне — `revealCommit(hash)` из `logStore` (после
  `setViewMode("log")`): ждёт первой загрузки лога (иначе она заменит строки и выберет
  новейший), выбирает загруженный или подтягивает по хэшу (`findCommitByHash`), прокрутку
  делает `LogTable` по флагу `revealPending`. `false` — сказать пользователю, а не молчать.
  **Выбор строки не прокручивает список**: любой выбор, сделанный стором сам (не стрелками
  панели), ставит `setRevealPending(true)` — так же `jumpToMatch` (Cmd+G): без этого
  найденное ниже видимых строк или на догруженной странице выделялось за экраном, и
  переход выглядел как «ничего не произошло». Второго флага не заводить. Из оверлея —
  `showCommitInLog(hash, shortHash)` (`log/actions/showInLog.ts`): сброс сравнения, режим,
  `revealCommit`, фокус списка или уведомление. Им пользуются история файла и blame; оверлей
  сперва закрывает себя сам.
- **История файла и blame друг друга заменяют, а не стопкой**: `openBlame` закрывает
  историю (`closeFileHistory`), «История файла» из blame закрывает blame. Два оверлея одного
  z-уровня делили бы фокус и Esc. Esc в blame сперва снимает шаг «blame до…», закрывает —
  только с первого.
- **Все видимые строки — через `src/i18n.ts`**, в обоих словарях. `ru` типизирован по `en`,
  пропущенный ключ ломает сборку. `d().x()` читать внутри JSX или tracked scope — вынос в
  модульную константу замораживает язык на момент импорта. Сообщения `Error::Rule` с бэка
  остаются английскими в обеих локалях сознательно.
- **Цвета только семантическими токенами** (`bg`, `bg-subtle`, `bg-muted`, `fg`, `fg-subtle`,
  `fg-muted`, `accent`, `success`, `warn`, `danger`, `border`). Значения — CSS-переменные в
  `src/styles.css`, тема переключается классом `dark` на `<html>`. Хардкод hex запрещён.
- **Воронка мутаций.** Любая команда, меняющая репозиторий, идёт через `run(promise, label)`
  из `store.ts`: он ставит busy, ловит ошибку в общий баннер, ставит свежий `RepoState` и
  ревалидирует выделение — **в том числе на отказе**: операция, упавшая в конфликт, репозиторий
  всё равно меняет, и без перечитывания полоса незавершённой операции и панель изменений
  показывают состояние «до». Компонент своего `try/catch` не пишет. Если нужен текст ошибки на
  руки (диалог должен остаться открытым) — `runResult()` из `actions/repoRefresh.ts`.
  После изменения ссылок или истории — `afterRepoChange()`: `run()` сам по себе не обновляет
  ни дерево веток (его ресурс ключом на путь репозитория), ни страницы лога.
  Единственный обход воронки — `fileWrite` из `editState.ts` (причина выше, в границе Tauri);
  правило «компонент не пишет своего `try/catch`» при этом держится: слой действий здесь —
  сам `editState.ts`, и ловит он. Второй вызывающий того же `fileWrite` — «Сохранить» в
  редакторе конфликта (запись файла не трогает ни индекс, ни ссылки); «Отметить
  разрешённым» и «весь файл: ours/theirs» там уже мутации и идут через `runResult()`.
- **Неошибочное уведомление — `setNotice` / `visibleNotice` из `store.ts`** (полоса под
  баннером ошибки, текст и одно действие). `run()` его **не** сбрасывает: `refresh()` на
  каждом фокусе окна — тоже `run()`, и предложение «Вернуть» исчезало бы, едва читатель
  вернулся к окну. Уходит по «×», по своему действию или когда его заменило следующее;
  привязано к `repoPath`.
- Брошенную бэком ошибку в баннер кладёт `reportError(e)` из `store.ts`, а не
  `setError(errText(e))`: он же запоминает `journalId`, и под баннером появляется «Показать
  вывод» — открыть `GitConsolePanel` на вкладке «Все» с раскрытой записью. Ссылка привязана
  к тексту, с которым её поставили: чужой `setError` её не унаследует.
- **Промис-модалки.** `confirmAction`, `promptText`, `chooseOption` из `store.ts` и
  `openDialog(spec)` из `actions/dialogs.tsx` (форма, которая остаётся открытой при ошибке
  валидации). Нативные `alert` / `confirm` / `prompt` в WebView Tauri не делают ничего и
  вешают вызывающего навсегда.
- **«Модалка открыта» — один флаг на приложение**: `store.modalOpen()`. Свой источник
  регистрируется через `registerModalSource(isOpen)`. Второй приватный флаг означает, что
  стрелки продолжают двигать список за невидимым диалогом.
- Пункт контекстного меню с `disabled` обязан нести `reason` — серая строка без объяснения
  отправляет читателя искать причину в репозитории.
- Правило поиска живёт в `searchPattern.ts` (чистое, покрыто харнессом);
  `searchMatch.ts` — реактивная обёртка над ним и своих правил не добавляет. Поиск
  подсвечивает и приглушает, но никогда не сужает выборку — сужение это `LogFilter`.
  Флаги `search()` (`.*`, `Cc`) не копировать в `LogFilter.regex` / `matchCase`: те же флаги
  управляют тем, как git матчит `--author`.
- `logStore` владеет фильтром, страницами, курсором и выделением. Фильтр по ветке ставится
  только через `setBranchScope` — не писать `filter.branch` руками. Бюджеты:
  `PAGE_LIMIT = 200`, `ROW_CAP = 20000`, `LANE_BUDGET = 12`.
- **Состояние выбора живёт в трёх местах** — коммит в `logStore`, ветка в `branchSelection.ts`,
  файл в `commitFileSelection.ts`. Четвёртого не заводить.
- `DiffSource` требует `parent`: короткий хэш родителя, `null` — корневой коммит. Компилятор
  отличит отсутствие поля, но не подставленный наугад `null`.
- **Раскладка путей живёт в `components/pathTree.ts`** — split по `/` плюс схлопывание
  цепочек с одним потомком, обобщено по элементу. Им пользуются оба файловых дерева
  (`ChangesView.tsx` и `log/CommitDetailsPane.tsx`); своей копии не писать. Ключ свёрнутости
  каталога — путь **нарисованной** строки, то есть схлопнутого узла (`treeDirPaths`), а не
  каждого промежуточного сегмента. Дерево веток (`log/BranchTree.tsx`) сознательно осталось
  отдельным — причины в его докблоке над `buildTree`.

## Где живёт состояние

- `<repo>/.git/changelists.json` — changelist'ы. Байт-совместим с TUI. **Панель Git его не
  читает и не пишет.**
- `refs/graft/discard` — копии перед откатом (`engine::discard`): своя цепочка коммитов от
  имени `Graft <graft@localhost>`, не больше 200, потом начинается заново, а старая становится
  недостижимой (её приберёт `gc`). Читается и людьми: `git log -p refs/graft/discard`. Из
  лога исключена (`--exclude=refs/graft/*`), в дерево веток не попадает (там только
  `refs/heads` + `refs/remotes`).
- `<app_data_dir>/rebase/<fnv1a корня рабочего дерева>/` — план интерактивного rebase,
  запущенного Graft: `todo`, `editor.sh`, `msg/<полный хэш>`, `head` (= `orig-head` этого
  rebase — так план узнаётся своим), `comment` (выбранный `core.commentChar`). **Вне
  репозитория.** Живёт, пока идёт его rebase: удаляется вызовом, который его запустил, abort'ом
  и `rebase::sweep` на каждом `build_state` без rebase (так уходит план rebase, законченного в
  терминале) — но не пока идёт действие пользователя (`OWN_ACTIONS`): между записью плана и
  появлением `rebase-merge/` параллельное чтение состояния удалило бы план из-под старта.
  Путь каталога данных — `AppState.data_dir`, резолвится один раз в `.setup()`.
- `<app_data_dir>/undo/<fnv1a канонического корня>.json` — цепочка Undo / Redo
  репозитория (`engine::undo`, `version: 1`, поле `repo` отличает коллизию хэша), атомарная
  запись, права 0600: там пути, ветки и темы коммитов, содержимого файлов нет — только id
  объектов. Переживает перезапуск; без каталога данных цепочка живёт только в памяти.
- `<repo>/.git/graft-ui.json` — настройки панели: избранные ветки, схлопнутые папки, ширины
  колонок, подсветка. Версионирован (`version: 1`), camelCase, атомарная запись. Отсутствующий
  файл — это состояние по умолчанию, битый файл — тоже: настройки не стоят неработающего
  приложения.
- `localStorage` — всё, что про окно и не про репозиторий: `viewMode`, `theme`, `fontSize`,
  `locale`, `lastRepo`, `recentRepos`, `showIgnored`, `groupByDir`, `leftPanelWidth`, `logTreeWidth`,
  `logSplitRatio`, `logDetailsWidth`, `diffSplitRatio`, `diffWhitespace`, `diffHighlight`,
  `logOrder`, `logDimNonMatching`, `branchMenuOptions` (как показывать выпадающий список
  веток), `recentBranches` (недавние ветки по репозиториям).
- Память процесса Rust: `AppState` — корень открытого репозитория, флаг «показывать
  игнорируемые» и наблюдатель за git-dir (`watcher`, пересоздаётся в `repo_open` при смене
  корня, старый останавливается drop'ом; не завёлся — открытие не падает, остаётся фокус).
  Флаг живёт сессию приложения, при старте `store.openInitial` переприменяет
  сохранённый выбор. Там же журнал команд (`engine::exec`, два статических кольца — 1000
  действий пользователя и 2000 фоновых чтений, общие для всех репозиториев, у записи есть поле
  `repo`; худший случай ≈ 512 МБ + 64 МБ, первое слагаемое достижимо только выводом команд
  пользователя и упавших); на диск не пишется и умирает с процессом.
- Модульные сигналы фронта — состояние обоих режимов на время жизни окна. Черновик правки
  файла и последний известный отпечаток — среди них (`editState.ts`), и это не украшение:
  `Cmd+1` / `Cmd+2` матчатся выше отсечки «пользователь набирает текст» в `hotkeys.ts`, так
  что смена режима размонтирует панель из-под открытого редактора. Черновик, живущий в
  компоненте, ушёл бы вместе с ней.

## Что уже кусало

- `e.key` вместо `e.code`: на нелатинской раскладке Cmd+L приезжает как `"д"`, и карта по
  ключу молча перестаёт работать. `hotkeys.ts` матчит только по `e.code`.
- `registerHotkey` **бросает** на повторную регистрацию той же комбинации — иначе вторая
  просто затенялась бы первой и выглядела как «шорткат не работает».
- `ContextMenu` про `modalOpen()` не знает и сам глушит клавиши в capture-фазе. Меню и
  диалоги рисуются в `Portal`: панели скроллятся внутри `overflow-auto`, и меню внутри
  контейнера обрезается им.
- Вход в режим Log ничего не фокусировал, и вся клавиатура читалась как мёртвая. `LogView`
  фокусирует панель коммитов через `queueMicrotask` — до этого дочерние панели ещё не
  зарегистрированы.
- `navigator.clipboard` определён только в secure context, а кастомная схема Tauri им не
  всегда считается: свойства просто нет, незащищённый вызов бросает. Отсюда textarea-фолбэк
  в `actions/clipboard.ts`, и отказ обоих путей сообщается, а не глотается.
- Сигналы в сторе читать синхронно, до первого `await`: реактивность не переживает границу
  await. Каждый запрос лога несёт монотонный `seq`, ответ старше последнего выданного
  отбрасывается — иначе быстрая смена фильтра показывает строки предыдущего.
- **Протухший курсор — доменная ошибка, а не тихо неверный граф.** Слот 0 `LogCursor.openLanes` —
  служебный заголовок `@<хэш вершины>:<отпечаток фильтра>`; курсор возвращать дословно, не
  конструировать и не резать. UI обязан поймать отказ и перезагрузиться с первой страницы.
  Не обнаруживается один случай: переписывание истории, не сдвинувшее вершину.
- Склейка страниц дедуплицирует по хэшу: поиск по хэш-подобной строке подставляет найденный
  коммит в первую страницу, и он может встретиться в выдаче ещё раз.
- Цвет лейна — его индекс по модулю 12, индекс назначает открывающий линию коммит. Цвет от
  хэша здесь невозможен: курсор несёт хэш ожидаемого родителя, а не открывшего коммита.
- «Фильтр разорвал историю» отдельным полем не едет: признак — пустые рёбра **и** нулевой
  лейн разом у всех загруженных строк, кроме подколотых поиском по хэшу (`graphSuppressed`).
  По одной строке вывести нельзя: одинокий корневой коммит выглядит так же.
- Ширина колонки графа — константа `LANE_BUDGET * LANE_W + gutter`, а не производная от
  данных: вывод даже по первой странице замораживал колонку на одном лейне, если вершина
  истории линейна, и всё ветвление ниже схлопывалось в слот переполнения.
- В строке лога показывается дата автора, а фильтр по периоду и порядок git считает по дате
  коммиттера. Не выдавать одно за другое.
- `branch_tree()` всегда отдаёт `is_favorite: false` — избранное приезжает из
  `ui_state_get()`, строку дерева собирает UI из двух источников.
- Ключи свёрнутых папок в дереве веток разведены по группам — `fav:` / `local:` / `remote:`.
  Избранное — третья группа, а не переиспользованный `local`: иначе свёрнутый в избранном
  `p2p` сворачивал бы `p2p` в Local.
- Пункт «Emphasis» в меню лога **приглушает, а не фильтрует**. Сужает выборку только
  `LogFilter`; `LogFilter.authors` — список, git получает `--author` по разу на имя.
- «Есть ли незакоммиченные изменения» спрашивается командой `repo_local_changes`. Вывод из
  changelist'ов, которые панель уже держит, даёт **другой** ответ: там нет untracked-файлов.
- `reset --hard` двигает вершину назад, поэтому проба «есть ли коммиты новее» его не замечает —
  после действия нужен явный `afterRepoChange()`.
- `op_skip` при merge отвергается доменным правилом: у merge нет `--skip`. Пункт меню
  отключать заранее, а не ловить отказ после нажатия.
- «Непримёрженная ветка» считается по критерию `git branch -d`: недостижима ни от HEAD, ни от
  своего upstream (`branch_unmerged_count`).
- `stash_list_app` возвращает строки `stash@{N}: On <branch>: mygit: switching to <target>`;
  возврат сделан через `apply`, запись остаётся в списке после успешного восстановления.
  Менеджер стешей — отдельная пара: `stash_list` отдаёт `StashEntry` по **всем** стешам с
  меткой `fromApp`, а `stash@{N}` перенумеровывается после каждого pop/drop, поэтому
  apply/pop/drop принимают ещё и `hash` записи и отказывают, если список уехал.
- Даты фильтра строятся из локальных частей, не через `toISOString()`: форматирование через
  UTC сдвигает день для всех, кто не на Гринвиче, и окно фильтра уползает при каждом
  переоткрытии. Верхняя граница дня — его последняя секунда, иначе «по 20-е» теряет 20-е.
- Пути от файлового диалога нормализуются в `filterValues.relativeToRepo`: сравнение корня
  регистронезависимо только там, где такова файловая система, а сам путь регистр сохраняет.
- Потолок разворота пропуска в `DiffView` двойной: `MAX_EXPAND_GAP` судит запрос, а
  `expandedLinesCeiling` — **пришедший ответ**. `-U` расширяет все хунки файла разом, так что
  один клик по пропуску легко даёт вчетверо больше `BIG_DIFF_LINES`, а сводка «большой diff»
  этого не ловит: она сознательно судится по первому, нерасширенному ответу. Отвергнутый ответ
  выбрасывается, на экране остаётся принятый, запрос откатывается к принятому контексту.
- **Размер текста — только в rem, px-геометрия — только через `scaledPx()`** (`store.ts`).
  Настройка «Font size» — одно число в px, размер основного текста (`body`, по умолчанию 13);
  корневой `font-size` равен `16 * uiScale()`. Всё, что задано в px в обход этого
  (`text-[11px]`, `font: 13px` на `body`, высота строки лога, `line-height` редактора),
  остаётся на месте, пока текст вокруг растёт: прежняя трёхступенчатая настройка так и
  двигала только часть окна. Ширина колонки графа (`LANE_W`) сознательно не масштабируется.
  Сам слайдер применяет размер на `change` (отпускание), а не на `input`: диалог настроек
  тоже в rem, на каждом шаге перетаскивания он менял ширину и центровку, дорожка уезжала
  из-под неподвижного курсора, и окно дёргалось между двумя размерами.
- Минимумы раскладки: окно `minWidth: 720` (`tauri.conf.json`), в режиме Log это ровно
  180 (дерево) + 220 (детали) + 320 (diff). Разделители сделаны нулевой ширины в потоке —
  иначе сумма не влезает.
- **Заглушить шорткат можно только снятием регистрации, не проверкой внутри обработчика:**
  `hotkeys.ts` зовёт `preventDefault()` на совпадении до того, как что-то запустит, и
  отказавшийся обработчик всё равно съедает нажатие. Отсюда `createEffect` вокруг
  `registerHotkey` в `DiffView`: перерегистрация — это перезапуск эффекта, Solid сначала
  утилизирует прошлый прогон вместе с `onCleanup` внутри `registerHotkey`. Так стрелки
  отдаются каретке, а `Cmd+S` берётся только на время открытого редактора.
- `onBlur` редактора закрывает его на уходе, но не при `modalOpen()` (диалог про внешнее
  изменение сам забирает фокус и возвращает) и не при `!document.hasFocus()` (Cmd+Tab — не
  уход пользователя из редактора). Пробовать `relatedTarget` бесполезно: он `null` и для
  клика по любой нефокусируемой части приложения, а это как раз уход.
- **`editRules.ts`, `lineSelection.ts`, `blame/blameRules.ts`, `conflicts/conflictRules.ts` и `rebase/rebaseRules.ts` не импортируют ничего и не должны начать** (у
  каждого свой вызов `build()` в харнессе — по той же причине): `check-log-filters.mjs`
  *транспилирует* точки входа, а не бандлит, и один `import` из `../../api` превращается в
  падение резолва модулей, читающееся как посторонняя поломка. Свой вызов `build()` ему тоже
  нужен: esbuild кладёт выход под общую базу списка точек входа, и в одном вызове с
  `pathTree.ts` файл уехал бы в `diff/editRules.js` мимо загрузчика.
- `digest: ""` перегружен — его отдаёт любой `blocked`, не только `missing`. Запись с ним по
  существующему файлу вернёт «изменён на диске», то есть пользователю назовут не ту причину.
  Поэтому на `blocked != null` правка не открывается вовсе.
- Гард «этот запрос уже отрисован» в `DiffView` верен для повторного запроса и неверен для
  всякого пути, изменившего файл (стейдж, откат, сохранение правки) — там нужен
  `dropAccepted()`. И `anchors.clear()` строго **до** публикации: после — выброшенными
  окажутся свежие записи из `ref`-колбэков, и прыжок к различию проскроллит в никуда молча.
- Действия над хунками и строками запрещаются по свежести **диффа**, а не по чистоте
  черновика: автозапись оставляет черновик чистым и сознательно ничего не пересчитывает, так
  что дифф на экране нарисован до неё, и бэк отказал бы его `digest` как `stale`. Запрет
  называет причину до щелчка, а не ложное «файл изменился» после.
- Редактор — одна textarea на весь файл, абсолютно позиционированная над правой половиной:
  левая колонка физически не перерисовывается во время набора, а `<For>` с полем на строку
  терял бы фокус на каждом нажатии. `wrap="off"` несущий — перенесённая строка рисуется как
  две, и нумерация в жёлобе уезжает от файла.
- **`git check-ref-format` ведёт себя по-разному в двух режимах.** `--branch` на плохом имени
  умирает с кодом 128, а не 1; раскрывает `@{-1}` в имя предыдущей ветки и выходит с 0
  (поэтому эхо сверяется с вводом); голый `@` пропускает — и git честно создаёт ветку `@`.
  Обычный режим (`refs/tags/<имя>`) выходит с 1, но ведущий `-` принимает: `refs/tags/-x`
  валиден. Отсюда ручные отказы в `check_branch_name` / `check_tag_name` поверх git.
- `rev-parse --abbrev-ref <имя>@{upstream}` на имени с ведущим `-` не падал, а **эхом
  возвращал аргумент**: апстримом ветки `-x` считалась строка `-x@{upstream}`. Отсюда
  `--verify --end-of-options` в `branches::upstream_of`. А `branch -f` (обновление
  нетекущей ветки из апстрима) на таком имени честно отказывает — git не обновляет ветку,
  имя которой не создал бы; это ответ про имя, а не «unknown switch».
- Журнал команд — один на процесс, и тесты гоняются параллельно в том же процессе: запись
  своего теста ищется по id из `Output.journal` или по пути временного репозитория, никогда
  по позиции «последняя запись».
- Опрос `journal_list` из `GitConsolePanel` — сознательное исключение из «read-only команда
  идёт через `createResource` + `refetch`»: список растёт догрузкой `after`, а не
  перечитыванием, и опрашивается только пока панель открыта.
- `onMouseDown` с `preventDefault()` на кнопке правки выглядит мусором, но без него textarea
  теряет фокус раньше клика, `onBlur` закрывает редактор, и клик открывает его заново.
- Любой новый скрытый ref под `refs/` попадает в лог через `--all`, если его не исключить, и
  не только лишними строками: вершина истории (`tip`, из которой собран заголовок курсора)
  начинает зависеть от него, и каждый откат делал бы все курсоры протухшими. `--exclude`
  действует на **следующий** `--all` — ставится до него.
- `git status` без `-uall` отдаёт неотслеживаемый каталог одной строкой `dir/`, и эта строка —
  обычная запись Unversioned с галочкой и «Откатить к HEAD», так что `rollback` её получает.
  Раньше откат возвращал Ok и не трогал ни одного файла: `rm -f` пути не из индекса падает,
  `remove_file` каталог не удаляет, и обе ошибки глотались. Теперь каталог раскрывается тем же
  `CliEngine::untracked_under` (`ls-files --others --exclude-standard`), что и копия отката:
  файлы удаляются, опустевшие каталоги убираются снизу вверх, игнорируемые файлы и держащие
  их каталоги остаются, ошибки удаления доезжают до UI.
- Путь из hunk-патча берётся у git (`apply --numstat -z`), а не из заголовка `diff --git`:
  при `core.quotePath` заголовок экранирован.
- **Наблюдатель за git-dir: пути канонизировать до `strip_prefix`.** macOS-поток FSEvents
  отдаёт `/private/var/...` для наблюдения, поставленного на `/var/...`, и сравнение двух
  написаний одного каталога молча не совпадает ни разу. `GitDirs::resolve` канонизирует оба
  каталога, наблюдение ставится на канонизированные.
- Фильтр наблюдателя матчит **компоненты от корня git-dir**, не суффикс: `worktrees/<другой>/HEAD`
  и `modules/<sub>/HEAD` тоже кончаются на `HEAD`, но это чужой worktree и субмодуль.
- `EventKind::Access` отбрасывается явно: inotify (Linux) сообщает о каждом *открытии*, и
  `git status` собственного refresh'а будил бы наблюдателя без конца. По той же причине не
  слушаются `index` (его переписывает `git status`) и `changelists.json` (его пишет
  `build_state`): refresh не должен становиться причиной следующего refresh'а.
- Refresh от наблюдателя не стартует, пока `busy()`: у `run()` нет `seq`, и чтение,
  начатое до мутации и отвеченное после неё, поставило бы состояние старше мутации. Событие
  ждёт конца мутации (эффект на `busy` в `repoWatch.ts`), а не теряется. Своё действие Graft
  подавлено ещё на бэке (`OWN_ACTIONS` + `OWN_GRACE`); чужое изменение, попавшее в это окно,
  неотличимо от своего — его подхватит перечитывание после самой мутации, следующее событие
  или фокус.
- Кулдаун фокуса (петля TCC-промптов, комментарий в `App.tsx`) считается от **последнего
  refresh'а любого рода** (`msSinceRefresh`): промпт, поднятый git'ом refresh'а от
  наблюдателя, возвращает фокус, и без общих часов тот сразу запустил бы git ещё раз.
  Внутри кулдауна фокус только толкает отложенное внешнее изменение (`nudgeRepoWatch`) —
  без него git не запускается.
- Пока открыт редактор файла, наблюдатель перечитывает только лог и дерево веток, а
  `RepoState` — после закрытия (или на фокусе): внешний коммит правимого файла убирает его из
  changelist'ов, выделение сбрасывается, и редактор закрылся бы вместе с ним.
- Refresh от наблюдателя лог **не перезагружает силой** (`afterRepoChange({ log: false })`):
  свежий `RepoState` сам проходит через `checkNewCommits` в `LogTable` — наверху списка это
  `reload({ keepSelection })`, прокрученному читателю — предложение «N новых». Принудительный
  reload двигал бы строки под ним, а выделение за первой страницей заменял бы новейшим
  коммитом: автообновление закрывало бы открытое. Явно лог перечитывается, только когда
  `RepoState` отложен из-за открытого редактора.
- Refresh от наблюдателя **не сбрасывает баннер ошибки**: `refreshKeepingError()` из
  `store.ts` = `run(…, "", { keepError: true })`. Фокус и кнопка Refresh — это возврат
  пользователя к окну, там успешное перечитывание ошибку снимает; наблюдатель срабатывает от
  терминала, когда в окно могли не смотреть, и стёр бы непрочитанный отказ push'а или конфликт.
- **Коммит списка забирал чужое подготовленное.** `commit_paths` делал `git add` файлов
  списка и `git commit` **всего** индекса: файл другого списка, подготовленный целиком или
  ханком, уезжал в этот коммит, а `git add` целых файлов затирал частичную подготовку самого
  списка. Теперь «индекс главнее»: коммит собирается во временном индексе
  (`GIT_INDEX_FILE`) — HEAD (для `--amend` тоже HEAD: git сам возьмёт родителей прежнего
  коммита, а всё, что тот менял, должно остаться), поверх — пути списка: подготовленная
  версия из реального индекса, если у файла есть подготовленное, иначе рабочее дерево через
  `git add`. Коммитит `git commit` — хуки, подпись, identity и cleanup сообщения остаются
  git'овыми, хуки видят временный индекс. Реальный индекс трогается только после
  состоявшегося коммита и только по путям списка (`reset -q HEAD -- :(literal)…`), так что
  отказ хука оставляет его байт в байт. Временный индекс — **копия** реального, сброшенная
  `read-tree --reset HEAD`: так сохраняются stat-данные, иначе рефреш в `git commit`
  перечитывает каждый файл дерева (замер: 240 МБ — 1.3 с против 0.07 с). Подготовленный
  `git mv` — одна строка снимка, новый путь; удаление источника идёт вместе с ним. Пара
  берётся **из снимка**, а не своим `diff -M`: при `status.renames=false` снимок
  показывает две строки, которые могут лежать в разных списках, а собственный детектор
  утащил бы чужое удаление. При `MERGE_HEAD` / `CHERRY_PICK_HEAD` / `REVERT_HEAD` — прежний
  коммит всего индекса: `git commit` читает эти маркеры из git-dir при любом индексе, и
  merge-коммит без смёрженных файлов вне списка утверждал бы слияние, которого не содержит.
  Признак — именно маркеры, а не `detect_state() != None`: остановка rebase на `edit` —
  обычный коммит, и там чужое подготовленное утекало бы по-прежнему.
- **Выбор строк привязан к диффу, в котором сделан.** `lineSelection` хранит `digest` и
  контекст того ответа; `forPayload` делает выбор пустым на любом другом. Иначе перечитывание
  заменило бы ответ, бэк посчитал бы новый дифф, отпечаток сошёлся бы — и старые индексы
  подготовили бы строки, которые теперь стоят на их местах. Контекст — `acceptedContext`
  нарисованного ответа, а не `context()`: после щелчка по пропуску тот уже расширен, пока ответ
  в пути, и действие получило бы ложный «файл изменился». По той же причине `samePayload`
  сравнивает `digest`: оставленный на экране старый ответ слал бы старый отпечаток, и каждое
  действие отказывало бы как `stale`.
- Построчные и ханковые действия запрещаются (с `reason`) при режиме пробелов, отличном от
  `none`, — и unstage тоже: бэк строит патч по диффу без игнорирования, отпечаток показанного
  не сойдётся, и пользователь увидел бы ложное «файл изменился».
- Пустая строка внутри хунка — пустая строка контекста (`diff.suppressBlankEmpty`), а не
  хвостовой артефакт: границу хунка задают счётчики заголовка. Прежний `parse_diff` её
  пропускал, и индексы строк расходились бы с построителем патча.
- `\ No newline at end of file` при частичном выборе: невыбранная последняя строка без
  перевода, после которой выбраны добавления, расщепляется на «-x без перевода» + «+x с
  переводом» — дописать строку после последней можно, только дав ей перевод строки.
- Клавиши выбора строк (только в Changes, только пока выбор возможен): Cmd/Ctrl+Shift+J / K —
  шаг диапазона, Cmd/Ctrl+Shift+S — подготовить, Cmd/Ctrl+Shift+U — убрать,
  Cmd/Ctrl+Shift+Backspace — откатить (с подтверждением). Все — `typing: false`: в сообщении
  коммита, поиске, консоли это клавиши поля. Действия регистрируются, только пока есть выбор и
  они разрешены: отказ внутри обработчика всё равно съел бы нажатие. Откат строк пишется в
  копию видом `lines` («Откачены строки»), ханковой кнопкой — `hunk`.
- **Полностью подготовленный файл рисовался в Unstaged как новый целиком.** `raw_diff`
  читал пустой дифф рабочего дерева при `none` как «файл неотслеживаемый» и подменял его
  `--no-index`-диффом против `/dev/null`; «откатить строки» на таком диффе удаляли настоящие
  строки файла. Пустой дифф теперь подменяется, только если git файла не знает
  (`is_tracked`; intent-to-add в индексе и имеет свой дифф). `selection_patch` на пустом диффе —
  `Error::Rule`. Тест прежней задачи, закреплявший подмену как «прежнее поведение `none`»,
  перевёрнут.
- Бинарность в `parse_diff` судится по строке-маркеру **заголовка** (`Binary files …` /
  `GIT binary patch` до первого `@@`), а не по тексту где угодно в диффе: изменённая строка с
  такими словами прятала текстовый дифф за «бинарный файл».
- **`--skip` с `--follow` не работает.** Под `--follow` ограничение по пути применяется на
  выводе, а не при обходе: `--skip=N` считает пройденные, но не показанные коммиты, а
  пропущенный коммит не диффается — и переключение на старое имя в нём теряется, все строки
  старше переименования молча пропадают (git 2.54: `--skip=1..4` давали те же две строки,
  `--skip=6` — ничего при двух оставшихся). `--max-count` считает показанные, поэтому
  страница истории файла — первые `skip + limit + 1` строк от закреплённого коммита, срез
  на месте. Продолжать от родителей граничного коммита тоже нельзя: в нелинейной истории
  теряются другие линии обхода. Закрепление на хэше и есть ответ на «протухший курсор»:
  обход от неизменяемого коммита не сдвигается, отказ (`Error::Rule`, «reload») — только
  если сам коммит пропал.
- **История файла не показывает merge-коммиты, фильтр `Paths` лога — показывает.** Первая —
  `git log --follow` без `-m`: merge не диффается, а ограничение по пути под `--follow`
  смотрит на дифф; правка, сделанная в самом merge (разрешение конфликта), там не видна —
  как и в git. Второй — `-- <путь>` с упрощением истории: merge, не совпадающий ни с одним
  родителем по файлу, остаётся. Это разные вопросы; подгонять одно под другое не надо.
- В формате `log --name-status` разделитель записи **ведущий** (`%x01` в начале): статус
  идёт после формата, и хвостовой `%x01`, как у `engine::log::FORMAT`, отдал бы статус
  коммита N записи N+1.
- **`FileStatus` уходил на границу в `snake_case`** (`old_path`), а `api.ts` читает `oldPath`
  — на фронте поле было всегда `undefined`, и «История файла» у подготовленного
  переименования в Changes открывалась по новому имени, которого в HEAD нет: пустая история.
  Теперь camelCase, как у всех типов границы; имя поля держит тест
  `a_changelist_file_crosses_the_boundary_in_camel_case` в `model.rs`. Тип, который едет на
  фронт, без `rename_all = "camelCase"` — ошибка: однословные поля совпадут, многословные
  молча станут `undefined`.
- `--no-textconv` в `raw_diff` меняет и то, что видно в Changes: файл с textconv-драйвером
  показывается как есть (часто — бинарным), потому что превращённый дифф не применяется.
- **`git blame` не принимает `--end-of-options`** (git 2.54: `blame --end-of-options HEAD --
  f` — `fatal: bad revision 'f'`): ревизии он разбирает сам. Поэтому `engine::blame` сперва
  резолвит ревизию через `rev-parse --end-of-options` и отдаёт blame полный хэш — опцией
  тот быть не может. Путь в blame — без `:(literal)`, после `--`.
- **Пути в `--line-porcelain` C-квотированы**, `-z` у blame нет: `filename` и `previous`
  приезжают как `"\320\266 \"q\".txt"` для `ж "q".txt`. Без `blame::unquote` «blame до
  изменения» просит у git файл, которого нет.
- **`boundary` у blame — не только край неглубокого клона.** Без `--root` git помечает так и
  корневой коммит, а привитый коммит неглубокого клона выглядит корневым в любом случае —
  различить их по выводу нельзя, поэтому причина в UI общая: «самая ранняя доступная
  версия». Нет `previous` без `boundary` — файл создан в этом коммите.
- Blame рабочего дерева: незакоммиченные строки — нулевой хэш с `previous` на `HEAD`,
  подготовленное переименование git прослеживает сам, а неотслеживаемый файл отвергает
  (`no such path in HEAD`) — поэтому `untracked` распознаётся заранее через `ls-files`, а не
  по тексту ошибки. `cat-file -s` отвечает и за каталог (размером дерева) — тип
  проверяется первым.
- **Во время rebase «ours» — не пользователь.** `--ours` / стадия 2 — ветка, на которую идёт
  rebase, `--theirs` / стадия 3 — переносимый коммит пользователя. Редактор конфликта
  показывает метки маркеров под заголовками колонок и строку пояснения при
  `operation.kind === "rebase"`; без неё берут не ту сторону.
- **Голый `=======` вне блока — это содержимое**, подчёркивание Markdown/reST, а не
  забытый маркер. Предупреждение «остались маркеры» (`leftoverMarkers`) считает только
  блоки, которые ещё читаются как конфликт, и место, где разбор сломался; одиночные
  `<<<<<<<` / `|||||||` / `>>>>>>>` вне блока — ошибка разбора с номером строки.
- **Маркеры другой длины не читаются как текст.** `conflict-marker-size` отдаёт бэк
  (`ConflictFile.markerSize`, `check-attr`), парсер берёт его параметром; если блоков нет,
  а в тексте есть тройка `<`×n / `=`×n / `>`×n другой длины — отказ `marker-size` с этой
  длиной (атрибут поменяли после слияния). Молчаливое «0 конфликтов» записало бы маркеры в
  файл и назвало его разрешённым.
- **Undo редактора конфликта — свой, не textarea.** Значение textarea переписывается на
  каждое действие с блоком, и родной undo WebKit откатывал бы в тексты, которых модель не
  знала. `Cmd+Z` / `Cmd+Shift+Z` (и `Cmd+Y`) берутся в `onKeyDown` оверлея **до** любых
  проверок «пользователь печатает», а родной undo, пришедший иначе (пункт Edit → Undo
  стандартного меню macOS), перехватывается как `beforeinput` с `historyUndo` /
  `historyRedo`. В живом приложении это не прокликано (агент без дисплея) — проверить руками.
- **Сторона, которой нет, берётся через `git rm`, не `checkout`.** `checkout --ours` у
  «удалено у нас» падает с «does not have our version»; `take` смотрит на набор стадий и
  для отсутствующей стороны делает `rm` (на unmerged-пути он работает без `-f`, даже с
  изменённым файлом). «Разрешено как есть» — `add -A`: у отсутствующего файла он ставит
  удаление, простой `add` падал бы на pathspec без совпадений.
- Вид конфликта берётся из набора стадий `ls-files -u` по таблице git (`kind_of`), а не из
  второго вызова `status`; тест сверяет его с буквами `status --porcelain=v2` на живом
  слиянии. Литеральный pathspec каталога совпадает со всем под ним, поэтому записи
  `ls-files -u -- :(literal)path` дополнительно фильтруются по равенству пути.

- **Интерактивный rebase: git исполняет, Graft подставляет редакторы.** `GIT_SEQUENCE_EDITOR`
  — `cp "$GRAFT_REBASE_TODO"`: git запускает редактор через `sh -c '<ed> "$@"'`, путь едет
  переменной окружения и не требует своего экранирования (пробел в `Application Support`,
  кавычки, `$`). Упавший `cp` — git не стартует rebase вовсе, а не исполняет свой todo
  (тест с `rebase.autoSquash` и `fixup!`-коммитом). Git for Windows зовёт редакторы своим
  `sh` с coreutils — там работает по построению, проверено только на macOS.
- **Сообщения плана — по хэшу из `done`, не по позиции и не `exec`-строками.** Git зовёт
  редактор раз на цепочку squash (после её последнего шага) и ещё раз на `--continue` после
  конфликта; `exec git commit --amend` после `--skip` переписал бы **предыдущий** коммит.
  Цепочка только из `fixup` редактор не открывает вовсе — её сообщение едет через `reword`
  головы. Поэтому `op_continue` / `op_skip` для своего rebase ставят тот же редактор: с
  `GIT_EDITOR=true` reword, вставший на конфликт, молча оставлял старое сообщение.
- Сообщение через редактор проходит cleanup `strip`: строка `#123 …` пропала бы. План
  выбирает `core.commentChar`, которым не начинается ни одна строка ни одного сообщения, и
  передаёт `-c core.commentChar=…` на старт, continue и skip.
- План сверяется с диапазоном **поштучно**: коммит, которого нет в todo, git считает
  удалённым, и диалог, устаревший за время появления нового коммита, молча удалил бы его.
  Хэши — только полный hex: это же закрывает инъекцию строк в todo и выход `msg/<hash>` из
  каталога.
- Reword HEAD — `commit --amend --only` без путей: коммитится дерево самого HEAD, а
  подготовленное пользователем остаётся в индексе и в коммит не едет.
- Грязное (отслеживаемое) дерево для rebase — отказ, не autostash: спрятанные изменения
  исчезли бы на всё время rebase, включая остановку `edit`, а changelist'ы, синхронизированные
  с чистым снимком, забыли бы, в каком списке лежали эти файлы.
- **`update-index --index-info`: удаление и добавление одного пути в одной пачке оставляют
  старую запись.** Строка `0 <нулевой oid>\t<путь>` и следом `<mode> <oid> 0\t<путь>` —
  и индекс не изменился вовсе. Поэтому `undo::set_index` шлёт две пачки: сначала удаления
  всех затронутых путей, потом нужные записи.
- **«Дерево не тронуто» по одним путям из `git status` — пустая истина для `reset --hard`
  на чистом дереве**: ни до, ни после статус не перечисляет ни одного файла, а файлы
  переписаны. `undo::untouched` смотрит ещё и на каждый путь, чья запись индекса сдвинулась
  (не перечисленный статусом путь побайтно равен своей записи индекса). Без этого hard reset
  записывался как `Soft`, и Undo двигал ветку, оставляя дерево новым.
- **`git bisect` не принимает `--end-of-options`** («unrecognized option»), ни у `start`, ни у
  `good`/`bad`. Поэтому каждая ревизия от клиента сначала резолвится `rev-parse --verify
  --end-of-options <rev>^{commit}`, и в `git bisect` уходит только полный hex-oid (с `-` не
  начинается); `start` закрывает ревизии `--` — дальше были бы pathspec.
- **Битый `BISECT_LOG` не должен ронять весь `RepoState`**: `detect_state` зовут
  `build_state` и драйверы. Строгий разбор (`bisect::read`, `parse_log`) отдаёт
  `Error::Parse`, а `detect_state` складывает его текст в `BisectState.problem` и отметок не
  отдаёт — полоса показывает ошибку и оставляет «Закончить» (`git bisect reset`, ему нужен
  только `BISECT_START`). `mark` на битом логе отказывает до запуска git. Снимки Undo берут
  вид через `detect_kind` — без чтения лога вообще.
- Голый `git bisect start` из терминала не двигает ни HEAD, ни ссылки — наблюдатель видит его
  только потому, что `BISECT_*` в allowlist `watch.rs`. `BISECT_START` при старте на
  detached HEAD хранит **oid**, а не пустую строку.
- «Остались только пропущенные» — `git bisect skip` выходит с кодом 2 («We cannot bisect
  more!»), но это ответ, а не отказ: `mark` возвращает `Ok` только при коде 2 **и** логе,
  который вырос этим ответом и несёт `# possible first …` — кандидаты от прошлого ответа
  превратили бы отказ git (скажем, залоченный ref) в молчаливый успех. Кандидаты едут в
  `BisectState.candidates`. Объявленный git'ом «X is
  the first bad commit», которого нет в логе, — `Error::Parse`: ответ, который не переживёт
  следующего чтения состояния, хуже ошибки.
- Полоса операции для bisect — своя (`BisectStrip` в `OperationBar.tsx`), не общая: bisect не
  продолжают, а отвечают. Причина выключенных пунктов при bisect — `operationReason()` из
  `repoRefresh.ts` («идёт поиск коммита с ошибкой»), а не общее «незавершённая операция».
  Проверки вида «идёт операция, значит это остановка моего действия» (`RebasePanel`,
  `ConflictPanel`) исключают `bisect` явно: у bisect нет своих конфликтов.
- Cmd/Ctrl+Z приложения зарегистрирован с `typing: false` и снимается, пока открыт
  редактор файла (`UndoButtons.tsx`, `createEffect` вокруг `registerHotkey`): в поле ввода
  и в редакторе это undo текста. Редактор конфликта — модалка со своим `onKeyDown`, под ней
  `hotkeys.ts` молчит.

## Инициативы и PRD

**Скилл `/prd` больше не используется** (с 2026-09-29, решение владельца: слишком
тяжёл). Крупные изменения делаются напрямую: короткий план, по коммиту на фичу. PRD 02 и
03, живые спеки и разборы в vault остаются справочником. Текущая работа — перенос идей из
git-клиента twig, список и обоснование в `<vault>/Projects/my-git/gui/PRD/analysis/twig-comparison.md`.

Где что лежит: `<vault>/Projects/my-git/gui/specs/` — **согласованное поведение
приложения**, пять доменов (`branches`, `diff`, `history`, `log`, `shell`); это источник
правды, и начинать разбираться в поведении надо с него. `<vault>/Projects/my-git/gui/PRD/` —
открытые PRD: требования, спек-дельты, задачи и отчёты; `PRD/run/prd_XX/dashboard.html` —
ход работы. Закрытые PRD — в `<vault>/Projects/my-git/gui/Archive/`. Нарратив инициативы,
`Итоговый отчёт.md`, ТЗ и журнал `Правки.md` — в том же бандле vault.

Два правила, которые стоит знать до того, как что-то менять: снять требование
из `prd_XX_manifest.md` может только человек, поставивший задачу, а спек-дельта
попадает в живую спеку не раньше команды `/prd archive NN`.

Работа прервалась на середине — фраза «продолжи PRD» поднимает состояние
из `run/prd_03/state.js`; ничего пересказывать не нужно.
<!-- prd:end -->
