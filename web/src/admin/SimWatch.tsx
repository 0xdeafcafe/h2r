// Clanker Training's Watch view: the runs on the page, each playing its run.
// Finished runs replay their recording on one shared fast-forward clock;
// running runs stream live at the sim's own pace. A batch shows as a grid
// of map tiles or as One map, every run of a map drawn together in one
// viewer; the training page shows One map for the runs it lists
// (docs/runtime/sim.md, "Watching in Uplink").

import { A } from "@solidjs/router";
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount, type Accessor, type JSX } from "solid-js";
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
  BatchClock, DEFAULT_WATCH_SPEED, MAX_RECORDING_READS, WATCH_SPEEDS, armsLabel, gameClock, groupByMap, limiter, mergeFrames,
  mergeRosters, orderTiles, runColour, scoreAt, streamingTiles, tileMode, tileTick,
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

/** The shared clock for a set of runs, with the state its controls show and
 *  the limit on recording downloads. Call it in the component that owns
 *  the controls. */
export function createWatchClock() {
  const clock = new BatchClock(DEFAULT_WATCH_SPEED);
  clock.play(performance.now());
  const read = limiter(MAX_RECORDING_READS);
  const [now, setNow] = createSignal(performance.now());
  const [playing, setPlaying] = createSignal(clock.playing);
  const [speed, setSpeed] = createSignal(clock.speed);

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
  return {
    clock, read, now, playing, speed,
    atEnd: () => !playing() && clock.end > 0 && clock.elapsed(now()) >= clock.end,
    // No recording has loaded yet: the clock waits at zero and plays when one does.
    loading: () => { now(); return clock.end === 0; },
    toggle: () => {
      const t = performance.now();
      if (clock.playing) clock.pause(t); else clock.play(t);
      changed();
    },
    restart: () => {
      const t = performance.now();
      clock.seek(0, t);
      clock.play(t);
      changed();
    },
    changeSpeed: (v: number) => { clock.setSpeed(v, performance.now()); changed(); },
  };
}
export type WatchClock = ReturnType<typeof createWatchClock>;

/** Play, Restart and Speed for the shared clock, shown when some run
 *  replays; `children` sit at the end of the row. */
function WatchControls(props: { watch: WatchClock; replays: boolean; children?: JSX.Element }) {
  return (
    <div class="adm-row" role="group" aria-label="Batch replay controls">
      <Show when={props.replays}>
        <button class="s-btn" disabled={props.watch.loading()} onClick={props.watch.toggle}>
          {props.watch.playing() || props.watch.loading() ? "Pause" : props.watch.atEnd() ? "Play again" : "Play"}
        </button>
        <button class="s-btn" disabled={props.watch.loading()} onClick={props.watch.restart}>Restart</button>
        <label class="adm-field">
          <span>Speed</span>
          <select value={props.watch.speed()} onChange={(e) => props.watch.changeSpeed(+e.currentTarget.value)}>
            <For each={WATCH_SPEEDS}>{(v) => <option value={v} selected={props.watch.speed() === v}>{v}×</option>}</For>
          </select>
        </label>
      </Show>
      {props.children}
      <Show when={props.replays}>
        <small class="adm-watch-note">Finished runs play in sync from their first frame. Running runs play live at the sim's own speed.</small>
      </Show>
    </div>
  );
}

/** The grid for one batch, with its replay controls and the switch to One
 *  map. `position` is the run list's log position, where live streams
 *  start. `highlight` is a run to emphasise in One map (the table row under
 *  the pointer). */
export function BatchWatch(props: { batch: string; runs: SimRun[]; position?: number | null; highlight?: Accessor<string | null>; onEnded: () => void }) {
  const ordered = createMemo(() => orderTiles(props.runs));
  const ids = createMemo(() => ordered().map((r) => r.game_id), [], {
    equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]),
  });
  const byId = createMemo(() => new Map(props.runs.map((r) => [r.game_id, r])));
  const [layout, setLayout] = createSignal<"grid" | "map">("grid");
  const [visible, setVisible] = createSignal<ReadonlySet<string>>(new Set());
  const streaming = createMemo(() => streamingTiles(ordered().filter((r) => tileMode(r) === "live").map((r) => r.game_id), visible()));
  const replays = () => ordered().some((r) => tileMode(r) === "replay");
  const watch = createWatchClock();
  const seen = (id: string, inView: boolean) => setVisible((old) => {
    if (old.has(id) === inView) return old;
    const next = new Set(old);
    if (inView) next.add(id); else next.delete(id);
    return next;
  });

  return (
    <section class="adm-watch" aria-label={`Watch batch ${props.batch}`}>
      <WatchControls watch={watch} replays={replays()}>
        <div class="adm-watch-switch" role="group" aria-label="Watch layout">
          <button type="button" aria-pressed={layout() === "grid"} onClick={() => setLayout("grid")}>Grid</button>
          <button type="button" aria-pressed={layout() === "map"} onClick={() => setLayout("map")}>One map</button>
        </div>
      </WatchControls>
      <Show when={layout() === "grid"} fallback={
        <MapOverlays runs={props.runs} watch={watch} position={props.position} highlight={props.highlight} onEnded={props.onEnded} />
      }>
        <div class="adm-watch-grid">
          <For each={ids()}>{(id) => (
            <Show when={byId().get(id)}>{(run) => (
              <WatchTile
                run={run} batch={props.batch} watch={watch} position={props.position}
                streaming={() => streaming().has(id)} onSeen={(inView) => seen(id, inView)} onEnded={props.onEnded}
              />
            )}</Show>
          )}</For>
        </div>
      </Show>
    </section>
  );
}

/** The runs listed on the training page, one viewer per map with every run
 *  drawn together, and the shared replay controls. */
export function OneMapWatch(props: { runs: SimRun[]; position?: number | null; highlight?: Accessor<string | null>; onEnded: () => void }) {
  const watch = createWatchClock();
  return (
    <section class="adm-watch" aria-label="Watch the listed runs on one map">
      <WatchControls watch={watch} replays={props.runs.some((r) => tileMode(r) === "replay")} />
      <MapOverlays runs={props.runs} watch={watch} position={props.position} highlight={props.highlight} onEnded={props.onEnded} />
    </section>
  );
}

interface RunSourceOptions {
  run: Accessor<SimRun>;
  watch: WatchClock;
  position?: number | null;
  /** The run's view has been near the screen: its recording may load. */
  active: Accessor<boolean>;
  /** The run holds one of the live stream slots. */
  streaming: Accessor<boolean>;
  onEnded: () => void;
}

/** Everything a view needs to play one run on the shared clock: a finished
 *  run's recording, read once `active`, or a running run's live stream while
 *  it holds a slot. The grid's tiles and the One map overlay both use it.
 *  Call it in a component of its own, one per run. */
function createRunSource(o: RunSourceOptions) {
  const id = o.run().game_id;
  const game = createMemo(() => gameOf(o.run()));
  const roster = createMemo(() => rosterFor(game()));
  const mode = createMemo(() => tileMode(o.run()));
  const [timeline, setTimeline] = createSignal<ReplayTimeline | null>(null);
  const [failed, setFailed] = createSignal(false);
  const [liveStart, setLiveStart] = createSignal<number | null>(null);

  // A finished run's recording, read once the run's view is first near the screen.
  createEffect(on([mode, o.active], ([m, active]) => {
    if (m !== "replay" || !active) return;
    const ac = new AbortController();
    setFailed(false);
    o.watch.read(() => ac.signal.aborted ? Promise.reject(new DOMException("aborted", "AbortError")) : fetchRecording(live.api(), id, ac.signal))
      .then(({ frames, events }) => {
        if (ac.signal.aborted) return;
        if (!frames.length) { setFailed(true); return; }
        const tl = new ReplayTimeline(frames, events?.length ? events : eventsFromState(game()));
        o.watch.clock.cover(tl.endTick - tl.startTick, performance.now());
        setTimeline(tl);
      }, (e: Error) => { if (e.name !== "AbortError") setFailed(true); });
    onCleanup(() => ac.abort());
  }));

  // A running run's live stream, while it holds one of the slots.
  createEffect(on([mode, o.streaming], ([m, streaming]) => {
    if (m !== "live" || !streaming) return;
    let anchored = false;
    const stop = streamSimRun(id, o.position ?? null, o.onEnded, (frame) => {
      if (anchored) return;
      anchored = true;
      setLiveStart(frame.tick - o.run().ticks);
    });
    onCleanup(() => { stop(); frameHub.drop(id); setLiveStart(null); });
  }));

  const replayTick = (now: number) => {
    const tl = timeline();
    return tl ? tileTick(o.watch.clock.elapsed(now), tl.startTick, tl.endTick) : 0;
  };
  const liveFrame = liveFrameSource(id, roster);
  const frame = (now: number): MapFrame => {
    if (mode() === "live") return o.streaming() ? liveFrame(now) : EMPTY;
    const tl = timeline();
    return tl ? replayFrame(tl, replayTick(now), roster()) : EMPTY;
  };

  const elapsed = createMemo(() => {
    const now = o.watch.now();
    const tl = timeline();
    if (mode() === "replay" && tl) return gameClock(replayTick(now) - tl.startTick, tl.ticksPerSecond);
    const start = liveStart();
    if (mode() === "live" && start !== null) return gameClock(frameHub.get(id).renderTick(now) - start);
    return gameClock(o.run().ticks);
  });
  const score = createMemo(() => {
    const tl = timeline();
    if (mode() !== "replay" || !tl?.scores.length) return scoreLabel(o.run());
    return scoreAt(tl.scoresAt(replayTick(o.watch.now())), game().team_play);
  });
  const showMap = () => (mode() === "replay" && !!timeline()) || (mode() === "live" && o.streaming());
  return { game, roster, mode, timeline, failed, frame, elapsed, score, showMap };
}
type RunSource = ReturnType<typeof createRunSource>;

function WatchTile(props: {
  run: Accessor<SimRun>;
  batch: string;
  watch: WatchClock;
  position?: number | null;
  streaming: Accessor<boolean>;
  onSeen: (inView: boolean) => void;
  onEnded: () => void;
}) {
  const id = props.run().game_id;
  const [seenOnce, setSeenOnce] = createSignal(false);
  const [near, setNear] = createSignal(false);
  const src = createRunSource({
    run: props.run, watch: props.watch, position: props.position, active: seenOnce, streaming: props.streaming, onEnded: props.onEnded,
  });
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

  const note = () => {
    if (src.mode() === "waiting") return "Replay after match";
    if (src.mode() === "live" && !props.streaming()) return near() ? "Live · open the run to watch" : "Live";
    if (src.mode() === "replay" && src.failed()) return "No recorded frames";
    if (src.mode() === "replay" && !src.timeline()) return "Loading recording…";
    return null;
  };

  return (
    <article class="adm-watch-tile" ref={tile}>
      <A href={trainingHref(id, props.batch)} aria-label={`Open the ${mapTitle(props.run().map)} run with seed ${props.run().sim.seed}`}>
        <span class="adm-watch-map">
          <Show when={src.showMap()} fallback={<MapThumb game={src.game()} class="adm-watch-thumb" />}>
            <TileMap mini game={src.game()} roster={src.roster} frame={src.frame} />
          </Show>
          <Show when={src.mode() === "live" && src.showMap()}><span class="s-live-tag">LIVE</span></Show>
          <span class="adm-watch-clock">{src.elapsed()}</span>
          <span class="adm-watch-score">{src.score()}</span>
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

/** The One map overlay: one viewer per map, each drawing all of that map's
 *  runs together on the shared clock. The live stream slots are shared by
 *  every viewer on screen. */
function MapOverlays(props: { runs: SimRun[]; watch: WatchClock; position?: number | null; highlight?: Accessor<string | null>; onEnded: () => void }) {
  const groups = createMemo(() => groupByMap(props.runs));
  const maps = createMemo(() => groups().map((g) => g.map), [], {
    equals: (a, b) => a.length === b.length && a.every((m, i) => m === b[i]),
  });
  const [seenMaps, setSeenMaps] = createSignal<ReadonlySet<string>>(new Set());
  const streaming = createMemo(() => {
    const all = groups();
    const onScreen = all.filter((g) => seenMaps().has(g.map));
    return streamingTiles(
      all.flatMap((g) => g.runs.filter((r) => tileMode(r) === "live").map((r) => r.game_id)),
      new Set(onScreen.flatMap((g) => g.runs.map((r) => r.game_id))),
    );
  });
  const seen = (map: string, inView: boolean) => setSeenMaps((old) => {
    if (old.has(map) === inView) return old;
    const next = new Set(old);
    if (inView) next.add(map); else next.delete(map);
    return next;
  });
  return (
    <For each={maps()}>{(map) => (
      <OneMapViewer
        map={map} runs={() => groups().find((g) => g.map === map)?.runs ?? []} watch={props.watch} position={props.position}
        streaming={streaming} highlight={props.highlight} onSeen={(inView) => seen(map, inView)} onEnded={props.onEnded}
      />
    )}</For>
  );
}

/** One map's viewer: every run's players drawn together, each run in its
 *  own colour, with a legend. Pointing at a legend row draws that run at
 *  full strength and the rest dimmed; clicking it holds that. */
function OneMapViewer(props: {
  map: string;
  runs: Accessor<SimRun[]>;
  watch: WatchClock;
  position?: number | null;
  streaming: Accessor<ReadonlySet<string>>;
  highlight?: Accessor<string | null>;
  onSeen: (inView: boolean) => void;
  onEnded: () => void;
}) {
  // Runs that upload their recording only after the match have nothing to draw yet.
  const shown = createMemo(() => props.runs().filter((r) => tileMode(r) !== "waiting"));
  const ids = createMemo(() => shown().map((r) => r.game_id), [], {
    equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]),
  });
  const byId = createMemo(() => new Map(props.runs().map((r) => [r.game_id, r])));
  /** A run's slot is its place among the map's runs in tile order, so its colour holds as others come and go. */
  const slotOf = (id: string) => props.runs().findIndex((r) => r.game_id === id);
  const [seenOnce, setSeenOnce] = createSignal(false);
  const [hovered, setHovered] = createSignal<string | null>(null);
  const [pinned, setPinned] = createSignal<string | null>(null);
  const focus = createMemo(() => {
    const f = hovered() ?? pinned() ?? props.highlight?.() ?? null;
    return f !== null && ids().includes(f) ? f : null;
  });
  const sources = new Map<string, RunSource>();
  const [registered, setRegistered] = createSignal(0);
  let view!: HTMLDivElement;

  onMount(() => {
    const io = new IntersectionObserver((es) => {
      const inView = es.some((e) => e.isIntersecting);
      if (inView) setSeenOnce(true);
      props.onSeen(inView);
    }, { rootMargin: "200px" });
    io.observe(view);
    onCleanup(() => { io.disconnect(); props.onSeen(false); });
  });

  const partOf = (id: string) => {
    const src = sources.get(id);
    const run = byId().get(id);
    if (!src || !run) return null;
    const slot = slotOf(id);
    return { slot, label: `Seed ${run.sim.seed}`, colour: runColour(slot), dim: focus() !== null && focus() !== id, src };
  };
  const roster = createMemo(() => {
    registered();
    return mergeRosters(ids().flatMap((id) => {
      const p = partOf(id);
      return p ? [{ ...p, roster: p.src.roster() }] : [];
    }));
  });
  const frame = (now: number): MapFrame => mergeFrames(ids().flatMap((id) => {
    const p = partOf(id);
    return p ? [{ ...p, frame: p.src.frame(now) }] : [];
  }));
  const game = createMemo(() => gameOf(props.runs()[0]!));
  const held = () => props.runs().length - shown().length;

  return (
    <section class="adm-onemap" aria-label={`${mapTitle(props.map)}, ${shown().length} runs on one map`}>
      <h3>{mapTitle(props.map)} <small>{shown().length} {shown().length === 1 ? "run" : "runs"}</small></h3>
      <Show when={shown().length > 0}>
        <div class="adm-onemap-body">
          <div class="adm-onemap-view" ref={view}>
            <TileMap game={game()} roster={roster} frame={frame} />
          </div>
          <ul class="adm-onemap-legend" aria-label="Runs on this map">
            <For each={ids()}>{(id) => (
              <Show when={byId().get(id)}>{(run) => (
                <OverlayRow
                  run={run} watch={props.watch} position={props.position} active={seenOnce}
                  streaming={() => props.streaming().has(id)} onEnded={props.onEnded}
                  colour={runColour(slotOf(id))} focused={() => focus() === id} pinned={() => pinned() === id}
                  onHover={(on) => setHovered(on ? id : null)} onPin={() => setPinned((p) => (p === id ? null : id))}
                  onSource={(src) => {
                    sources.set(id, src);
                    setRegistered((n) => n + 1);
                    onCleanup(() => { sources.delete(id); setRegistered((n) => n + 1); });
                  }}
                />
              )}</Show>
            )}</For>
          </ul>
        </div>
      </Show>
      <Show when={held() > 0}>
        <small class="adm-watch-note">{held()} {held() === 1 ? "run is" : "runs are"} left out until the recording uploads after the match.</small>
      </Show>
    </section>
  );
}

/** A legend row: the run's colour, seed, arms and live score, and a link to
 *  the run. It owns the run's source, which the viewer draws from. */
function OverlayRow(props: {
  run: Accessor<SimRun>;
  watch: WatchClock;
  position?: number | null;
  active: Accessor<boolean>;
  streaming: Accessor<boolean>;
  colour: string;
  focused: Accessor<boolean>;
  pinned: Accessor<boolean>;
  onHover: (on: boolean) => void;
  onPin: () => void;
  onSource: (src: RunSource) => void;
  onEnded: () => void;
}) {
  const src = createRunSource({
    run: props.run, watch: props.watch, position: props.position, active: props.active, streaming: props.streaming, onEnded: props.onEnded,
  });
  props.onSource(src);
  const note = () => {
    if (src.mode() === "live") return props.streaming() ? "Live" : "Live · not streaming";
    if (src.failed()) return "No recorded frames";
    return src.timeline() ? null : "Loading recording…";
  };
  return (
    <li
      classList={{ on: props.focused(), pinned: props.pinned() }}
      onPointerEnter={() => props.onHover(true)} onPointerLeave={() => props.onHover(false)}
    >
      <button type="button" aria-pressed={props.pinned()} onClick={props.onPin} onFocus={() => props.onHover(true)} onBlur={() => props.onHover(false)}>
        <i style={{ background: props.colour }} aria-hidden="true" />
        <b>Seed {props.run().sim.seed}</b>
        <span class="adm-onemap-arms">{armsLabel(props.run())}</span>
        <span class="adm-onemap-score">{src.score()}</span>
        <Show when={note()}>{(text) => <small>{text()}</small>}</Show>
      </button>
      <A href={trainingHref(props.run().game_id, props.run().sim.batch_id)} aria-label={`Open the ${mapTitle(props.run().map)} run with seed ${props.run().sim.seed}`}>Open</A>
    </li>
  );
}

/** The map under a tile or a viewer: its top view when the server has one,
 *  else its outline, drawn by the shared map renderers. `mini` is a tile's
 *  small thumbnail; otherwise it fills its container. */
function TileMap(props: { mini?: boolean; game: GameState; roster: () => ReturnType<typeof rosterFor>; frame: (now: number) => MapFrame }) {
  const outline = outlineFor(() => props.game.map);
  const geometry = geometryFor(() => (geometryChanged(props.game) ? undefined : props.game.map), () => props.game.map_id);
  const [broken, setBroken] = createSignal(false);
  const top = () => (broken() ? null : topView(geometry()?.views));
  const label = () => `Map of ${mapTitle(props.game.map)}`;
  return (
    <Show when={top()} fallback={<MapCanvas mini={props.mini} frame={props.frame} roster={props.roster} outline={outline} label={label()} />}>
      {(v) => (
        <span class="s-thumb s-thumb--top adm-watch-thumb">
          <ViewMap mini={props.mini} frame={props.frame} roster={props.roster} views={geometry()!.views!} view={v().id} image={geometry()!.image} label={label()} onError={() => setBroken(true)} />
        </span>
      )}
    </Show>
  );
}
