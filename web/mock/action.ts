// The mock's action: what the frame contract's animation fields carry,
// made up for every game's frames so the views have something to show.
// Players and AI get a `stance` and a `speed`; now and then they jump (and
// leave the ground for a moment), crouch, throw a grenade, swing, reload
// after a burst, and report a weapon swap; whoever fires turns on someone
// near and the frame's `hits` say who was hit and how hard, a grenade's
// blast hits whoever is near where it lands, and a death comes with the
// hit that did it. Amounts are shares of the victim's vitality, as the
// contract has them.
//
// Two frame hooks (World.frameHooks and World.recordHooks): the first
// decorates each live frame, the second gives the frame recorded once a
// second everything that happened in that second, as a real sender does.
// It keeps its own random numbers, so the world's simulation is the same
// with or without it.

import type { Frame, FrameActor, FrameHit, FramePlayer } from "../src/api/types";
import type { FrameHook } from "./world";

type V3 = [number, number, number];
type Body = FramePlayer | FrameActor;

interface Mover {
  pos: V3; tick: number; alive: boolean; weapon: string | null;
  airFrom: number; airUntil: number; crouchUntil: number;
  /** Was firing last frame (a reload may follow), and who at. */
  firing: boolean; target: string | null; lastHitBy: string | null;
  /** Actions since the last recorded frame. */
  since: string[];
}
interface Blast { tick: number; pos: V3; from: string }
interface State { rand: () => number; movers: Map<string, Mover>; hits: FrameHit[]; blasts: Blast[] }

function rng(seed: number) {
  let s = seed >>> 0 || 1;
  return () => { s = (Math.imul(s, 1664525) + 1013904223) >>> 0; return s / 0x100000000; };
}
const hash = (s: string) => { let h = 11; for (let i = 0; i < s.length; i++) h = (Math.imul(h, 33) + s.charCodeAt(i)) | 0; return h >>> 0; };
const r2 = (n: number) => Math.round(n * 100) / 100;
const r4 = (n: number) => Math.round(n * 1e4) / 1e4;
const idOf = (b: Body) => ("index" in b ? "p" + b.index : b.id);
/** An actor is in the frame while it lives; a player says. */
const living = (b: Body) => !("index" in b) || b.alive;

export const JUMP_TICKS = 20;
export const JUMP_HEIGHT = 0.9;
const MAX_ACTIONS = 8, MAX_HITS = 64;

/** The live hook and the recorded-frame hook, sharing what they know of each game. */
export function actionHooks(): { frame: FrameHook; record: FrameHook } {
  const games = new Map<string, State>();
  const frame: FrameHook = (f, game) => {
    let st = games.get(game.id);
    if (!st) {
      st = { rand: rng(hash(game.id)), movers: new Map(), hits: [], blasts: [] };
      games.set(game.id, st);
      if (games.size > 12) games.delete(games.keys().next().value!);
    }
    const { rand, movers } = st;
    const bodies: Body[] = [...f.players, ...(f.actors ?? [])];
    const hits: FrameHit[] = [];
    const nearest = (from: Body, reach: number): Body | null => {
      let best: Body | null = null, bd = reach;
      for (const o of bodies) {
        if (o === from || !living(o)) continue;
        // The AI doesn't fight itself.
        if (!("index" in from) && !("index" in o) && from.team === o.team) continue;
        const d = Math.hypot(o.pos[0] - from.pos[0], o.pos[1] - from.pos[1], o.pos[2] - from.pos[2]);
        if (d < bd) { bd = d; best = o; }
      }
      return best;
    };
    for (const b of bodies) {
      const id = idOf(b), player = "index" in b, alive = living(b);
      let m = movers.get(id);
      if (!m) { m = { pos: [...b.pos], tick: game.tick, alive, weapon: b.weapon ?? null, airFrom: 0, airUntil: 0, crouchUntil: 0, firing: false, target: null, lastHitBy: null, since: [] }; movers.set(id, m); }
      const dt = Math.max(1, game.tick - m.tick) / 30;
      const speed = Math.hypot(b.pos[0] - m.pos[0], b.pos[1] - m.pos[1]) / dt;
      const actions: string[] = [];
      // Per second, for a player; the AI does less.
      const chance = (perSecond: number) => rand() < perSecond * dt * (player ? 1 : 0.35);
      if (alive && !b.vehicle) {
        const grounded = game.tick >= m.airUntil;
        if (grounded && chance(0.09)) { actions.push("jump"); m.airFrom = game.tick; m.airUntil = game.tick + JUMP_TICKS; m.crouchUntil = 0; }
        else if (grounded && game.tick >= m.crouchUntil && chance(0.05)) m.crouchUntil = game.tick + 45 + Math.floor(rand() * 75);
        if (chance(0.07)) {
          actions.push("grenade");
          const aim = b.aim ?? [Math.cos(b.yaw), Math.sin(b.yaw), 0];
          st.blasts.push({ tick: game.tick + 34, pos: [b.pos[0] + aim[0] * 6, b.pos[1] + aim[1] * 6, b.pos[2]], from: id });
        }
        if (chance(0.03)) {
          actions.push("melee");
          const t = nearest(b, 1.6);
          if (t) hits.push({ from: id, to: idOf(t), weapon: b.weapon ?? undefined, pos: [...t.pos], amount: 0.7, kind: "melee" });
        }
        if (m.firing && !b.shots && rand() < 0.3) actions.push("reload");
        if (b.weapon && m.weapon && b.weapon !== m.weapon) actions.push("switch");
        // Firing: turn on whoever is near, and some of the shots land.
        if (b.shots) {
          const t = (m.firing && m.target ? bodies.find((o) => idOf(o) === m!.target && living(o)) : null) ?? nearest(b, 22);
          m.target = t ? idOf(t) : null;
          if (t) {
            const dx = t.pos[0] - b.pos[0], dy = t.pos[1] - b.pos[1], dz = t.pos[2] - b.pos[2], d = Math.hypot(dx, dy, dz) || 1;
            b.yaw = r2(Math.atan2(dy, dx));
            b.aim = [r4(dx / d), r4(dy / d), r4(dz / d)];
            if (rand() < (player ? 0.6 : 0.3)) {
              hits.push({ from: id, to: idOf(t), weapon: b.weapon ?? undefined, pos: [...t.pos], amount: Math.round((4 + rand() * 9) * b.shots * 10) / 1000, kind: "bullet" });
              const tm = movers.get(idOf(t));
              if (tm) tm.lastHitBy = id;
            }
          }
        }
      }
      // A death comes with the hit that did it.
      if (player && m.alive && !alive) hits.push({ ...(m.lastHitBy ? { from: m.lastHitBy } : {}), to: id, pos: [...m.pos], amount: 0.45, kind: m.lastHitBy ? "bullet" : "fall" });
      if (!alive) { m.airUntil = 0; m.crouchUntil = 0; m.lastHitBy = null; }
      const air = alive && game.tick < m.airUntil;
      if (air) {
        const u = (game.tick - m.airFrom) / JUMP_TICKS;
        b.pos = [b.pos[0], b.pos[1], r2(b.pos[2] + JUMP_HEIGHT * 4 * u * (1 - u))];
      }
      b.stance = !alive ? "dead" : b.vehicle ? "vehicle" : air ? "airborne" : game.tick < m.crouchUntil ? "crouch" : "stand";
      b.speed = r2(alive ? speed : 0);
      if (player) b.grenade = (b as FramePlayer).index % 2 ? "plasma" : "frag";
      else if (actions.includes("grenade")) b.grenade = "plasma";
      if (actions.length) { b.actions = actions.slice(0, MAX_ACTIONS); m.since.push(...actions); }
      m.firing = !!b.shots;
      // The jump's height isn't part of where they are for the next speed.
      m.pos = [b.pos[0], b.pos[1], air ? m.pos[2] : b.pos[2]];
      m.tick = game.tick; m.alive = alive; m.weapon = b.weapon ?? null;
    }
    // Grenades land: whoever is near takes the blast.
    st.blasts = st.blasts.filter((x) => {
      if (game.tick < x.tick) return true;
      for (const o of bodies) {
        if (!living(o)) continue;
        const d = Math.hypot(o.pos[0] - x.pos[0], o.pos[1] - x.pos[1], o.pos[2] - x.pos[2]);
        if (d < 3.5) hits.push({ from: x.from, to: idOf(o), weapon: "grenade", pos: [...o.pos], amount: Math.round(60 * (1 - d / 4)) / 100, kind: "explosion" });
      }
      return false;
    });
    if (hits.length) { f.hits = hits.slice(0, MAX_HITS); st.hits.push(...hits); }
  };
  const record: FrameHook = (kept, game) => {
    const st = games.get(game.id);
    if (!st) return;
    const give = <T extends Body>(b: T): T => {
      const m = st.movers.get(idOf(b));
      const { actions: _, ...rest } = b;
      const since = m?.since.splice(0) ?? [];
      return (since.length ? { ...rest, actions: since.slice(0, MAX_ACTIONS) } : rest) as T;
    };
    kept.players = kept.players.map(give);
    if (kept.actors) kept.actors = kept.actors.map(give);
    const hits = st.hits.splice(0);
    if (hits.length) kept.hits = hits.slice(-MAX_HITS);
    else delete (kept as Partial<Frame>).hits;
  };
  return { frame, record };
}
