// The 3D viewer's window: a panel floating over the page that can be
// dragged by its title bar, resized from its corner, maximised, closed
// (Escape too) or popped out into a browser window of its own. The viewer
// inside is a separate chunk, fetched when the window first opens.

import { Show, Suspense, createSignal, lazy, onCleanup, onMount, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import type { Viewer3DProps } from "./Viewer3D";
import { placeWindow, type Rect } from "./window";
import "./viewer.css";

const Viewer3D = lazy(() => import("./Viewer3D"));

let remembered: Rect | null = null;

export default function ViewerWindow(props: Viewer3DProps & {
  title: string;
  /** Where the pop-out window goes; no pop-out button without it. */
  popout?: string;
  onClose: () => void;
  /** Controls under the view (the replay's transport). */
  children?: JSX.Element;
}) {
  const view = () => ({ w: window.innerWidth, h: window.innerHeight });
  const [rect, setRect] = createSignal<Rect>(placeWindow(remembered, view()));
  const [max, setMax] = createSignal(false);
  let el!: HTMLDivElement;

  const drag = (e: PointerEvent, kind: "move" | "size") => {
    if (e.button !== 0 || max()) return;
    if ((e.target as HTMLElement).closest("button, a")) return;
    e.preventDefault();
    const start = rect(), x0 = e.clientX, y0 = e.clientY;
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      const dx = m.clientX - x0, dy = m.clientY - y0;
      const next = kind === "move" ? { ...start, x: start.x + dx, y: start.y + dy } : { ...start, w: start.w + dx, h: start.h + dy };
      remembered = placeWindow(next, view());
      setRect(remembered);
    };
    const up = () => { target.removeEventListener("pointermove", move); target.removeEventListener("pointerup", up); target.removeEventListener("pointercancel", up); };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
    target.addEventListener("pointercancel", up);
  };

  // Arrow keys on the title bar move the window; with Shift they size it.
  const key = (e: KeyboardEvent) => {
    const d = { ArrowLeft: [-24, 0], ArrowRight: [24, 0], ArrowUp: [0, -24], ArrowDown: [0, 24] }[e.key];
    if (!d || max()) return;
    e.preventDefault();
    const r = rect();
    remembered = placeWindow(e.shiftKey ? { ...r, w: r.w + d[0]!, h: r.h + d[1]! } : { ...r, x: r.x + d[0]!, y: r.y + d[1]! }, view());
    setRect(remembered);
  };

  onMount(() => {
    const refit = () => setRect(placeWindow(rect(), view()));
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape" && el.contains(document.activeElement)) props.onClose(); };
    window.addEventListener("resize", refit);
    window.addEventListener("keydown", esc);
    onCleanup(() => { window.removeEventListener("resize", refit); window.removeEventListener("keydown", esc); });
    el.focus({ preventScroll: true });
  });

  const popout = () => {
    const r = rect();
    const w = window.open(props.popout!, "h2live-3d", `popup=yes,width=${Math.max(720, Math.round(r.w))},height=${Math.max(520, Math.round(r.h))}`);
    if (w) props.onClose();
  };

  return (
    <Portal>
      <div
        ref={el}
        class="v3-window stats"
        classList={{ "v3-window--max": max() }}
        role="dialog"
        aria-label={props.title}
        tabindex="-1"
        style={max() ? undefined : { left: rect().x + "px", top: rect().y + "px", width: rect().w + "px", height: rect().h + "px" }}
      >
        <header class="v3-bar" onPointerDown={(e) => drag(e, "move")} onDblClick={() => setMax(!max())} onKeyDown={key} tabindex="0" aria-label="3D view window: arrow keys move it, Shift and arrow keys resize it">
          <h3>{props.title}</h3>
          <span class="v3-bar-buttons">
            <Show when={props.popout}><button onClick={popout} title="Open in a separate browser window" aria-label="Pop out">⧉</button></Show>
            <button onClick={() => setMax(!max())} title={max() ? "Restore" : "Maximise"} aria-label={max() ? "Restore" : "Maximise"}>{max() ? "❐" : "□"}</button>
            <button onClick={() => props.onClose()} title="Close" aria-label="Close">✕</button>
          </span>
        </header>
        <div class="v3-body">
          <Suspense fallback={<div class="v3-note v3-note--mid">Loading the 3D view…</div>}>
            <Viewer3D {...props} />
          </Suspense>
        </div>
        {props.children}
        <Show when={!max()}><div class="v3-grip" onPointerDown={(e) => drag(e, "size")} aria-hidden="true" /></Show>
      </div>
    </Portal>
  );
}
