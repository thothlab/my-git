import { For, Show, createSignal } from "solid-js";
import { branchCheckout, branchCreate, branchList, type BranchInfo } from "../api";
import { chooseOption, promptText, run, state } from "../store";

export default function BranchMenu() {
  const [open, setOpen] = createSignal(false);
  const [branches, setBranches] = createSignal<BranchInfo[]>([]);
  const [filter, setFilter] = createSignal("");

  const current = () => (state()?.detached ? "detached" : state()?.branch ?? "—");

  const toggle = async () => {
    if (!open()) {
      try {
        setBranches(await branchList());
        setFilter("");
      } catch {
        /* error surfaces via run() elsewhere */
      }
    }
    setOpen((v) => !v);
  };

  const dirty = () =>
    (state()?.changelists ?? []).some((c) => !c.isUnversioned && c.files.length > 0);

  const doCheckout = async (b: BranchInfo) => {
    setOpen(false);
    const target = b.isRemote ? b.name.split("/").slice(1).join("/") : b.name;
    let stash = false;
    if (dirty()) {
      const k = await chooseOption(
        `Есть незакоммиченные изменения. Переключиться на "${target}"?`,
        [
          { key: "stash", label: "Спрятать в stash и переключиться" },
          { key: "switch", label: "Переключиться как есть" },
          { key: "cancel", label: "Отмена" },
        ],
      );
      if (!k || k === "cancel") return;
      stash = k === "stash";
    }
    await run(branchCheckout(target, stash));
  };

  const newBranch = async () => {
    setOpen(false);
    const name = await promptText("Новая ветка от HEAD", "");
    if (name && name.trim()) await run(branchCreate(name.trim()));
  };

  const shown = () => {
    const f = filter().toLowerCase();
    return branches().filter((b) => b.name.toLowerCase().includes(f));
  };

  return (
    <div class="relative">
      <button
        class="rounded border border-border bg-bg px-1.5 py-0.5 font-mono text-xs hover:bg-bg-muted"
        onClick={() => void toggle()}
      >
        ⎇ {current()} ▾
      </button>
      <Show when={open()}>
        <>
          <div class="fixed inset-0 z-20" onClick={() => setOpen(false)} />
          <div class="absolute left-0 top-full z-30 mt-1 max-h-96 w-64 overflow-auto rounded-md border border-border bg-bg py-1 text-xs shadow-lg">
            <input
              class="mx-2 mb-1 w-[calc(100%-1rem)] rounded border border-border bg-bg-muted px-1.5 py-0.5 outline-none focus:border-accent"
              placeholder="фильтр веток"
              value={filter()}
              onInput={(e) => setFilter(e.currentTarget.value)}
            />
            <button
              class="block w-full px-3 py-1 text-left text-accent hover:bg-bg-muted"
              onClick={() => void newBranch()}
            >
              ＋ Новая ветка…
            </button>
            <Section
              title="Локальные"
              items={shown().filter((b) => !b.isRemote)}
              onPick={doCheckout}
            />
            <Section
              title="Удалённые"
              items={shown().filter((b) => b.isRemote)}
              onPick={doCheckout}
            />
          </div>
        </>
      </Show>
    </div>
  );
}

function Section(props: {
  title: string;
  items: BranchInfo[];
  onPick: (b: BranchInfo) => void;
}) {
  return (
    <Show when={props.items.length > 0}>
      <div class="mt-1 border-t border-border px-3 py-0.5 text-[10px] uppercase text-fg-muted">
        {props.title}
      </div>
      <For each={props.items}>
        {(b) => (
          <button
            class="block w-full px-3 py-1 text-left font-mono hover:bg-bg-muted"
            classList={{ "text-accent": b.isCurrent }}
            onClick={() => props.onPick(b)}
          >
            {b.isCurrent ? "● " : "  "}
            {b.name}
          </button>
        )}
      </For>
    </Show>
  );
}
