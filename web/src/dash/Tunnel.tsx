// The wireframe tunnel behind the dashboard, drawn on a canvas: a bent,
// twisting tube of uneven, part-triangulated mesh that drifts slowly
// towards the viewer.
// The vanishing point follows the screen (it sits behind the orb on the
// menu), so changing screen turns the tunnel.

import { createEffect, onCleanup, onMount } from "solid-js";
import { hash01 } from "./shape";

export interface TunnelAim { x: number; y: number; dim: number }

const RINGS = 46;
const RAILS = 88;
/** Rings passing the viewer each second. */
const SPEED = 0.14;
/** Shared clock so the tunnel doesn't restart when the route remounts. */
const T0 = typeof performance !== "undefined" ? performance.now() : 0;
/** Where the tunnel was last pointing, kept across remounts. */
const cur: TunnelAim = { x: 0.35, y: 0.42, dim: 0 };

export default function Tunnel(props: { aim: TunnelAim }) {
  let canvas!: HTMLCanvasElement;
  let target: TunnelAim = props.aim;
  createEffect(() => { target = props.aim; });

  onMount(() => {
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    let w = 0, h = 0, dpr = 1, raf = 0, last = 0;
    const bufA = new Float32Array(RAILS * 2), bufB = new Float32Array(RAILS * 2);

    const resize = () => {
      dpr = Math.min(2, window.devicePixelRatio || 1);
      w = canvas.clientWidth; h = canvas.clientHeight;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
      paint(performance.now());
    };

    function paint(now: number) {
      const t = still.matches ? 40 : (now - T0) / 1000;
      const k = still.matches ? 1 : 0.07;
      cur.x += (target.x - cur.x) * k; cur.y += (target.y - cur.y) * k; cur.dim += (target.dim - cur.dim) * k;
      ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx!.clearRect(0, 0, w, h);
      const vx = cur.x * w, vy = cur.y * h;
      const size = Math.max(w, h * 1.5);
      // Rings keep their own shape as they come closer: ring `i` this
      // frame is the mesh row `i - gen`.
      const gen = Math.floor(t * SPEED);
      const scroll = t * SPEED - gen;
      const spin = t * 0.01;
      const reach = Math.hypot(Math.max(vx, w - vx), Math.max(vy, h - vy)) * 1.25;

      let prev: Float32Array | null = null;
      let ring = bufA;
      ctx!.lineJoin = "round";
      for (let i = 0; i < RINGS; i++) {
        const row = i - gen;
        const d = (i + scroll) / RINGS; // 0 far .. 1 near
        const r = size * (0.02 + 1.5 * Math.pow(d, 1.55));
        // The tube bends away as it recedes: far rings sit at the
        // vanishing point, near ones swing out to the lower right.
        const bend = Math.pow(d, 1.5);
        const cx = vx + bend * size * 0.22 + Math.sin(t * 0.07 + d * 3) * size * 0.01;
        const cy = vy + bend * size * 0.08 + Math.cos(t * 0.05 + d * 2) * size * 0.008;
        const twist = spin + d * 1.1;
        for (let j = 0; j < RAILS; j++) {
          // Each corner is nudged off the grid, so the cells come out
          // uneven like a hand-built mesh.
          const a = ((j + (hash01(row, j) - 0.5) * 0.36) / RAILS) * Math.PI * 2 + twist;
          const wob = 1 + 0.06 * Math.sin(a * 3 + d * 7 + t * 0.1) + 0.035 * Math.sin(a * 5 - d * 11) + (hash01(row, j + 4096) - 0.5) * 0.028;
          ring[j * 2] = cx + Math.cos(a) * r * 1.24 * wob;
          ring[j * 2 + 1] = cy + Math.sin(a) * r * 0.9 * wob;
        }
        // Fade in from the far end; rings wholly off screen are skipped.
        const a = Math.min(1, d * 9) * (0.27 - cur.dim * 0.12);
        if (a > 0.01 && r * 0.8 < reach) {
          ctx!.lineWidth = 0.7 + d * 0.9;
          ctx!.strokeStyle = `rgba(76, 190, 44, ${a.toFixed(3)})`;
          ctx!.beginPath();
          for (let j = 0; j <= RAILS; j++) {
            const q = (j % RAILS) * 2;
            if (j === 0) ctx!.moveTo(ring[q]!, ring[q + 1]!); else ctx!.lineTo(ring[q]!, ring[q + 1]!);
          }
          if (prev) {
            for (let j = 0; j < RAILS; j++) {
              const q = j * 2, n = ((j + 1) % RAILS) * 2;
              const x = ring[q]!, y = ring[q + 1]!;
              if (x < -80 && ring[n]! < -80 || x > w + 80 && ring[n]! > w + 80 || y < -80 && ring[n + 1]! < -80 || y > h + 80 && ring[n + 1]! > h + 80) continue;
              ctx!.moveTo(prev[q]!, prev[q + 1]!);
              ctx!.lineTo(x, y);
              // Most cells get a diagonal, one way or the other.
              const cut = hash01(row, j + 8192);
              if (cut < 0.22) { ctx!.moveTo(prev[q]!, prev[q + 1]!); ctx!.lineTo(ring[n]!, ring[n + 1]!); }
              else if (cut < 0.44) { ctx!.moveTo(prev[n]!, prev[n + 1]!); ctx!.lineTo(x, y); }
            }
          }
          ctx!.stroke();
        }
        prev = ring;
        ring = ring === bufA ? bufB : bufA;
      }
      canvas.parentElement?.style.setProperty("--vx", `${(cur.x * 100).toFixed(2)}%`);
      canvas.parentElement?.style.setProperty("--vy", `${(cur.y * 100).toFixed(2)}%`);
    }

    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      if (document.hidden || now - last < 30) return;
      last = now;
      paint(now);
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    const settle = () => { cancelAnimationFrame(raf); if (!still.matches) raf = requestAnimationFrame(loop); else paint(performance.now()); };
    settle();
    still.addEventListener("change", settle);
    // Reduced motion still needs a repaint when the aim changes.
    createEffect(() => { void props.aim; if (still.matches) queueMicrotask(() => paint(performance.now())); });
    onCleanup(() => { cancelAnimationFrame(raf); ro.disconnect(); still.removeEventListener("change", settle); });
  });

  return (
    <div class="dx-bg" aria-hidden="true">
      <div class="dx-bg-glow" />
      <div class="dx-bg-bands" />
      <canvas class="dx-tunnel" ref={canvas} />
      <div class="dx-bg-vignette" />
    </div>
  );
}
