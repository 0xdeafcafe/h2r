import { gameMapTitle, packageLookup } from "../state/maps";
import { isHalo1Map, mapTitle } from "../state/gameart";
import { identity } from "../state/identity";
import { reportMap } from "../admin/content";
import { For, Show, createMemo, createSignal, createEffect, on } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { isCampaign, type GameState, type Kill } from "../state/reducer";
import { LOST_NOTE, deathsOf } from "../state/derive";
import { difficultyName, gameTypeLine } from "../state/gametype";
import CampaignReport, { PlayerName, campaignPlayers } from "./CampaignReport";
import Scoreboard from "../components/Scoreboard";
import KillFeed from "../components/KillFeed";
import { VerifySummary } from "../components/bits";
import { Block, Crumbs, Title } from "./Layout";
import Carnage from "./Carnage";
import GametypeIcon from "../components/GametypeIcon";
import { SkullBadges } from "../components/SkullIcon";
import { MapThumb, gameTitle, hostName, resultWords } from "./Side";
import { hms, usDateTime, agoWords } from "./fmt";
import { teamStandings, standings } from "../state/reducer";
import { colourFor, elapsedSeconds, gameSeconds, liveFrameSource, rosterFor, teamName } from "../lib/game";
import { clock } from "../lib/format";
import { now } from "../lib/now";
import { TICKS_PER_SECOND } from "../state/frames";
import TacticalMap from "../viewer/TacticalMap";
import Transport, { replayKey } from "../viewer/Transport";
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
  const crumb = () => {
    const g = game();
    if (g?.status !== "ended") return "Game Details";
    return isCampaign(g) ? "Mission Report" : "Carnage Report";
  };
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Games", "/stats/games"], [crumb()]]} />
      <Show
        when={game()}
        fallback={
          <div class="s-full s-game">
            <Title class="s-title--bar" back={["see all games", "/stats/games"]}>Game Details</Title>
            <Show when={missing()} fallback={<div class="s-loading">Loading game…</div>}>
              <p class="s-pad">This game isn't available on this server. <A href="/stats/games">Back to games</A></p>
            </Show>
          </div>
        }
      >
        {(g) => (
          <div class="s-full s-game">
            <GameView game={g()} />
            <Show when={isCampaign(g())} fallback={<Carnage game={g()} />}><CampaignReport game={g()} /></Show>
          </div>
        )}
      </Show>
    </>
  );
}

function Header(props: { game: GameState }) {
  const g = () => props.game;
  const campaign = () => isCampaign(g());
  const result = () => resultWords(g());
  const host = () => hostName(g());
  return (
    <>
      <Title class="s-title--bar" back={["see all games", "/stats/games"]}>Game Details</Title>
      <div class="s-overview">
        <MapThumb game={g()} top />
        <ul class="s-summary">
          <Show
            when={campaign()}
            fallback={<li class="first"><GametypeIcon variant={g().variant} size={28} /><span>{gameTitle(g())}<small>on {gameMapTitle(g())}</small></span></li>}
          >
            <li class="first"><span>{gameMapTitle(g())}<small>{gameTitle(g())}</small></span></li>
          </Show>
          <li class="s-match-status"><Show when={g().status === "live"} fallback={<span class="s-final">FINAL</span>}><span class="s-live-tag">LIVE</span></Show><span title={g().end_reason === "lost" ? LOST_NOTE : undefined}>{result()}</span></li>
          <Show
            when={campaign()}
            fallback={<li class="styled">{gameTypeLine(g())}{g().score_limit ? `, first to ${g().score_limit}` : ""}{g().time_limit_s ? `, ${Math.round(g().time_limit_s! / 60)} minutes` : ""}</li>}
          >
            <li class="styled">Difficulty - {difficultyName(g().difficulty) ?? "not reported"}</li>
          </Show>
          <CustomMapLine game={g()} />
          <Show when={g().skulls.length}><li class="s-game-skulls"><SkullBadges skulls={g().skulls} size={22} names /></li></Show>
          <li title={g().ended_at ? `Ended ${agoWords(g().ended_at, now())}` : undefined}>{usDateTime(g().started_at)}</li>
          <li title={lengthNote(g(), now())}>Length: <span class="s-match-length">{hms(gameSeconds(g(), now()))}</span></li>
        </ul>
        <section class="s-booty" aria-label="Game record">
          <header>Game Record</header>
          <dl class="s-booty-body">
            <div><dt>Players</dt><dd>{Object.keys(g().players).length}</dd></div>
            <Show
              when={campaign()}
              fallback={<div><dt>Total kills</dt><dd>{Object.values(g().scores).reduce((n, s) => n + s.kills, 0)}</dd></div>}
            >
              <div><dt>Deaths</dt><dd>{deathsOf(g(), Object.keys(g().players).map(Number))}</dd></div>
            </Show>
            <Show when={host()}><div><dt>Host</dt><dd title={g().console ?? undefined}>{host()}</dd></div></Show>
            <div><dt>Recording</dt><dd>{g().status === "live" ? "Live stream" : "Recorded game"}</dd></div>
          </dl>
          <footer><VerifySummary positions={() => g().positions} /></footer>
        </section>
      </div>
    </>
  );
}

/** For a game on a custom map: who made it, on which base map, and a way
 *  for a signed-in player to report it to the admins. */
function CustomMapLine(props: { game: GameState }) {
  const lookup = () => packageLookup(props.game.map_id);
  const pkg = () => { const l = lookup(); return l?.state === "ok" ? l.map : null; };
  const [said, setSaid] = createSignal("");
  async function report(mapId: string) {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk) return;
    const reason = typeof prompt === "function" ? prompt("Why should the admins look at this map?") : null;
    if (reason === null || !reason.trim()) return;
    try {
      await reportMap(live.server().api, wk.id, me, mapId, reason);
      setSaid("reported, thanks");
    } catch (e) {
      setSaid(`couldn't report: ${e instanceof Error ? e.message : String(e)}`);
    }
  }
  return (
    <Show when={props.game.map_id}>
      {(id) => (
        <li class="s-custom-map">
          <Show when={pkg()} fallback={lookup()?.state === "gone" ? "Custom map, unavailable" : isHalo1Map(props.game.map) ? "Halo 1 map" : `Custom map on ${mapTitle(props.game.map)}`}>
            {(m) => <>Custom map by {m().author.gamertag} on {mapTitle(m().base.map)}</>}
          </Show>
          <Show when={identity.me() && pkg() && !said()}>
            {" "}<button type="button" class="s-linkish" onClick={() => void report(id())}>Report</button>
          </Show>
          <Show when={said()}> <small>({said()})</small></Show>
        </li>
      )}
    </Show>
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
        <Block title="Tactical Map" right={`${gameMapTitle(props.game)}`} class="s-sec">
          <TacticalMap game={props.game} frame={liveFrameSource(props.game.game_id, roster)} roster={roster} highlight={hover} label={`Live map of ${gameMapTitle(props.game)}`} />
          <Legend game={props.game} />
          <div class="s-live-clock"><span>Match in progress</span><b>{hms(elapsedSeconds(props.game, now()))}</b></div>
        </Block>
        <aside class="s-viewer-side">
          <Show when={isCampaign(props.game)} fallback={<Block title="Scoreboard" class="s-sec"><Scoreboard game={props.game} onHover={setHover} /></Block>}>
            <Block title="Players" class="s-sec"><Fireteam game={props.game} onHover={setHover} /></Block>
          </Show>
          <Block title={isCampaign(props.game) ? "Deaths" : "Field Report"} right="newest first" class="s-sec"><KillFeed game={props.game} kills={props.game.kills} /></Block>
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
    if (replayKey(session, e)) return;
    if (e.key === "ArrowRight") { e.preventDefault(); session.seek(tick() + 5 * TICKS_PER_SECOND); }
    if (e.key === "ArrowLeft") { e.preventDefault(); session.seek(tick() - 5 * TICKS_PER_SECOND); }
  };

  return (
    <>
      <Header game={props.game} />
      <Title right={<span class="s-viewer-status">Recorded replay</span>}>Game Viewer</Title>
      <div class="s-viewer">
        <Block title="Tactical Map" right={gameMapTitle(props.game)} class="s-sec">
          <Show when={!error()} fallback={<p class="s-pad s-bad">Couldn't load the replay: {error()}</p>}>
            <Show when={timeline()} fallback={<div class="s-loading">Loading recorded frames…</div>}>
              {(tl) => (
                <Show when={tl().frames.length} fallback={<p class="s-empty">No recorded frames for this game. The scoreboard and field report are still here.</p>}>
                  <TacticalMap
                    game={props.game} frame={session.frame} roster={roster} highlight={hover} label={`Replay of ${gameMapTitle(props.game)}`}
                    onKeyDown={onKey} keysLabel="Replay map. Space plays or pauses; arrow keys step five seconds; comma and full stop step a frame; less-than and greater-than jump between kills; k watches the latest kill."
                    soughtAt={session.soughtAt}
                    camera={session.camera}
                    transport={() => <Transport session={session} killColour={killColour} />}
                  />
                  <Legend game={props.game} />
                  <div class="s-timeline-head"><span>Match Timeline</span><small>Marks show kills, deaths and cuts · click one to watch it · Space to play</small></div>
                  <Transport session={session} killColour={killColour} />
                </Show>
              )}
            </Show>
          </Show>
        </Block>
        <aside class="s-viewer-side">
          <Show
            when={isCampaign(props.game)}
            fallback={<Block title="Scoreboard" right={`at ${clock((tick() - (timeline()?.startTick ?? 0)) / TICKS_PER_SECOND)}`} class="s-sec"><Scoreboard game={snapshot()} onHover={setHover} /></Block>}
          >
            <Block title="Players" class="s-sec"><Fireteam game={props.game} onHover={setHover} /></Block>
          </Show>
          <Block title={isCampaign(props.game) ? "Deaths" : "Field Report"} right="newest first" class="s-sec"><KillFeed game={props.game} kills={kills()} /></Block>
        </aside>
      </div>
    </>
  );
}

/** A campaign game's players beside the map: names and deaths, no scores. */
function Fireteam(props: { game: GameState; onHover?: (i: number | null) => void }) {
  const rows = createMemo(() => campaignPlayers(props.game));
  return (
    <table class="s-stats s-fireteam" aria-label="Players">
      <colgroup><col /><col style={{ width: "64px" }} /></colgroup>
      <thead><tr><th>Player</th><th>Deaths</th></tr></thead>
      <tbody>
        <For each={rows()}>
          {(r) => (
            <tr classList={{ gone: r.left }} onMouseEnter={() => props.onHover?.(r.index)} onMouseLeave={() => props.onHover?.(null)}>
              <td><PlayerName r={r} /></td><td>{r.deaths}</td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  );
}

function zeroTeams(g: GameState): Record<number, number> {
  const t: Record<number, number> = {};
  for (const k of Object.keys(g.teams)) t[+k] = 0;
  return t;
}
