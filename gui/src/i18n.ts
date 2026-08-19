import { createSignal } from "solid-js";

// Lightweight in-app i18n. Two locales, English is the default. Each entry is a
// function so interpolated strings share the same shape as static ones, and the
// `const ru: Dict` annotation makes TypeScript reject a missing or mistyped key.
//
// Reactivity: `d()` reads the `locale` signal, so `{d().refresh()}` in JSX (or
// any tracked scope) re-renders on a language switch. Do NOT hoist `d().x()`
// into a module-level constant — it would freeze at import time. Backend
// (Error::Rule) messages stay English in both locales by design.

export type Locale = "en" | "ru";

const stored = localStorage.getItem("locale");
const [locale, setLocaleSignal] = createSignal<Locale>(stored === "ru" ? "ru" : "en");
export { locale };

export function setLocale(l: Locale) {
  setLocaleSignal(l);
  localStorage.setItem("locale", l);
  document.documentElement.lang = l;
}
export function toggleLocale() {
  setLocale(locale() === "ru" ? "en" : "ru");
}
document.documentElement.lang = locale();

// Russian has three plural forms (one / few / many); pick by the standard rule.
const ruPlural = (n: number, one: string, few: string, many: string) => {
  const m10 = n % 10;
  const m100 = n % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
};

const en = {
  // App / error boundary
  uiCrashTitle: () => "Something went wrong in the UI",
  reloadState: () => "Reload state",
  reloadWindow: () => "Reload window",
  // Toolbar
  diverged: () => "Branches have diverged. Run push --force-with-lease?",
  themeTip: () => "Theme: auto → light → dark",
  refreshTip: () => "Refresh",
  langTip: () => "Language: English / Русский",
  openRepoBtn: () => "Open…",
  openRepoTitle: () => "Open repository",
  recentProjects: () => "Recent projects",
  noRepository: () => "No repository",
  // BranchMenu
  switchDirty: (target: string) => `You have uncommitted changes. Switch to "${target}"?`,
  stashAndSwitch: () => "Stash and switch",
  switchAsIs: () => "Switch as is",
  cancel: () => "Cancel",
  newBranchFromHead: () => "New branch from HEAD",
  filterBranches: () => "filter branches",
  newBranchItem: () => "New branch…",
  local: () => "Local",
  remote: () => "Remote",
  // Modals
  confirm: () => "Confirm",
  // CommitPanel
  commitColon: () => "Commit:",
  selectedCount: (n: number) => ` · ${n} selected`,
  filesCount: (n: number) => `${n} ${n === 1 ? "file" : "files"}`,
  commitMessage: () => "Commit message",
  amendLast: () => "Amend last commit",
  commitAndPushTip: () => "Commit and Push",
  untrackedSelectTip: () => "Untracked files are committed by selection",
  // ChangesView
  changes: () => "Changes",
  newListBtn: () => "+ list",
  newChangelist: () => "New changelist",
  cleanTree: () => "No changes — the working tree is clean.",
  active: () => "active",
  rollbackTip: () => "Rollback selected to HEAD",
  rollbackConfirm: (n: number) =>
    `Revert ${n} selected file(s) to HEAD? Local changes will be lost.`,
  collapseAll: () => "Collapse all",
  expandAll: () => "Expand all",
  groupByDirTip: () => "Group by directory",
  showIgnoredTip: () => "Show ignored files",
  viewOptionsTip: () => "View options",
  groupByHeader: () => "Group By",
  directory: () => "Directory",
  showHeader: () => "Show",
  ignoredFiles: () => "Ignored Files",
  settings: () => "Settings",
  docs: () => "Docs",
  about: () => "About",
  themeLabel: () => "Theme",
  languageLabel: () => "Language",
  themeAuto: () => "Auto",
  themeLight: () => "Light",
  themeDark: () => "Dark",
  close: () => "Close",
  aboutBlurb: () => "Native desktop git manager (Tauri + SolidJS).",
  sourceOnGithub: () => "Source on GitHub",
  revertFileConfirm: (path: string) => `Revert ${path} to HEAD? Local changes will be lost.`,
  revertListConfirm: (name: string) =>
    `Revert all files in "${name}" to HEAD? Local changes will be lost.`,
  renameChangelist: () => "Rename changelist",
  deleteListConfirm: (name: string) => `Delete list "${name}"? Files will return to Default.`,
  moveTo: () => "Move to",
  revertToHead: () => "Revert to HEAD",
  makeActive: () => "Make active",
  renameItem: () => "Rename…",
  deleteList: () => "Delete list",
  revertListToHead: () => "Revert list to HEAD",
  // DiffView
  unstaged: () => "Unstaged",
  staged: () => "Staged",
  vsHead: () => "vs HEAD",
  selectFileHint: () => "Select a file on the left to see its diff.",
  diffUnavailable: () => "Diff unavailable for this state.",
  binaryFile: () => "Binary file.",
  noChangesForBase: () => "No changes for this base.",
  revertHunkConfirm: () => "Revert this hunk in the working tree? Changes will be lost.",
  // StatusBar
  changesCount: (n: number) => `${n} ${n === 1 ? "change" : "changes"}`,
  // Window modes
  modeChanges: () => "Changes",
  modeLog: () => "Log",
  modeChangesTip: () => "Local changes (Cmd/Ctrl+1)",
  modeLogTip: () => "Git history (Cmd/Ctrl+2)",
  focusHint: () => "Tab / Shift+Tab moves focus between panels",
  // Busy phases
  busyFetch: () => "Fetching…",
  busyPull: () => "Pulling…",
  busyPush: () => "Pushing…",
  // Log mode — panels
  branchesTitle: () => "Branches",
  logTitle: () => "Log",
  commitDetailsTitle: () => "Commit details",
  diffTitle: () => "Diff",
  changedFiles: () => "Changed files",
  favorites: () => "Favorites",
  detachedHead: (hash: string) => (hash ? `HEAD (detached at ${hash})` : "HEAD (detached)"),
  onBranch: (name: string) => `HEAD → ${name}`,
  // Log mode — empty and special states
  noCommitsTitle: () => "This repository has no commits yet",
  noCommitsHint: () => "Make the first commit in the Changes mode.",
  noRemoteBranches: () => "No remote branches",
  noBranchesYet: () => "No branches to show",
  loadingHistory: () => "Loading history…",
  selectCommitHint: () => "Select a commit to see its details.",
  historyPending: () => "History is not wired up yet",
  actionPending: () => "Not available yet — needs the history backend",
  // Log mode — toolbars
  expandAllTip: () => "Expand all",
  collapseAllTip: () => "Collapse all",
  favoritesOnlyTip: () => "Show favourites only",
  newBranchTip: () => "New branch…",
  filterCommits: () => "filter commits",
};

type Dict = typeof en;

const ru: Dict = {
  uiCrashTitle: () => "Что-то пошло не так в UI",
  reloadState: () => "Перечитать состояние",
  reloadWindow: () => "Перезагрузить окно",
  diverged: () => "Ветки разошлись. Выполнить push --force-with-lease?",
  themeTip: () => "Тема: auto → light → dark",
  refreshTip: () => "Обновить",
  langTip: () => "Язык: English / Русский",
  openRepoBtn: () => "Открыть…",
  openRepoTitle: () => "Открыть репозиторий",
  recentProjects: () => "Недавние проекты",
  noRepository: () => "Нет репозитория",
  switchDirty: (target) =>
    `Есть незакоммиченные изменения. Переключиться на "${target}"?`,
  stashAndSwitch: () => "Спрятать в stash и переключиться",
  switchAsIs: () => "Переключиться как есть",
  cancel: () => "Отмена",
  newBranchFromHead: () => "Новая ветка от HEAD",
  filterBranches: () => "фильтр веток",
  newBranchItem: () => "Новая ветка…",
  local: () => "Локальные",
  remote: () => "Удалённые",
  confirm: () => "Подтвердить",
  commitColon: () => "Коммит:",
  selectedCount: (n) => ` · выбрано ${n}`,
  filesCount: (n) => `${n} ${ruPlural(n, "файл", "файла", "файлов")}`,
  commitMessage: () => "Сообщение коммита",
  amendLast: () => "Изменить последний коммит",
  commitAndPushTip: () => "Коммит и Push",
  untrackedSelectTip: () => "Untracked-файлы коммитятся выбором",
  changes: () => "Изменения",
  newListBtn: () => "+ список",
  newChangelist: () => "Новый changelist",
  cleanTree: () => "Нет изменений — рабочее дерево чистое.",
  active: () => "активный",
  rollbackTip: () => "Откатить отмеченные к HEAD",
  rollbackConfirm: (n) =>
    `Откатить отмеченные файлы (${n}) к HEAD? Локальные правки будут потеряны.`,
  collapseAll: () => "Свернуть всё",
  expandAll: () => "Развернуть всё",
  groupByDirTip: () => "Группировать по каталогам",
  showIgnoredTip: () => "Показывать игнорируемые",
  viewOptionsTip: () => "Параметры вида",
  groupByHeader: () => "Группировать по",
  directory: () => "Каталогам",
  showHeader: () => "Показывать",
  ignoredFiles: () => "Игнорируемые файлы",
  settings: () => "Настройки",
  docs: () => "Документация",
  about: () => "О программе",
  themeLabel: () => "Тема",
  languageLabel: () => "Язык",
  themeAuto: () => "Авто",
  themeLight: () => "Светлая",
  themeDark: () => "Тёмная",
  close: () => "Закрыть",
  aboutBlurb: () => "Нативный десктоп git-менеджер (Tauri + SolidJS).",
  sourceOnGithub: () => "Исходники на GitHub",
  revertFileConfirm: (path) =>
    `Откатить ${path} к HEAD? Локальные правки будут потеряны.`,
  revertListConfirm: (name) =>
    `Откатить все файлы списка "${name}" к HEAD? Локальные правки будут потеряны.`,
  renameChangelist: () => "Переименовать changelist",
  deleteListConfirm: (name) =>
    `Удалить список "${name}"? Файлы вернутся в Default.`,
  moveTo: () => "Переместить в",
  revertToHead: () => "Откатить к HEAD",
  makeActive: () => "Сделать активным",
  renameItem: () => "Переименовать…",
  deleteList: () => "Удалить список",
  revertListToHead: () => "Откатить список к HEAD",
  unstaged: () => "Не в индексе",
  staged: () => "В индексе",
  vsHead: () => "vs HEAD",
  selectFileHint: () => "Выберите файл слева, чтобы увидеть diff.",
  diffUnavailable: () => "diff недоступен для этого состояния",
  binaryFile: () => "Бинарный файл",
  noChangesForBase: () => "Нет изменений для этой базы.",
  revertHunkConfirm: () =>
    "Откатить этот hunk в рабочем дереве? Правки будут потеряны.",
  changesCount: (n) => `${n} ${ruPlural(n, "изменение", "изменения", "изменений")}`,
  modeChanges: () => "Изменения",
  modeLog: () => "Лог",
  modeChangesTip: () => "Локальные изменения (Cmd/Ctrl+1)",
  modeLogTip: () => "История git (Cmd/Ctrl+2)",
  focusHint: () => "Tab / Shift+Tab переключают фокус между панелями",
  busyFetch: () => "Забираем изменения…",
  busyPull: () => "Подтягиваем изменения…",
  busyPush: () => "Отправляем изменения…",
  branchesTitle: () => "Ветки",
  logTitle: () => "Лог",
  commitDetailsTitle: () => "Детали коммита",
  diffTitle: () => "Diff",
  changedFiles: () => "Изменённые файлы",
  favorites: () => "Избранное",
  detachedHead: (hash) => (hash ? `HEAD (отделён на ${hash})` : "HEAD (отделён)"),
  onBranch: (name) => `HEAD → ${name}`,
  noCommitsTitle: () => "В репозитории пока нет коммитов",
  noCommitsHint: () => "Сделайте первый коммит в режиме «Изменения».",
  noRemoteBranches: () => "Нет удалённых веток",
  noBranchesYet: () => "Веток пока нет",
  loadingHistory: () => "Загружаем историю…",
  selectCommitHint: () => "Выберите коммит, чтобы увидеть детали.",
  historyPending: () => "История ещё не подключена",
  actionPending: () => "Пока недоступно — нужен бэкенд истории",
  expandAllTip: () => "Развернуть всё",
  collapseAllTip: () => "Свернуть всё",
  favoritesOnlyTip: () => "Только избранные",
  newBranchTip: () => "Новая ветка…",
  filterCommits: () => "фильтр коммитов",
};

/** Current locale's dictionary. Reactive: reads the `locale` signal. */
export const d = () => (locale() === "ru" ? ru : en);
