// Causal variant of H2AR's field transforms. Every reference is in this
// sample. Preserve the v7 oct grid, including upper/lower octant seams.
import { unoct } from "../../src/api/pack";
const TAU = 2 * Math.PI,
  M = 16382;
const vectors = ["aim", "forward", "up", "f", "u"];
function oct([x, y, z]) {
  const d = Math.abs(x) + Math.abs(y) + Math.abs(z);
  x /= d;
  y /= d;
  if (z < 0) {
    const ox = x;
    x = (1 - Math.abs(y)) * (x >= 0 ? 1 : -1);
    y = (1 - Math.abs(ox)) * (y >= 0 ? 1 : -1);
  }
  return [x, y].map((c) =>
    Math.round((Math.min(1, Math.max(-1, c)) + 1) * 0.5 * M),
  );
}
function rotated(base, angle) {
  const c = Math.cos(angle),
    s = Math.sin(angle);
  return oct([base[0] * c - base[1] * s, base[0] * s + base[1] * c, base[2]]);
}
function dyaw(q) {
  const a = (q * TAU) / 16384;
  return a > Math.PI ? a - TAU : a;
}
function same(a, b) {
  return a.every((v, i) => Object.is(v, b[i]));
}

export function project(frame) {
  const out = { ...frame };
  for (const [name, rows] of Object.entries(frame)) {
    if (!Array.isArray(rows)) continue;
    const units = name === "players" || name === "actors";
    out[name] = rows.map((e, i) => {
      if (!e || typeof e !== "object" || Array.isArray(e)) return e;
      const row = { ...e };
      if (typeof e.yaw === "number") {
        const q = ((Math.round((e.yaw / TAU) * 16384) % 16384) + 16384) % 16384;
        if (Object.is(dyaw(q), e.yaw)) {
          row.$y = q;
          delete row.yaw;
        }
      }
      if (name === "hits") {
        const p = frame.players.findIndex((p) => same(p.pos, e.pos));
        if (p >= 0) {
          row.$p = p;
          delete row.pos;
        }
      }
      for (const k of vectors) {
        if (!Array.isArray(e[k])) continue;
        let xy = oct(e[k]);
        if (!same(e[k], unoct(xy[0], xy[1], M))) continue;
        let tag = "$" + k;
        if (
          units &&
          k === "aim" &&
          i &&
          rows[0].aim &&
          Number.isFinite(e.yaw) &&
          Number.isFinite(rows[0].yaw)
        ) {
          const ref = rotated(rows[0].aim, e.yaw - rows[0].yaw);
          const delta = [xy[0] - ref[0], xy[1] - ref[1]];
          if (delta.every((x) => Math.abs(x) <= 16)) {
            tag = "$r";
            xy = delta;
          }
        }
        row[tag] = xy;
        delete row[k];
      }
      return row;
    });
  }
  return out;
}

export function restore(frame) {
  for (const [name, rows] of Object.entries(frame)) {
    if (!Array.isArray(rows)) continue;
    for (const row of rows) {
      if (!row || typeof row !== "object" || Array.isArray(row)) continue;
      if ("$y" in row) {
        row.yaw = dyaw(row.$y);
        delete row.$y;
      }
      for (const k of [...vectors, "r"]) {
        const tag = "$" + k;
        if (!(tag in row)) continue;
        let [x, y] = row[tag];
        if (k === "r") {
          const ref = rotated(rows[0].aim, row.yaw - rows[0].yaw);
          x += ref[0];
          y += ref[1];
        }
        row[k === "r" ? "aim" : k] = unoct(x, y, M);
        delete row[tag];
      }
      if (name === "hits" && "$p" in row) {
        row.pos = frame.players[row.$p].pos;
        delete row.$p;
      }
    }
  }
  return frame;
}
