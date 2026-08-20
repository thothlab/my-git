import { For, Show } from "solid-js";
import type { LogCommit } from "../../api";
import { LANE_BUDGET } from "../../logStore";

/**
 * The graph cell of one commit row.
 *
 * The backend hands each row its lane and the edges **leaving** that row
 * downwards, and nothing else; two segment classes have to be synthesized here
 * or lines break:
 *
 *  - the commit's own lane carries no incoming edge (the lane is freed before
 *    the row's edges are produced), so the top half of the row is drawn from
 *    the lanes that were open **above** it — that is, the `toLane` set of the
 *    previous row;
 *  - when a second child bends into a lane that is already waiting, the
 *    pass-through of that lane is left out of the row's edges, so a lane that
 *    is open above, is open below and is the source of no edge gets its bottom
 *    half drawn here too. Without this clause a fork row shows a gap in the
 *    mainline.
 *
 * Every bend produced by this backend happens below the node, which is why the
 * top half is always vertical. Because "open above" comes from the previous row
 * of the joined list rather than from the page, page boundaries need no special
 * case: the second page continues the first.
 *
 * One SVG per row, sized to the row. A single layer spanning the virtual height
 * would be a 440 000px-tall element on a capped list, and the whole rule above
 * is local to a 22px box anyway.
 */

/** Horizontal step between lanes, and thus the width of one lane column. */
export const LANE_W = 12;
/** Length of the dashed stub standing in for a line the column cannot draw. */
const STUB = 4;

const color = (lane: number) => `rgb(var(--lane-${lane % LANE_BUDGET}))`;

/** Lanes that stay open below this row: where its edges point. */
export const lanesBelow = (c: LogCommit): number[] => c.edges.map((e) => e.toLane);

/**
 * Lines of this row that the column has no room to draw. They are counted by the
 * "+N" marker instead, including the commit's own lane when that is one of them:
 * the node is still drawn, but in the shared overflow slot, and the count is what
 * says the slot stands for more than one line.
 */
export function overflowLanes(c: LogCommit, openAbove: number[], capacity: number): number[] {
  const all = new Set<number>([c.lane, ...openAbove]);
  for (const e of c.edges) {
    all.add(e.fromLane);
    all.add(e.toLane);
  }
  return [...all].filter((l) => l >= capacity).sort((a, b) => a - b);
}

export default function LogGraph(props: {
  commit: LogCommit;
  /** `toLane` values of the previous row in display order; empty for the first */
  openAbove: number[];
  /** row height in px — the graph is drawn to it exactly, never estimated */
  height: number;
  /** width of the graph column in px */
  width: number;
  /** lanes the column has room for: decided by the first page, never by paging.
   * Anything deeper folds into the row's "+N" marker instead of widening the
   * column and pushing every row's text sideways. Never above the lane budget. */
  capacity: number;
}) {
  // Lanes past the column's capacity are not drawn as lines. Routing them into a
  // shared slot made them indistinguishable from each other and from a real
  // neighbouring line, and an edge clamped into that slot read as a genuine
  // connection to the lane next door. So: no line for them at all, a short dashed
  // stub where one leaves or arrives, and the "+N" marker for the count. The
  // overflow slot holds a node only, in a shape that is not a lane node.
  const OVER = props.capacity;
  const px = (lane: number) => lane * LANE_W + LANE_W / 2;
  const overX = () => px(OVER);
  const fits = (lane: number) => lane < props.capacity;
  const mid = () => props.height / 2;

  const above = () => [...new Set(props.openAbove)].filter(fits);

  // Bottom half: one segment per edge, plus the pass-through the backend omits
  // for a lane that another line bends into.
  const segments = () => {
    const c = props.commit;
    const below = new Set(lanesBelow(c));
    const sources = new Set(c.edges.map((e) => e.fromLane));
    const all = c.edges.map((e) => ({ from: e.fromLane, to: e.toLane, lane: e.toLane }));
    for (const k of [...new Set(props.openAbove)]) {
      if (below.has(k) && !sources.has(k)) all.push({ from: k, to: k, lane: k });
    }
    return all;
  };

  /** Segments both of whose ends the column can draw. */
  const real = () => segments().filter((g) => fits(g.from) && fits(g.to));
  /**
   * A line that leaves a drawn lane for one the column has no room for, or
   * arrives from one. Drawn as a stub that stops short of anything, dashed, so
   * it cannot be read as reaching the next lane along.
   */
  const stubs = () =>
    segments()
      .filter((g) => fits(g.from) !== fits(g.to))
      .map((g) =>
        fits(g.from)
          ? { x1: px(g.from), y1: mid(), x2: px(g.from) + STUB, y2: mid() + STUB, lane: g.lane }
          : { x1: px(g.to) + STUB, y1: props.height - STUB, x2: px(g.to), y2: props.height, lane: g.to },
      );

  const over = () => overflowLanes(props.commit, props.openAbove, props.capacity);

  return (
    <svg
      class="block shrink-0"
      width={props.width}
      height={props.height}
      viewBox={`0 0 ${props.width} ${props.height}`}
      aria-hidden="true"
    >
      <For each={above()}>
        {(lane) => (
          <line
            x1={px(lane)}
            y1={0}
            x2={px(lane)}
            y2={mid()}
            stroke={color(lane)}
            stroke-width="1.5"
          />
        )}
      </For>
      <For each={real()}>
        {(g) => (
          <line
            x1={px(g.from)}
            y1={mid()}
            x2={px(g.to)}
            y2={props.height}
            stroke={color(g.lane)}
            stroke-width="1.5"
          />
        )}
      </For>
      <For each={stubs()}>
        {(g) => (
          <line
            x1={g.x1}
            y1={g.y1}
            x2={g.x2}
            y2={g.y2}
            stroke={color(g.lane)}
            stroke-width="1.5"
            stroke-dasharray="2 2"
          />
        )}
      </For>
      <Show
        when={fits(props.commit.lane)}
        fallback={
          // Not a lane node: a square in the overflow slot, so a commit whose lane
          // the column cannot draw is visibly "somewhere past the edge" rather
          // than sitting on a line that belongs to someone else.
          <rect
            x={overX() - 3}
            y={mid() - 3}
            width={6}
            height={6}
            fill="rgb(var(--bg))"
            stroke={color(props.commit.lane)}
            stroke-width="1.5"
          />
        }
      >
        <circle
          cx={px(props.commit.lane)}
          cy={mid()}
          r={props.commit.parents.length > 1 ? 3.5 : 3}
          fill={props.commit.parents.length > 1 ? "rgb(var(--bg))" : color(props.commit.lane)}
          stroke={color(props.commit.lane)}
          stroke-width="1.5"
        />
      </Show>
      <Show when={over().length > 0}>
        <text
          x={props.width - 2}
          y={mid() + 3.5}
          text-anchor="end"
          font-size="9"
          fill="rgb(var(--fg-muted))"
        >
          +{over().length}
        </text>
      </Show>
    </svg>
  );
}
