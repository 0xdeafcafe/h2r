// Replays of ended games: 1 Hz recorded frames plus the game's events,
// sampled at any tick.

import type { Frame, PlayerKilled, PlayerScore, ScoreChanged, WireEvent } from "../api/types";
import { sampleActors, sampleBsp, sampleFrames, sampleHits, sampleMachines, sampleVehicles, type SampledMachine, type SampledPlayer, type SampledVehicle, TICKS_PER_SECOND } from "./frames";

export interface ReplayKill {
  tick: number;
  victim: number;
  killer: number | null;
  weapon: string | null;
  kind: PlayerKilled["kind"];
  position: number;
  /** Where the victim died: the event's `pos`, else from the frames. */
  at: { x: number; y: number; z: number } | null;
}

export interface ScoreSnapshot {
  tick: number;
  players: Record<number, PlayerScore>;
  teams: Record<number, number>;
}

export class ReplayTimeline {
  readonly frames: Frame[];
  readonly kills: ReplayKill[];
  readonly scores: ScoreSnapshot[];
  readonly startTick: number;
  readonly endTick: number;

  constructor(frames: Frame[], events: WireEvent[]) {
    this.frames = [...frames].sort((a, b) => a.tick - b.tick);
    // Recorded frames may also arrive as GameFrame events; merge them in.
    const seen = new Set(this.frames.map((f) => f.tick));
    for (const ev of events) {
      if (ev.type === "GameFrame") {
        const f = ev.data as Frame;
        if (f && typeof f.tick === "number" && !seen.has(f.tick)) { this.frames.push(f); seen.add(f.tick); }
      }
    }
    this.frames.sort((a, b) => a.tick - b.tick);

    const sorted = [...events].sort((a, b) => a.position - b.position);
    this.kills = [];
    this.scores = [];
    let endTick = this.frames.length ? this.frames[this.frames.length - 1]!.tick : 0;
    let running: ScoreSnapshot = { tick: 0, players: {}, teams: {} };
    for (const ev of sorted) {
      if (ev.type === "PlayerKilled") {
        const d = ev.data as PlayerKilled;
        // The event carries the victim's position; older logs may not, so
        // fall back to the frames.
        let at: ReplayKill["at"] = d.pos ? { x: d.pos[0], y: d.pos[1], z: d.pos[2] } : null;
        if (!at && this.frames.length) {
          const p = sampleFrames(this.frames, d.tick).find((q) => q.index === d.victim);
          if (p) at = { x: p.x, y: p.y, z: p.z };
        }
        this.kills.push({
          tick: d.tick, victim: d.victim, killer: d.killer ?? null, weapon: d.weapon ?? null,
          kind: d.kind ?? "kill", position: ev.position, at,
        });
        endTick = Math.max(endTick, d.tick);
      } else if (ev.type === "ScoreChanged") {
        const d = ev.data as ScoreChanged;
        running = {
          tick: d.tick,
          players: { ...running.players },
          teams: { ...running.teams },
        };
        for (const p of d.players ?? []) running.players[p.index] = { ...p };
        for (const t of d.teams ?? []) running.teams[t.team] = t.score;
        this.scores.push(running);
        endTick = Math.max(endTick, d.tick);
      } else if (ev.type === "GameEnded") {
        endTick = Math.max(endTick, (ev.data as { tick?: number }).tick ?? 0);
      }
    }
    this.kills.sort((a, b) => a.tick - b.tick || a.position - b.position);
    this.scores.sort((a, b) => a.tick - b.tick);
    this.startTick = this.frames.length ? this.frames[0]!.tick : 0;
    this.endTick = Math.max(endTick, this.startTick);
  }

  get durationTicks(): number {
    return this.endTick - this.startTick;
  }

  get durationSeconds(): number {
    return this.durationTicks / TICKS_PER_SECOND;
  }

  clamp(tick: number): number {
    return Math.min(this.endTick, Math.max(this.startTick, tick));
  }

  sample(tick: number): SampledPlayer[] {
    const players = sampleFrames(this.frames, this.clamp(tick));
    // A victim stays down from its kill until the next recorded frame that
    // shows it alive; 1 Hz frames would otherwise let it walk on briefly.
    const recent = this.killsBetween(tick - TICKS_PER_SECOND, tick);
    for (const k of recent) {
      const p = players.find((q) => q.index === k.victim);
      if (p) p.alive = false;
    }
    return players;
  }

  /** AI-controlled bipeds at `tick` (campaign), sampled like players. */
  sampleActors(tick: number): SampledPlayer[] {
    return sampleActors(this.frames, this.clamp(tick));
  }

  /** Vehicles with someone in them at `tick`. */
  sampleVehicles(tick: number): SampledVehicle[] {
    return sampleVehicles(this.frames, this.clamp(tick));
  }

  /** Damage in the recorded frame at or before `tick`. */
  sampleHits(tick: number) {
    return sampleHits(this.frames, this.clamp(tick));
  }

  /** The level zone (structure BSP) loaded at `tick`. */
  sampleBsp(tick: number): number | null {
    return sampleBsp(this.frames, this.clamp(tick));
  }

  /** Machines at `tick`. */
  sampleMachines(tick: number): SampledMachine[] {
    return sampleMachines(this.frames, this.clamp(tick));
  }

  /** Kills with from < tick <= to, oldest first. */
  killsBetween(from: number, to: number): ReplayKill[] {
    const out: ReplayKill[] = [];
    for (const k of this.kills) {
      if (k.tick > to) break;
      if (k.tick > from) out.push(k);
    }
    return out;
  }

  /** The scoreboard as it stood at `tick`. */
  scoresAt(tick: number): ScoreSnapshot {
    let lo = 0, hi = this.scores.length - 1, ans = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (this.scores[mid]!.tick <= tick) { ans = mid; lo = mid + 1; } else hi = mid - 1;
    }
    return ans < 0 ? { tick: this.startTick, players: {}, teams: {} } : this.scores[ans]!;
  }
}

/** Play/pause/speed/seek over a timeline, driven by wall-clock time. */
export class ReplayClock {
  playing = false;
  speed = 1;
  private baseTick: number;
  private baseTime = 0;

  constructor(readonly timeline: ReplayTimeline) {
    this.baseTick = timeline.startTick;
  }

  tickAt(now: number): number {
    if (!this.playing) return this.baseTick;
    const t = this.baseTick + ((now - this.baseTime) / 1000) * TICKS_PER_SECOND * this.speed;
    if (t >= this.timeline.endTick) {
      this.playing = false;
      this.baseTick = this.timeline.endTick;
      return this.baseTick;
    }
    return t;
  }

  play(now: number): void {
    if (this.playing) return;
    if (this.baseTick >= this.timeline.endTick) this.baseTick = this.timeline.startTick;
    this.baseTime = now;
    this.playing = true;
  }

  pause(now: number): void {
    this.baseTick = this.tickAt(now);
    this.playing = false;
  }

  seek(tick: number, now: number): void {
    this.baseTick = this.timeline.clamp(tick);
    this.baseTime = now;
  }

  setSpeed(speed: number, now: number): void {
    this.baseTick = this.tickAt(now);
    this.baseTime = now;
    this.speed = speed;
  }
}
