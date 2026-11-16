// The 2D map over a pre-drawn picture of the level: one of the extractor's
// views (top-down, or an iso corner) with players and knockout marks
// plotted on it by the view's world_to_pixel matrix. Wheel (with Ctrl, or
// once the map has been clicked), pinch, drag and the buttons pan and zoom
// it; the view eases to where it is going unless reduced motion is asked for.

import { For, createEffect, on, onCleanup, onMount } from "solid-js";
import type { MapFrame } from "../components/MapCanvas";
import { onAnimationFrame } from "./scheduler";
import { readPalette, type MapPalette } from "./palette";
import type { RosterEntry } from "./render";
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

  const fit = () => fitPicture(w, h, def().width, def().height, 10);
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
    wrap.addEventListener("wheel", wheel, { passive: false });
    canvas.addEventListener("pointerdown", down);
    canvas.addEventListener("pointermove", move);
    canvas.addEventListener("pointerup", up);
    canvas.addEventListener("pointercancel", up);
    canvas.addEventListener("pointerleave", leave);
    canvas.addEventListener("dblclick", dbl);
    document.addEventListener("pointerdown", away);

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
        const x = sx(px), y = sy(py), r = 5;
        ctx.globalAlpha = Math.max(0, Math.min(1, k.alpha));
        ctx.lineCap = "round";
        for (const [colour, width] of [["rgba(0,0,0,.8)", 4], [k.colour, 2]] as const) {
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
      canvas.style.cursor = pointers.size ? "grabbing" : hover !== null ? "pointer" : target.k > fit().k * 1.001 ? "grab" : "default";
      const r = 5;
      for (const q of placed) {
        const info = roster.get(q.p.index);
        const colour = info?.colour ?? "#999";
        const picked = hover === q.p.index || highlight === q.p.index;
        const rr = picked ? r * 1.4 : r;
        // A nose for facing, then the dot over it.
        const a = yawOnView(m, q.p.yaw);
        ctx.fillStyle = colour;
        ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(q.x + Math.cos(a) * (rr + 5), q.y + Math.sin(a) * (rr + 5));
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
    });
    onCleanup(() => {
      stop(); ro.disconnect(); io.disconnect();
      wrap.removeEventListener("wheel", wheel);
      document.removeEventListener("pointerdown", away);
    });
  });

  return (
    <div ref={wrap} class="map-canvas map-view">
      <canvas ref={canvas} role="img" aria-label={props.label ?? "Map"} />
      <div class="map-zoom" role="group" aria-label="Zoom">
        <button onClick={() => zoom(1.6)} aria-label="Zoom in" title="Zoom in">+</button>
        <button onClick={() => zoom(1 / 1.6)} aria-label="Zoom out" title="Zoom out">−</button>
        <button onClick={() => { target = fit(); if (still) cur = { ...target }; }} aria-label="Fit the whole map" title="Fit the whole map">⤢</button>
      </div>
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
