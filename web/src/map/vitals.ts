// Damage on the 2D map and 3D tags (docs/live/viewer-effects.md, "Damage").
// The damage report identifies a hit. The observed vitality change sizes it.

import type { SampledHit, SampledPlayer, SampledVehicle } from "../state/frames";
import { locator } from "./events";

export type DamageMode = "off" | "bars" | "numbers";
export const DAMAGE_KEY = "h2.viewer.damage";

function damageMode(value: unknown): DamageMode | null {
  return value === "off" || value === "bars" || value === "numbers" ? value : null;
}

function storage(): Storage | null {
  try { return typeof localStorage === "undefined" ? null : localStorage; } catch { return null; }
}

/** The address takes precedence over the saved preference. */
export function damagePref(address?: string | null, store?: Pick<Storage, "getItem"> | null): DamageMode {
  const mode = damageMode(address);
  if (mode) return mode;
  try { return damageMode((store === undefined ? storage() : store)?.getItem(DAMAGE_KEY)) ?? "bars"; } catch { return "bars"; }
}

export function saveDamagePref(mode: DamageMode, store: Pick<Storage, "setItem"> | null = storage()): void {
  try { store?.setItem(DAMAGE_KEY, mode); } catch { /* Storage may be disabled. */ }
}

type Position = { x: number; y: number; z: number };

export interface Vitals {
  shield: number;
  body: number;
  /** Total vitality at the start of the burst, while its chip shows. */
  chipFrom: number | null;
  /** Percent of a fresh spawn's vitality lost in this burst. */
  lost: number;
  state: "steady" | "hit" | "broken" | "recharging" | "over";
  from: Position | null;
  kind: string | null;
  /** Far edge of each visible chip, in the same units as the bar. */
  shieldChip: number | null;
  bodyChip: number | null;
  showBody: boolean;
  flare: boolean;
  breaking: boolean;
}

export function bodyColour(body: number): string {
  return body > 0.5 ? "#ffd36b" : body >= 0.25 ? "#ff8a3d" : "#f2452e";
}

export function vitalsNumber(vitals: Vitals | undefined): string {
  return vitals && vitals.chipFrom !== null ? String(Math.max(1, Math.round(vitals.lost))) : "";
}

interface RecentHit {
  tick: number;
  kind: string | null;
  from: Position | null;
}

interface Burst {
  body: number;
  shield: number;
  at: number;
  lost: number;
  hit?: RecentHit;
}

interface UnitVitals {
  shield: number;
  body: number;
  alive: boolean;
  broken: boolean;
  recharging: boolean;
  burst?: Burst;
  bodyDrop: number;
  shieldBreak: number;
}

interface VitalsFrame {
  players: readonly SampledPlayer[];
  actors?: readonly SampledPlayer[];
  vehicles?: readonly SampledVehicle[];
  hits?: { tick: number; hits: readonly SampledHit[] } | null;
}

const BURST_MS = 600;
const CHIP_HOLD_MS = 350;
const CHIP_DRAIN_MS = 250;
const HIT_TICKS = 3;

function chipRemaining(age: number, reducedMotion: boolean): number {
  if (age < BURST_MS) return 1;
  if (reducedMotion) return 0;
  return 1 - Math.min(1, Math.max(0, (age - BURST_MS - CHIP_HOLD_MS) / CHIP_DRAIN_MS));
}

function chipEdge(value: number, from: number, remaining: number): number | null {
  return remaining > 0 && from > value ? value + (from - value) * remaining : null;
}

/** One tracker per view. A quiet sample seeds a new baseline on a seek. */
export class VitalsTrack {
  private units = new Map<number, UnitVitals>();
  private hits = new Map<string, RecentHit>();
  private lastHitFrame: number | undefined;
  private lastSample: number | undefined;

  clear(): void {
    this.units.clear();
    this.hits.clear();
    this.lastHitFrame = undefined;
    this.lastSample = undefined;
  }

  observe(frame: VitalsFrame, now: number, opts: { quiet?: boolean; reducedMotion?: boolean } = {}): ReadonlyMap<number, Vitals> {
    const tick = frame.players.find(p => p.vitalsTick !== undefined)?.vitalsTick
      ?? frame.actors?.find(p => p.vitalsTick !== undefined)?.vitalsTick;
    if (opts.quiet || (tick !== undefined && this.lastSample !== undefined && tick < this.lastSample)) this.clear();
    this.lastSample = tick;
    if (opts.quiet) this.lastHitFrame = frame.hits?.tick;
    else if (frame.hits) this.observeHits(frame, frame.hits);

    const out = new Map<number, Vitals>();
    for (const units of [frame.players, frame.actors ?? []]) {
      for (const player of units) {
        const vitals = this.updateUnit(player, now, !!opts.reducedMotion, frame.hits?.tick);
        if (vitals) out.set(player.index, vitals);
      }
    }
    for (const id of this.units.keys()) if (!out.has(id)) this.units.delete(id);
    return out;
  }

  private observeHits(frame: VitalsFrame, batch: NonNullable<VitalsFrame["hits"]>): void {
    if (batch.tick === this.lastHitFrame) return;
    this.lastHitFrame = batch.tick;
    for (const [id, hit] of this.hits) if (batch.tick - hit.tick > HIT_TICKS) this.hits.delete(id);
    const where = locator(frame.players, frame.actors, frame.vehicles);
    for (const hit of batch.hits) {
      if (!hit.to || hit.amount === 0) continue;
      const tick = batch.tick - (hit.ago ?? 0);
      if (batch.tick - tick > HIT_TICKS || tick < (this.hits.get(hit.to)?.tick ?? -Infinity)) continue;
      const at = hit.from && hit.from !== hit.to ? where(hit.from) : null;
      this.hits.set(hit.to, { tick, kind: hit.kind, from: at && { x: at.x, y: at.y, z: at.z } });
    }
  }

  private updateUnit(player: SampledPlayer, now: number, reducedMotion: boolean, hitFrame: number | undefined): Vitals | null {
    const { body, shield } = player;
    if (body === undefined || shield === undefined) return null;
    const alive = player.vitalsAlive ?? (player.alive && player.stance !== "dead");
    const previous = this.units.get(player.index);
    const unit: UnitVitals = previous && (previous.alive || !alive)
      ? previous
      : { body, shield, alive, broken: false, recharging: false, bodyDrop: -Infinity, shieldBreak: -Infinity };
    const priorBody = unit.body, priorShield = unit.shield;
    const bodyLoss = Math.max(0, priorBody - body), shieldLoss = Math.max(0, priorShield - shield);
    if (bodyLoss > 0 || shieldLoss > 0) {
      if (!unit.burst || now - unit.burst.at >= BURST_MS) {
        unit.burst = { body: priorBody, shield: priorShield, at: now, lost: 0 };
      }
      const hit = this.hits.get(player.actor?.id ?? "p" + player.index);
      const tick = player.vitalsTick ?? hitFrame;
      unit.burst.hit = hit && tick !== undefined && Math.abs(tick - hit.tick) <= HIT_TICKS ? hit : undefined;
      unit.burst.at = now;
      unit.burst.lost += (bodyLoss + shieldLoss) * 50;
      if (bodyLoss > 0) unit.bodyDrop = now;
    }
    if (alive && priorShield > 0 && shield === 0) unit.shieldBreak = now;
    unit.broken = alive && shield === 0 && (priorShield > 0 || unit.broken);
    unit.recharging = alive && shield < 1 && (shield > priorShield || (shield === priorShield && unit.recharging));
    unit.body = body;
    unit.shield = shield;
    unit.alive = alive;
    this.units.set(player.index, unit);

    const age = now - (unit.burst?.at ?? -Infinity);
    const burst = unit.burst;
    const remaining = burst ? chipRemaining(age, reducedMotion) : 0;
    if (remaining === 0) unit.burst = undefined;
    const breakAge = now - unit.shieldBreak;
    const state = shield > 1 ? "over" : unit.broken ? "broken" : unit.recharging ? "recharging" : age < BURST_MS ? "hit" : "steady";
    return {
      body, shield, state,
      chipFrom: burst && remaining > 0 ? burst.body + burst.shield : null,
      lost: burst && remaining > 0 ? burst.lost : 0,
      from: burst && remaining > 0 ? burst.hit?.from ?? null : null,
      kind: burst && remaining > 0 ? burst.hit?.kind ?? null : null,
      bodyChip: burst ? chipEdge(body, burst.body, remaining) : null,
      shieldChip: burst ? chipEdge(shield, burst.shield, remaining) : null,
      showBody: body < 0.995 || now - unit.bodyDrop < 3000,
      flare: age < 120,
      breaking: reducedMotion ? breakAge < 300 : breakAge < 90 || (breakAge >= 180 && breakAge < 270),
    };
  }
}
