import { For, Show, createMemo, createSignal, onCleanup, createEffect, on } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { live } from "../state/live";
import type { GameState, Kill } from "../state/reducer";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import Scoreboard from "../components/Scoreboard";
import KillFeed from "../components/KillFeed";
import { Empty, VerifySummary } from "../components/bits";
import { colourFor, elapsedSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { clock, titleCase, ago } from "../lib/format";
import { now } from "../lib/now";
import { outlineFor } from "../state/outlines";
import { ReplayClock, ReplayTimeline } from "../state/replay";
import { TICKS_PER_SECOND } from "../state/frames";
import type { WireEvent } from "../api/types";
import { onAnimationFrame } from "../map/scheduler";

export default function Game() {
  const params = useParams<{ id: string }>();
  const game = () => live.state.games[params.id];
  return (
    <div class="page game-page">
      <Show
        when={game()}
        fallback={
          <Show when={live.state.caughtUp} fallback={<div class="loading">Looking for that game…</div>}>
            <Empty title="We couldn't find that game" mood="wow">
              Maybe it was on another server? <A href="/">Back home</A>
            </Empty>
          </Show>
        }
      >
        {(g) => <GameView game={g()} />}
      </Show>
    </div>
  );
}

function Header(props: { game: GameState; replayTick?: number }) {
  const g = () => props.game;
  const winner = () => {
    const w = g().winner;
    if (w === null || g().status !== "ended") return null;
    return g().team_play ? `${teamName(w)} team wins!` : `${g().players[w]?.name ?? "Someone"} wins!`;
  };
  return (
    <header class="game-head">
      <div class="game-head__main">
        <A href="/" class="back">← home</A>
        <h1>{titleCase(g().map)}</h1>
        <div class="game-head__meta">
          <span class="tag">{titleCase(g().variant ?? g().mode)}</span>
          <span class="tag">{g().team_play ? "teams" : "free for all"}</span>
          <Show when={g().score_limit}><span class="tag">first to {g().score_limit}</span></Show>
          <Show when={g().time_limit_s}><span class="tag">{Math.round(g().time_limit_s! / 60)} min</span></Show>
        </div>
      </div>
      <div class="game-head__side">
        <Show
          when={g().status === "live"}
          fallback={<span class="status status--ended">ended {ago(g().ended_at, now())}{g().end_reason && g().end_reason !== "finished" ? ` (${g().end_reason})` : ""}</span>}
        >
          <span class="status status--live">live</span>
        </Show>
        <span class="game-clock">{clock(props.replayTick !== undefined ? props.replayTick / TICKS_PER_SECOND : elapsedSeconds(g(), now()))}</span>
        <Show when={winner()}><span class="winner">{winner()}</span></Show>
        <VerifySummary positions={() => g().positions} />
      </div>
    </header>
  );
}

function LayerPicker(props: { count: number; value: number | null; onChange: (v: number | null) => void }) {
  return (
    <Show when={props.count > 1}>
      <div class="layers" role="radiogroup" aria-label="Floor">
        <button classList={{ on: props.value === null }} onClick={() => props.onChange(null)}>all floors</button>
        <For each={Array.from({ length: props.count }, (_, i) => i)}>
          {(i) => <button classList={{ on: props.value === i }} onClick={() => props.onChange(i)}>floor {i + 1}</button>}
        </For>
      </div>
    </Show>
  );
}

function GameView(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const [layer, setLayer] = createSignal<number | null>(null);
  const [hover, setHover] = createSignal<number | null>(null);
  return (
    <Show when={props.game.status === "live"} fallback={<Replay game={props.game} />}>
      <Header game={props.game} />
      <div class="game-layout">
        <div class="game-map">
          <MapCanvas frame={liveFrameSource(props.game.game_id, roster)} roster={roster} outline={outline} layer={layer} highlight={hover} label={`Live map of ${props.game.map}`} />
          <LayerPicker count={outline()?.layers.length ?? 0} value={layer()} onChange={setLayer} />
          <Show when={!outline()}><div class="map-note">No outline for this map yet, so we're sketching it from where players have been.</div></Show>
        </div>
        <aside class="game-side">
          <Scoreboard game={props.game} onHover={setHover} />
          <h3 class="side-title">Knockouts</h3>
          <KillFeed game={props.game} kills={props.game.kills} />
        </aside>
      </div>
    </Show>
  );
}

const SPEEDS = [0.5, 1, 2, 4, 8];

function Replay(props: { game: GameState }) {
  const [timeline, setTimeline] = createSignal<ReplayTimeline | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [tick, setTick] = createSignal(0);
  const [playing, setPlaying] = createSignal(false);
  const [speed, setSpeed] = createSignal(1);
  const [layer, setLayer] = createSignal<number | null>(null);
  const [hover, setHover] = createSignal<number | null>(null);
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  let clockRef: ReplayClock | null = null;

  createEffect(on(() => props.game.game_id, (id) => {
    const ac = new AbortController();
    setTimeline(null);
    setError(null);
    const a = live.api();
    Promise.all([
      a.frames(id, undefined, undefined, ac.signal),
      a.streamEvents("game-" + id, ac.signal).catch(() => eventsFromState(props.game)),
    ]).then(([frames, events]) => {
      const t = new ReplayTimeline(frames, events.length ? events : eventsFromState(props.game));
      clockRef = new ReplayClock(t);
      setTimeline(t);
      setTick(t.startTick);
      if (t.frames.length) { clockRef.play(performance.now()); setPlaying(true); }
    }, (e: Error) => { if (e.name !== "AbortError") setError(e.message); });
    onCleanup(() => ac.abort());
  }));

  // Push the clock's tick into Solid ~10 times a second for the UI; the
  // canvas reads the clock directly every frame.
  let last = 0;
  const stop = onAnimationFrame((t) => {
    if (!clockRef || t - last < 100) return;
    last = t;
    setTick(clockRef.tickAt(t));
    if (playing() !== clockRef.playing) setPlaying(clockRef.playing);
  });
  onCleanup(stop);

  const frame = (t: number): MapFrame => {
    const tl = timeline();
    if (!tl || !clockRef) return { players: [], markers: [], seen: null };
    const at = clockRef.tickAt(t);
    const r = roster();
    const fade = 6 * TICKS_PER_SECOND;
    return {
      players: tl.sample(at),
      markers: tl.killsBetween(at - fade, at).filter((k) => k.at).map((k) => ({
        ...k.at!, alpha: 1 - (at - k.tick) / fade, colour: r.get(k.victim)?.colour ?? "#555",
      })),
      seen: boundsOf(tl),
    };
  };

  const snapshot = createMemo((): GameState => {
    const tl = timeline();
    if (!tl) return props.game;
    const s = tl.scoresAt(tick());
    return { ...props.game, scores: s.players, teams: { ...zeroTeams(props.game), ...s.teams } };
  });
  const kills = createMemo((): Kill[] => {
    const tl = timeline();
    if (!tl) return [];
    return tl.killsBetween(-Infinity, tick()).map((k) => ({ ...k, at: "", killer: k.killer, weapon: k.weapon }));
  });

  const toggle = () => {
    if (!clockRef) return;
    const n = performance.now();
    if (clockRef.playing) clockRef.pause(n); else clockRef.play(n);
    setPlaying(clockRef.playing);
  };
  const seek = (t: number) => { clockRef?.seek(t, performance.now()); setTick(clockRef?.tickAt(performance.now()) ?? t); };
  const onKey = (e: KeyboardEvent) => {
    if ((e.target as HTMLElement).tagName === "INPUT" && (e.target as HTMLInputElement).type !== "range") return;
    if (e.key === " ") { e.preventDefault(); toggle(); }
    if (e.key === "ArrowRight") seek(tick() + 5 * TICKS_PER_SECOND);
    if (e.key === "ArrowLeft") seek(tick() - 5 * TICKS_PER_SECOND);
  };
  document.addEventListener("keydown", onKey);
  onCleanup(() => document.removeEventListener("keydown", onKey));

  return (
    <>
      <Header game={props.game} replayTick={timeline() ? tick() - timeline()!.startTick : undefined} />
      <div class="game-layout">
        <div class="game-map">
          <Show when={!error()} fallback={<Empty title="The replay tape got tangled" mood="wow">{error()}</Empty>}>
            <Show when={timeline()} fallback={<div class="loading">Rewinding the tape…</div>}>
              {(tl) => (
                <Show when={tl().frames.length} fallback={<Empty title="No recorded frames for this game" mood="sleepy">The scoreboard and knockouts are still here.</Empty>}>
                  <MapCanvas frame={frame} roster={roster} outline={outline} layer={layer} highlight={hover} label={`Replay of ${props.game.map}`} />
                  <LayerPicker count={outline()?.layers.length ?? 0} value={layer()} onChange={setLayer} />
                  <div class="replay-bar">
                    <button class="play" onClick={toggle} aria-label={playing() ? "Pause" : "Play"}>{playing() ? "❚❚" : "▶"}</button>
                    <div class="scrub">
                      <input
                        type="range"
                        min={tl().startTick}
                        max={tl().endTick}
                        step={1}
                        value={tick()}
                        onInput={(e) => seek(+e.currentTarget.value)}
                        aria-label="Replay position"
                      />
                      <div class="scrub__kills" aria-hidden="true">
                        <For each={tl().kills}>
                          {(k) => (
                            <span
                              class="scrub__kill"
                              style={{
                                left: `${((k.tick - tl().startTick) / Math.max(1, tl().durationTicks)) * 100}%`,
                                background: k.killer !== null ? colourFor(props.game, props.game.players[k.killer]?.team ?? 0, k.killer) : undefined,
                              }}
                            />
                          )}
                        </For>
                      </div>
                    </div>
                    <span class="replay-time">{clock((tick() - tl().startTick) / TICKS_PER_SECOND)} / {clock(tl().durationSeconds)}</span>
                    <select value={speed()} onChange={(e) => { const v = +e.currentTarget.value; setSpeed(v); clockRef?.setSpeed(v, performance.now()); }} aria-label="Speed">
                      <For each={SPEEDS}>{(s) => <option value={s}>{s}×</option>}</For>
                    </select>
                  </div>
                </Show>
              )}
            </Show>
          </Show>
        </div>
        <aside class="game-side">
          <Scoreboard game={snapshot()} onHover={setHover} />
          <h3 class="side-title">Knockouts</h3>
          <KillFeed game={props.game} kills={kills()} />
        </aside>
      </div>
    </>
  );
}

function zeroTeams(g: GameState): Record<number, number> {
  const t: Record<number, number> = {};
  for (const k of Object.keys(g.teams)) t[+k] = 0;
  return t;
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
