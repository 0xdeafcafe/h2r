// Made-up maps for the mock. Outlines are invented shapes; one map has no
// outline at all so the web app's auto-fitted grid gets exercised.

import type { MapOutline } from "../src/api/types";

export interface MockMap {
  name: string;
  outline: MapOutline | null;
  waypoints: [number, number, number][];
}

function rect(x0: number, y0: number, x1: number, y1: number): [number, number][] {
  return [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
}

function blob(cx: number, cy: number, r: number, n: number, wobble: number, seed: number): [number, number][] {
  const out: [number, number][] = [];
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2;
    const rr = r * (1 + wobble * Math.sin(a * 3 + seed) * Math.cos(a * 2 - seed));
    out.push([+(cx + Math.cos(a) * rr).toFixed(2), +(cy + Math.sin(a) * rr).toFixed(2)]);
  }
  return out;
}

function inPoly(p: [number, number], poly: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i]!, [xj, yj] = poly[j]!;
    if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

function gridWaypoints(polys: [number, number][][], z: number, step: number, bounds: [number, number, number, number]) {
  const out: [number, number, number][] = [];
  for (let x = bounds[0]; x <= bounds[2]; x += step)
    for (let y = bounds[1]; y <= bounds[3]; y += step)
      if (polys.some((p) => inPoly([x, y], p))) out.push([x, y, z]);
  return out;
}

// Teacup: a cross-shaped courtyard with a ring of balconies above it.
const teacupGround: [number, number][][] = [
  rect(-18, -18, 18, 18),
  rect(-42, -8, -18, 8), rect(18, -8, 42, 8), rect(-8, -42, 8, -18), rect(-8, 18, 8, 42),
  blob(-34, -34, 7, 18, 0.12, 1), blob(34, 34, 7, 18, 0.12, 2), blob(34, -34, 7, 18, 0.12, 3), blob(-34, 34, 7, 18, 0.12, 4),
];
const teacupUpper: [number, number][][] = [
  rect(-30, 22, 30, 30), rect(-30, -30, 30, -22), rect(-30, -22, -22, 22), rect(22, -22, 30, 22),
];

// Orchard: soft islands joined by little bridges.
const orchardFloor: [number, number][][] = [
  blob(0, 0, 16, 28, 0.15, 0.5), blob(-38, 14, 13, 24, 0.2, 1.7), blob(36, -12, 14, 24, 0.18, 2.9),
  blob(8, 40, 11, 22, 0.2, 4.1), blob(-6, -40, 12, 22, 0.2, 5.3),
  rect(-28, 2, -12, 8), rect(12, -8, 24, -3), rect(2, 14, 7, 30), rect(-6, -30, -1, -14),
];

export const MAPS: MockMap[] = [
  {
    name: "teacup",
    outline: {
      map: "teacup",
      bounds: { min: [-45, -45, -2], max: [45, 45, 12] },
      layers: [
        { z: [-2, 4], polygons: teacupGround },
        { z: [4, 12], polygons: teacupUpper },
      ],
    },
    waypoints: [
      ...gridWaypoints(teacupGround, 0.5, 6, [-42, -42, 42, 42]),
      ...gridWaypoints(teacupUpper, 8, 6, [-30, -30, 30, 30]),
    ],
  },
  {
    name: "orchard",
    outline: {
      map: "orchard",
      bounds: { min: [-55, -55, -1], max: [55, 55, 6] },
      layers: [{ z: [-1, 6], polygons: orchardFloor }],
    },
    waypoints: gridWaypoints(orchardFloor, 1, 5, [-55, -55, 55, 55]),
  },
  {
    // No outline: the app sketches it from where players go.
    name: "lighthouse",
    outline: null,
    waypoints: (() => {
      const out: [number, number, number][] = [];
      for (let r = 6; r <= 36; r += 6)
        for (let i = 0; i < r; i++) {
          const a = (i / r) * Math.PI * 2;
          out.push([+(Math.cos(a) * r * 1.4).toFixed(1), +(Math.sin(a) * r).toFixed(1), r < 12 ? 6 : 0]);
        }
      return out;
    })(),
  },
];
