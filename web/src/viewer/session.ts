// The game page's 2D map, 3D view and scrubber share one playback session
// from the live buffer through to the finished recording.

import { batch, createEffect, createMemo, createSignal, on, onCleanup, type Accessor } from "solid-js";
import type { Frame, WireEvent } from "../api/types";
import type { MapFrame } from "../components/MapCanvas";
import { liveFrameSource } from "../lib/game";
import type { RosterEntry } from "../map/render";
import { onAnimationFrame } from "../map/scheduler";
import { TICKS_PER_SECOND, type FrameBuffer } from "../state/frames";
import { frameHub, live } from "../state/live";
import type { GameState } from "../state/reducer";
import { ReplayTimeline, fetchRecording, type ReplayKill } from "../state/replay";
import { ArchiveWindow } from "./archive";
import { ViewerPlayback } from "./playback";
import { applyClock, clockAgrees, isSyncMessage, nextCamera, readClock, syncChannelName, type CameraRequest, type SyncMessage } from "./sync";

export interface ReplaySession {
  timeline: Accessor<ReplayTimeline | null>;
  error: Accessor<string | null>;
  /** The clock's tick, refreshed about ten times a second for the UI. */
  tick: Accessor<number>;
  playing: Accessor<boolean>;
  speed: Accessor<number>;
  isLive: Accessor<boolean>;
  followingLive: Accessor<boolean>;
  goLive(): void;
  toggle(): void;
  seek(tick: number): void;
  setSpeed(speed: number): void;
  /** The map frame at wall time `now`, read straight off the clock. */
  frame(now: number): MapFrame;
  /** Wall time of the last seek, so a view can jump rather than glide. */
  soughtAt(): number;
  /** Pauses and steps to the next (1) or previous (-1) recorded frame. */
  stepFrame(dir: 1 | -1): void;
  /** Seeks to the next or previous mark (kill, death, cut, zone change). */
  stepMark(dir: 1 | -1): void;
  /** Plays a kill from a little before it, slowed, through the killer's
   *  eyes (the 3D view takes `camera`). */
  watchKill(kill: ReplayKill): void;
  /** The camera the 3D views are asked for, newest last (`seq` grows). */
  camera: Accessor<CameraRequest | null>;
}

export const SPEEDS = [0.25, 0.5, 1, 2, 4, 8];

export function createReplaySession(
  game: Accessor<GameState>,
  roster: Accessor<Map<number, RosterEntry>>,
  startTime: Accessor<string | string[] | undefined>,
): ReplaySession {
  const [timeline, setTimeline] = createSignal<ReplayTimeline | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [tick, setTick] = createSignal(0);
  const [playing, setPlaying] = createSignal(false);
  const [speed, setSpeedSignal] = createSignal(1);
  const [followingLive, setFollowingLive] = createSignal(false);
  const [camera, setCamera] = createSignal<CameraRequest | null>(null);
  const gameId = createMemo(() => game().game_id);
  const archiveRevision = createMemo(() => game().archiveRevision);
  const isLive = createMemo(() => game().status === "live");
  let playback = new ViewerPlayback(false);
  let liveEdge = (_now: number) => 0;
  let liveFrame: ReturnType<typeof liveFrameSource> | null = null;
  let sought = 0;
  let archive: ArchiveWindow | undefined;
  const tickAt = (now: number) => playback.tickAt(now, playback.followingLive ? liveEdge(now) : 0);

  // Explicit replay controls also apply in other tabs on this game. A new
  // tab takes its initial position from its own link.
  const me = Math.random().toString(36).slice(2);
  let channel: BroadcastChannel | null = null;
  const post = (m: SyncMessage) => { try { channel?.postMessage(m); } catch { /* closed */ } };
  const announce = (now: number) => {
    if (playback.clock) post({ kind: "state", from: me, state: readClock(playback.clock, now) });
  };
  const reflect = (now: number) => batch(() => {
    setTimeline(playback.clock?.timeline ?? null);
    setTick(tickAt(now));
    setPlaying(playback.playing);
    setSpeedSignal(playback.clock?.speed ?? 1);
    setFollowingLive(playback.followingLive);
  });

  createEffect(on(gameId, (id) => {
    const ac = new AbortController();
    const openedLive = isLive();
    playback = new ViewerPlayback(openedLive);
    liveFrame = liveFrameSource(id, roster);
    liveEdge = (now) => frameHub.get(id).renderTick(now);
    let buffer: FrameBuffer | undefined;
    let releaseHistory: (() => void) | undefined;
    let lastFrames: Frame[] | undefined;
    let newest: Frame | undefined;
    const refreshLive = (now: number) => {
      const active = frameHub.get(id);
      if (buffer !== active) {
        releaseHistory?.();
        buffer = active;
        releaseHistory = buffer.keepPlaybackHistory();
      }
      const frames = buffer.frames;
      const first = frames[0], last = frames.at(-1);
      if (last === newest) return;
      if (newest && lastFrames !== frames) {
        // The live buffer was reset on a checkpoint/restart or reconnect.
        playback = new ViewerPlayback(true);
        sought = now;
      }
      lastFrames = frames;
      newest = last;
      if (!first || !last) return;
      const events = eventsFromState(game(), first.tick, last.tick);
      playback.updateLive(new ReplayTimeline(frames, events), now);
    };
    setCamera(null);
    setError(null);
    sought = performance.now();
    reflect(sought);

    let archiveFramesChanged: (() => void) | undefined;
    let pendingWindow: Promise<void> | null = null;
    let recordingOpen = false;
    const refreshWindow = (target: number) => {
      if (!archive) return;
      const request = archive.at(target);
      if (!request || request === pendingWindow) return;
      const selected = archive;
      pendingWindow = request;
      void request.then(() => {
        if (archive === selected && !ac.signal.aborted && archive.frames.length) archiveFramesChanged?.();
      }, (e: Error) => {
        if (archive === selected && !ac.signal.aborted) { setError(e.message); archive = undefined; }
      }).finally(() => { if (pendingWindow === request) pendingWindow = null; });
    };
    createEffect(on([isLive, archiveRevision], ([matchIsLive, revision]) => {
      const n = performance.now();
      if (matchIsLive) {
        refreshLive(n);
        reflect(n);
        return;
      }
      if (openedLive && !recordingOpen && releaseHistory) {
        refreshLive(n);
        playback.endLive(n, liveEdge(n));
        releaseHistory?.();
        releaseHistory = undefined;
      }
      reflect(n);
      const request = new AbortController();
      onCleanup(() => { request.abort(); archive = undefined; archiveFramesChanged = undefined; });
      const signal = request.signal;
      const time = startTime();
      const open = (recording: ReplayTimeline) => {
        const now = performance.now();
        if (recordingOpen) playback.updateRecording(recording, now);
        else { playback.openRecording(recording, now, time); sought = now; }
        recordingOpen = true;
        setError(null);
        reflect(now);
      };
      const api = live.api();
      const read = async () => {
        const index = await api.chunks(id, signal, revision);
        if (signal.aborted) return;
        if (index?.archive_sha256) {
          const window = new ArchiveWindow(index, (c, segmentSignal) => api.chunk(id, c, AbortSignal.any([signal, segmentSignal])));
          const bounds = new ReplayTimeline([], [], window.segments);
          const clock = playback.clock;
          const seconds = Number(Array.isArray(time) ? time[0] : time);
          const target = clock
            ? (recordingOpen ? clock.tickAt(performance.now()) : bounds.tickFromLatestRun(clock.tickAt(performance.now())))
            : bounds.startTick + (Number.isFinite(seconds) ? seconds * TICKS_PER_SECOND : 0);
          await window.at(bounds.clamp(target));
          if (signal.aborted) return;
          archive = window;
          let events = eventsFromState(game());
          archiveFramesChanged = () => open(new ReplayTimeline(window.frames, events, window.segments));
          archiveFramesChanged();
          // Seek and play can start while the signed event history downloads.
          void api.streamEvents("game-" + id, signal, {exclude:"GameFrame"}).then(log => {
            if (signal.aborted || !log.length) return;
            live.checkGameEvents(id, log);
            events = log;
            archiveFramesChanged?.();
          }).catch(() => { /* the state snapshot supplies the available events */ });
        } else {
          const {frames, events} = await fetchRecording({
            frames: (game, from, to, s) => api.frames(game, from, to, s, index),
            streamEvents: api.streamEvents,
          }, id, signal);
          if (signal.aborted) return;
          if (events?.length) live.checkGameEvents(id, events);
          const timeline = new ReplayTimeline(frames, events?.length ? events : eventsFromState(game()));
          open(timeline.frames.length ? timeline : playback.clock?.timeline ?? timeline);
        }
      };
      void read().then(() => {
        if (signal.aborted || channel) return;
        if (typeof BroadcastChannel === "undefined") return;
        channel = new BroadcastChannel(syncChannelName(id));
        channel.onmessage = (e) => {
          const m: unknown = e.data;
          const clock = playback.clock;
          if (!isSyncMessage(m) || m.from === me || !clock) return;
          if (m.kind === "camera") { setCamera(m.camera); return; }
          if (m.kind !== "state") return;
          const now = performance.now();
          if (clockAgrees(clock, m.state, now)) return;
          applyClock(clock, m.state, now);
          sought = now;
          reflect(now);
        };
      }, (e: Error) => {
        if (signal.aborted || e.name === "AbortError") return;
        if (playback.clock && !recordingOpen) open(playback.clock.timeline);
        setError(e.message);
      });
    }));

    // The controls need only about ten updates a second. The canvases
    // read the interpolated live frame or replay clock directly.
    let last = -Infinity;
    const stop = onAnimationFrame((now) => {
      if (now - last < 100) return;
      last = now;
      if (isLive()) refreshLive(now);
      else refreshWindow(tickAt(now));
      reflect(now);
    });
    onCleanup(() => {
      stop();
      releaseHistory?.();
      ac.abort();
      channel?.close();
      channel = null;
    });
  }));

  const frame = (now: number): MapFrame => {
    if (playback.followingLive && liveFrame) return liveFrame(now);
    const tl = playback.clock?.timeline;
    const tick = tickAt(now);
    if (!tl || (archive && !archive.ready(tick))) return { players: [], markers: [], seen: null };
    return replayFrame(tl, tick, roster());
  };

  const changed = (now: number) => {
    sought = now;
    reflect(now);
    announce(now);
  };

  return {
    timeline, error, tick, playing, speed, isLive, followingLive, frame,
    soughtAt: () => sought,
    goLive() {
      if (!isLive()) return;
      const now = performance.now();
      playback.goLive(now);
      changed(now);
    },
    toggle() {
      const clock = playback.clock;
      if (!clock) return;
      const now = performance.now();
      if (playback.followingLive) playback.seek(tickAt(now), now);
      else if (clock.playing) clock.pause(now);
      else clock.play(now);
      changed(now);
    },
    seek(t) {
      if (!playback.clock) return;
      const now = performance.now();
      playback.seek(t, now);
      changed(now);
    },
    setSpeed(v) {
      if (!playback.clock || playback.followingLive) return;
      const now = performance.now();
      playback.clock.setSpeed(v, now);
      reflect(now);
      announce(now);
    },
    stepFrame(dir) {
      const clock = playback.clock;
      if (!clock) return;
      const now = performance.now();
      const at = tickAt(now);
      clock.pause(now);
      playback.seek(clock.timeline.frameStep(at, dir), now);
      changed(now);
    },
    stepMark(dir) {
      const clock = playback.clock;
      if (!clock) return;
      const now = performance.now();
      const t = clock.timeline.markStep(tickAt(now), dir);
      if (t === null) return;
      playback.seek(t, now);
      changed(now);
    },
    watchKill(kill) {
      const clock = playback.clock;
      if (!clock) return;
      const k = clock.timeline.killcam(kill);
      const now = performance.now();
      clock.pause(now);
      clock.setSpeed(k.speed, now);
      playback.seek(k.from, now);
      clock.play(now);
      const c = nextCamera(camera(), "eyes", k.follow);
      setCamera(c);
      post({ kind: "camera", from: me, camera: c });
      changed(now);
    },
    camera,
  };
}

/** The map frame of a recording at tick `at`: players, actors, vehicles
 *  and the knockouts of the last six seconds, fading. The game page's
 *  replay and the stats home's "last game" loop both draw from it. */
export function replayFrame(tl: ReplayTimeline, at: number, r: Map<number, RosterEntry>): MapFrame {
  const fade = 6 * TICKS_PER_SECOND;
  return {
    players: tl.sample(at),
    actors: tl.sampleActors(at),
    vehicles: tl.sampleVehicles(at),
    hits: tl.sampleHits(at),
    bsp: tl.sampleBsp(at),
    machines: tl.sampleMachines(at),
    objects: tl.sampleObjects(at),
    cinematic: tl.cinematicAt(at),
    markers: tl.killsBetween(at - fade, at).filter((k) => k.at).map((k) => ({
      ...k.at!, alpha: 1 - (at - k.tick) / fade, colour: r.get(k.victim)?.colour ?? "#555",
    })),
    seen: boundsOf(tl),
  };
}

const boundsCache = new WeakMap<ReplayTimeline, MapFrame["seen"]>();
function boundsOf(tl: ReplayTimeline): MapFrame["seen"] {
  let b = boundsCache.get(tl);
  if (!b) {
    b = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
    for (const f of tl.frames) for (const p of f.players) {
      b.minX = Math.min(b.minX, p.pos[0]); b.maxX = Math.max(b.maxX, p.pos[0]);
      b.minY = Math.min(b.minY, p.pos[1]); b.maxY = Math.max(b.maxY, p.pos[1]);
    }
    boundsCache.set(tl, b);
  }
  return b;
}

/** Kill and score events rebuilt from client state, if the REST log is unavailable. */
export function eventsFromState(g: GameState, from = -Infinity, to = Infinity): WireEvent[] {
  return g.kills.filter((k) => k.tick >= from && k.tick <= to).map((k) => ({
    position: k.position, stream: "game-" + g.game_id, version: 0, type: "PlayerKilled", at: k.at,
    data: { tick: k.tick, victim: k.victim, killer: k.killer, weapon: k.weapon, kind: k.kind },
  }));
}
