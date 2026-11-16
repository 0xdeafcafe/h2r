// The 2D map over a pre-drawn picture of the level: one of the extractor's
// views (top-down, or an iso corner) with players and knockout marks
// plotted on it by the view's world_to_pixel matrix. Wheel (with Ctrl, or
// once the map has been clicked), pinch, drag and the buttons pan and zoom
// it; the view eases to where it is going unless reduced motion is asked for.

import { For, Show, createEffect, on, onCleanup, onMount } from "solid-js";
import type { MapFrame } from "../components/MapCanvas";
import { onAnimationFrame } from "./scheduler";
import { readPalette, type MapPalette } from "./palette";
import { EMPTY_VEHICLE_ALPHA, EMPTY_VEHICLE_COLOUR, drawActor, drawGrenade, drawHit, drawVehicle, strokeAim, strokeTracer, vehicleColour, type RosterEntry } from "./render";
import { actorColour, vehicleBox } from "../viewer/models";
import { ShotQueue } from "./shots";
import { EventFeed, grenadeTrail } from "./events";
import {
  clampPan, depthOf, easePanZoom, fitPicture, viewAxis, viewLabel, worldToPixel, yawOnView, zoomAbout,
  type MapViewDef, type MapViews, type PanZoom,
} from "./views";

const MAX_ZOOM = 8;

export function ViewPicker(props: { views: MapViews; value: string; onChange: (id: string) => void }) {
  return (
    <div class="layers" role="group" aria-label="Map view">
      <For each={props.views.views}>
        {(v) => (
          <button aria-pressed={props.value === v.id} classList={{ on: props.value === v.id }} onClick={() => props.onChange(v.id)} title={v.id === "top" ? "Straight down" : `From the ${viewLabel(v.id)} corner`}>
            {viewLabel(v.id)}
          </button>
        )}
      </For>
    </div>
  );
}

export default function ViewMap(props: {
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  views: MapViews;
  /** The view to show (an id in `views`). */
  view: string;
  image: (id: string) => string;
  highlight?: () => number | null;
  label?: string;
  /** A thumbnail: small dots, no names, and no panning or zooming. */
  mini?: boolean;
  /** The picture couldn't be loaded: the caller falls back to the grid. */
  onError?: () => void;
}) {
  let wrap!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  let pal: MapPalette = readPalette();
  let w = 0, h = 0, dpr = 1;
  let visible = true;
  let cur: PanZoom | null = null, target: PanZoom | null = null;
  let last = 0;
  let mx: number | null = null, my: number | null = null;
  let engaged = false;
  const images = new Map<string, HTMLImageElement>();
  const shots = new ShotQueue();
  const feed = new EventFeed();
  const still = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  const def = (): MapViewDef => props.views.views.find((v) => v.id === props.view) ?? props.views.views[0]!;
  const picture = (id: string): HTMLImageElement => {
    let img = images.get(id);
    if (!img) {
      img = new Image();
      img.decoding = "async";
      img.onerror = () => props.onError?.();
      img.src = props.image(id);
      images.set(id, img);
    }
    return img;
  };

  const fit = () => fitPicture(w, h, def().width, def().height, props.mini ? 3 : 10);
  const settle = (p: PanZoom) => clampPan(p, w, h, def().width, def().height);
  const refit = () => { target = fit(); cur = { ...target }; };
  const zoom = (factor: number, cx = w / 2, cy = h / 2) => {
    if (!target) return;
    const f = fit();
    target = settle(zoomAbout(target, cx, cy, factor, f.k, f.k * MAX_ZOOM));
    if (still) cur = { ...target };
  };

  // A different view (or map): start again from the whole picture.
  createEffect(on(() => [props.view, props.views] as const, () => { if (w) refit(); }, { defer: true }));

  onMount(() => {
    pal = readPalette(wrap);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const resize = () => {
      dpr = Math.min(2, window.devicePixelRatio || 1);
      const r = wrap.getBoundingClientRect();
      const first = !w;
      const wasFit = !first && target !== null && Math.abs(target.k - fit().k) < 1e-9;
      w = Math.max(1, Math.round(r.width));
      h = Math.max(1, Math.round(r.height));
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      canvas.style.width = w + "px";
      canvas.style.height = h + "px";
      if (first || wasFit || !target) refit();
      else { target = settle(target); cur = { ...target }; }
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(wrap);
    const io = new IntersectionObserver((es) => { visible = es.some((e) => e.isIntersecting); });
    io.observe(wrap);

    const at = (e: { clientX: number; clientY: number }) => {
      const r = wrap.getBoundingClientRect();
      return [e.clientX - r.left, e.clientY - r.top] as const;
    };
    // Wheel zooms once the map is in use (or with Ctrl, which is also how a
    // trackpad pinch arrives); otherwise it stays the page's scroll.
    const wheel = (e: WheelEvent) => {
      if (!engaged && !e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      const [x, y] = at(e);
      zoom(Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0018)), x, y);
    };
    // Drag pans; two fingers pinch.
    const pointers = new Map<number, { x: number; y: number }>();
    let pinch = 0;
    const down = (e: PointerEvent) => {
      engaged = true;
      const [x, y] = at(e);
      pointers.set(e.pointerId, { x, y });
      canvas.setPointerCapture(e.pointerId);
      if (pointers.size === 2) { const [a, b] = [...pointers.values()]; pinch = Math.hypot(a!.x - b!.x, a!.y - b!.y); }
    };
    const move = (e: PointerEvent) => {
      const [x, y] = at(e);
      mx = x; my = y;
      const p = pointers.get(e.pointerId);
      if (!p || !target) return;
      if (pointers.size === 1) {
        target = settle({ ...target, x: target.x + x - p.x, y: target.y + y - p.y });
        cur = { ...target };
      }
      p.x = x; p.y = y;
      if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        const d = Math.hypot(a!.x - b!.x, a!.y - b!.y);
        if (pinch > 0 && d > 0) { zoom(d / pinch, (a!.x + b!.x) / 2, (a!.y + b!.y) / 2); cur = { ...target }; }
        pinch = d;
      }
    };
    const up = (e: PointerEvent) => { pointers.delete(e.pointerId); pinch = 0; };
    const leave = () => { mx = null; my = null; };
    const dbl = (e: MouseEvent) => { const [x, y] = at(e); zoom(e.shiftKey ? 0.5 : 2, x, y); };
    const away = (e: PointerEvent) => { if (!wrap.contains(e.target as Node)) engaged = false; };
    if (!props.mini) {
      wrap.addEventListener("wheel", wheel, { passive: false });
      canvas.addEventListener("pointerdown", down);
      canvas.addEventListener("pointermove", move);
      canvas.addEventListener("pointerup", up);
      canvas.addEventListener("pointercancel", up);
      canvas.addEventListener("pointerleave", leave);
      canvas.addEventListener("dblclick", dbl);
      document.addEventListener("pointerdown", away);
    }

    const stop = onAnimationFrame((now) => {
      if (!visible || document.hidden || !cur || !target) return;
      const dt = last ? now - last : 16;
      last = now;
      cur = still ? { ...target } : easePanZoom(cur, target, dt);
      const v = def();
      const m = v.world_to_pixel;
      const f = props.frame(now);
      const roster = props.roster();
      const highlight = props.highlight?.() ?? null;
      const sx = (px: number) => cur!.x + px * cur!.k, sy = (py: number) => cur!.y + py * cur!.k;

      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const img = picture(v.id);
      if (img.complete && img.naturalWidth) {
        ctx.imageSmoothingQuality = "high";
        ctx.drawImage(img, cur.x, cur.y, v.width * cur.k, v.height * cur.k);
      }

      // Knockout marks.
      for (const k of f.markers) {
        const [px, py] = worldToPixel(m, k.x, k.y, k.z);
        const x = sx(px), y = sy(py), r = props.mini ? 2.5 : 5;
        ctx.globalAlpha = Math.max(0, Math.min(1, k.alpha));
        ctx.lineCap = "round";
        for (const [colour, width] of (props.mini ? [["rgba(0,0,0,.8)", 2.5], [k.colour, 1.25]] : [["rgba(0,0,0,.8)", 4], [k.colour, 2]]) as [string, number][]) {
          ctx.strokeStyle = colour; ctx.lineWidth = width;
          ctx.beginPath();
          ctx.moveTo(x - r, y - r); ctx.lineTo(x + r, y + r);
          ctx.moveTo(x + r, y - r); ctx.lineTo(x - r, y + r);
          ctx.stroke();
        }
      }
      ctx.globalAlpha = 1;

      // Players, far ones first, with the one under the pointer named.
      const axis = viewAxis(m);
      const alive = f.players.filter((p) => p.alive).sort((a, b) => depthOf(axis, a.x, a.y, a.z) - depthOf(axis, b.x, b.y, b.z));
      const placed = alive.map((p) => { const [px, py] = worldToPixel(m, p.x, p.y, p.z); return { p, x: sx(px), y: sy(py) }; });
      let hover: number | null = null;
      if (mx !== null && my !== null && !pointers.size) {
        let best = 14;
        for (const q of placed) { const d = Math.hypot(q.x - mx, q.y - my); if (d <= best) { best = d; hover = q.p.index; } }
      }
      if (!props.mini) canvas.style.cursor = pointers.size ? "grabbing" : hover !== null ? "pointer" : target.k > fit().k * 1.001 ? "grab" : "default";
      const r = props.mini ? 3 : 5;
      // Shots: a fading line from the player along their aim, plotted in the
      // world so an iso view shows it rise and fall. Reduced motion has no
      // fading lines: the aim line is drawn bright while they fire instead.
      let firing: ReadonlySet<number> | null = null;
      if (!props.mini) {
        shots.observe(f.players, now);
        if (f.actors?.length) shots.observe(f.actors, now);
        if (still) firing = shots.firingNow(now);
        else for (const shot of shots.live(now)) {
          const [ax, ay, az] = shot.aim;
          const [p0x, p0y] = worldToPixel(m, shot.x, shot.y, shot.z), [p1x, p1y] = worldToPixel(m, shot.x + ax * 9, shot.y + ay * 9, shot.z + az * 9);
          const x0 = sx(p0x), y0 = sy(p0y), dx = sx(p1x) - x0, dy = sy(p1y) - y0, len = Math.hypot(dx, dy);
          if (len < 2) continue;
          const k = Math.max(1, 46 / len);
          strokeTracer(ctx, x0 + (dx / len) * r, y0 + (dy / len) * r, x0 + dx * k, y0 + dy * k, shot.alpha);
        }
      }
      // Vehicles in use, then the AI (small squares, never named), under the players.
      if (!props.mini) for (const veh of f.vehicles ?? []) {
        const [px, py] = worldToPixel(m, veh.x, veh.y, veh.z), [fx, fy] = worldToPixel(m, veh.x + veh.forward[0], veh.y + veh.forward[1], veh.z + veh.forward[2]);
        const [len, wid] = vehicleBox(veh.kind), unit = Math.hypot(m[0][0], m[0][1], m[0][2]) * cur.k, k = Math.max(unit, 9 / wid);
        if (!veh.occupied) ctx.globalAlpha = EMPTY_VEHICLE_ALPHA;
        drawVehicle(ctx, sx(px), sy(py), Math.atan2(fy - py, fx - px), len * k, wid * k, veh.occupied ? vehicleColour(veh.id, f.players, f.actors, roster) : EMPTY_VEHICLE_COLOUR);
        ctx.globalAlpha = 1;
      }
      for (const a of f.actors ?? []) {
        if (!a.alive || a.vehicle) continue;
        const [px, py] = worldToPixel(m, a.x, a.y, a.z);
        drawActor(ctx, sx(px), sy(py), props.mini ? 1.5 : 2.5, actorColour(a.actor?.team));
      }
      for (const q of placed) {
        const info = roster.get(q.p.index);
        const colour = info?.colour ?? "#999";
        const picked = hover === q.p.index || highlight === q.p.index;
        const rr = picked ? r * 1.4 : r;
        // Where they are looking, when the frame says.
        if (!props.mini && q.p.aim) {
          const [ax, ay, az] = q.p.aim;
          const [ex, ey] = worldToPixel(m, q.p.x + ax, q.p.y + ay, q.p.z + az), [ox, oy] = worldToPixel(m, q.p.x, q.p.y, q.p.z);
          const hot = firing?.has(q.p.index) ?? false;
          const flat = Math.min(1, Math.hypot(ex - ox, ey - oy) / Math.max(1e-6, Math.hypot(m[0][0], m[0][1], m[0][2])));
          strokeAim(ctx, q.x, q.y, ex - ox, ey - oy, rr, rr + (hot ? 22 : 11) * flat, colour, hot);
        }
        // A nose for facing, then the dot over it.
        const a = yawOnView(m, q.p.yaw);
        ctx.fillStyle = colour;
        ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(q.x + Math.cos(a) * (rr + (props.mini ? 3 : 5)), q.y + Math.sin(a) * (rr + (props.mini ? 3 : 5)));
        ctx.lineTo(q.x + Math.cos(a + 0.7) * rr, q.y + Math.sin(a + 0.7) * rr);
        ctx.lineTo(q.x + Math.cos(a - 0.7) * rr, q.y + Math.sin(a - 0.7) * rr);
        ctx.closePath();
        ctx.fill(); ctx.stroke();
        ctx.beginPath();
        ctx.arc(q.x, q.y, rr, 0, Math.PI * 2);
        ctx.fill(); ctx.stroke();
      }
      for (const q of placed) {
        const info = roster.get(q.p.index);
        if (info && (hover === q.p.index || highlight === q.p.index)) label(ctx, pal, q.x, q.y, r, info.name);
      }
      // Over everyone: grenades in flight and damage, plotted in the world like the shots.
      if (!props.mini) {
        feed.observe(f, now);
        const at = (x: number, y: number, z: number) => { const [px, py] = worldToPixel(m, x, y, z); return [sx(px), sy(py)] as const; };
        if (!still) for (const g of feed.liveGrenades(now)) drawGrenade(ctx, grenadeTrail(g, g.age).map((q) => at(...q)), g.alpha, g.type);
        for (const hit of feed.liveHits(now)) drawHit(ctx, hit.from ? at(hit.from.x, hit.from.y, hit.from.z + 0.5) : null, at(hit.x, hit.y, hit.z), hit, still);
      }
    });
    onCleanup(() => {
      stop(); ro.disconnect(); io.disconnect();
      wrap.removeEventListener("wheel", wheel);
      document.removeEventListener("pointerdown", away);
    });
  });

  return (
    <div ref={wrap} class="map-canvas map-view" classList={{ "map-view--mini": !!props.mini }}>
      <canvas ref={canvas} role="img" aria-label={props.label ?? "Map"} />
      <Show when={!props.mini}><div class="map-zoom" role="group" aria-label="Zoom">
        <button onClick={() => zoom(1.6)} aria-label="Zoom in" title="Zoom in">+</button>
        <button onClick={() => zoom(1 / 1.6)} aria-label="Zoom out" title="Zoom out">−</button>
        <button onClick={() => { target = fit(); if (still) cur = { ...target }; }} aria-label="Fit the whole map" title="Fit the whole map">⤢</button>
      </div></Show>
    </div>
  );
}

function label(ctx: CanvasRenderingContext2D, pal: MapPalette, x: number, y: number, r: number, name: string) {
  ctx.font = "600 12px " + pal.font;
  const tw = ctx.measureText(name).width;
  const ly = y - r - 12;
  ctx.fillStyle = "rgba(0, 0, 0, 0.82)";
  ctx.beginPath();
  ctx.roundRect(x - tw / 2 - 6, ly - 9, tw + 12, 18, 3);
  ctx.fill();
  ctx.fillStyle = "#fff";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(name, x, ly + 0.5);
}
