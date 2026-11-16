// The 3D view itself: a ViewerScene in a box, with its camera controls.
// Loaded lazily (it brings three.js), by ViewerWindow and the pop-out page.

import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import type { MapFrame } from "../components/MapCanvas";
import type { RosterEntry } from "../map/render";
import { onAnimationFrame } from "../map/scheduler";
import { ViewerScene, hasWebGl, type ViewMode } from "./scene";
import "./viewer.css";

export interface Viewer3DProps {
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  /** The map's glb, or null when the server has none (a grid floor then). */
  glb: () => string | null;
  highlight?: () => number | null;
  /** Wall time of the last seek: players jump instead of gliding. */
  soughtAt?: () => number;
  /** Camera to open with, and a player to follow. */
  mode?: ViewMode;
  follow?: number | null;
  ao?: boolean;
}

const reducedMotion = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

export default function Viewer3D(props: Viewer3DProps) {
  let host!: HTMLDivElement;
  let scene: ViewerScene | null = null;
  const [failed, setFailed] = createSignal<string | null>(null);
  const [mode, setMode] = createSignal<ViewMode>(props.mode ?? "orbit");
  const [follow, setFollow] = createSignal<number | null>(props.follow ?? null);
  const [ao, setAo] = createSignal(props.ao ?? true);
  const [mapState, setMapState] = createSignal<"loading" | "map" | "grid">("grid");
  // The server's art manifest takes a moment: only say "no geometry" once it has had one.
  const [late, setLate] = createSignal(false);
  const lateTimer = setTimeout(() => setLate(true), 2500);
  onCleanup(() => clearTimeout(lateTimer));
  const people = createMemo(() => [...props.roster()].map(([index, r]) => ({ index, ...r })).sort((a, b) => a.name.localeCompare(b.name)));

  const pick = (m: ViewMode, index: number | null = follow()) => {
    if (m === "follow" && index === null) index = people()[0]?.index ?? null;
    if (m === "follow" && index === null) return;
    setMode(m);
    setFollow(m === "follow" ? index : follow());
    scene?.setMode(m, index);
  };

  onMount(() => {
    if (!hasWebGl()) { setFailed("This browser can't draw 3D here (WebGL is off or unavailable). The 2D map still shows the game."); return; }
    try {
      scene = new ViewerScene(host, { reducedMotion: reducedMotion(), onPick: (i) => pick("follow", i) });
    } catch {
      setFailed("The 3D view couldn't start on this device. The 2D map still shows the game.");
      return;
    }
    const s = scene;
    s.onAoDropped = () => setAo(false);
    if (ao()) setAo(s.setAo(true));
    const resize = () => { const r = host.getBoundingClientRect(); s.resize(Math.round(r.width), Math.round(r.height)); };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(host);
    let visible = true;
    const io = new IntersectionObserver((es) => { visible = es.some((e) => e.isIntersecting); });
    io.observe(host);
    let seen = 0, started = false;
    const stop = onAnimationFrame((now) => {
      if (!visible || document.hidden) return;
      const sought = props.soughtAt?.() ?? 0;
      const frame = props.frame(now);
      s.update(now, frame, props.roster(), props.highlight?.() ?? null, sought !== seen);
      seen = sought;
      // The opening camera, once there are players to follow.
      if (!started && frame.players.length) { started = true; if (mode() !== "orbit") pick(mode(), follow()); }
    });
    onCleanup(() => { stop(); ro.disconnect(); io.disconnect(); s.dispose(); scene = null; });
  });

  // The map: load it when its address is known; a grid floor otherwise.
  createEffect(on(() => props.glb(), (url) => {
    if (!scene) return;
    if (!url) { setMapState("grid"); return; }
    setMapState("loading");
    void scene.loadMap(url).then((ok) => { if (props.glb() === url) setMapState(ok ? "map" : "grid"); });
  }));

  return (
    <div class="v3">
      <Show when={!failed()} fallback={<p class="v3-fail">{failed()}</p>}>
        <div ref={host} class="v3-host" />
        <div class="v3-tools" role="group" aria-label="Camera">
          <button classList={{ on: mode() === "orbit" }} aria-pressed={mode() === "orbit"} onClick={() => pick("orbit")} title="Drag to orbit, scroll to zoom, right-drag to pan">Orbit</button>
          <button classList={{ on: mode() === "top" }} aria-pressed={mode() === "top"} onClick={() => pick("top")} title="Straight down, north up">Top-down</button>
          <select
            classList={{ on: mode() === "follow" }}
            aria-label="Follow a player"
            value={mode() === "follow" && follow() !== null ? String(follow()) : ""}
            onChange={(e) => { const v = e.currentTarget.value; if (v === "") pick("orbit"); else pick("follow", +v); }}
          >
            <option value="">Follow…</option>
            <For each={people()}>{(p) => <option value={p.index}>{p.name}</option>}</For>
          </select>
          <button onClick={() => scene?.reset()} title="Back to the whole map">Reset</button>
          <button classList={{ on: ao() }} aria-pressed={ao()} onClick={() => setAo(scene?.setAo(!ao()) ?? false)} title="Ambient occlusion: soft shading in corners (costs frame rate)">AO</button>
        </div>
        <Show when={mapState() === "loading"}><div class="v3-note">Loading the map…</div></Show>
        <Show when={mapState() === "grid" && late()}><div class="v3-note">No 3D geometry for this map; the floor grid is fitted to where players have been.</div></Show>
      </Show>
    </div>
  );
}
