// One replay of an ended game: its recorded frames and events, a clock, and
// the play / pause / seek / speed controls. The game page's 2D map, its
// scrubber and the 3D window all read the same session, so they move
// together; a popped-out 3D window runs a session of its own and the two
// keep in step over a BroadcastChannel (viewer/sync.ts).

import { createEffect, createSignal, on, onCleanup, type Accessor } from "solid-js";
import type { WireEvent } from "../api/types";
import type { MapFrame } from "../components/MapCanvas";
import type { RosterEntry } from "../map/render";
import { onAnimationFrame } from "../map/scheduler";
import { TICKS_PER_SECOND } from "../state/frames";
import { live } from "../state/live";
import type { GameState } from "../state/reducer";
import { ReplayClock, ReplayTimeline, fetchRecording, type ReplayKill } from "../state/replay";
import { applyClock, clockAgrees, isSyncMessage, nextCamera, readClock, syncChannelName, type CameraRequest, type SyncMessage } from "./sync";

export interface ReplaySession {
  timeline: Accessor<ReplayTimeline | null>;
  error: Accessor<string | null>;
  /** The clock's tick, refreshed about ten times a second for the UI. */
  tick: Accessor<number>;
  playing: Accessor<boolean>;
  speed: Accessor<number>;
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

export function createReplaySession(game: Accessor<GameState>, roster: Accessor<Map<number, RosterEntry>>): ReplaySession {
  const [timeline, setTimeline] = createSignal<ReplayTimeline | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [tick, setTick] = createSignal(0);
  const [playing, setPlaying] = createSignal(false);
  const [speed, setSpeedSignal] = createSignal(1);
  const [camera, setCamera] = createSignal<CameraRequest | null>(null);
  let clock: ReplayClock | null = null;
  let sought = 0;

  // Other windows showing this game.
  const me = Math.random().toString(36).slice(2);
  let channel: BroadcastChannel | null = null;
  /** True while this window made the last change: it then repeats its state now and again. */
  let leading = false;
  const post = (m: SyncMessage) => { try { channel?.postMessage(m); } catch { /* closed */ } };
  const announce = () => {
    if (!clock) return;
    leading = true;
    post({ kind: "state", from: me, state: readClock(clock, performance.now()) });
  };
  const reflect = () => {
    if (!clock) return;
    const n = performance.now();
    setTick(clock.tickAt(n));
    setPlaying(clock.playing);
    setSpeedSignal(clock.speed);
  };

  createEffect(on(() => game().game_id, (id) => {
    const ac = new AbortController();
    setTimeline(null);
    setError(null);
    clock = null;
    if (typeof BroadcastChannel !== "undefined") {
      channel = new BroadcastChannel(syncChannelName(id));
      channel.onmessage = (e) => {
        const m: unknown = e.data;
        if (!isSyncMessage(m) || m.from === me || !clock) return;
        if (m.kind === "hello") { post({ kind: "state", from: me, state: readClock(clock, performance.now()) }); return; }
        if (m.kind === "camera") { setCamera(m.camera); return; }
        leading = false;
        const n = performance.now();
        if (clockAgrees(clock, m.state, n)) return;
        applyClock(clock, m.state, n);
        sought = n;
        reflect();
      };
    }
    const a = live.api();
    fetchRecording(a, id, ac.signal).then(({ frames, events }) => {
      // The game's whole stream: also what the Game Record's signature
      // count covers, since a snapshot-loaded game knows no event positions.
      if (events?.length) live.checkGameEvents(id, events);
      const t = new ReplayTimeline(frames, events?.length ? events : eventsFromState(game()));
      clock = new ReplayClock(t);
      setTimeline(t);
      // Open on the final moment; play is the viewer's choice. Another
      // window already showing this game answers the hello with where it is.
      clock.seek(t.endTick, performance.now());
      sought = performance.now();
      setTick(t.endTick);
      post({ kind: "hello", from: me });
    }, (e: Error) => { if (e.name !== "AbortError") setError(e.message); });
    onCleanup(() => { ac.abort(); channel?.close(); channel = null; });
  }));

  // Push the clock's tick into Solid ~10 times a second for the UI; the
  // canvases read the clock directly every frame.
  let last = 0, lastPost = 0;
  const stop = onAnimationFrame((t) => {
    if (!clock || t - last < 100) return;
    last = t;
    setTick(clock.tickAt(t));
    if (playing() !== clock.playing) setPlaying(clock.playing);
    if (leading && clock.playing && t - lastPost > 2000) { lastPost = t; announce(); }
  });
  onCleanup(stop);

  const frame = (t: number): MapFrame => {
    const tl = timeline();
    if (!tl || !clock) return { players: [], markers: [], seen: null };
    const at = clock.tickAt(t);
    const r = roster();
    const fade = 6 * TICKS_PER_SECOND;
    return {
      players: tl.sample(at),
      actors: tl.sampleActors(at),
      vehicles: tl.sampleVehicles(at),
      hits: tl.sampleHits(at),
      bsp: tl.sampleBsp(at),
      machines: tl.sampleMachines(at),
      objects: tl.sampleObjects(at),
      markers: tl.killsBetween(at - fade, at).filter((k) => k.at).map((k) => ({
        ...k.at!, alpha: 1 - (at - k.tick) / fade, colour: r.get(k.victim)?.colour ?? "#555",
      })),
      seen: boundsOf(tl),
    };
  };

  return {
    timeline, error, tick, playing, speed, frame,
    soughtAt: () => sought,
    toggle() {
      if (!clock) return;
      const n = performance.now();
      if (clock.playing) clock.pause(n); else clock.play(n);
      // `play` from the end starts over, which is a jump.
      sought = n;
      reflect();
      announce();
    },
    seek(t) {
      if (!clock) return;
      const n = performance.now();
      clock.seek(t, n);
      sought = n;
      reflect();
      announce();
    },
    setSpeed(v) {
      if (!clock) return;
      clock.setSpeed(v, performance.now());
      reflect();
      announce();
    },
    stepFrame(dir) {
      const tl = timeline();
      if (!clock || !tl) return;
      const n = performance.now();
      if (clock.playing) clock.pause(n);
      clock.seek(tl.frameStep(clock.tickAt(n), dir), n);
      sought = n;
      reflect();
      announce();
    },
    stepMark(dir) {
      const tl = timeline();
      if (!clock || !tl) return;
      const n = performance.now();
      const t = tl.markStep(clock.tickAt(n), dir);
      if (t === null) return;
      clock.seek(t, n);
      sought = n;
      reflect();
      announce();
    },
    watchKill(kill) {
      const tl = timeline();
      if (!clock || !tl) return;
      const k = tl.killcam(kill);
      const n = performance.now();
      if (clock.playing) clock.pause(n);
      clock.setSpeed(k.speed, n);
      clock.seek(k.from, n);
      clock.play(n);
      sought = n;
      const c = nextCamera(camera(), "eyes", k.follow);
      setCamera(c);
      post({ kind: "camera", from: me, camera: c });
      reflect();
      announce();
    },
    camera,
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
function eventsFromState(g: GameState): WireEvent[] {
  return g.kills.map((k) => ({
    position: k.position, stream: "game-" + g.game_id, version: 0, type: "PlayerKilled", at: k.at,
    data: { tick: k.tick, victim: k.victim, killer: k.killer, weapon: k.weapon, kind: k.kind },
  }));
}
