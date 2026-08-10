import { Show, createEffect } from "solid-js";
import {
  confirmState,
  setConfirmState,
  promptState,
  setPromptState,
} from "../store";

/** Renders whichever modal is active. Mounted once in App. */
export function ModalHost() {
  return (
    <>
      <ConfirmHost />
      <PromptHost />
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
  const done = (ok: boolean) => {
    const s = confirmState();
    setConfirmState(null);
    s?.resolve(ok);
  };
  return (
    <Show when={confirmState()}>
      {(s) => (
        <Backdrop>
          <div class="mb-4 whitespace-pre-wrap text-sm">{s().message}</div>
          <div class="flex justify-end gap-2">
            <button
              class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
              onClick={() => done(false)}
            >
              Отмена
            </button>
            <button
              class="rounded px-3 py-1 text-sm text-white"
              classList={{ "bg-danger": s().danger, "bg-accent": !s().danger }}
              onClick={() => done(true)}
            >
              Подтвердить
            </button>
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
              Отмена
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
