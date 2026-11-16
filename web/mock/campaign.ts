// The mock's campaign: AI-controlled bipeds and vehicles added to a game's
// frames, as the frame contract's `actors` and `vehicles`. It is a frame
// hook (World.frameHooks), so nothing else in the mock knows about it: any
// game whose mode is "campaign" gets them, and with H2_MOCK_ACTORS=1 every
// game does, so the views can be tried on the mock's ordinary games too.
//
// About thirty actors wander between the map's waypoints, a mix of sides
// and species (some with no stand-in model, so the fallbacks run), firing
// a burst now and then. Two vehicles: one driven by the game's first player
// with an AI gunner aboard, one driven by an AI. It keeps its own random
// numbers, so the world's simulation is the same with or without it.

import type { FrameActor, FrameVehicle } from "../src/api/types";
import type { FrameHook } from "./world";

type V3 = [number, number, number];

const CAST: Array<{ species: string; team: string; weapon: string; count: number; speed: number }> = [
  { species: "grunt", team: "covenant", weapon: "plasma_pistol", count: 12, speed: 2.2 },
  { species: "jackal", team: "covenant", weapon: "plasma_pistol", count: 6, speed: 2.8 },
  { species: "elite", team: "covenant", weapon: "plasma_rifle", count: 3, speed: 3.6 },
  { species: "hunter", team: "covenant", weapon: "hunter_cannon", count: 2, speed: 1.8 },
  { species: "marine", team: "human", weapon: "battle_rifle", count: 6, speed: 3.2 },
];
export const MOCK_ACTORS = CAST.reduce((n, c) => n + c.count, 0) + 2; // and the two aboard vehicles

interface Walker { id: string; species: string; team: string; weapon: string; pos: V3; yaw: number; target: number; speed: number; burst: number }
interface State { rand: () => number; walkers: Walker[]; ghost: Walker; gunner: Walker; lastTick: number }

function rng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => { s = (Math.imul(s, 1664525) + 1013904223) >>> 0; return s / 0x100000000; };
}

const r2 = (n: number) => Math.round(n * 100) / 100;
const r4 = (n: number) => Math.round(n * 1e4) / 1e4;
const hash = (s: string) => { let h = 7; for (let i = 0; i < s.length; i++) h = (Math.imul(h, 31) + s.charCodeAt(i)) | 0; return h >>> 0; };

/** A frame hook that puts the campaign's AI and vehicles into the frames of
 *  the games `wants` picks. */
export function campaignHook(wants: (game: { mode?: string }) => boolean = (g) => g.mode === "campaign"): FrameHook {
  const games = new Map<string, State>();
  return (frame, game) => {
    if (!wants(game) || !game.map.waypoints.length) return;
    const wps = game.map.waypoints;
    let st = games.get(game.id);
    if (!st) {
      const rand = rng(hash(game.id));
      let n = 0;
      const walker = (species: string, team: string, weapon: string, speed: number): Walker => {
        const w = wps[Math.floor(rand() * wps.length)]!;
        return { id: "e" + (0x1000 + n++).toString(16), species, team, weapon, pos: [w[0], w[1], w[2]], yaw: rand() * Math.PI * 2, target: Math.floor(rand() * wps.length), speed: speed * (0.8 + rand() * 0.4), burst: 0 };
      };
      st = {
        rand,
        walkers: CAST.flatMap((c) => Array.from({ length: c.count }, () => walker(c.species, c.team, c.weapon, c.speed))),
        ghost: walker("elite", "covenant", "plasma_rifle", 7),
        gunner: walker("marine", "human", "battle_rifle", 0),
        lastTick: game.tick,
      };
      games.set(game.id, st);
      if (games.size > 8) games.delete(games.keys().next().value!);
    }
    const dt = Math.min(0.5, Math.max(0, (game.tick - st.lastTick) / 30));
    st.lastTick = game.tick;
    const { rand } = st;
    const walk = (w: Walker) => {
      const t = wps[w.target]!;
      const dx = t[0] - w.pos[0], dy = t[1] - w.pos[1], d = Math.hypot(dx, dy);
      if (d < 1) {
        const near = wps.map((p, i) => [i, Math.hypot(p[0] - t[0], p[1] - t[1]) + Math.abs(p[2] - t[2]) * 2] as const).filter(([, dd]) => dd > 3 && dd < 9);
        w.target = near.length ? near[Math.floor(rand() * near.length)]![0] : Math.floor(rand() * wps.length);
        return;
      }
      const step = Math.min(d, w.speed * dt);
      w.pos = [w.pos[0] + (dx / d) * step, w.pos[1] + (dy / d) * step, w.pos[2] + ((t[2] - w.pos[2]) / Math.max(d, 1)) * step];
      let diff = Math.atan2(dy, dx) - w.yaw;
      while (diff > Math.PI) diff -= Math.PI * 2;
      while (diff < -Math.PI) diff += Math.PI * 2;
      w.yaw += diff * 0.25;
      if (w.burst <= 0 && rand() < 0.12 * dt) w.burst = 2 + Math.floor(rand() * 5);
    };
    const actor = (w: Walker, vehicle?: string): FrameActor => {
      const a: FrameActor = {
        id: w.id, species: w.species, pos: [r2(w.pos[0]), r2(w.pos[1]), r2(w.pos[2])], yaw: r2(w.yaw),
        aim: [r4(Math.cos(w.yaw)), r4(Math.sin(w.yaw)), 0], weapon: w.weapon, team: w.team,
      };
      if (w.burst > 0) { w.burst -= 1; a.shots = 1; }
      if (vehicle) a.vehicle = vehicle;
      return a;
    };
    const vehicle = (id: string, kind: string, pos: V3, yaw: number): FrameVehicle =>
      ({ id, kind, pos: [r2(pos[0]), r2(pos[1]), r2(pos[2])], forward: [r4(Math.cos(yaw)), r4(Math.sin(yaw)), 0], up: [0, 0, 1] });

    const actors: FrameActor[] = [], vehicles: FrameVehicle[] = [];
    for (const w of st.walkers) { walk(w); actors.push(actor(w)); }
    // An AI on a fast bike of its own.
    walk(st.ghost);
    vehicles.push(vehicle("f2001", "ghost", st.ghost.pos, st.ghost.yaw));
    actors.push(actor(st.ghost, "f2001"));
    // The first player drives, an AI on the gun; on foot again while they are down.
    const driver = frame.players[0];
    if (driver?.alive) {
      driver.vehicle = "f2000";
      vehicles.push(vehicle("f2000", "warthog", driver.pos, driver.yaw));
      st.gunner.pos = driver.pos;
      st.gunner.yaw = driver.yaw + Math.sin(game.tick / 40) * 1.2;
      if (st.gunner.burst <= 0 && rand() < 0.3 * dt) st.gunner.burst = 6;
      actors.push(actor(st.gunner, "f2000"));
    }
    frame.actors = actors;
    frame.vehicles = vehicles;
  };
}
