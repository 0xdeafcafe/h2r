import { For, Show, createMemo, createSignal, createEffect, on } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { live } from "../state/live";
import type { GameState, Kill } from "../state/reducer";
import Scoreboard from "../components/Scoreboard";
import KillFeed from "../components/KillFeed";
import { VerifySummary } from "../components/bits";
import { Block, Crumbs, Title } from "./Layout";
import Carnage from "./Carnage";
import GametypeIcon from "../components/GametypeIcon";
import { MapThumb, gameTitle, hostName, playlist } from "./Side";
import { hms, usDateTime, agoWords } from "./fmt";
import { teamStandings, standings } from "../state/reducer";
import { colourFor, elapsedSeconds, gameSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { clock, titleCase } from "../lib/format";
import { now } from "../lib/now";
import { TICKS_PER_SECOND } from "../state/frames";
import TacticalMap from "../viewer/TacticalMap";
import Transport from "../viewer/Transport";
import { createReplaySession } from "../viewer/session";
import "../styles/stats-game.css";

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
      <Show
        when={game()}
        fallback={
          <div class="s-full s-game">
            <Title class="s-title--bar" back={["see all games", "/stats"]}>Game Details</Title>
            <Show when={missing()} fallback={<div class="s-loading">Loading game…</div>}>
              <p class="s-pad">This game isn't available on this server. <A href="/stats">Back to games</A></p>
            </Show>
          </div>
        }
      >
        {(g) => (
          <div class="s-full s-game">
            <GameView game={g()} />
            <Carnage game={g()} />
          </div>
        )}
      </Show>
    </>
  );
}

function Header(props: { game: GameState }) {
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
        <MapThumb game={g()} top />
        <ul class="s-summary">
          <li class="first"><GametypeIcon variant={g().variant} size={28} /><span>{gameTitle(g())}<small>on {titleCase(g().map)}</small></span></li>
          <li class="s-match-status"><Show when={g().status === "live"} fallback={<span class="s-final">FINAL</span>}><span class="s-live-tag">LIVE</span></Show><span>{result()}</span></li>
          <li class="styled">Playlist - {playlist(g())}{g().score_limit ? `, first to ${g().score_limit}` : ""}{g().time_limit_s ? `, ${Math.round(g().time_limit_s! / 60)} minutes` : ""}</li>
          <li title={g().ended_at ? `Ended ${agoWords(g().ended_at, now())}` : undefined}>{usDateTime(g().started_at)}</li>
          <li title={lengthNote(g(), now())}>Length: <span class="s-match-length">{hms(gameSeconds(g(), now()))}</span></li>
        </ul>
        <section class="s-booty" aria-label="Game record">
          <header>Game Record</header>
          <dl class="s-booty-body">
            <div><dt>Players</dt><dd>{Object.keys(g().players).length}</dd></div>
            <div><dt>Total kills</dt><dd>{Object.values(g().scores).reduce((n, s) => n + s.kills, 0)}</dd></div>
            <Show when={host()}><div><dt>Host</dt><dd title={g().console ?? undefined}>{host()}</dd></div></Show>
            <div><dt>Recording</dt><dd>{g().status === "live" ? "Live stream" : "Recorded game"}</dd></div>
          </dl>
          <footer><VerifySummary positions={() => g().positions} /></footer>
        </section>
      </div>
    </>
  );
}

/** Says which clock the length is on, and the other clock when they differ. */
function lengthNote(g: GameState, now: number): string | undefined {
  if (g.status !== "ended") return undefined;
  const game = gameSeconds(g, now), wall = elapsedSeconds(g, now);
  if (Math.abs(wall - game) < 5) return "Game time";
  const why = g.end_reason === "lost" ? "; a lost game ends when the server stops waiting for the console" : "";
  return `Game time. Start to end on the server's clock: ${hms(wall)}${why}`;
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

function GameView(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const [hover, setHover] = createSignal<number | null>(null);
  return (
    <Show when={props.game.status === "live"} fallback={<Replay game={props.game} />}>
      <Header game={props.game} />
      <Title right={<span class="s-viewer-status"><i />Live coverage</span>}>Game Viewer</Title>
      <div class="s-viewer">
        <Block title="Tactical Map" right={`${titleCase(props.game.map)}`} class="s-sec">
          <TacticalMap game={props.game} frame={liveFrameSource(props.game.game_id, roster)} roster={roster} highlight={hover} label={`Live map of ${props.game.map}`} />
          <Legend game={props.game} />
          <div class="s-live-clock"><span>Match in progress</span><b>{hms(elapsedSeconds(props.game, now()))}</b></div>
        </Block>
        <aside class="s-viewer-side">
          <Block title="Scoreboard" class="s-sec"><Scoreboard game={props.game} onHover={setHover} /></Block>
          <Block title="Field Report" right="newest first" class="s-sec"><KillFeed game={props.game} kills={props.game.kills} /></Block>
        </aside>
      </div>
    </Show>
  );
}

function Replay(props: { game: GameState }) {
  const [hover, setHover] = createSignal<number | null>(null);
  const roster = createMemo(() => rosterFor(props.game));
  // One session drives the 2D map, the scrubber and the 3D window alike.
  const session = createReplaySession(() => props.game, roster);
  const { timeline, error, tick } = session;
  const killColour = (k: number | null) => (k !== null ? colourFor(props.game, props.game.players[k]?.team ?? 0, k) : undefined);

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

  const onKey = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === " ") { e.preventDefault(); session.toggle(); }
    if (e.key === "ArrowRight") { e.preventDefault(); session.seek(tick() + 5 * TICKS_PER_SECOND); }
    if (e.key === "ArrowLeft") { e.preventDefault(); session.seek(tick() - 5 * TICKS_PER_SECOND); }
  };

  return (
    <>
      <Header game={props.game} />
      <Title right={<span class="s-viewer-status">Recorded replay</span>}>Game Viewer</Title>
      <div class="s-viewer">
        <Block title="Tactical Map" right={titleCase(props.game.map)} class="s-sec">
          <Show when={!error()} fallback={<p class="s-pad s-bad">Couldn't load the replay: {error()}</p>}>
            <Show when={timeline()} fallback={<div class="s-loading">Loading recorded frames…</div>}>
              {(tl) => (
                <Show when={tl().frames.length} fallback={<p class="s-empty">No recorded frames for this game. The scoreboard and field report are still here.</p>}>
                  <TacticalMap
                    game={props.game} frame={session.frame} roster={roster} highlight={hover} label={`Replay of ${props.game.map}`}
                    onKeyDown={onKey} keysLabel="Replay map. Space plays or pauses; arrow keys step five seconds."
                    soughtAt={session.soughtAt}
                    transport={() => <Transport session={session} killColour={killColour} />}
                  />
                  <Legend game={props.game} />
                  <div class="s-timeline-head"><span>Match Timeline</span><small>Markers show kills · Space to play</small></div>
                  <Transport session={session} killColour={killColour} />
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
