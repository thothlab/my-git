import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import {
  fontSize,
  registerModalSource,
  setFontSize,
  setTheme,
  theme,
  type FontSize,
  type Theme,
} from "../store";
import { d, dateLocale, locale, setLocale, type Locale } from "../i18n";
import {
  checkForUpdatesNow,
  installPendingUpdate,
  isCheckingForUpdates,
  pendingUpdate,
  updaterLastCheckedAt,
  updatesSupported,
} from "../updater";
import { DISABLED_CLASS } from "./IconButton";
import { openGitConsole } from "./GitConsolePanel";

const REPO_URL = "https://github.com/thothlab/my-git";

/**
 * The window's own menu — Settings, Docs, About — behind one icon at the right
 * end of the toolbar.
 *
 * It used to be a block pinned under the changelist tree, which put the
 * application's settings inside one of its two modes and left them unreachable
 * from the other. The toolbar belongs to the window, so the menu lives there.
 *
 * `modal` is module state, not component state, and it is registered with
 * `registerModalSource`: reachable from both modes, these dialogs are now up
 * while the Log panels are mounted, and an unregistered dialog would leave the
 * arrows driving the commit list behind it.
 */
const [modal, setModal] = createSignal<"settings" | "about" | null>(null);
registerModalSource(() => modal() !== null);

export default function AppMenu() {
  const [open, setOpen] = createSignal(false);
  onCleanup(() => setOpen(false));

  // The macOS app menu's About item (see src-tauri/src/lib.rs) has no dialog
  // of its own — it emits this event so both entry points open the exact
  // same modal instead of the native panel diverging from it. Registering
  // this once relies on AppMenu living in Toolbar, which App mounts
  // unconditionally — a remounting AppMenu would stack listeners.
  onMount(() => {
    const unlisten = listen("open-about", () => setModal("about"));
    onCleanup(() => void unlisten.then((f) => f()));
  });

  // Same native-menu wiring for "Check for Updates…" (see src-tauri/src/lib.rs):
  // the item has no UI of its own, so it opens About - the update controls live
  // there - and starts a check immediately, matching the platform convention of
  // Chrome/Slack-style "Check for Updates…" items.
  onMount(() => {
    const unlisten = listen("check-for-updates", () => {
      setModal("about");
      void checkForUpdatesNow();
    });
    onCleanup(() => void unlisten.then((f) => f()));
  });

  const pick = (fn: () => void) => {
    setOpen(false);
    fn();
  };

  return (
    <>
      <div class="relative">
        <button
          class="flex items-center rounded border border-border px-1.5 py-1 text-fg-subtle hover:bg-bg hover:text-fg"
          title={d().appMenuTip()}
          aria-haspopup="menu"
          aria-expanded={open()}
          onClick={() => setOpen((v) => !v)}
        >
          <SlidersIcon />
        </button>
        <Show when={open()}>
          <>
            {/* A click anywhere else closes it. `fixed inset-0` rather than a
                blur handler: the menu items are inside the button's own subtree
                and a blur would fire before the click they were opened for. */}
            <div class="fixed inset-0 z-30" onClick={() => setOpen(false)} />
            <div class="absolute right-0 top-full z-40 mt-1 w-44 rounded-md border border-border bg-bg py-1 shadow-lg">
              <MenuItem icon={<SettingsIcon />} label={d().settings()} onClick={() => pick(() => setModal("settings"))} />
              <MenuItem icon={<ConsoleIcon />} label={d().gitConsole()} onClick={() => pick(openGitConsole)} />
              <MenuItem icon={<BookIcon />} label={d().docs()} onClick={() => pick(() => void openUrl(REPO_URL))} />
              <MenuItem icon={<InfoIcon />} label={d().about()} onClick={() => pick(() => setModal("about"))} />
            </div>
          </>
        </Show>
      </div>

      <Show when={modal() === "settings"}>
        <SettingsModal onClose={() => setModal(null)} />
      </Show>
      <Show when={modal() === "about"}>
        <AboutModal onClose={() => setModal(null)} />
      </Show>
    </>
  );
}

function MenuItem(props: { icon: any; label: string; onClick: () => void }) {
  return (
    <button
      class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm text-fg hover:bg-bg-muted"
      onClick={props.onClick}
    >
      <span class="shrink-0 text-fg-subtle">{props.icon}</span>
      <span>{props.label}</span>
    </button>
  );
}

/** The trigger's own mark: sliders, not a gear — the gear is one item inside. */
function SlidersIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="4" y1="8" x2="20" y2="8" />
      <line x1="4" y1="16" x2="20" y2="16" />
      <circle cx="15" cy="8" r="2.5" />
      <circle cx="9" cy="16" r="2.5" />
    </svg>
  );
}

// ── Modals ───────────────────────────────────────────────────────────────────

function ModalShell(props: { title: string; onClose: () => void; children: any }) {
  return (
    <Portal>
      <div
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
        onClick={props.onClose}
      >
      <div
        class="w-[min(24rem,90vw)] rounded-lg border border-border bg-bg p-4 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div class="mb-3 text-sm font-semibold">{props.title}</div>
        {props.children}
        <div class="mt-4 flex justify-end">
          <button
            class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
            onClick={props.onClose}
          >
            {d().close()}
          </button>
        </div>
      </div>
      </div>
    </Portal>
  );
}

type SettingsSection = "appearance" | "language";

// Its own shell rather than `ModalShell`: that one is a narrow box with a
// bottom Close button, sized for About. Settings needs a header with an X,
// room for a sidebar, and enough width for two-column setting rows.
function SettingsModal(props: { onClose: () => void }) {
  const [section, setSection] = createSignal<SettingsSection>("appearance");

  return (
    <Portal>
      <div
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
        onClick={props.onClose}
      >
        <div
          class="flex h-[min(28rem,80vh)] w-[min(40rem,90vw)] flex-col overflow-hidden rounded-lg border border-border bg-bg shadow-xl"
          onClick={(e) => e.stopPropagation()}
        >
          <div class="flex shrink-0 items-center justify-between border-b border-border px-4 py-3">
            <div class="text-sm font-semibold">{d().settings()}</div>
            <button
              class="rounded p-1 text-fg-subtle hover:bg-bg-muted hover:text-fg"
              title={d().close()}
              onClick={props.onClose}
            >
              <CloseIcon />
            </button>
          </div>
          <div class="flex min-h-0 flex-1">
            <div class="w-36 shrink-0 space-y-0.5 overflow-auto border-r border-border p-2">
              <SettingsNavItem
                label={d().settingsAppearance()}
                active={section() === "appearance"}
                onClick={() => setSection("appearance")}
              />
              <SettingsNavItem
                label={d().languageLabel()}
                active={section() === "language"}
                onClick={() => setSection("language")}
              />
            </div>
            <div class="flex-1 overflow-auto p-4">
              <Show when={section() === "appearance"}>
                <div class="divide-y divide-border">
                  <SettingRow
                    label={d().themeLabel()}
                    description={d().themeDesc()}
                    control={
                      <Segmented
                        value={theme()}
                        options={[
                          { v: "auto", label: d().themeAuto() },
                          { v: "light", label: d().themeLight() },
                          { v: "dark", label: d().themeDark() },
                        ]}
                        onPick={(v) => setTheme(v as Theme)}
                      />
                    }
                  />
                  <SettingRow
                    label={d().fontSizeLabel()}
                    description={d().fontSizeDesc()}
                    control={
                      <Segmented
                        value={fontSize()}
                        options={[
                          { v: "small", label: d().fontSizeSmall() },
                          { v: "medium", label: d().fontSizeMedium() },
                          { v: "large", label: d().fontSizeLarge() },
                        ]}
                        onPick={(v) => setFontSize(v as FontSize)}
                      />
                    }
                  />
                </div>
              </Show>
              <Show when={section() === "language"}>
                <div class="divide-y divide-border">
                  <SettingRow
                    label={d().languageLabel()}
                    description={d().languageDesc()}
                    control={
                      <Segmented
                        value={locale()}
                        options={[
                          { v: "en", label: "English" },
                          { v: "ru", label: "Русский" },
                        ]}
                        onPick={(v) => setLocale(v as Locale)}
                      />
                    }
                  />
                </div>
              </Show>
            </div>
          </div>
        </div>
      </div>
    </Portal>
  );
}

function SettingsNavItem(props: { label: string; active: boolean; onClick: () => void }) {
  return (
    <button
      class="block w-full rounded px-2 py-1 text-left text-xs"
      classList={{
        "bg-bg-muted font-medium text-fg": props.active,
        "text-fg-subtle hover:bg-bg-muted hover:text-fg": !props.active,
      }}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}

function SettingRow(props: { label: string; description: string; control: any }) {
  return (
    <div class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
      <div class="min-w-0">
        <div class="text-sm">{props.label}</div>
        <div class="text-xs text-fg-subtle">{props.description}</div>
      </div>
      <div class="shrink-0">{props.control}</div>
    </div>
  );
}

function Segmented(props: {
  value: string;
  options: { v: string; label: string }[];
  onPick: (v: string) => void;
}) {
  return (
    <div class="flex overflow-hidden rounded border border-border">
      <For each={props.options}>
        {(o) => (
          <button
            class="flex-1 whitespace-nowrap px-2 py-1 text-xs"
            classList={{
              "bg-accent text-white": props.value === o.v,
              "hover:bg-bg-muted": props.value !== o.v,
            }}
            onClick={() => props.onPick(o.v)}
          >
            {o.label}
          </button>
        )}
      </For>
    </div>
  );
}

function AboutModal(props: { onClose: () => void }) {
  const [version] = createResource(() => getVersion());
  // The outcome line of the last *manual* check. Rendered here rather than
  // through the store modals: About lives in a Portal appended after them, so a
  // confirm/choose dialog would be painted underneath it and read as "the
  // button did nothing".
  const [notice, setNotice] = createSignal<{ text: string; bad: boolean } | null>(null);
  const [installing, setInstalling] = createSignal(false);

  const onCheck = async () => {
    setNotice(null);
    const res = await checkForUpdatesNow();
    // All three endings answer: silence here is indistinguishable from a broken
    // button.
    if (res.error) setNotice({ text: d().updUnreachable(), bad: true });
    else if (res.found) setNotice({ text: d().updFound(pendingUpdate()!.version), bad: false });
    else setNotice({ text: d().updUpToDate(), bad: false });
  };

  const onInstall = async () => {
    setInstalling(true);
    try {
      const err = await installPendingUpdate();
      if (err) setNotice({ text: d().updInstallFailed(err), bad: true });
    } finally {
      setInstalling(false);
    }
  };

  return (
    <ModalShell title={d().about()} onClose={props.onClose}>
      <div class="flex flex-col items-center gap-2 text-center">
        <AppMark />
        <div class="text-base font-semibold">Graft</div>
        <div class="text-xs text-fg-muted">v{version() ?? ""}</div>
        <div class="text-xs text-fg-subtle">{d().aboutBlurb()}</div>
        <button
          class="mt-1 text-xs text-accent hover:underline"
          onClick={() => void openUrl(REPO_URL)}
        >
          {d().sourceOnGithub()}
        </button>

        <Show when={updatesSupported}>
          <div class="mt-2 flex w-full flex-col items-center gap-2 border-t border-border pt-3">
            <div class="flex items-center gap-2">
              <Show when={pendingUpdate()}>
                {(u) => (
                  <button
                    class={`rounded bg-accent px-3 py-1 text-xs text-white ${DISABLED_CLASS}`}
                    disabled={installing()}
                    onClick={() => void onInstall()}
                  >
                    {installing() ? d().updInstalling() : d().updUpdateTo(u().version)}
                  </button>
                )}
              </Show>
              <button
                class={`rounded border border-border px-3 py-1 text-xs hover:bg-bg-muted ${DISABLED_CLASS}`}
                disabled={isCheckingForUpdates() || installing()}
                onClick={() => void onCheck()}
              >
                {isCheckingForUpdates() ? d().updChecking() : d().checkForUpdates()}
              </button>
            </div>
            <Show when={notice()}>
              {(n) => (
                <div
                  class="text-xs"
                  classList={{ "text-danger": n().bad, "text-fg-muted": !n().bad }}
                >
                  {n().text}
                </div>
              )}
            </Show>
            <Show when={updaterLastCheckedAt() && !notice()}>
              <div class="text-xs text-fg-subtle">
                {d().updLastChecked(updaterLastCheckedAt()!.toLocaleTimeString(dateLocale()))}
              </div>
            </Show>
          </div>
        </Show>
      </div>
    </ModalShell>
  );
}

// ── Icons (lucide-style strokes) ─────────────────────────────────────────────

function SettingsIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M20 7h-9" />
      <path d="M14 17H5" />
      <circle cx="17" cy="17" r="3" />
      <circle cx="7" cy="7" r="3" />
    </svg>
  );
}
function ConsoleIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="4 17 10 11 4 5" />
      <line x1="12" y1="19" x2="20" y2="19" />
    </svg>
  );
}
function BookIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" />
      <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z" />
    </svg>
  );
}
function InfoIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="10" />
      <path d="M12 16v-4" />
      <path d="M12 8h.01" />
    </svg>
  );
}
function CloseIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M18 6 6 18" />
      <path d="m6 6 12 12" />
    </svg>
  );
}
function AppMark() {
  return (
    <svg width="44" height="44" viewBox="0 0 1024 1024">
      <rect x="96" y="96" width="832" height="832" rx="190" class="fill-accent" />
      <g fill="none" stroke="#ffffff" stroke-width="44" stroke-linecap="round" stroke-linejoin="round">
        <line x1="390" y1="386" x2="390" y2="638" />
        <path d="M390 610 C390 560 540 560 582 560" />
        <circle cx="390" cy="322" r="64" />
        <circle cx="390" cy="702" r="64" />
        <circle cx="640" cy="560" r="58" />
      </g>
    </svg>
  );
}
