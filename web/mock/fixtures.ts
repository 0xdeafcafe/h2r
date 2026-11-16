// The mock's map furniture, as the frame and geometry contracts' addendum
// on machines and features describes it: per map, a lift platform that
// rides up and down (a `machines` child of the map's glb, moved by the
// frames' `machines`), a big walking thing the glb has no node for (drawn
// from `models.objects.<kind>` or as a box), two pairs of teleporters with
// their links, and a gravity lift. All made up, and all worked out from the
// map's waypoints, so the glb (mock/arena.ts) and the frames agree.
//
// Two frame hooks, like mock/action.ts: a live frame carries only the
// machines that moved since the last one (and all of them the first time),
// a recorded frame all of them.

import type { FrameMachine } from "../src/api/types";
import type { MockMap } from "./maps";
import type { FrameHook } from "./world";

type V3 = [number, number, number];

export interface MockFixtures {
  platform: { node: string; id: string; kind: string; base: V3; travel: number; size: V3 };
  walker: { id: string; kind: string; from: V3; to: V3 };
  features: {
    teleporters: Array<{ id: string; pos: V3; facing: number; role: "sender" | "receiver" | "two_way"; channel: number }>;
    links: Array<{ from: string; to: string }>;
    lifts: Array<{ id: string; pos: V3; dir: V3; length: number; radius: number; source: string }>;
  };
}

const r2 = (n: number) => Math.round(n * 100) / 100;
const r4 = (n: number) => Math.round(n * 1e4) / 1e4;

/** A map's furniture; null for a map too small to have any. */
export function mockFixtures(map: MockMap): MockFixtures | null {
  const w = map.waypoints, n = w.length;
  if (n < 8) return null;
  const at = (f: number): V3 => [...w[Math.min(n - 1, Math.floor(n * f))]!];
  const facing = (p: V3) => r2(Math.atan2(-p[1], -p[0])); // towards the middle of the map
  const tele = (id: string, f: number, role: "sender" | "receiver" | "two_way", channel: number) => { const pos = at(f); return { id, pos, facing: facing(pos), role, channel }; };
  const p = at(0.36);
  return {
    platform: { node: "platform_0", id: "d3000", kind: "platform", base: [p[0] + 2.5, p[1] + 2.5, p[2]], travel: 5, size: [3.2, 3.2, 0.3] },
    walker: { id: "d3001", kind: "walker", from: at(0.2), to: at(0.8) },
    features: {
      teleporters: [tele("t0", 0, "sender", 0), tele("t1", 0.97, "receiver", 0), tele("t2", 0.3, "two_way", 1), tele("t3", 0.7, "two_way", 1)],
      links: [{ from: "t0", to: "t1" }, { from: "t2", to: "t3" }, { from: "t3", to: "t2" }],
      lifts: [{ id: "l0", pos: at(0.5), dir: [0, 0, 1], length: 6, radius: 1.2, source: "mock" }],
    },
  };
}

/** The platform's cycle: up for 4 s, 2 s at the top, down for 4 s, 2 s at the bottom. */
export function platformPosition(tick: number): number {
  const t = ((tick / 30) % 12 + 12) % 12;
  const u = t < 4 ? t / 4 : t < 6 ? 1 : t < 10 ? 1 - (t - 6) / 4 : 0;
  return u * u * (3 - 2 * u);
}

/** Every machine of a map at a tick. */
export function mockMachines(fx: MockFixtures, tick: number): FrameMachine[] {
  const pos = platformPosition(tick), b = fx.platform.base;
  // The walker paces between two points, a minute there and back, turning to face its way.
  const s = (tick / 30 / 60) % 1, there = s < 0.5, u = there ? s * 2 : 2 - s * 2;
  const { from, to } = fx.walker, dx = to[0] - from[0], dy = to[1] - from[1], d = Math.hypot(dx, dy) || 1, sign = there ? 1 : -1;
  return [
    { node: fx.platform.node, id: fx.platform.id, kind: fx.platform.kind, pos: [b[0], b[1], r2(b[2] + fx.platform.travel * pos)], forward: [1, 0, 0], up: [0, 0, 1], position: r4(pos) },
    { id: fx.walker.id, kind: fx.walker.kind, pos: [r2(from[0] + dx * u), r2(from[1] + dy * u), r2(from[2] + (to[2] - from[2]) * u)], forward: [r4((dx / d) * sign), r4((dy / d) * sign), 0], up: [0, 0, 1] },
  ];
}

/** The live hook (what moved) and the recorded-frame hook (everything). */
export function machineHooks(): { frame: FrameHook; record: FrameHook } {
  const sent = new Map<string, Map<string, string>>();
  const frame: FrameHook = (f, game) => {
    const fx = mockFixtures(game.map);
    if (!fx) return;
    let last = sent.get(game.id);
    if (!last) { last = new Map(); sent.set(game.id, last); if (sent.size > 12) sent.delete(sent.keys().next().value!); }
    const moved = mockMachines(fx, game.tick).filter((m) => {
      const key = JSON.stringify([m.pos, m.forward, m.position]);
      if (last!.get(m.id) === key) return false;
      last!.set(m.id, key);
      return true;
    });
    if (moved.length) f.machines = moved;
  };
  const record: FrameHook = (kept, game) => {
    const fx = mockFixtures(game.map);
    if (fx) kept.machines = mockMachines(fx, game.tick);
  };
  return { frame, record };
}
