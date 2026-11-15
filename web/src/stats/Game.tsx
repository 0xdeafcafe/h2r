import { For, Show, createMemo, createSignal, onCleanup, createEffect, on } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { live } from "../state/live";
import type { GameState, Kill } from "../state/reducer";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import Scoreboard from "../components/Scoreboard";
import KillFeed from "../components/KillFeed";
import { VerifySummary } from "../components/bits";
import { Block, Crumbs, Strip, Title } from "./Layout";
import Carnage from "./Carnage";
import GametypeIcon from "../components/GametypeIcon";
import { MapThumb, gameTitle, hostName, playlist } from "./Side";
import { hms, usDateTime, agoWords } from "./fmt";
import { teamStandings, standings } from "../state/reducer";
import { colourFor, elapsedSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { clock, titleCase } from "../lib/format";
import { now } from "../lib/now";
import { outlineFor } from "../state/outlines";
import { ReplayClock, ReplayTimeline } from "../state/replay";
import { TICKS_PER_SECOND } from "../state/frames";
import type { WireEvent } from "../api/types";
import { onAnimationFrame } from "../map/scheduler";

export default function Game() {
  const params = useParams<{ id: string }>();
  const game = () => live.state.games[params.id];
  // A game outside the snapshot (older, or a deep link): fetch its detail.
  const [missing, setMissing] = createSignal(false);
  createEffect(on(() => [params.id, live.loadMode()] as const, ([id, mode]) => {
    setMissing(false);
    if (mode === "snapshot") return;
    void live.ensureGame(id).then((ok) => setMissing(!ok && !live.state.games[id]));
  }));
  return (
    <>
      <Crumbs trail={[["Home", "/"], ["h2 live Online", "/stats"], [game()?.status === "ended" ? "Carnage Report" : "Game Details"]]} />
      <Strip kind="game" />
      <Show
        when={game()}
        fallback={
          <div class="s-full">
            <Title class="s-title--bar" back={["see all games", "/stats"]}>Game Details</Title>
            <Show when={missing()} fallback={<div class="s-loading">Loading game…</div>}>
              <p class="s-pad">This game isn't available on this server. <A href="/stats">Back to games</A></p>
            </Show>
          </div>
        }
      >
        {(g) => (
          <div class="s-full">
            <GameView game={g()} />
            <Carnage game={g()} />
          </div>
        )}
      </Show>
    </>
  );
}

function Header(props: { game: GameState; replayTick?: number }) {
  const g = () => props.game;
  const result = () => {
    const w = g().winner;
    if (g().status !== "ended") return "In progress";
    if (!w) return g().end_reason === "quit" ? "Ended early" : g().end_reason === "lost" ? "Connection lost" : "No winner";
    return "team" in w ? `${teamName(w.team)} Team won` : `${g().players[w.player]?.name ?? "Someone"} placed 1st`;
  };
  const host = () => hostName(g());
  return (
    <>
      <Title class="s-title--bar" back={["see all games", "/stats"]}>Game Details</Title>
      <div class="s-overview">
        <MapThumb game={g()} />
        <ul class="s-summary">
          <li class="first"><GametypeIcon variant={g().variant} size={20} />{gameTitle(g())} on {titleCase(g().map)} <Show when={g().status === "live"} fallback={<span class="s-final">FINAL</span>}><span class="s-live-tag">LIVE</span></Show></li>
          <li class="break styled">{result()}</li>
          <li class="styled">Playlist - {playlist(g())}{g().score_limit ? `, first to ${g().score_limit}` : ""}{g().time_limit_s ? `, ${Math.round(g().time_limit_s! / 60)} minutes` : ""}</li>
          <li>{usDateTime(g().started_at)}<Show when={g().ended_at}> (ended {agoWords(g().ended_at, now())})</Show></li>
          <li>{props.replayTick !== undefined ? "Replay at" : "Length:"} {hms(props.replayTick !== undefined ? props.replayTick / TICKS_PER_SECOND : elapsedSeconds(g(), now()))}</li>
        </ul>
        <div class="s-booty">
          <header><span>Game Record ({g().positions.length})</span><span><VerifySummary positions={() => g().positions} /></span></header>
          <ul class="s-booty-body">
            <li>Players ({Object.keys(g().players).length})</li>
            <li>Knockouts ({Object.values(g().scores).reduce((n, s) => n + s.kills, 0)})</li>
            <Show when={host()}><li>Host: {host()}<Show when={g().console && g().console !== host() && !live.state.accounts[g().console!]}> ({g().console})</Show></li></Show>
            <li>{g().status === "live" ? "Positions streamed at up to 15 Hz" : "Replay recorded at 1 Hz"}</li>
            <li>Every event signed and chained</li>
          </ul>
        </div>
      </div>
    </>
  );
}

function Legend(props: { game: GameState }) {
  return (
    <div class="s-legend" aria-label="Map legend">
      <Show
        when={props.game.team_play}
        fallback={<For each={standings(props.game)}>{(s) => <span><i style={{ background: colourFor(props.game, s.team, s.index) }} />{s.name}</span>}</For>}
      >
        <For each={teamStandings(props.game)}>{(t) => <span><i style={{ background: colourFor(props.game, t.team, 0) }} />{teamName(t.team)} Team</span>}</For>
      </Show>
      <span><i class="wedge" />facing</span>
      <span><i class="x" />knockout</span>
    </div>
  );
}

function LayerPicker(props: { count: number; value: number | null; onChange: (v: number | null) => void }) {
  return (
    <Show when={props.count > 1}>
      <div class="layers" role="radiogroup" aria-label="Floor">
        <button classList={{ on: props.value === null }} onClick={() => props.onChange(null)}>All floors</button>
        <For each={Array.from({ length: props.count }, (_, i) => i)}>
          {(i) => <button classList={{ on: props.value === i }} onClick={() => props.onChange(i)}>Floor {i + 1}</button>}
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
      <Title right={<span class="s-live-tag">LIVE · up to 15 Hz</span>}>Game Viewer</Title>
      <div class="s-viewer">
        <Block title="Tactical Map" right={`${titleCase(props.game.map)}`} class="s-sec">
          <div class="s-map">
            <MapCanvas frame={liveFrameSource(props.game.game_id, roster)} roster={roster} outline={outline} layer={layer} highlight={hover} label={`Live map of ${props.game.map}`} />
            <LayerPicker count={outline()?.layers.length ?? 0} value={layer()} onChange={setLayer} />
            <Show when={!outline()}><div class="s-map-note">Map outline not available; the grid is fitted to where players have been.</div></Show>
          </div>
          <Legend game={props.game} />
        </Block>
        <aside class="s-viewer-side">
          <Block title="Scoreboard" class="s-sec"><Scoreboard game={props.game} onHover={setHover} /></Block>
          <Block title="Field Report" right="newest first" class="s-sec"><KillFeed game={props.game} kills={props.game.kills} /></Block>
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
      // Open on the final moment; play is the viewer's choice.
      clockRef.seek(t.endTick, performance.now());
      setTick(t.endTick);
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
    return tl.killsBetween(-Infinity, tick()).map((k) => ({ ...k, at: "", pos: k.at ? [k.at.x, k.at.y, k.at.z] : null }));
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
      <Title right={<span class="s-dim">replay · recorded at 1 Hz · space plays, arrows step</span>}>Game Viewer</Title>
      <div class="s-viewer">
        <Block title="Tactical Map" right={titleCase(props.game.map)} class="s-sec">
          <Show when={!error()} fallback={<p class="s-pad s-bad">Couldn't load the replay: {error()}</p>}>
            <Show when={timeline()} fallback={<div class="s-loading">Loading recorded frames…</div>}>
              {(tl) => (
                <Show when={tl().frames.length} fallback={<p class="s-empty">No recorded frames for this game. The scoreboard and field report are still here.</p>}>
                  <div class="s-map">
                  <MapCanvas frame={frame} roster={roster} outline={outline} layer={layer} highlight={hover} label={`Replay of ${props.game.map}`} />
                  <LayerPicker count={outline()?.layers.length ?? 0} value={layer()} onChange={setLayer} />
                  </div>
                  <Legend game={props.game} />
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
        </Block>
        <aside class="s-viewer-side">
          <Block title="Scoreboard" right={`at ${clock((tick() - (timeline()?.startTick ?? 0)) / TICKS_PER_SECOND)}`} class="s-sec"><Scoreboard game={snapshot()} onHover={setHover} /></Block>
          <Block title="Field Report" right="newest first" class="s-sec"><KillFeed game={props.game} kills={kills()} /></Block>
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
