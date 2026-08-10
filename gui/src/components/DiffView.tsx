import { Show } from "solid-js";
import { selectedPath } from "../store";

// Filled in task_04 (side-by-side / unified diff + hunk stage/revert).
export default function DiffView() {
  return (
    <div class="flex h-full items-center justify-center p-4 text-center text-xs text-fg-muted">
      <Show when={selectedPath()} fallback={<span>Выберите файл слева, чтобы увидеть diff.</span>}>
        <span>diff: {selectedPath()} — рендер в task_04</span>
      </Show>
    </div>
  );
}
