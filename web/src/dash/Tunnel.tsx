// The wireframe tunnel behind the dashboard, drawn on a canvas: a bent,
// twisting tube of rings and rails that drifts slowly towards the viewer.
// The vanishing point follows the screen (it sits behind the orb on the
// menu), so changing screen turns the tunnel.

import { createEffect, onCleanup, onMount } from "solid-js";

export interface TunnelAim { x: number; y: number; dim: number }

const RINGS = 54;
const RAILS = 84;
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
      const scroll = (t * 0.045) % 1;
      const spin = t * 0.012;

      // Each ring's points; ring 0 is the farthest.
      const pts: Array<Float32Array> = [];
      const alpha: number[] = [];
      for (let i = 0; i < RINGS; i++) {
        const d = (i + scroll) / RINGS; // 0 far .. 1 near
        const z = 1 / (0.045 + Math.pow(d, 2.3) * 2.6);
        const r = size * 0.9 / z;
        // The tube bends away as it recedes: far rings sit at the
        // vanishing point, near ones swing out to the lower right.
        const bend = Math.pow(d, 1.6);
        const cx = vx + bend * size * 0.2 + Math.sin(t * 0.07 + d * 3) * size * 0.012;
        const cy = vy + bend * size * 0.09 + Math.cos(t * 0.05 + d * 2) * size * 0.01;
        const twist = spin + d * 1.25;
        const ring = new Float32Array(RAILS * 2);
        for (let j = 0; j < RAILS; j++) {
          const a = (j / RAILS) * Math.PI * 2 + twist;
          const wob = 1 + 0.07 * Math.sin(a * 3 + d * 7 + t * 0.1) + 0.04 * Math.sin(a * 5 - d * 11);
          ring[j * 2] = cx + Math.cos(a) * r * 1.22 * wob;
          ring[j * 2 + 1] = cy + Math.sin(a) * r * 0.86 * wob;
        }
        pts.push(ring);
        // Fade in from the far end and out as rings pass the viewer.
        alpha.push(Math.min(1, d * 7) * Math.min(1, (1.04 - d) * 5));
      }

      const base = 0.36 - cur.dim * 0.27;
      ctx!.lineJoin = "round";
      for (let i = 0; i < RINGS; i++) {
        const ring = pts[i]!;
        const a = alpha[i]! * base;
        if (a <= 0.01) continue;
        ctx!.lineWidth = 0.55 + (i / RINGS) * 0.6;
        ctx!.strokeStyle = `rgba(118, 214, 52, ${a.toFixed(3)})`;
        ctx!.beginPath();
        for (let j = 0; j <= RAILS; j++) {
          const q = (j % RAILS) * 2;
          if (j === 0) ctx!.moveTo(ring[q]!, ring[q + 1]!); else ctx!.lineTo(ring[q]!, ring[q + 1]!);
        }
        ctx!.stroke();
        if (i === 0) continue;
        const prev = pts[i - 1]!;
        ctx!.strokeStyle = `rgba(104, 196, 44, ${(a * 0.8).toFixed(3)})`;
        ctx!.beginPath();
        for (let j = 0; j < RAILS; j++) {
          ctx!.moveTo(prev[j * 2]!, prev[j * 2 + 1]!);
          ctx!.lineTo(ring[j * 2]!, ring[j * 2 + 1]!);
        }
        ctx!.stroke();
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
      <div class="dx-bg-art" />
      <canvas class="dx-tunnel" ref={canvas} />
      <div class="dx-bg-vignette" />
    </div>
  );
}
