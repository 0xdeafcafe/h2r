// Things that happen at a moment and are drawn for a moment after: actions
// (a grenade thrown, with its arc), and damage (a line from attacker to
// victim, a hit marker and a number). Like map/shots.ts, a view watches the
// sampled frames go by, notes each frame's events once and fades them out.
// Pure: the 2D maps and the 3D scene draw them their own way.

import { actorIndex, aimOf, type Action, type Aim, type SampledHit, type SampledPlayer, type SampledVehicle } from "../state/frames";

// ---- actions ----

export interface ActionEvent { index: number; action: Action; player: SampledPlayer }

/** Notes each player's new actions once, however often their frame is sampled. */
export class ActionWatch {
  private seen = new Map<number, number>();

  /** The actions that started since the last look. `quiet` (a seek) notes
   *  them without reporting them. */
  observe(players: readonly SampledPlayer[], opts: { quiet?: boolean } = {}): ActionEvent[] {
    const out: ActionEvent[] = [];
    for (const p of players) {
      if (!p.actions?.length || p.actionTick === undefined) continue;
      if (this.seen.get(p.index) === p.actionTick) continue;
      this.seen.set(p.index, p.actionTick);
      if (opts.quiet) continue;
      for (const action of p.actions) out.push({ index: p.index, action, player: p });
    }
    return out;
  }

  clear(): void { this.seen.clear(); }
}

// ---- grenades ----

/** How long a thrown grenade is drawn in flight (ms). */
export const GRENADE_MS = 1150;
/** The throw, game units and seconds: speed along the aim, extra lift, and the pull down. */
export const GRENADE_SPEED = 6.5;
export const GRENADE_LOFT = 2.4;
export const GRENADE_GRAVITY = 8;
/** It leaves the hand this far above the feet. */
export const GRENADE_HAND = 0.55;
export const MAX_LIVE_GRENADES = 16;

export interface Grenade {
  id: number;
  index: number;
  born: number;
  /** Where the thrower's feet were, and where they were looking. */
  x: number; y: number; z: number;
  aim: Aim;
  /** "frag", "plasma", or null when the frame didn't say. */
  type: string | null;
}

/** Where a grenade is `ageMs` after the throw: a plain ballistic arc from
 *  the hand along the aim, resting where it comes back down to the
 *  thrower's footing. The frames don't carry the grenade itself, so this
 *  shows the throw, not where it really went. */
export function grenadeAt(g: Pick<Grenade, "x" | "y" | "z" | "aim">, ageMs: number): [number, number, number] {
  const vz = g.aim[2] * GRENADE_SPEED + GRENADE_LOFT;
  // When it is back at foot height: the positive root of hand + vz t - g t²/2 = 0.
  const land = (vz + Math.sqrt(vz * vz + 2 * GRENADE_GRAVITY * GRENADE_HAND)) / GRENADE_GRAVITY;
  const t = Math.min(Math.max(0, ageMs) / 1000, land);
  return [
    g.x + g.aim[0] * GRENADE_SPEED * t,
    g.y + g.aim[1] * GRENADE_SPEED * t,
    g.z + GRENADE_HAND + vz * t - (GRENADE_GRAVITY * t * t) / 2,
  ];
}

/** The path flown so far, as `n` points from the hand to where it is now. */
export function grenadeTrail(g: Pick<Grenade, "x" | "y" | "z" | "aim">, ageMs: number, n = 12): [number, number, number][] {
  return Array.from({ length: n }, (_, i) => grenadeAt(g, (ageMs * i) / (n - 1)));
}

export class GrenadeQueue {
  private list: Grenade[] = [];
  private nextId = 1;

  /** A throw seen now. */
  add(p: SampledPlayer, now: number): void {
    this.list.push({ id: this.nextId++, index: p.index, born: now, x: p.x, y: p.y, z: p.z, aim: aimOf(p), type: p.grenade ?? null });
    if (this.list.length > MAX_LIVE_GRENADES) this.list.shift();
  }

  /** The grenades in flight, each with its age and a fade over its last fifth. */
  live(now: number): Array<Grenade & { age: number; alpha: number }> {
    this.list = this.list.filter((g) => now - g.born < GRENADE_MS);
    return this.list.map((g) => {
      const age = now - g.born;
      return { ...g, age, alpha: Math.min(1, (GRENADE_MS - age) / (GRENADE_MS * 0.2)) };
    });
  }

  get busy(): boolean { return this.list.length > 0; }
  clear(): void { this.list = []; }
}

// ---- who and where ----

/** Who a hit names: a player index for "p<n>", else the index an actor of
 *  that id is drawn under. */
export function entityIndex(id: string): number {
  const m = /^p(\d+)$/.exec(id);
  return m ? Number(m[1]) : actorIndex(id);
}

export interface Where { x: number; y: number; z: number; alive: boolean }

/** Finds who or what a hit names among this moment's players, actors and
 *  vehicles; null when they aren't in the frame. */
export function locator(players: readonly SampledPlayer[], actors: readonly SampledPlayer[] | undefined, vehicles: readonly SampledVehicle[] | undefined): (id: string) => Where | null {
  return (id) => {
    const index = entityIndex(id);
    const p = (index >= 0 ? players : actors)?.find((q) => q.index === index);
    if (p) return { x: p.x, y: p.y, z: p.z, alive: p.alive && p.stance !== "dead" };
    const v = vehicles?.find((q) => q.id === id);
    return v ? { x: v.x, y: v.y, z: v.z, alive: true } : null;
  };
}

// ---- damage ----

/** The line from attacker to victim fades this fast; the marker and number stay longer; a kill longer still (ms). */
export const HIT_LINE_MS = 260;
export const HIT_MARK_MS = 850;
export const KILL_MARK_MS = 1500;
export const MAX_LIVE_HITS = 48;
/** Hits by the same attacker on the same victim within this long of a marker's start add to it (ms). */
export const HIT_FOLD_MS = 400;

export interface Hit {
  id: number;
  born: number;
  fromId: string | null;
  toId: string | null;
  /** Where the attacker was when the hit was seen (their feet); null when unknown or not in the frame. */
  from: { x: number; y: number; z: number } | null;
  /** Where the victim was. */
  x: number; y: number; z: number;
  amount: number | null;
  kind: string | null;
  weapon: string | null;
  /** The victim died of it (or within the marker's life). */
  kill: boolean;
}

export interface LiveHit extends Hit {
  age: number;
  /** The attacker-to-victim line's brightness: 1 to 0 over HIT_LINE_MS. */
  line: number;
  /** The marker's and number's brightness: holds, then fades over the last third. */
  mark: number;
  /** 0 to 1 over the marker's life: how far the number has floated up. */
  rise: number;
  /** The damage as shown, in percent of the victim's vitality: "35"; "" when the frame gave none. */
  text: string;
}

/** A hit's damage as shown. The frames give it as a share of the victim's
 *  vitality (body plus shield), so it reads in percent: "35" took a third. */
export function hitText(amount: number | null): string {
  if (amount === null) return "";
  const pct = amount * 100;
  return pct >= 1 ? String(Math.round(pct)) : pct > 0 ? "<1" : "0";
}

/** One frame's hits with the same attacker, victim and kind folded into
 *  one (a shotgun's pellets, a burst), their damage added up. A hit that
 *  says it did no damage at all (`amount` 0) is left out: recorded games
 *  carry runs of them a few ticks apart on one player (kind `guardians`,
 *  no attacker: 1358 of a Cyclotron game's 1580 hits), each drawn as a "0"
 *  marker on them. */
export function mergeHits(hits: readonly SampledHit[]): SampledHit[] {
  const out = new Map<string, SampledHit>();
  let n = 0;
  for (const h of hits) {
    if (h.amount === 0) continue;
    const key = h.to === null ? "#" + n++ : `${h.from}|${h.to}|${h.kind}`;
    const m = out.get(key);
    if (!m) { out.set(key, { ...h }); continue; }
    if (h.amount !== null) m.amount = (m.amount ?? 0) + h.amount;
    m.x = h.x; m.y = h.y; m.z = h.z;
  }
  return [...out.values()];
}

export class HitQueue {
  private list: Hit[] = [];
  private lastTick: number | null = null;
  private nextId = 1;

  /** Queues a frame's damage, once per frame. `where` finds the attacker
   *  (and tells whether the victim is already down). */
  observe(frame: { tick: number; hits: readonly SampledHit[] } | null | undefined, now: number, where: (id: string) => Where | null, opts: { quiet?: boolean } = {}): void {
    if (!frame || frame.tick === this.lastTick) return;
    this.lastTick = frame.tick;
    if (opts.quiet) return;
    for (const h of mergeHits(frame.hits)) {
      const a = h.from ? where(h.from) : null;
      // Sustained fire on one victim is one marker with a rising number, not a marker a frame.
      const going = h.to === null ? undefined : this.list.find((q) => q.fromId === h.from && q.toId === h.to && q.kind === h.kind && now - q.born < HIT_FOLD_MS);
      if (going) {
        if (h.amount !== null) going.amount = (going.amount ?? 0) + h.amount;
        going.x = h.x; going.y = h.y; going.z = h.z;
        if (a && going.from) going.from = { x: a.x, y: a.y, z: a.z };
        continue;
      }
      this.list.push({
        id: this.nextId++, born: now, fromId: h.from, toId: h.to,
        from: a && h.from !== h.to ? { x: a.x, y: a.y, z: a.z } : null,
        x: h.x, y: h.y, z: h.z, amount: h.amount, kind: h.kind, weapon: h.weapon,
        kill: false,
      });
    }
    this.markKills(now, where);
    if (this.list.length > MAX_LIVE_HITS) this.list.splice(0, this.list.length - MAX_LIVE_HITS);
  }

  /** A victim found dead while their markers are up was killed by the last
   *  of them: that one hit becomes the kill, the rest stay hits. */
  private markKills(now: number, where: (id: string) => Where | null): void {
    const last = new Map<string, Hit | null>();
    for (const h of this.list) {
      if (!h.toId) continue;
      // One kill a death: a victim with a kill marker up already has theirs.
      if (h.kill) last.set(h.toId, null);
      else if (now - h.born < HIT_MARK_MS && last.get(h.toId) !== null) last.set(h.toId, h);
    }
    for (const [id, h] of last) if (h) { const v = where(id); if (v && !v.alive) h.kill = true; }
  }

  /** The hits on screen. A kill is bigger, and stays longer. */
  live(now: number, where?: (id: string) => Where | null): LiveHit[] {
    const out: LiveHit[] = [];
    if (where) this.markKills(now, where);
    this.list = this.list.filter((h) => {
      const age = now - h.born;
      const life = h.kill ? KILL_MARK_MS : HIT_MARK_MS;
      if (age >= life) return false;
      const u = age / life;
      out.push({ ...h, age, line: age < HIT_LINE_MS ? (1 - age / HIT_LINE_MS) ** 2 : 0, mark: u < 0.66 ? 1 : (1 - u) / 0.34, rise: u, text: hitText(h.amount) });
      return true;
    });
    return out;
  }

  get busy(): boolean { return this.list.length > 0; }
  clear(): void { this.list = []; this.lastTick = null; }
}

// ---- all of it, for one view ----

/** What a view watches go by: actions, the grenades they throw, and damage. */
export class EventFeed {
  readonly actions = new ActionWatch();
  readonly grenades = new GrenadeQueue();
  readonly hits = new HitQueue();
  private where: (id: string) => Where | null = () => null;

  /** Takes in this moment's frame; returns the actions that just started. */
  observe(f: { players: readonly SampledPlayer[]; actors?: readonly SampledPlayer[]; vehicles?: readonly SampledVehicle[]; hits?: { tick: number; hits: readonly SampledHit[] } | null }, now: number, opts: { quiet?: boolean } = {}): ActionEvent[] {
    this.where = locator(f.players, f.actors, f.vehicles);
    const events = this.actions.observe(f.players, opts);
    if (f.actors?.length) events.push(...this.actions.observe(f.actors, opts));
    for (const e of events) if (e.action === "grenade" && e.player.alive) this.grenades.add(e.player, now);
    this.hits.observe(f.hits, now, this.where, opts);
    return events;
  }

  liveHits(now: number): LiveHit[] { return this.hits.live(now, this.where); }
  liveGrenades(now: number) { return this.grenades.live(now); }
  get busy(): boolean { return this.hits.busy || this.grenades.busy; }
}

/** The colours damage is drawn in: a kill red, a hit on a shield blue-white, anything else warm white. */
export function hitColour(h: Pick<Hit, "kill" | "kind">): string {
  return h.kill ? "#ff5a4d" : h.kind === "shield" ? "#9fd8ff" : "#ffe9a8";
}
export const grenadeColour = (type: string | null) => (type === "plasma" ? "#6fd0ff" : "#d8e86a");
