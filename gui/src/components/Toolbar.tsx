import { Show } from "solid-js";
import { fetchRemote, pull, push } from "../api";
import { busy, confirmAction, run, state } from "../store";
import { d } from "../i18n";
import BranchMenu from "./BranchMenu";
import RepoMenu from "./RepoMenu";

export default function Toolbar() {
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
      <span class="font-semibold">Graft</span>
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
