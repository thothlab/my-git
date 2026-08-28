import type { JSX } from "solid-js";

/**
 * One manner for every toolbar control in the window.
 *
 * Panel headers used to carry two different button styles — the CHANGES
 * toolbar's square icon buttons and the Log panels' bordered text glyphs — and
 * the same action (expand all, refresh) was drawn with a different mark in each.
 * Both now come from here, so a control means the same thing wherever it sits.
 *
 * The size is fixed (`h-6 w-6`): a panel header is 28px tall and the branch tree
 * can be dragged down to 180px, where six labelled buttons would not fit at all.
 */

/**
 * The single dimming of a control that cannot be pressed. The model is the
 * inactive half of the Changes/Log switch — `text-fg-muted` on the panel
 * background — and `opacity-60` lands on it in both themes. Nothing in the
 * window is allowed a second value: a control that is merely paler than its
 * neighbour reads as a different kind of control, not as a disabled one.
 */
export const DISABLED_CLASS = "disabled:cursor-not-allowed disabled:opacity-60";

/**
 * Square toolbar button. Every one carries a tooltip; a disabled one says why it
 * is disabled instead of silently doing nothing on click.
 *
 * `active` is for toggles that are currently on (a filter, a view mode). It is
 * deliberately loud — a pressed toggle must not be mistaken for a plain command
 * button that happens to be hovered.
 */
export function IconButton(props: {
  tip: string;
  onClick: () => void;
  disabled?: boolean;
  disabledTip?: string;
  active?: boolean;
  class?: string;
  children: JSX.Element;
}) {
  return (
    <button
      class={`flex h-6 w-6 shrink-0 items-center justify-center rounded text-sm text-fg-muted hover:bg-bg-muted hover:text-fg disabled:hover:bg-transparent disabled:hover:text-fg-muted ${DISABLED_CLASS} ${props.class ?? ""}`}
      classList={{ "bg-accent/15 text-accent hover:text-accent": props.active }}
      title={props.disabled ? (props.disabledTip ?? props.tip) : props.tip}
      disabled={props.disabled}
      aria-pressed={props.active === undefined ? undefined : props.active}
      onClick={props.onClick}
    >
      {props.children}
    </button>
  );
}

// ── Shared toolbar glyphs ────────────────────────────────────────────────────
// One mark per meaning, used by every toolbar. Android-Studio-style chevrons for
// expand/collapse; the rest are lucide-ish single-stroke shapes.

export function IconRefresh() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M13.5 8a5.5 5.5 0 1 1-1.7-3.9" />
      <path d="M13.6 2.4v3.2h-3.2" />
    </svg>
  );
}

/** Counter-clockwise arrow — revert / roll back. Mirrors `IconRefresh` so the
 *  two neighbouring circular arrows in the CHANGES toolbar read as a pair. */
export function IconRollback() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M2.5 8a5.5 5.5 0 1 0 1.7-3.9" />
      <path d="M2.4 2.4v3.2h3.2" />
    </svg>
  );
}

/** Chevrons pointing apart — expand all. */
export function IconExpandAll() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M4 6l4-3 4 3" />
      <path d="M4 10l4 3 4-3" />
    </svg>
  );
}

/** Chevrons converging — collapse all. */
export function IconCollapseAll() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M4 3l4 3 4-3" />
      <path d="M4 13l4-3 4 3" />
    </svg>
  );
}

/** Arrow into a tray — fetch from the remote. */
export function IconFetch() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M8 2v7" />
      <path d="M5 6.5L8 9.5l3-3" />
      <path d="M2.5 12.5h11" />
    </svg>
  );
}

export function IconPlus() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
    >
      <path d="M8 3.5v9M3.5 8h9" />
    </svg>
  );
}

/**
 * Star. `filled` is the whole of the favourite/not-favourite signal — the mark
 * changes shape, not merely its opacity, so the state is readable without
 * hovering and without comparing two rows against each other.
 */
export function IconStar(props: { filled?: boolean }) {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill={props.filled ? "currentColor" : "none"}
      stroke="currentColor"
      stroke-width="1.4"
      stroke-linejoin="round"
    >
      <path d="M8 1.8l1.9 3.9 4.3.6-3.1 3 .7 4.2L8 11.6l-3.8 2 .7-4.2-3.1-3 4.3-.6z" />
    </svg>
  );
}

/** Prompt chevron + line — the git console. */
export function IconConsole() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.6"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M3 4.5l4 3.5-4 3.5" />
      <path d="M8.5 12.5h4.5" />
    </svg>
  );
}

/** Archive box — stashed changes: put aside, not thrown away. */
export function IconStash() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M1.8 3.4h12.4v2.6H1.8z" />
      <path d="M2.9 6v6.6h10.2V6" />
      <path d="M6.3 8.6h3.4" />
    </svg>
  );
}
