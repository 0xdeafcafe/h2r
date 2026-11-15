import { createEffect, onCleanup, onMount } from "solid-js";
import type { MapOutline } from "../api/types";
import type { SampledPlayer } from "../state/frames";
import { onAnimationFrame } from "../map/scheduler";
import { draw, fitView, newView, sceneBounds, type Bounds, type Marker, type RosterEntry } from "../map/render";
import { readPalette, type MapPalette } from "../map/palette";

export interface MapFrame {
  players: SampledPlayer[];
  /** Death markers with their colour and age-based alpha already worked out. */
  markers: Marker[];
  seen: Bounds | null;
}

export default function MapCanvas(props: {
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  outline: () => MapOutline | null;
  layer?: () => number | null;
  mini?: boolean;
  highlight?: () => number | null;
  class?: string;
  label?: string;
}) {
  let wrap!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  const view = newView();
  let pal: MapPalette = readPalette();
  let visible = true;
  let w = 0, h = 0, dpr = 1;

  // Re-fit from scratch when the map outline changes.
  createEffect(() => { props.outline(); view.ready = false; });

  onMount(() => {
    pal = readPalette(wrap);
    const ctx = canvas.getContext("2d", { alpha: false });
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
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(wrap);
    const io = new IntersectionObserver((es) => { visible = es.some((e) => e.isIntersecting); });
    io.observe(wrap);
    const stop = onAnimationFrame((now) => {
      if (!visible || document.hidden) return;
      const f = props.frame(now);
      const scene = {
        players: f.players,
        markers: f.markers,
        roster: props.roster(),
        outline: props.outline(),
        seen: f.seen,
        layer: props.layer?.() ?? null,
        mini: !!props.mini,
        highlight: props.highlight?.() ?? null,
      };
      fitView(view, sceneBounds(scene), w, h, props.mini ? 8 : 36);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      draw(ctx, w, h, scene, view, pal);
    });
    onCleanup(() => { stop(); ro.disconnect(); io.disconnect(); });
  });

  return (
    <div ref={wrap} class={`map-canvas ${props.mini ? "map-canvas--mini" : ""} ${props.class ?? ""}`}>
      <canvas ref={canvas} role="img" aria-label={props.label ?? "Live map"} />
    </div>
  );
}
