import { For, Show, createResource, createSignal } from "solid-js";
import { Portal } from "solid-js/web";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { setTheme, theme, type Theme } from "../store";
import { d, dateLocale, locale, setLocale, type Locale } from "../i18n";
import {
  checkForUpdatesNow,
  installPendingUpdate,
  isCheckingForUpdates,
  pendingUpdate,
  updaterLastCheckedAt,
  updatesSupported,
} from "../updater";

const REPO_URL = "https://github.com/thothlab/my-git";

// Pane-style secondary block pinned to the bottom of the CHANGES panel.
export default function SidebarFooter() {
  const [modal, setModal] = createSignal<"settings" | "about" | null>(null);

  return (
    <>
      <div class="shrink-0 border-t border-border p-1">
        <FooterItem icon={<GearIcon />} label={d().settings()} onClick={() => setModal("settings")} />
        <FooterItem icon={<BookIcon />} label={d().docs()} onClick={() => void openUrl(REPO_URL)} />
        <FooterItem icon={<InfoIcon />} label={d().about()} onClick={() => setModal("about")} />
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

function FooterItem(props: { icon: any; label: string; onClick: () => void }) {
  return (
    <button
      class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-sm text-fg hover:bg-bg-muted"
      onClick={props.onClick}
    >
      <span class="shrink-0 text-fg-subtle">{props.icon}</span>
      <span>{props.label}</span>
    </button>
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

function SettingsModal(props: { onClose: () => void }) {
  return (
    <ModalShell title={d().settings()} onClose={props.onClose}>
      <div class="space-y-4">
        <Segmented
          label={d().themeLabel()}
          value={theme()}
          options={[
            { v: "auto", label: d().themeAuto() },
            { v: "light", label: d().themeLight() },
            { v: "dark", label: d().themeDark() },
          ]}
          onPick={(v) => setTheme(v as Theme)}
        />
        <Segmented
          label={d().languageLabel()}
          value={locale()}
          options={[
            { v: "en", label: "English" },
            { v: "ru", label: "Русский" },
          ]}
          onPick={(v) => setLocale(v as Locale)}
        />
      </div>
    </ModalShell>
  );
}

function Segmented(props: {
  label: string;
  value: string;
  options: { v: string; label: string }[];
  onPick: (v: string) => void;
}) {
  return (
    <div>
      <div class="mb-1 text-xs font-medium text-fg-subtle">{props.label}</div>
      <div class="flex overflow-hidden rounded border border-border">
        <For each={props.options}>
          {(o) => (
            <button
              class="flex-1 px-2 py-1 text-xs"
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
                    class="rounded bg-accent px-3 py-1 text-xs text-white disabled:opacity-60"
                    disabled={installing()}
                    onClick={() => void onInstall()}
                  >
                    {installing() ? d().updInstalling() : d().updUpdateTo(u().version)}
                  </button>
                )}
              </Show>
              <button
                class="rounded border border-border px-3 py-1 text-xs hover:bg-bg-muted disabled:opacity-60"
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

function GearIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
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
