// Clanker Training's Watch view: one map tile per run in a batch, each
// playing its run. Finished runs replay their recording on one shared
// fast-forward clock; running runs stream live at the sim's own pace
// (docs/runtime/sim.md, "Watching in Uplink").

import { A } from "@solidjs/router";
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount, type Accessor } from "solid-js";
import type { Frame, PlayerKilled } from "../api/types";
import { wsUrl } from "../api/server";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import { liveFrameSource, rosterFor } from "../lib/game";
import { geometryFor } from "../map/geometry";
import { onAnimationFrame } from "../map/scheduler";
import { topView } from "../map/views";
import ViewMap from "../map/ViewMap";
import { mapTitle } from "../state/gameart";
import { frameHub, live } from "../state/live";
import { geometryChanged } from "../state/maps";
import { outlineFor } from "../state/outlines";
import { applyGameSnapshot, initialState, type GameState } from "../state/reducer";
import { ReplayTimeline, fetchRecording } from "../state/replay";
import { StreamClient, type SocketLike } from "../state/stream";
import { MapThumb } from "../stats/Side";
import { eventsFromState, replayFrame } from "../viewer/session";
import { scoreLabel, trainingHref, type SimRun } from "./simRuns";
import {
  BatchClock, DEFAULT_WATCH_SPEED, MAX_RECORDING_READS, WATCH_SPEEDS, armsLabel, gameClock, limiter, orderTiles, scoreAt,
  streamingTiles, tileMode, tileTick,
} from "./batchWatch";

const EMPTY: MapFrame = { players: [], markers: [], seen: null };
/** The tiles' labels refresh four times a second; the maps every frame. */
const LABEL_MS = 250;

/** Streams one running sim into `frameHub`: its frames and where its
 *  bots died. `onEnded` hears the match end. Returns the stop. */
export function streamSimRun(id: string, from: number | null, onEnded: () => void, onFrame?: (frame: Frame) => void): () => void {
  const api = live.api();
  const stream = new StreamClient({
    url: (at) => wsUrl(api.streamUrl(at, true, id)),
    auth: api.readToken,
    connect: (url) => {
      const socket = new WebSocket(url);
      socket.binaryType = "arraybuffer";
      return socket as unknown as SocketLike;
    },
  }, {
    onEvent: (event) => {
      if (event.type === "GameEnded") onEnded();
      if (event.type === "PlayerKilled") {
        const kill = event.data as PlayerKilled;
        frameHub.get(id).addDeath(kill.victim, kill.tick, performance.now(), kill.pos);
      }
    },
    onFrame: (frame) => {
      if (frame.game_id !== id) return;
      frameHub.push(frame, performance.now());
      onFrame?.(frame);
    },
    onCaughtUp: () => {},
  });
  stream.start(from);
  return () => stream.stop();
}

function gameOf(run: SimRun): GameState {
  const state = initialState();
  applyGameSnapshot(state, run);
  return state.games[run.game_id]!;
}

/** The grid for one batch, with its replay controls. `position` is the
 *  run list's log position, where live streams start. */
export function BatchWatch(props: { batch: string; runs: SimRun[]; position?: number | null; onEnded: () => void }) {
  const ordered = createMemo(() => orderTiles(props.runs));
  const ids = createMemo(() => ordered().map((r) => r.game_id), [], {
    equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]),
  });
  const byId = createMemo(() => new Map(props.runs.map((r) => [r.game_id, r])));
  const [visible, setVisible] = createSignal<ReadonlySet<string>>(new Set());
  const streaming = createMemo(() => streamingTiles(ordered().filter((r) => tileMode(r) === "live").map((r) => r.game_id), visible()));
  const replays = () => ordered().some((r) => tileMode(r) === "replay");
  const clock = new BatchClock(DEFAULT_WATCH_SPEED);
  clock.play(performance.now());
  const read = limiter(MAX_RECORDING_READS);
  const [now, setNow] = createSignal(performance.now());
  const [playing, setPlaying] = createSignal(clock.playing);
  const [speed, setSpeed] = createSignal(clock.speed);
  const atEnd = () => !playing() && clock.end > 0 && clock.elapsed(now()) >= clock.end;
  // No recording has loaded yet: the clock waits at zero and plays when one does.
  const loading = () => { now(); return clock.end === 0; };

  onMount(() => {
    let last = -Infinity;
    const stop = onAnimationFrame((t) => {
      if (t - last < LABEL_MS || document.hidden) return;
      last = t;
      clock.elapsed(t);
      setNow(t);
      setPlaying(clock.playing);
    });
    onCleanup(stop);
  });

  const changed = () => {
    const t = performance.now();
    setNow(t);
    setPlaying(clock.playing);
    setSpeed(clock.speed);
  };
  const toggle = () => {
    const t = performance.now();
    if (clock.playing) clock.pause(t); else clock.play(t);
    changed();
  };
  const restart = () => {
    const t = performance.now();
    clock.seek(0, t);
    clock.play(t);
    changed();
  };
  const seen = (id: string, inView: boolean) => setVisible((old) => {
    if (old.has(id) === inView) return old;
    const next = new Set(old);
    if (inView) next.add(id); else next.delete(id);
    return next;
  });

  return (
    <section class="adm-watch" aria-label={`Watch batch ${props.batch}`}>
      <Show when={replays()}>
        <div class="adm-row" role="group" aria-label="Batch replay controls">
          <button class="s-btn" disabled={loading()} onClick={toggle}>{playing() || loading() ? "Pause" : atEnd() ? "Play again" : "Play"}</button>
          <button class="s-btn" disabled={loading()} onClick={restart}>Restart</button>
          <label class="adm-field">
            <span>Speed</span>
            <select value={speed()} onChange={(e) => { clock.setSpeed(+e.currentTarget.value, performance.now()); changed(); }}>
              <For each={WATCH_SPEEDS}>{(v) => <option value={v} selected={speed() === v}>{v}×</option>}</For>
            </select>
          </label>
          <small class="adm-watch-note">Finished runs play in sync from their first frame. Running runs play live at the sim's own speed.</small>
        </div>
      </Show>
      <div class="adm-watch-grid">
        <For each={ids()}>{(id) => (
          <Show when={byId().get(id)}>{(run) => (
            <WatchTile
              run={run} batch={props.batch} clock={clock} now={now} position={props.position}
              streaming={() => streaming().has(id)} read={read} onSeen={(inView) => seen(id, inView)} onEnded={props.onEnded}
            />
          )}</Show>
        )}</For>
      </div>
    </section>
  );
}

function WatchTile(props: {
  run: Accessor<SimRun>;
  batch: string;
  clock: BatchClock;
  now: Accessor<number>;
  position?: number | null;
  streaming: Accessor<boolean>;
  read: ReturnType<typeof limiter>;
  onSeen: (inView: boolean) => void;
  onEnded: () => void;
}) {
  const id = props.run().game_id;
  const game = createMemo(() => gameOf(props.run()));
  const roster = createMemo(() => rosterFor(game()));
  const mode = createMemo(() => tileMode(props.run()));
  const [seenOnce, setSeenOnce] = createSignal(false);
  const [near, setNear] = createSignal(false);
  const [timeline, setTimeline] = createSignal<ReplayTimeline | null>(null);
  const [failed, setFailed] = createSignal(false);
  const [liveStart, setLiveStart] = createSignal<number | null>(null);
  let tile!: HTMLElement;

  onMount(() => {
    const io = new IntersectionObserver((es) => {
      const inView = es.some((e) => e.isIntersecting);
      if (inView) setSeenOnce(true);
      setNear(inView);
      props.onSeen(inView);
    }, { rootMargin: "200px" });
    io.observe(tile);
    onCleanup(() => { io.disconnect(); props.onSeen(false); });
  });

  // A finished run's recording, read once the tile is first near the screen.
  createEffect(on([mode, seenOnce], ([m, seen]) => {
    if (m !== "replay" || !seen) return;
    const ac = new AbortController();
    setFailed(false);
    props.read(() => ac.signal.aborted ? Promise.reject(new DOMException("aborted", "AbortError")) : fetchRecording(live.api(), id, ac.signal))
      .then(({ frames, events }) => {
        if (ac.signal.aborted) return;
        if (!frames.length) { setFailed(true); return; }
        const tl = new ReplayTimeline(frames, events?.length ? events : eventsFromState(game()));
        props.clock.cover(tl.endTick - tl.startTick, performance.now());
        setTimeline(tl);
      }, (e: Error) => { if (e.name !== "AbortError") setFailed(true); });
    onCleanup(() => ac.abort());
  }));

  // A running run's live stream, while the tile holds one of the slots.
  createEffect(on([mode, props.streaming], ([m, streaming]) => {
    if (m !== "live" || !streaming) return;
    let anchored = false;
    const stop = streamSimRun(id, props.position ?? null, props.onEnded, (frame) => {
      if (anchored) return;
      anchored = true;
      setLiveStart(frame.tick - props.run().ticks);
    });
    onCleanup(() => { stop(); frameHub.drop(id); setLiveStart(null); });
  }));

  const replayTick = (now: number) => {
    const tl = timeline();
    return tl ? tileTick(props.clock.elapsed(now), tl.startTick, tl.endTick) : 0;
  };
  const liveFrame = liveFrameSource(id, roster);
  const frame = (now: number): MapFrame => {
    if (mode() === "live") return props.streaming() ? liveFrame(now) : EMPTY;
    const tl = timeline();
    return tl ? replayFrame(tl, replayTick(now), roster()) : EMPTY;
  };

  const elapsed = createMemo(() => {
    const now = props.now();
    const tl = timeline();
    if (mode() === "replay" && tl) return gameClock(replayTick(now) - tl.startTick, tl.ticksPerSecond);
    const start = liveStart();
    if (mode() === "live" && start !== null) return gameClock(frameHub.get(id).renderTick(now) - start);
    return gameClock(props.run().ticks);
  });
  const score = createMemo(() => {
    const tl = timeline();
    if (mode() !== "replay" || !tl?.scores.length) return scoreLabel(props.run());
    return scoreAt(tl.scoresAt(replayTick(props.now())), game().team_play);
  });
  const showMap = () => (mode() === "replay" && !!timeline()) || (mode() === "live" && props.streaming());
  const note = () => {
    if (mode() === "waiting") return "Replay after match";
    if (mode() === "live" && !props.streaming()) return near() ? "Live · open the run to watch" : "Live";
    if (mode() === "replay" && failed()) return "No recorded frames";
    if (mode() === "replay" && !timeline()) return "Loading recording…";
    return null;
  };

  return (
    <article class="adm-watch-tile" ref={tile}>
      <A href={trainingHref(id, props.batch)} aria-label={`Open the ${mapTitle(props.run().map)} run with seed ${props.run().sim.seed}`}>
        <span class="adm-watch-map">
          <Show when={showMap()} fallback={<MapThumb game={game()} class="adm-watch-thumb" />}>
            <TileMap game={game()} roster={roster} frame={frame} />
          </Show>
          <Show when={mode() === "live" && showMap()}><span class="s-live-tag">LIVE</span></Show>
          <span class="adm-watch-clock">{elapsed()}</span>
          <span class="adm-watch-score">{score()}</span>
          <Show when={note()}>{(text) => <span class="adm-watch-wait">{text()}</span>}</Show>
        </span>
        <span class="adm-watch-info">
          <b>{mapTitle(props.run().map)}</b>
          <small>Seed {props.run().sim.seed}</small>
          <small>{armsLabel(props.run())}</small>
        </span>
      </A>
    </article>
  );
}

/** The map under one tile: its top view when the server has one, else
 *  its outline, drawn small by the shared map renderers. */
function TileMap(props: { game: GameState; roster: () => ReturnType<typeof rosterFor>; frame: (now: number) => MapFrame }) {
  const outline = outlineFor(() => props.game.map);
  const geometry = geometryFor(() => (geometryChanged(props.game) ? undefined : props.game.map), () => props.game.map_id);
  const [broken, setBroken] = createSignal(false);
  const top = () => (broken() ? null : topView(geometry()?.views));
  const label = () => `Map of ${mapTitle(props.game.map)}`;
  return (
    <Show when={top()} fallback={<MapCanvas mini frame={props.frame} roster={props.roster} outline={outline} label={label()} />}>
      {(v) => (
        <span class="s-thumb s-thumb--top adm-watch-thumb">
          <ViewMap mini frame={props.frame} roster={props.roster} views={geometry()!.views!} view={v().id} image={geometry()!.image} label={label()} onError={() => setBroken(true)} />
        </span>
      )}
    </Show>
  );
}
