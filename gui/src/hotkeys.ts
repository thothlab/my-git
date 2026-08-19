import { createSignal, onCleanup } from "solid-js";
import { modalOpen } from "./store";

/**
 * Keyboard layer for the window: panel focus ring, in-panel navigation and
 * application shortcuts.
 *
 * Three rules this module exists to keep in one place:
 *
 *  1. **Shortcuts always carry Cmd/Ctrl and are matched on `e.code`.** `e.key`
 *     is the produced character, so on a non-Latin layout Cmd+L arrives as
 *     `e.key === "д"` and a key-based map silently stops working. `e.code` is
 *     the physical key and is layout-independent.
 *  2. **Typing never triggers actions.** Anything originating in an input,
 *     textarea, select or contenteditable is ignored unless it is a Cmd/Ctrl
 *     combination, so plain letters (and Tab out of a field) behave natively.
 *  3. **A modal wins.** While a modal is open the global handler stands down;
 *     the modal itself also listens in the capture phase and stops the event
 *     before it can reach here.
 */

/**
 * Closed vocabulary of focusable panels. A new panel is added here first, so a
 * typo in a caller is a type error instead of a phantom entry in the focus cycle.
 */
export type PanelId = "branches" | "commits" | "details" | "diff";

export interface PanelHandlers {
  /** container element — receives DOM focus when the panel is focused */
  element?: () => HTMLElement | undefined;
  /** ArrowUp/ArrowDown (±1) and PageUp/PageDown (±10) */
  moveSelection?: (delta: number) => void;
  /** Home (-1) / End (+1) */
  moveToEdge?: (edge: -1 | 1) => void;
  /** Enter */
  activate?: () => void;
  /** Cmd/Ctrl+Enter or the context-menu key */
  contextMenu?: () => void;
  /** panel-specific key; return true when handled */
  onKey?: (e: KeyboardEvent) => boolean;
}

const panels: { id: PanelId; handlers: PanelHandlers }[] = [];
const [focused, setFocused] = createSignal<PanelId | null>(null);

/** Id of the panel that currently owns keyboard focus, or null. */
export const focusedPanel = focused;

/**
 * Register a panel in the focus cycle. Order of registration is the cycle
 * order, which mirrors mount order and therefore the visual layout.
 * Returns an unregister function; also unregisters on component cleanup.
 */
export function registerPanel(id: PanelId, handlers: PanelHandlers): () => void {
  const entry = { id, handlers };
  panels.push(entry);
  const off = () => {
    const i = panels.indexOf(entry);
    if (i >= 0) panels.splice(i, 1);
    if (focused() === id) setFocused(null);
  };
  onCleanup(off);
  return off;
}

/** Move DOM focus to a registered panel. */
export function focusPanel(id: PanelId): void {
  const p = panels.find((x) => x.id === id);
  if (!p) return;
  setFocused(id);
  p.handlers.element?.()?.focus();
}

/** Cycle focus over the registered panels; wraps around in both directions. */
export function focusNext(dir: 1 | -1 = 1): void {
  if (panels.length === 0) return;
  const i = panels.findIndex((p) => p.id === focused());
  const next = i < 0 ? (dir === 1 ? 0 : panels.length - 1) : (i + dir + panels.length) % panels.length;
  focusPanel(panels[next].id);
}

/** True for elements that own their keystrokes (text entry). */
export function isTypingTarget(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el || !el.tagName) return false;
  const tag = el.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select" || el.isContentEditable;
}

const isMod = (e: KeyboardEvent) => e.metaKey || e.ctrlKey;

interface Shortcut {
  code: string;
  shift: boolean;
  run: () => void;
}
const shortcuts: Shortcut[] = [];

/**
 * Register an application shortcut. Always Cmd/Ctrl + a physical key code
 * (`"Digit1"`, `"KeyF"`, …) — bare letters, Space and `/` are never taken.
 */
export function registerHotkey(code: string, run: () => void, opts?: { shift?: boolean }): () => void {
  const shift = opts?.shift ?? false;
  // A second registration of the same combination would be shadowed by the first
  // and show up as "the shortcut does nothing", with nothing said about why.
  if (shortcuts.some((s) => s.code === code && s.shift === shift)) {
    throw new Error(`hotkeys: shortcut ${shift ? "Shift+" : ""}${code} is already registered`);
  }
  const sc: Shortcut = { code, shift, run };
  shortcuts.push(sc);
  const off = () => {
    const i = shortcuts.indexOf(sc);
    if (i >= 0) shortcuts.splice(i, 1);
  };
  onCleanup(off);
  return off;
}

function handle(e: KeyboardEvent): void {
  if (modalOpen()) return; // a modal owns the keyboard while it is open
  const typing = isTypingTarget(e.target);

  if (isMod(e)) {
    const sc = shortcuts.find((s) => s.code === e.code && s.shift === e.shiftKey);
    if (sc) {
      e.preventDefault();
      sc.run();
      return;
    }
  }
  // Beyond this point everything is a bare key: never taken from a text field.
  if (typing) return;

  const p = panels.find((x) => x.id === focused());

  if (e.code === "Tab") {
    // Only inside the panel area — elsewhere Tab keeps its native traversal.
    if (!p) return;
    e.preventDefault();
    focusNext(e.shiftKey ? -1 : 1);
    return;
  }
  if (!p) return;
  const h = p.handlers;
  if (h.onKey?.(e)) {
    e.preventDefault();
    return;
  }
  switch (e.code) {
    case "ArrowDown":
      h.moveSelection?.(1);
      break;
    case "ArrowUp":
      h.moveSelection?.(-1);
      break;
    case "PageDown":
      h.moveSelection?.(10);
      break;
    case "PageUp":
      h.moveSelection?.(-10);
      break;
    case "Home":
      h.moveToEdge?.(-1);
      break;
    case "End":
      h.moveToEdge?.(1);
      break;
    case "Enter":
      if (isMod(e)) h.contextMenu?.();
      else h.activate?.();
      break;
    case "ContextMenu":
      h.contextMenu?.();
      break;
    default:
      return;
  }
  e.preventDefault();
}

let installed = false;
/** Install the single window-level listener. Called once, from App. */
export function startHotkeys(): () => void {
  if (installed) return () => {};
  installed = true;
  window.addEventListener("keydown", handle);
  return () => {
    window.removeEventListener("keydown", handle);
    installed = false;
  };
}

/**
 * Component-side ergonomics over registerPanel: spread the focus ring and the
 * ref onto the panel container.
 *
 *   const p = usePanel("commits", { moveSelection: … });
 *   <div ref={p.ref} tabindex={0} classList={{ "ring-1 ring-accent": p.focused() }}
 *        onFocusIn={p.onFocusIn} />
 */
export function usePanel(id: PanelId, handlers: Omit<PanelHandlers, "element">) {
  let el: HTMLElement | undefined;
  registerPanel(id, { ...handlers, element: () => el });
  return {
    ref: (node: HTMLElement) => {
      el = node;
    },
    focused: () => focused() === id,
    onFocusIn: () => setFocused(id),
  };
}
