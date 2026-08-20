import type { JSX } from "solid-js";
import { usePanel, type PanelHandlers, type PanelId } from "../../hotkeys";
import { IconButton } from "../IconButton";

/**
 * Frame shared by the three Log-mode panels: header with a title and a toolbar,
 * scrollable body, and the focus ring that makes "which panel has focus"
 * visible. Registering the panel with the keyboard layer happens here, so a
 * panel component only supplies its handlers.
 */
export function PanelChrome(props: {
  id: PanelId;
  title: string;
  toolbar?: JSX.Element;
  handlers: Omit<PanelHandlers, "element">;
  children: JSX.Element;
}) {
  const panel = usePanel(props.id, props.handlers);
  let el: HTMLElement | undefined;
  return (
    <section
      ref={(node) => {
        el = node;
        panel.ref(node);
      }}
      tabindex={0}
      onFocusIn={panel.onFocusIn}
      // WKWebView does not always focus a tabindex container when a child is
      // clicked; without this the ring would not follow the mouse.
      onPointerDown={(e) => {
        if (!(e.target as HTMLElement).closest("input, textarea, button")) el?.focus();
      }}
      class="flex h-full min-h-0 min-w-0 flex-col bg-bg outline-none ring-inset"
      classList={{ "ring-1 ring-accent": panel.focused() }}
      aria-label={props.title}
    >
      <header class="flex h-7 shrink-0 items-center gap-0.5 border-b border-border bg-bg-muted px-1.5 text-xs">
        <span class="truncate font-semibold">{props.title}</span>
        <div class="ml-auto flex items-center gap-0.5">{props.toolbar}</div>
      </header>
      <div class="min-h-0 flex-1 overflow-auto">{props.children}</div>
    </section>
  );
}

/**
 * Toolbar button of a Log panel. A thin naming layer over the window-wide
 * `IconButton` (`components/IconButton.tsx`) — the header of a Log panel and the
 * header of the CHANGES panel draw the same control, so neither owns its look.
 * Every button carries a tooltip (R49i); one whose action is unavailable is
 * disabled and its tooltip says why, instead of silently doing nothing on click.
 */
export function PanelBtn(props: {
  label: JSX.Element;
  tip: string;
  disabled?: boolean;
  disabledTip?: string;
  active?: boolean;
  onClick?: () => void;
}) {
  return (
    <IconButton
      tip={props.tip}
      disabled={props.disabled}
      disabledTip={props.disabledTip}
      active={props.active}
      onClick={() => props.onClick?.()}
    >
      {props.label}
    </IconButton>
  );
}

/** Centred note for empty / pending states. Never gates the container itself. */
export function PanelNote(props: { title: string; hint?: string }) {
  return (
    <div class="flex h-full flex-col items-center justify-center gap-1 p-4 text-center">
      <div class="text-xs text-fg-muted">{props.title}</div>
      {props.hint ? <div class="text-xs text-fg-subtle">{props.hint}</div> : null}
    </div>
  );
}
