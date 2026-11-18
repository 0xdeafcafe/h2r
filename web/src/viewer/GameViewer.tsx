// The game page's viewer, in the page: the 2D tactical map or the 3D view
// in one box across the content column, the replay's transport (or the
// live clock) under it, and a button that puts the whole box full screen.
// The box is about 16:9 of the column, kept short enough that the
// scoreboard under it still shows on a laptop. Its keys (the replay's, the
// 3D view's) are only its own while it has the focus or the pointer is
// over it, so the page keeps Space and the arrows otherwise. Live games and
// replays both use it, each with its own frame source.

import { Show, Suspense, createEffect, createSignal, lazy, on, onCleanup, onMount, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import type { MapFrame } from "../components/MapCanvas";
import { GEOMETRY_CHANGED_NOTE } from "../lib/maps";
import { geometryFor } from "../map/geometry";
import type { RosterEntry } from "../map/render";
import { geometryChanged } from "../state/maps";
import type { GameState } from "../state/reducer";
import { afterChange, setFullscreen, type FullscreenDoc, type FullscreenState } from "./fullscreen";
import type { ViewMode } from "./scene";
import type { CameraRequest } from "./sync";
import TacticalMap from "./TacticalMap";
import "./viewer.css";

const Viewer3D = lazy(() => import("./Viewer3D"));

export type ViewerMode = "2d" | "3d";

/** Which view the box shows: the 3D one when the address asks for it
 *  (`?view=3d`) and the map's geometry is the base map's; else the 2D map. */
export function viewerMode(view: string | undefined, geometryOk: boolean): ViewerMode {
  return view === "3d" && geometryOk ? "3d" : "2d";
}

/** Whether a key pressed in the viewer is for a control in it (a button,
 *  the scrubber, a menu) rather than for the replay or the 3D view. */
export function keyIsForControl(target: EventTarget | null): boolean {
  const el = target as { closest?: (s: string) => Element | null } | null;
  return !!el?.closest?.("input, select, textarea, button, a, summary, [contenteditable]");
}

const reducedMotion = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

export default function GameViewer(props: {
  game: GameState;
  title: string;
  /** "Live coverage" or "Recorded replay", beside the title. */
  status: JSX.Element;
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  highlight: () => number | null;
  label: string;
  /** Replays: when the last seek was (players jump), and the killcam's camera requests. */
  soughtAt?: () => number;
  camera?: () => CameraRequest | null;
  /** The replay's keys; true when the key was one. */
  onKey?: (e: KeyboardEvent) => boolean;
  /** The keys, for screen readers. */
  keysLabel?: string;
  /** Under the view, full screen too: the replay's transport or the live clock. */
  controls?: JSX.Element;
  /** Under that, not full screen: the legend. */
  children?: JSX.Element;
}) {
  let box!: HTMLElement;
  const [params, setParams] = useSearchParams<{ view?: string; cam?: string; ao?: string; cut?: string; see?: string }>();
  const geometry = geometryFor(() => props.game.map);
  const mode = () => viewerMode(params.view, !geometryChanged(props.game));
  const setMode = (m: ViewerMode) => setParams({ view: m === "3d" ? "3d" : undefined }, { replace: true });
  // A killcam wants the 3D view: switch to it (it takes the camera as it opens).
  createEffect(on(() => props.camera?.() ?? null, (c) => { if (c && mode() !== "3d" && !geometryChanged(props.game)) setMode("3d"); }, { defer: true }));

  // ---- full screen ----
  const [full, setFull] = createSignal<FullscreenState>(null);
  const doc = () => document as unknown as FullscreenDoc;
  const toggleFull = async () => {
    const next = await setFullscreen(box, full() === null, doc(), full());
    setFull(next);
    focusInside();
  };
  onMount(() => {
    const changed = () => setFull(afterChange(box, doc(), full()));
    // Over the window (no Fullscreen API), Escape leaves as the browser's would.
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape" && full() === "window") { e.preventDefault(); setFull(null); } };
    document.addEventListener("fullscreenchange", changed);
    document.addEventListener("webkitfullscreenchange", changed);
    window.addEventListener("keydown", esc);
    onCleanup(() => {
      document.removeEventListener("fullscreenchange", changed);
      document.removeEventListener("webkitfullscreenchange", changed);
      window.removeEventListener("keydown", esc);
      cancelAnimationFrame(waiting);
      if (full() === "api") void setFullscreen(box, false, doc(), "api");
    });
    // Sent here from the old pop-out address (or a link to the 3D view): bring the viewer into view.
    if (params.view === "3d") {
      box.scrollIntoView?.({ block: "start", behavior: reducedMotion() ? "auto" : "smooth" });
      focusInside();
      focus3dWhenReady();
    }
  });
  // The page under a box laid over the window doesn't scroll.
  createEffect(() => {
    const over = full() === "window";
    document.documentElement.classList.toggle("gv-locked", over);
    onCleanup(() => document.documentElement.classList.remove("gv-locked"));
  });

  // ---- keys: only while focused or hovered ----
  // The 3D view takes its own keys (f, c, x, v, [ ]) on its root; the
  // replay's keys are taken here, from the stage or the 3D view alike.
  const focusInside = () => {
    const target = box.querySelector<HTMLElement>(".v3") ?? box.querySelector<HTMLElement>(".gv-stage");
    target?.focus({ preventScroll: true });
  };
  // The 3D view is fetched the first time it shows: once it is there, it
  // takes the focus from the stage (if nothing else has taken it since).
  let waiting = 0;
  const focus3dWhenReady = () => {
    cancelAnimationFrame(waiting);
    let frames = 240;
    const tick = () => {
      const v3 = box.querySelector<HTMLElement>(".v3");
      const a = document.activeElement;
      const free = !a || a === document.body || box.contains(a);
      if (v3 && free) { v3.focus({ preventScroll: true }); return; }
      if (free && --frames > 0 && mode() === "3d") waiting = requestAnimationFrame(tick);
    };
    tick();
  };
  // Pointing at the viewer with nothing else focused gives it the keys;
  // leaving takes them back, unless it was clicked in the meantime.
  let byHover = false;
  const enter = () => {
    const a = document.activeElement;
    if (a && a !== document.body && a !== document.documentElement) return;
    focusInside();
    byHover = box.contains(document.activeElement);
  };
  const leave = () => {
    if (!byHover || full()) return;
    byHover = false;
    const a = document.activeElement as HTMLElement | null;
    if (a && box.contains(a)) a.blur();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.defaultPrevented || keyIsForControl(e.target)) return;
    props.onKey?.(e);
  };

  const view3d = () => ({
    glb: () => geometry()?.glb ?? null,
    mode: (params.cam === "top" || params.cam === "follow" || params.cam === "eyes" ? params.cam : undefined) as ViewMode | undefined,
    ao: params.ao !== "0",
    // `?cut=<height>` opens with the height cut there; `?see=0` without the see-through.
    cut: params.cut !== undefined && Number.isFinite(Number(params.cut)) ? Number(params.cut) : null,
    seeThrough: params.see !== "0",
  });

  return (
    <section
      ref={box}
      class="gv"
      classList={{ "gv--full": full() !== null, "gv--window": full() === "window" }}
      aria-label={`Game viewer: ${props.title}`}
      onPointerEnter={enter}
      onPointerLeave={leave}
      onPointerDown={() => { byHover = false; }}
      onKeyDown={onKeyDown}
    >
      <header class="gv-bar">
        <h2>Game Viewer<small>{props.title}</small></h2>
        <span class="gv-status">{props.status}</span>
        <span class="gv-buttons">
          <span class="gv-switch" role="group" aria-label="View">
            <button aria-pressed={mode() === "2d"} classList={{ on: mode() === "2d" }} onClick={() => { setMode("2d"); focusInside(); }} title="The tactical map, from above">2D map</button>
            <Show
              when={!geometryChanged(props.game)}
              fallback={<button disabled title={`This custom map's ${GEOMETRY_CHANGED_NOTE}`}>3D view</button>}
            >
              <button aria-pressed={mode() === "3d"} classList={{ on: mode() === "3d" }} onClick={() => { setMode("3d"); focusInside(); focus3dWhenReady(); }} title="The level in 3D: orbit, follow a player, fly">3D view</button>
            </Show>
          </span>
          <button
            class="gv-full-button"
            aria-pressed={full() !== null}
            onClick={() => void toggleFull()}
            aria-label={full() ? "Leave full screen" : "Full screen"}
            title={full() ? "Leave full screen (Esc)" : "Full screen"}
          >
            <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
              <Show when={full()} fallback={<path d="M1 6V1h5M10 1h5v5M15 10v5h-5M6 15H1v-5" fill="none" stroke="currentColor" stroke-width="2" />}>
                <path d="M6 1v5H1M15 6h-5V1M10 15v-5h5M1 10h5v5" fill="none" stroke="currentColor" stroke-width="2" />
              </Show>
            </svg>
            <span class="gv-full-label">{full() ? "Exit" : "Full screen"}</span>
          </button>
        </span>
      </header>
      <div
        class="gv-stage"
        classList={{ "gv-stage--3d": mode() === "3d" }}
        tabindex="-1"
        role="group"
        aria-label={props.keysLabel ?? props.label}
      >
        <Show
          when={mode() === "3d"}
          fallback={<TacticalMap game={props.game} frame={props.frame} roster={props.roster} highlight={props.highlight} label={props.label} />}
        >
          <div class="v3-body">
            <Suspense fallback={<div class="v3-note v3-note--mid">Loading the 3D view…</div>}>
              <Viewer3D frame={props.frame} roster={props.roster} highlight={props.highlight} soughtAt={props.soughtAt} camera={props.camera} {...view3d()} />
            </Suspense>
          </div>
        </Show>
      </div>
      <Show when={props.controls}><div class="gv-controls">{props.controls}</div></Show>
      <Show when={props.children}><div class="gv-below">{props.children}</div></Show>
    </section>
  );
}
