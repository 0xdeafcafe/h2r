import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import type { FilmWorld, FrameObject, MapOutline } from "../api/types";
import type { SampledHit, SampledMachine, SampledMachinePosition, SampledPlayer, SampledVehicle } from "../state/frames";
import type { ProjectileTrail } from "../state/projectile-trails";
import type { MapFeatures } from "../map/features";
import { onAnimationFrame } from "../map/scheduler";
import { draw, fitView, newView, sceneBounds, type Bounds, type Marker, type RosterEntry } from "../map/render";
import { readPalette, type MapPalette } from "../map/palette";
import { ShotQueue } from "../map/shots";
import { EventFeed } from "../map/events";
import { damagePref, VitalsTrack, type DamageMode } from "../map/vitals";

export interface MapFrame {
  players: SampledPlayer[];
  /** Death markers with their colour and age-based alpha already worked out. */
  markers: Marker[];
  seen: Bounds | null;
  /** AI-controlled bipeds (campaign) and vehicles in use, when the frames have them. */
  actors?: SampledPlayer[];
  vehicles?: SampledVehicle[];
  /** Damage in the frame behind this moment, with that frame's tick. */
  hits?: { tick: number; hits: SampledHit[] } | null;
  /** Recorded projectile paths up to the frame's render tick. */
  projectileTrails?: ProjectileTrail[];
  /** The level zone (structure BSP) loaded now, when the frames say. */
  bsp?: number | null;
  /** Machines that move (lift platforms, doors, big scripted things), when the frames have them. */
  machines?: SampledMachine[];
  /** Device animation endpoints by scenario placement index. */
  machinePositions?: ReadonlyMap<number, SampledMachinePosition>;
  /** Scenario objects moved or gone (`Frame.objects`), when the frames have them. */
  objects?: FrameObject[] | null;
  /** A cutscene is playing: there is no one to draw, and the views say so. */
  cinematic?: boolean;
  filmWorld?: FilmWorld;
  filmEffects?: import("../viewer/film-world").VisibleFilmEffect[];
}

export default function MapCanvas(props: {
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  outline: () => MapOutline | null;
  layer?: () => number | null;
  mini?: boolean;
  /** A picture that rarely changes (a finished game): redraw twice a second. */
  still?: boolean;
  highlight?: () => number | null;
  follow?: () => number | null;
  soughtAt?: () => number;
  damage?: DamageMode;
  /** The map's teleporters and lifts, to draw under the players. */
  features?: () => MapFeatures | null;
  class?: string;
  label?: string;
}) {
  let wrap!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  const view = newView();
  let pal: MapPalette = readPalette();
  let visible = true;
  let w = 0, h = 0, dpr = 1;
  let drawnAt = -Infinity;
  // Inside the stats site the map draws flat (plain dots, labels only for
  // the player under the pointer or the selected one).
  let flat = false;
  let mx: number | null = null, my: number | null = null;
  let hover: number | null = null;
  const shots = new ShotQueue();
  const feed = new EventFeed();
  const vitals = new VitalsTrack();
  // A cutscene: the frames hold no one, and the map says why.
  const [cutscene, setCutscene] = createSignal(false);
  const still = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Re-fit from scratch when the map outline changes.
  createEffect(() => { props.outline(); view.ready = false; });

  onMount(() => {
    pal = readPalette(wrap);
    flat = !!wrap.closest(".stats");
    const defaultDamage = damagePref(new URLSearchParams(window.location.search).get("dmg"));
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const resize = () => {
      dpr = Math.min(2, window.devicePixelRatio || 1);
      const r = wrap.getBoundingClientRect();
      w = Math.max(1, Math.round(r.width));
      h = Math.max(1, Math.round(r.height));
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = w + "px";
      canvas.style.height = h + "px";
      drawnAt = -Infinity;
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(wrap);
    const io = new IntersectionObserver((es) => { visible = es.some((e) => e.isIntersecting); });
    io.observe(wrap);
    const move = (e: PointerEvent) => {
      const r = wrap.getBoundingClientRect();
      mx = e.clientX - r.left;
      my = e.clientY - r.top;
    };
    const leave = () => { mx = null; my = null; };
    wrap.addEventListener("pointermove", move);
    wrap.addEventListener("pointerleave", leave);
    let lastSeek = 0;
    const stop = onAnimationFrame((now) => {
      // Off screen: draw once so there is a picture, then wait to be seen.
      if ((!visible && view.ready && drawnAt > 0) || document.hidden) return;
      if (props.still && now - drawnAt < 500 && view.ready) return;
      drawnAt = now;
      const f = props.frame(now);
      if (!!f.cinematic !== cutscene()) setCutscene(!!f.cinematic);
      // In flat style, hovering a dot is how a name shows: find the nearest
      // player to the pointer in canvas pixels.
      if (flat) {
        let found: number | null = null;
        if (mx !== null && my !== null && view.ready) {
          let best = 14;
          for (const p of f.players) {
            if (!p.alive) continue;
            const dx = w / 2 + (p.x - view.cx) * view.scale - mx;
            const dy = h / 2 - (p.y - view.cy) * view.scale - my;
            const d = Math.hypot(dx, dy);
            if (d <= best) { best = d; found = p.index; }
          }
        }
        hover = found;
        canvas.style.cursor = found === null ? "default" : "pointer";
      }
      const soughtAt = props.soughtAt?.() ?? 0;
      const quiet = soughtAt !== lastSeek;
      lastSeek = soughtAt;
      const mode = props.mini ? "off" : props.damage ?? defaultDamage;
      if (!props.mini) {
        shots.observe(f.players, now);
        if (f.actors?.length) shots.observe(f.actors, now);
        if (quiet) feed.hits.clear();
        feed.observe(f, now, { quiet });
      }
      const currentVitals = props.mini ? undefined : vitals.observe(f, now, { quiet, reducedMotion: still });
      const hits = props.mini ? [] : feed.liveHits(now);
      const scene = {
        shots: props.mini || still ? undefined : shots.live(now),
        firing: props.mini || !still ? undefined : shots.firingNow(now),
        damage: mode !== "off" && currentVitals ? {
          hits, vitals: currentVitals, numbers: mode === "numbers", followed: props.follow?.() ?? null,
        } : undefined,
        projectileTrails: props.mini || still ? undefined : f.projectileTrails,
        still,
        features: props.mini ? null : props.features?.() ?? null,
        players: f.players,
        actors: f.actors,
        vehicles: f.vehicles,
        markers: f.markers,
        roster: props.roster(),
        outline: props.outline(),
        seen: f.seen,
        layer: props.layer?.() ?? null,
        mini: !!props.mini,
        highlight: props.highlight?.() ?? null,
        flat,
        hover,
      };
      fitView(view, sceneBounds(scene), w, h, props.mini ? 8 : 36);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      draw(ctx, w, h, scene, view, pal);
    });
    onCleanup(() => {
      stop(); ro.disconnect(); io.disconnect();
      wrap.removeEventListener("pointermove", move);
      wrap.removeEventListener("pointerleave", leave);
    });
  });

  return (
    <div ref={wrap} class={`map-canvas ${props.mini ? "map-canvas--mini" : ""} ${props.class ?? ""}`}>
      <canvas ref={canvas} role="img" aria-label={props.label ?? "Live map"} />
      <Show when={cutscene() && !props.mini}>
        <div class="map-cutscene" role="status">Cutscene</div>
      </Show>
    </div>
  );
}
