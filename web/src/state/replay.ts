// Replays of ended games: 1 Hz recorded frames plus the game's events,
// sampled at any tick.

import type { Frame, FrameObject, PlayerKilled, PlayerScore, ScoreChanged, WireEvent } from "../api/types";
import { recordedFrame } from "../api/pack";
import { sampleActors, sampleBsp, sampleFrames, sampleHits, sampleMachines, sampleObjects, sampleVehicles, withoutUnplaced, type SampledMachine, type SampledPlayer, type SampledVehicle, TICKS_PER_SECOND } from "./frames";

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

  /** Where the recording's ticks ran backwards (a campaign checkpoint
   *  revert or mission restart, recorded before the game's clock kept
   *  counting through them): the first tick of each later run. */
  readonly cuts: number[];

  /** `frames` as fetched (log order, oldest first) and the game's events.
   *  The events' own `GameFrame`s, when they cover the fetched frames, are
   *  read instead: their log positions place the other events among them. */
  constructor(frames: Frame[], events: WireEvent[]) {
    const sorted = [...events].sort((a, b) => a.position - b.position);
    const clock = new GameClock();
    const ticks = new Map<WireEvent, number>();
    const logged = new Map<WireEvent, Frame>();
    for (const ev of sorted) {
      if (ev.type !== "GameFrame") continue;
      const f = recordedFrame(ev.data);
      if (isFrame(f)) logged.set(ev, f);
    }
    this.frames = [];
    if (logged.size && logged.size >= frames.length) {
      for (const ev of sorted) {
        const f = logged.get(ev);
        if (f) {
          this.frames.push(retick(f, clock.tick(f.tick, true)));
          continue;
        }
        const raw = eventTick(ev);
        if (raw !== null) ticks.set(ev, clock.tick(raw, false));
      }
    } else {
      for (const f of frames) if (isFrame(f)) this.frames.push(retick(f, clock.tick(f.tick, true)));
      clock.place(sorted, ticks);
    }
    this.cuts = clock.cuts;
    const tickOf = (ev: WireEvent, raw: number) => ticks.get(ev) ?? raw;

    this.kills = [];
    this.scores = [];
    let endTick = this.frames.length ? this.frames[this.frames.length - 1]!.tick : 0;
    let running: ScoreSnapshot = { tick: 0, players: {}, teams: {} };
    for (const ev of sorted) {
      if (ev.type === "PlayerKilled") {
        const d = { ...(ev.data as PlayerKilled) };
        d.tick = tickOf(ev, d.tick);
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
        const tick = tickOf(ev, d.tick);
        running = {
          tick,
          players: { ...running.players },
          teams: { ...running.teams },
        };
        for (const p of d.players ?? []) running.players[p.index] = { ...p };
        for (const t of d.teams ?? []) running.teams[t.team] = t.score;
        this.scores.push(running);
        endTick = Math.max(endTick, tick);
      } else if (ev.type === "GameEnded") {
        const raw = eventTick(ev);
        if (raw !== null) endTick = Math.max(endTick, tickOf(ev, raw));
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

  /** Scenario objects out of place at `tick`. */
  sampleObjects(tick: number): FrameObject[] | null {
    return sampleObjects(this.frames, this.clamp(tick));
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

/** Ticks back from the run's newest that still read as the same run:
 *  events and recorded frames reach the log a little out of step. */
const REWIND_SLACK = 2 * TICKS_PER_SECOND;

/** The game's clock over a recording read in log order. The game's ticks
 *  only go up, but campaign recordings from before the sender kept them
 *  counting through checkpoint reverts and mission restarts run backwards
 *  there; each such run is moved on to start just after the last. Without
 *  it frames from different runs interleave by tick and a replay flicks
 *  between them. */
export class GameClock {
  /** The first tick of each run after the first. */
  readonly cuts: number[] = [];
  /** Each run's raw tick span and what is added to its ticks. */
  readonly runs: { lo: number; hi: number; offset: number }[] = [];
  private high = -Infinity;
  private frameHigh = -Infinity;
  private out = -Infinity;

  /** The game tick for `raw`, the next tick in log order. Recorded frames
   *  (`frame`) are one source, strictly in order: any step back is a new
   *  run. Other events step back a little against them now and again. */
  tick(raw: number, frame: boolean): number {
    let run = this.runs[this.runs.length - 1];
    if (!run || raw < this.high - REWIND_SLACK || (frame && raw <= this.frameHigh)) {
      const offset = run ? this.out + 2 - raw : 0;
      run = { lo: raw, hi: raw, offset };
      if (this.runs.length) this.cuts.push(raw + offset);
      this.runs.push(run);
      this.high = this.frameHigh = -Infinity;
    }
    run.lo = Math.min(run.lo, raw);
    run.hi = Math.max(run.hi, raw);
    this.high = Math.max(this.high, raw);
    if (frame) this.frameHigh = Math.max(this.frameHigh, raw);
    const t = raw + run.offset;
    this.out = Math.max(this.out, t);
    return t;
  }

  /** Puts events with no recorded frames among them (only fetched frames
   *  were read) into the frames' runs: in log order they move on to a later
   *  run when they step back, to the first that spans them. Into `ticks`. */
  place(sorted: readonly WireEvent[], ticks: Map<WireEvent, number>): void {
    if (this.runs.length < 2) return;
    let at = 0, high = -Infinity;
    for (const ev of sorted) {
      const raw = eventTick(ev);
      if (raw === null) continue;
      if (raw < high - REWIND_SLACK && at + 1 < this.runs.length) {
        let next = at + 1;
        for (let i = at + 1; i < this.runs.length; i++) {
          const r = this.runs[i]!;
          if (raw >= r.lo - REWIND_SLACK && raw <= r.hi + REWIND_SLACK) { next = i; break; }
        }
        at = next;
        high = -Infinity;
      }
      high = Math.max(high, raw);
      ticks.set(ev, raw + this.runs[at]!.offset);
    }
  }
}

function isFrame(f: Frame | null | undefined): f is Frame {
  return !!f && typeof f.tick === "number" && Number.isFinite(f.tick) && Array.isArray(f.players);
}

/** `f` at game tick `tick`, without players it had nowhere to put. */
function retick(f: Frame, tick: number): Frame {
  const g = withoutUnplaced(f);
  return tick === g.tick ? g : { ...g, tick };
}

/** A kill's, score's or end's tick, else null. */
function eventTick(ev: WireEvent): number | null {
  if (ev.type !== "PlayerKilled" && ev.type !== "ScoreChanged" && ev.type !== "GameEnded") return null;
  const t = (ev.data as { tick?: unknown } | null)?.tick;
  return typeof t === "number" && Number.isFinite(t) ? t : null;
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
