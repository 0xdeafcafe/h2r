import { TICKS_PER_SECOND } from "../state/frames";
import { ReplayClock, type ReplayTimeline } from "../state/replay";

/** The playhead for one match, as it moves from the live buffer to a recording. */
export class ViewerPlayback {
  clock: ReplayClock | null = null;
  followingLive: boolean;

  constructor(live: boolean) {
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

  openRecording(timeline: ReplayTimeline, now: number, time?: string | string[]): void {
    const previous = this.clock;
    const clock = new ReplayClock(timeline);
    const seconds = Number(Array.isArray(time) ? time[0] : time);
    let tick = timeline.startTick + (Number.isFinite(seconds) ? Math.max(0, seconds) * TICKS_PER_SECOND : 0);
    let playing = true;
    if (previous) {
      tick = timeline.tickFromLatestRun(previous.tickAt(now));
      playing = previous.playing;
      clock.setSpeed(previous.speed, now);
    }
    clock.seek(tick, now);
    if (playing && timeline.clamp(tick) < timeline.endTick) clock.play(now);
    this.clock = clock;
    this.followingLive = false;
  }
}
