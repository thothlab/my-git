/**
 * Copying to the clipboard from inside the WebView.
 *
 * `navigator.clipboard` is only defined in a secure context, and the custom
 * scheme Tauri serves the window from is not always treated as one — on the
 * platforms where it is not, the property is simply missing and an unguarded
 * call throws. The textarea path is the fallback that works everywhere the
 * window can focus an element; a failure of both is reported rather than
 * swallowed, because a copy that silently did nothing is indistinguishable from
 * one that worked until the paste.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the textarea path */
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}
