import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

/**
 * The context menu shared by the branch tree and the log.
 *
 * Three things it exists to keep in one place:
 *
 *  1. **A disabled item still says why.** `reason` is required whenever
 *     `disabled` is set — a grey line with no explanation sends the reader to
 *     look for the cause in the repository (PRD История 59).
 *  2. **It renders in a Portal.** Both panels scroll inside `overflow-auto`
 *     containers, and a menu drawn inside one is clipped by it.
 *  3. **It owns the keyboard while it is up.** `store.modalOpen()` does not know
 *     about this menu, so the application key layer would still be live; the
 *     handler below runs in the capture phase and stops every key before it can
 *     reach that layer.
 *
 * Items are passed as an accessor, not an array: what a menu offers depends on
 * answers that arrive after it is open (whether the commit is already contained,
 * how many commits a delete would lose), and the open menu has to follow them.
 */
export interface MenuAction {
  kind?: "item";
  label: string;
  /** Shown as the item's tooltip; required by convention when `disabled`. */
  reason?: string;
  disabled?: boolean;
  danger?: boolean;
  run?: () => void | Promise<void>;
}
export interface MenuSeparator {
  kind: "sep";
}
export type MenuEntry = MenuAction | MenuSeparator;

const isAction = (e: MenuEntry): e is MenuAction => e.kind !== "sep";

/** Where a menu was asked for, plus what it was asked about. */
export interface MenuAnchor {
  x: number;
  y: number;
}

/** Anchor for a menu opened from the keyboard: the middle of the current row. */
export function anchorOfElement(el: HTMLElement | undefined | null): MenuAnchor {
  const r = el?.getBoundingClientRect();
  if (!r) return { x: 120, y: 120 };
  return { x: Math.round(r.left + 24), y: Math.round(r.bottom) };
}

const MENU_W = 280;

export default function ContextMenu(props: {
  anchor: MenuAnchor;
  items: () => MenuEntry[];
  onClose: () => void;
}) {
  const [cursor, setCursor] = createSignal(-1);
  // Measured rather than read off the element inside the position getter: the
  // element does not exist on the first render, and a getter that read it once
  // would leave a menu opened near the bottom hanging off the window.
  const [height, setHeight] = createSignal(0);
  let el: HTMLDivElement | undefined;

  const actions = () => props.items().filter(isAction);
  const enabledIndexes = () =>
    actions()
      .map((a, i) => (a.disabled ? -1 : i))
      .filter((i) => i >= 0);

  const runAt = (i: number) => {
    const a = actions()[i];
    if (!a || a.disabled) return;
    props.onClose();
    void a.run?.();
  };

  const step = (dir: 1 | -1) => {
    const list = enabledIndexes();
    if (list.length === 0) return;
    const pos = list.indexOf(cursor());
    const next = pos < 0 ? (dir === 1 ? 0 : list.length - 1) : (pos + dir + list.length) % list.length;
    setCursor(list[next]);
  };

  // Capture phase: the menu takes the key before the panel layer sees it.
  createEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      e.stopPropagation();
      if (e.code === "Escape") {
        e.preventDefault();
        props.onClose();
      } else if (e.code === "ArrowDown") {
        e.preventDefault();
        step(1);
      } else if (e.code === "ArrowUp") {
        e.preventDefault();
        step(-1);
      } else if (e.code === "Enter" || e.code === "Space") {
        e.preventDefault();
        runAt(cursor());
      }
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  // Keep the whole menu on screen: opened near the bottom or the right edge it
  // would otherwise hang outside the window with no way to scroll to it.
  onMount(() => setHeight(el?.offsetHeight ?? 0));

  const pos = () => {
    const h = height();
    const x = Math.min(props.anchor.x, Math.max(0, window.innerWidth - MENU_W - 8));
    const y = h > 0 ? Math.min(props.anchor.y, Math.max(0, window.innerHeight - h - 8)) : props.anchor.y;
    return { x, y };
  };

  return (
    <Portal>
      <div
        class="fixed inset-0 z-40"
        onPointerDown={props.onClose}
        onContextMenu={(e) => {
          e.preventDefault();
          props.onClose();
        }}
      />
      <div
        ref={el}
        class="fixed z-50 overflow-hidden rounded-md border border-border bg-bg py-1 text-xs shadow-lg"
        style={{ left: `${pos().x}px`, top: `${pos().y}px`, width: `${MENU_W}px` }}
      >
        <For each={props.items()}>
          {(entry, i) => (
            <Show
              when={isAction(entry) ? (entry as MenuAction) : null}
              fallback={<div class="my-1 border-t border-border" />}
            >
              {(a) => {
                const actionIndex = () =>
                  props.items().slice(0, i()).filter(isAction).length;
                return (
                  <button
                    class="block w-full cursor-default px-3 py-1 text-left disabled:cursor-not-allowed disabled:opacity-40"
                    classList={{
                      "text-danger": !!a().danger && !a().disabled,
                      "bg-bg-muted": cursor() === actionIndex() && !a().disabled,
                      "hover:bg-bg-muted": !a().disabled,
                    }}
                    disabled={a().disabled}
                    title={a().reason}
                    onPointerEnter={() => !a().disabled && setCursor(actionIndex())}
                    onClick={() => runAt(actionIndex())}
                  >
                    {a().label}
                    <Show when={a().disabled && a().reason}>
                      <span class="block text-[10px] leading-3 text-fg-subtle">{a().reason}</span>
                    </Show>
                  </button>
                );
              }}
            </Show>
          )}
        </For>
      </div>
    </Portal>
  );
}

/** Open/close state of one panel's menu. */
export function createMenuController() {
  const [anchor, setAnchor] = createSignal<MenuAnchor | null>(null);
  return {
    anchor,
    open: (a: MenuAnchor) => setAnchor(a),
    openAt: (el: HTMLElement | undefined | null) => setAnchor(anchorOfElement(el)),
    close: () => setAnchor(null),
    isOpen: () => anchor() !== null,
  };
}
