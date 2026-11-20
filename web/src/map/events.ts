// Things that happen at a moment and are drawn for a moment after: actions
// and damage (a direction, a focused line and a kill confirm). A view watches the
// sampled frames go by, notes each frame's events once and fades them out.
// Pure: the 2D maps and the 3D scene draw them their own way.

import { actorIndex, type Action, type SampledHit, type SampledPlayer, type SampledVehicle } from "../state/frames";

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

// ---- who and where ----

/** Who a hit names: a player index for "p<n>", else the index an actor of
 *  that id is drawn under. */
export function entityIndex(id: string): number {
  const m = /^p(\d+)$/.exec(id);
  return m ? Number(m[1]) : actorIndex(id);
}

export interface Where { x: number; y: number; z: number; alive: boolean; vehicle?: boolean }

/** Finds who or what a hit names among this moment's players, actors and
 *  vehicles; null when they aren't in the frame. */
export function locator(players: readonly SampledPlayer[], actors: readonly SampledPlayer[] | undefined, vehicles: readonly SampledVehicle[] | undefined): (id: string) => Where | null {
  return (id) => {
    const index = entityIndex(id);
    const p = (index >= 0 ? players : actors)?.find((q) => q.index === index);
    if (p) return { x: p.x, y: p.y, z: p.z, alive: p.vitalsAlive ?? (p.alive && p.stance !== "dead") };
    const v = vehicles?.find((q) => q.id === id);
    return v ? { x: v.x, y: v.y, z: v.z, alive: true, vehicle: true } : null;
  };
}

// ---- damage ----

export const HIT_LINE_MS = 260;
export const HIT_MARK_MS = 450;
export const KILL_LINE_MS = 400;
export const KILL_MARK_MS = 900;
export const MAX_LIVE_HITS = 48;
export const HIT_FOLD_MS = 400;

export interface Hit {
  id: number;
  /** Time of the newest hit contributing to this mark. */
  born: number;
  fromId: string | null;
  toId: string | null;
  /** The attacker's recorded position, when known. */
  from: { x: number; y: number; z: number } | null;
  x: number; y: number; z: number;
  kind: string | null;
  weapon: string | null;
  vehicle: boolean;
  kill: boolean;
  killedAt: number | null;
}

export interface LiveHit extends Hit {
  age: number;
  line: number;
  mark: number;
  direction: number;
}

/** One frame's hits with the same attacker, victim and kind folded into
 *  one (a shotgun's pellets, a burst), their damage added up. A hit that
 *  says it did no damage (`amount` 0) is left out. */
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
  private down = new Set<string>();

  /** Queues a frame's damage, once per frame. `where` finds the attacker
   *  (and tells whether the victim is already down). */
  observe(frame: { tick: number; hits: readonly SampledHit[] } | null | undefined, now: number, where: (id: string) => Where | null, opts: { quiet?: boolean } = {}): void {
    if (opts.quiet) {
      this.clear();
      this.lastTick = frame?.tick ?? null;
      return;
    }
    if (!frame || frame.tick === this.lastTick) return;
    this.lastTick = frame.tick;
    for (const h of mergeHits(frame.hits)) {
      const a = h.from ? where(h.from) : null;
      const from = a && h.from !== h.to ? { x: a.x, y: a.y, z: a.z } : null;
      // Repeated hits from the same attack refresh one direction tick.
      const going = h.to === null ? undefined : this.list.find(q => !q.kill
        && q.fromId === h.from && q.toId === h.to && q.kind === h.kind && now - q.born < HIT_FOLD_MS);
      if (going) {
        Object.assign(going, { born: now, from, x: h.x, y: h.y, z: h.z });
        continue;
      }
      this.list.push({
        id: this.nextId++, born: now, fromId: h.from, toId: h.to,
        from, x: h.x, y: h.y, z: h.z, kind: h.kind, weapon: h.weapon,
        vehicle: !!(h.to && where(h.to)?.vehicle),
        kill: false, killedAt: null,
      });
    }
    this.markKills(now, where);
    if (this.list.length > MAX_LIVE_HITS) this.list.splice(0, this.list.length - MAX_LIVE_HITS);
  }

  /** A victim found dead while their markers are up was killed by the last
   *  of them: that one hit becomes the kill, the rest stay hits. */
  private markKills(now: number, where: (id: string) => Where | null): void {
    for (const id of this.down) {
      const victim = where(id);
      if (!victim || victim.alive) this.down.delete(id);
    }
    const last = new Map<string, Hit>();
    for (const h of this.list) {
      if (!h.toId || h.kill || h.vehicle || this.down.has(h.toId) || now - h.born >= HIT_MARK_MS) continue;
      if (h.born >= (last.get(h.toId)?.born ?? -Infinity)) last.set(h.toId, h);
    }
    for (const [id, h] of last) {
      const victim = where(id);
      if (!victim || victim.alive) continue;
      h.kill = true;
      h.killedAt = now;
      this.down.add(id);
    }
  }

  live(now: number, where?: (id: string) => Where | null): LiveHit[] {
    const out: LiveHit[] = [];
    if (where) this.markKills(now, where);
    this.list = this.list.filter(h => {
      const age = Math.max(0, now - (h.killedAt ?? h.born));
      const life = h.kill ? KILL_MARK_MS : HIT_MARK_MS;
      const lineLife = h.kill ? KILL_LINE_MS : HIT_LINE_MS;
      if (age >= life) return false;
      const u = age / life;
      const direction = Math.max(0, 1 - (now - h.born) / HIT_MARK_MS);
      out.push({
        ...h, age, direction,
        line: age < lineLife ? (1 - age / lineLife) ** 2 : 0,
        mark: h.kill ? Math.min(1, (1 - u) * 3) : direction,
      });
      return true;
    });
    return out;
  }

  get busy(): boolean { return this.list.length > 0; }
  clear(): void { this.list = []; this.lastTick = null; this.down.clear(); }
}

// ---- all of it, for one view ----

/** What a view watches go by: actions and damage. */
export class EventFeed {
  readonly actions = new ActionWatch();
  readonly hits = new HitQueue();
  private where: (id: string) => Where | null = () => null;

  /** Takes in this moment's frame; returns the actions that just started. */
  observe(f: { players: readonly SampledPlayer[]; actors?: readonly SampledPlayer[]; vehicles?: readonly SampledVehicle[]; hits?: { tick: number; hits: readonly SampledHit[] } | null }, now: number, opts: { quiet?: boolean } = {}): ActionEvent[] {
    this.where = locator(f.players, f.actors, f.vehicles);
    if (opts.quiet) this.actions.clear();
    const events = this.actions.observe(f.players, opts);
    if (f.actors?.length) events.push(...this.actions.observe(f.actors, opts));
    this.hits.observe(f.hits, now, this.where, opts);
    return events;
  }

  liveHits(now: number): LiveHit[] { return this.hits.live(now, this.where); }
  get busy(): boolean { return this.hits.busy; }
}

/** The colours damage is drawn in: a kill red, a hit on a shield blue-white, anything else warm white. */
export function hitColour(h: Pick<Hit, "kill" | "kind">): string {
  return h.kill ? "#ff5a4d" : h.kind === "shield" ? "#9fd8ff" : "#ffe9a8";
}
