import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { d } from "../../../i18n";
import { registerModalSource } from "../../../store";
import { DISABLED_CLASS } from "../../IconButton";

/**
 * The form dialogs of the panel's actions: a branch name, a tag with an optional
 * message, and the like.
 *
 * Why not `promptText` from the store: a name that git refuses has to leave the
 * dialog **open** with what was typed still in it (PRD §Валидация, История 22),
 * and the store's prompt resolves and closes the moment OK is pressed. So the
 * submit handler here returns the error text instead of a value, and an error
 * keeps the dialog up.
 *
 * Native `alert` / `confirm` / `prompt` are not an option at all: inside the
 * Tauri WebView they do nothing and the caller waits forever.
 *
 * Three seams this file is careful about:
 *
 *  1. **"A modal is open" is one flag for the whole application.** The dialog
 *     registers itself with the store, so the keyboard layer stands down for it
 *     exactly as it does for the store's own modals — otherwise the arrows keep
 *     moving the list behind a dialog and Cmd+1 walks the user out of the mode,
 *     leaving the dialog's promise unresolved forever.
 *  2. **One dialog at a time, and every promise is resolved.** A second request
 *     while one is up is refused rather than replacing it, and the view resolves
 *     the specification it was showing — not whatever is current by then.
 *  3. **The host that renders is chosen reactively.** Both panels mount one; the
 *     first live host owns the screen, and when it goes away the next takes over
 *     instead of leaving every dialog invisible and every action silently hung.
 */
export interface DialogField {
  key: string;
  label: string;
  value?: string;
  placeholder?: string;
  /** Optional field: an empty value is allowed. */
  optional?: boolean;
}

export interface DialogSpec {
  title: string;
  /** Explanatory line under the title — what the action will do beyond the fields. */
  note?: string;
  fields: DialogField[];
  checkbox?: { label: string; checked?: boolean };
  submitLabel: string;
  /** Returns an error message to keep the dialog open, or null when done. */
  submit: (values: Record<string, string>, checked: boolean) => Promise<string | null>;
}

interface OpenDialog extends DialogSpec {
  resolve: (completed: boolean) => void;
}

const [current, setCurrent] = createSignal<OpenDialog | null>(null);

/** True while an action dialog is up. Registered as an application modal below. */
export const actionDialogOpen = () => current() !== null;

registerModalSource(actionDialogOpen);

/**
 * Show a form dialog; resolves true when it completed, false when it was
 * cancelled — including the cancellation that comes from the panel being
 * unmounted, so no caller is left awaiting a promise that can never settle.
 *
 * A second dialog while one is open is refused: stacking them would leave the
 * first one's promise dangling and put the second one's fields on the screen of
 * the first.
 */
export function openDialog(spec: DialogSpec): Promise<boolean> {
  if (current()) return Promise.resolve(false);
  return new Promise((resolve) => setCurrent({ ...spec, resolve }));
}

/** Close whatever is open, answering "cancelled" to whoever is waiting. */
function cancelCurrent(): void {
  const s = current();
  if (!s) return;
  setCurrent(null);
  s.resolve(false);
}

// Both panels mount a host; the first one in the list renders. Ownership is
// derived from the list rather than claimed once, so a panel disappearing hands
// the screen to the next host instead of hiding every dialog from then on.
const [hosts, setHosts] = createSignal<symbol[]>([]);

export function ActionDialogHost() {
  const id = Symbol("action-dialog-host");
  onMount(() => setHosts((l) => [...l, id]));
  onCleanup(() => {
    setHosts((l) => l.filter((x) => x !== id));
    // The last host is gone: nothing can render the dialog any more, so it is
    // cancelled rather than left on a screen that no longer exists.
    if (hosts().length === 0) cancelCurrent();
  });
  const mine = () => hosts()[0] === id;
  return (
    <Show when={mine() ? current() : null} keyed>
      {(dlg) => <DialogView spec={dlg} />}
    </Show>
  );
}

function DialogView(props: { spec: OpenDialog }) {
  const [values, setValues] = createSignal<Record<string, string>>(
    Object.fromEntries(props.spec.fields.map((f) => [f.key, f.value ?? ""])),
  );
  const [checked, setChecked] = createSignal(props.spec.checkbox?.checked ?? false);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let first: HTMLInputElement | undefined;

  // Resolves the specification this view was created for. Reading the module
  // signal instead would answer for whatever is current at the time, which is
  // not necessarily what the user was looking at.
  const close = (completed: boolean) => {
    if (current() === props.spec) setCurrent(null);
    props.spec.resolve(completed);
  };

  const canSubmit = () =>
    props.spec.fields.every((f) => f.optional || (values()[f.key] ?? "").trim() !== "");

  const submit = async () => {
    if (busy() || !canSubmit()) return;
    setBusy(true);
    const err = await props.spec.submit(values(), checked());
    setBusy(false);
    if (err) {
      setError(err); // stays open, keeping what was typed
      return;
    }
    close(true);
  };

  createEffect(() => {
    if (props.spec) queueMicrotask(() => first?.focus());
  });

  // The application key layer does not know about this dialog; stop everything
  // originating inside it before that layer can act on it.
  const onKeyDown = (e: KeyboardEvent) => {
    e.stopPropagation();
    if (e.code === "Escape") {
      e.preventDefault();
      close(false);
    } else if (e.code === "Enter") {
      e.preventDefault();
      void submit();
    }
  };

  return (
    <Portal>
      <div class="fixed inset-0 z-[60] flex items-center justify-center bg-black/40">
        <div
          class="w-[min(28rem,90vw)] rounded-lg border border-border bg-bg p-4 shadow-xl"
          onKeyDown={onKeyDown}
        >
          <div class="mb-1 text-sm font-semibold">{props.spec.title}</div>
          <Show when={props.spec.note}>
            <div class="mb-3 text-xs text-fg-subtle">{props.spec.note}</div>
          </Show>
          <For each={props.spec.fields}>
            {(f, i) => (
              <label class="mb-3 block text-xs text-fg-muted">
                {f.label}
                <input
                  ref={(el) => {
                    if (i() === 0) first = el;
                  }}
                  class="mt-1 w-full rounded border border-border bg-bg-muted px-2 py-1 text-sm text-fg outline-none focus:border-accent"
                  placeholder={f.placeholder}
                  value={values()[f.key] ?? ""}
                  onInput={(e) =>
                    setValues({ ...values(), [f.key]: e.currentTarget.value })
                  }
                />
              </label>
            )}
          </For>
          <Show when={props.spec.checkbox}>
            {(cb) => (
              <label class="mb-3 flex items-center gap-2 text-xs text-fg">
                <input
                  type="checkbox"
                  checked={checked()}
                  onChange={(e) => setChecked(e.currentTarget.checked)}
                />
                {cb().label}
              </label>
            )}
          </Show>
          <Show when={error()}>
            <pre class="mb-3 max-h-32 overflow-auto whitespace-pre-wrap rounded border border-danger/40 bg-danger/10 p-2 font-mono text-[11px] text-danger">
              {error()}
            </pre>
          </Show>
          <div class="flex justify-end gap-2">
            <button
              class="rounded border border-border px-3 py-1 text-sm hover:bg-bg-muted"
              onClick={() => close(false)}
            >
              {d().cancel()}
            </button>
            <button
              class={`rounded bg-accent px-3 py-1 text-sm text-white ${DISABLED_CLASS}`}
              disabled={busy() || !canSubmit()}
              onClick={() => void submit()}
            >
              {props.spec.submitLabel}
            </button>
          </div>
        </div>
      </div>
    </Portal>
  );
}
