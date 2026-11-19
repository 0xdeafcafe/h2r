import { TICKS_PER_SECOND } from "../state/frames";
import { ReplayClock, type ReplayTimeline, type TickRange } from "../state/replay";

/** The playhead for one match, as it moves from the live buffer to a recording. */
export class ViewerPlayback {
  clock: ReplayClock | null = null;
  followingLive: boolean;

  constructor(live: boolean, private readonly range?: TickRange) {
    this.followingLive = live;
  }

  get playing(): boolean {
    return !!this.clock && (this.followingLive || this.clock.playing);
  }

  tickAt(now: number, edgeTick: number): number {
    if (!this.clock) return 0;
    return this.followingLive ? this.clock.timeline.clamp(edgeTick) : this.clock.tickAt(now);
  }

  updateLive(timeline: ReplayTimeline, now: number): void {
    const previous = this.clock;
    this.clock = new ReplayClock(timeline, true);
    if (previous && !this.followingLive) {
      this.clock.setSpeed(previous.speed, now);
      this.clock.seek(previous.tickAt(now), now);
      if (previous.playing) this.clock.play(now);
    }
  }

  seek(tick: number, now: number): void {
    this.followingLive = false;
    this.clock?.seek(tick, now);
  }

  goLive(now: number): void {
    this.followingLive = true;
    this.clock?.pause(now);
    this.clock?.setSpeed(1, now);
  }

  endLive(now: number, edgeTick: number): void {
    if (!this.followingLive) return;
    this.seek(edgeTick, now);
    this.clock?.play(now);
  }

  /** Replace a recording's loaded window while keeping its playback clock. */
  updateRecording(timeline: ReplayTimeline, now: number): void {
    const previous = this.clock;
    if (!previous) return this.openRecording(timeline, now);
    const tick = previous.tickAt(now);
    const clock = new ReplayClock(timeline, false, this.range);
    clock.setSpeed(previous.speed, now);
    clock.seek(tick, now);
    if (previous.playing && tick < (clock.loop?.to_tick ?? timeline.endTick)) clock.play(now);
    this.clock = clock;
  }

  openRecording(timeline: ReplayTimeline, now: number, time?: string | string[]): void {
    const previous = this.clock;
    const clock = new ReplayClock(timeline, false, this.range);
    const seconds = Number(Array.isArray(time) ? time[0] : time);
    let tick = timeline.startTick + (Number.isFinite(seconds) ? Math.max(0, seconds) * TICKS_PER_SECOND : 0);
    let playing = true;
    if (previous) {
      tick = timeline.tickFromLatestRun(previous.tickAt(now));
      playing = previous.playing;
      clock.setSpeed(previous.speed, now);
    }
    clock.seek(clock.loop?.from_tick ?? tick, now);
    if (playing && (this.range ? !!clock.loop : timeline.clamp(tick) < timeline.endTick)) clock.play(now);
    this.clock = clock;
    this.followingLive = false;
  }
}
