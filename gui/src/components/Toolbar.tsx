import { Show, createEffect, createSignal } from "solid-js";
import { fetchRemote, pull, push } from "../api";
import { busy, confirmAction, refresh, run, state } from "../store";
import { d, locale, toggleLocale } from "../i18n";
import BranchMenu from "./BranchMenu";
import RepoMenu from "./RepoMenu";

type Theme = "auto" | "light" | "dark";

export default function Toolbar() {
  const [theme, setTheme] = createSignal<Theme>(
    (localStorage.getItem("theme") as Theme) || "auto",
  );
  createEffect(() => {
    const t = theme();
    const dark =
      t === "dark" ||
      (t === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
    document.documentElement.classList.toggle("dark", dark);
    localStorage.setItem("theme", t);
  });
  const cycleTheme = () =>
    setTheme((t) => (t === "auto" ? "light" : t === "light" ? "dark" : "auto"));

  const doPush = async () => {
    const s = state();
    if (!s) return;
    if (!s.upstream) {
      await run(push("upstream")); // first push sets upstream (-u)
      return;
    }
    if (s.ahead > 0 && s.behind > 0) {
      if (await confirmAction(d().diverged()))
        await run(push("force"));
      return;
    }
    await run(push("normal"));
  };

  return (
    <header class="flex items-center gap-2 border-b border-border bg-bg-muted px-3 py-1.5 text-sm">
      <span class="font-semibold">my-git</span>
      <RepoMenu />
      <BranchMenu />
      <Show when={state()?.upstream}>
        <span class="font-mono text-xs text-warn" title={state()?.upstream ?? ""}>
          ↑{state()!.ahead} ↓{state()!.behind}
        </span>
      </Show>

      <div class="ml-3 flex items-center gap-1">
        <TBtn label="Fetch" onClick={() => void run(fetchRemote())} />
        <TBtn label="Pull" onClick={() => void run(pull())} />
        <TBtn label="Push" onClick={() => void doPush()} accent />
      </div>

      <div class="ml-auto flex items-center gap-2">
        <button
          class="rounded border border-border px-2 py-0.5 text-xs uppercase hover:bg-bg"
          title={d().langTip()}
          onClick={toggleLocale}
        >
          {locale()}
        </button>
        <button
          class="rounded border border-border px-2 py-0.5 text-xs hover:bg-bg"
          title={d().themeTip()}
          onClick={cycleTheme}
        >
          {theme()}
        </button>
        <button
          class="rounded border border-border px-2 py-0.5 text-xs hover:bg-bg"
          onClick={() => void refresh()}
          disabled={busy()}
          title={d().refreshTip()}
        >
          {busy() ? "…" : "↻"}
        </button>
      </div>
    </header>
  );
}

function TBtn(props: { label: string; accent?: boolean; onClick: () => void }) {
  return (
    <button
      class="rounded border px-2 py-0.5 text-xs disabled:opacity-40"
      classList={{
        "border-accent bg-accent text-white hover:opacity-90": props.accent,
        "border-border hover:bg-bg": !props.accent,
      }}
      disabled={busy()}
      onClick={props.onClick}
    >
      {props.label}
    </button>
  );
}
