import { For, Show, createEffect, onCleanup } from "solid-js";
import { d } from "../i18n";
import {
  chooseState,
  setChooseState,
  confirmState,
  setConfirmState,
  promptState,
  setPromptState,
} from "../store";

/**
 * While a modal is up it owns Escape, and it takes it in the capture phase so no
 * panel or shortcut can see the key first. Everything else is left alone: the
 * application shortcut layer already stands down on `modalOpen()`, and grabbing
 * the rest here would keep the prompt field from receiving what is typed into it.
 */
function useModalEscape(active: () => boolean, onEscape: () => void) {
  createEffect(() => {
    if (!active()) return;
    const h = (e: KeyboardEvent) => {
      if (e.code !== "Escape") return;
      e.stopPropagation();
      e.preventDefault();
      onEscape();
    };
    window.addEventListener("keydown", h, true);
    onCleanup(() => window.removeEventListener("keydown", h, true));
  });
}

/** Renders whichever modal is active. Mounted once in App. */
export function ModalHost() {
  return (
    <>
      <ConfirmHost />
      <PromptHost />
      <ChooseHost />
    </>
  );
}

function Backdrop(props: { children: any }) {
  return (
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <div class="w-[min(28rem,90vw)] rounded-lg border border-border bg-bg p-4 shadow-xl">
        {props.children}
      </div>
    </div>
  );
}

function ConfirmHost() {
  let cancelBtn: HTMLButtonElement | undefined;
  const done = (ok: boolean) => {
    const s = confirmState();
    setConfirmState(null);
    s?.resolve(ok);
  };
  useModalEscape(() => confirmState() !== null, () => done(false));
  // Cancel takes focus, so a reflex Enter on a destructive confirmation cancels
  // it instead of performing it.
  createEffect(() => {
    if (confirmState()) queueMicrotask(() => cancelBtn?.focus());
  });
  return (
    <Show when={confirmState()}>
      {(s) => (
        <Backdrop>
          <div class="mb-4 whitespace-pre-wrap text-sm">{s().message}</div>
          <div class="flex justify-end gap-2">
            <button
              ref={cancelBtn}
              class="rounded border border-border px-3 py-1 text-sm outline-none hover:bg-bg-muted focus:border-accent"
              onClick={() => done(false)}
            >
              {d().cancel()}
            </button>
            <button
              class="rounded px-3 py-1 text-sm text-white"
              classList={{ "bg-danger": s().danger, "bg-accent": !s().danger }}
              onClick={() => done(true)}
            >
              {d().confirm()}
            </button>
          </div>
        </Backdrop>
      )}
    </Show>
  );
}

function ChooseHost() {
  const done = (key: string | null) => {
    const s = chooseState();
    setChooseState(null);
    s?.resolve(key);
  };
  useModalEscape(() => chooseState() !== null, () => done(null));
  return (
    <Show when={chooseState()}>
      {(s) => (
        <Backdrop>
          <div class="mb-4 whitespace-pre-wrap text-sm">{s().message}</div>
          <div class="flex flex-col gap-2">
            <For each={s().options}>
              {(o) => (
                <button
                  class="rounded border border-border px-3 py-1.5 text-left text-sm hover:bg-bg-muted"
                  classList={{ "text-danger": o.danger }}
                  onClick={() => done(o.key)}
                >
                  {o.label}
                </button>
              )}
            </For>
          </div>
        </Backdrop>
      )}
    </Show>
  );
}

function PromptHost() {
  let input: HTMLInputElement | undefined;
  const done = (v: string | null) => {
    const s = promptState();
    setPromptState(null);
    s?.resolve(v);
  };
  useModalEscape(() => promptState() !== null, () => done(null));
  createEffect(() => {
    if (promptState()) queueMicrotask(() => input?.focus());
  });
  return (
    <Show when={promptState()}>
      {(s) => (
        <Backdrop>
          <div class="mb-2 text-sm font-medium">{s().title}</div>
          <input
            ref={input}
            class="mb-4 w-full rounded border border-border bg-bg-muted px-2 py-1 text-sm outline-none focus:border-accent"
            value={s().value}
            onInput={(e) => setPromptState({ ...s(), value: e.currentTarget.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter") done(s().value);
              if (e.key === "Escape") done(null);
            }}
          />
          <div class="flex justify-end gap-2">
            <button
              class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
              onClick={() => done(null)}
            >
              {d().cancel()}
            </button>
            <button
              class="rounded bg-accent px-3 py-1 text-sm text-white"
              onClick={() => done(s().value)}
            >
              OK
            </button>
          </div>
        </Backdrop>
      )}
    </Show>
  );
}
