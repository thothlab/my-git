/**
 * Shared drag lifecycle for the resizable dividers. Uses pointer capture so a
 * drag keeps tracking when the cursor moves over the webview content or is
 * released outside the window (the classic "stuck divider" bug). This is the
 * single place that fragile WKWebView pointer-capture ordering has to be right;
 * both the column divider (px) and the diff-split divider (ratio) call it.
 *
 * onMove receives each pointermove; the caller does its own geometry (px delta
 * vs clientX→ratio). onEnd fires exactly once, on release/cancel.
 */
export function beginDrag(
  handle: HTMLElement,
  pointerId: number,
  onMove: (ev: PointerEvent) => void,
  onEnd?: () => void,
  cursor: "col-resize" | "row-resize" = "col-resize",
) {
  handle.setPointerCapture(pointerId);
  let done = false;

  const move = (ev: PointerEvent) => {
    if (ev.buttons === 0) return end(); // released while off-window
    onMove(ev);
  };
  const end = () => {
    if (done) return;
    done = true;
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("pointerup", end);
    handle.removeEventListener("pointercancel", end);
    handle.removeEventListener("lostpointercapture", end);
    document.body.style.cursor = "";
    onEnd?.();
  };

  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", end);
  handle.addEventListener("pointercancel", end);
  handle.addEventListener("lostpointercapture", end);
  document.body.style.cursor = cursor;
}

/**
 * Draggable vertical divider between two horizontally-arranged panels.
 * Dumb by design: reports the raw target width (start width + drag delta) via
 * setWidth; the parent owns clamping and persistence.
 *
 * Occupies **zero** layout width: the visible hairline and the grab area are
 * absolutely positioned. In the Log mode two dividers sit between three panels
 * whose minimums add up to exactly the window minimum (180+220+320 = 720), so a
 * divider that took even 1px of layout would clip a panel at that width.
 */
export default function Resizer(props: {
  getWidth: () => number;
  setWidth: (w: number) => void;
  onCommit?: () => void;
}) {
  const onPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = props.getWidth();
    beginDrag(
      e.currentTarget as HTMLElement,
      e.pointerId,
      (ev) => props.setWidth(startW + (ev.clientX - startX)),
      () => props.onCommit?.(),
    );
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      class="group relative z-10 w-0 shrink-0 cursor-col-resize"
      onPointerDown={onPointerDown}
    >
      <div class="absolute inset-y-0 -left-1 w-2" />
      <div class="pointer-events-none absolute inset-y-0 left-0 w-px bg-border transition-colors group-hover:bg-accent" />
    </div>
  );
}

/**
 * Draggable horizontal divider between two vertically-stacked panels. Reports a
 * ratio (0..1) of the container height, so the split survives window resizes;
 * the parent clamps and persists it. Zero layout height, like Resizer.
 */
export function RowResizer(props: {
  container: () => HTMLElement | undefined;
  setRatio: (r: number) => void;
  onCommit?: () => void;
}) {
  const onPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const box = props.container()?.getBoundingClientRect();
    if (!box || box.height <= 0) return;
    beginDrag(
      e.currentTarget as HTMLElement,
      e.pointerId,
      (ev) => props.setRatio((ev.clientY - box.top) / box.height),
      () => props.onCommit?.(),
      "row-resize",
    );
  };

  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      class="group relative z-10 h-0 shrink-0 cursor-row-resize"
      onPointerDown={onPointerDown}
    >
      <div class="absolute inset-x-0 -top-1 h-2" />
      <div class="pointer-events-none absolute inset-x-0 top-0 h-px bg-border transition-colors group-hover:bg-accent" />
    </div>
  );
}
