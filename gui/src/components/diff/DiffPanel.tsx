import { Show } from "solid-js";
import DiffView, { type DiffApi } from "../DiffView";
import { PanelChrome, PanelNote } from "../log/PanelChrome";
import { d } from "../../i18n";
import type { DiffSource } from "./model";

/**
 * The diff panel as a member of the Log mode's focus cycle.
 *
 * It exists because `PanelChrome` registers the panel id, and a panel that
 * enters the cycle must answer the navigation keys — otherwise the keyboard
 * layer calls `preventDefault()` on the arrows and nothing happens instead of
 * the native scroll (the condition inherited from task 02). The handlers below
 * drive the viewer through the `DiffApi` it hands out:
 *
 *   arrows / PageUp / PageDown — scroll,  Home / End — ends of the file,
 *   Enter — next difference,   Cmd/Ctrl + Up / Down — previous / next difference.
 *
 * Wiring: `LogView` renders this instead of its own `PanelChrome id="diff"`,
 * passing the file selected in the commit details pane.
 */
export default function DiffPanel(props: { source: DiffSource | null }) {
  let api: DiffApi | undefined;
  return (
    <PanelChrome
      id="diff"
      title={d().diffTitle()}
      handlers={{
        moveSelection: (delta) => api?.scrollLines(delta),
        moveToEdge: (edge) => api?.scrollToEdge(edge),
        activate: () => api?.next(),
        onKey: (e) => {
          if (!(e.metaKey || e.ctrlKey)) return false;
          if (e.code === "ArrowDown") {
            api?.next();
            return true;
          }
          if (e.code === "ArrowUp") {
            api?.prev();
            return true;
          }
          return false;
        },
      }}
    >
      <Show
        when={props.source}
        fallback={<PanelNote title={d().selectFileHint()} />}
      >
        <DiffView source={props.source} api={(a) => (api = a)} />
      </Show>
    </PanelChrome>
  );
}
