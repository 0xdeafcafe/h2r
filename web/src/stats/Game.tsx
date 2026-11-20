import { gameMapTitle, packageLookup } from "../state/maps";
import { isHalo1Map, mapTitle } from "../state/gameart";
import { identity } from "../state/identity";
import { reportMap } from "../admin/content";
import { For, Show, createMemo, createSignal, createEffect, createResource, on, onCleanup, onMount } from "solid-js";
import { A, useLocation, useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { isCampaign, type GameState } from "../state/reducer";
import { LOST_NOTE, deathsOf } from "../state/derive";
import { difficultyName, gameTypeLine } from "../state/gametype";
import CampaignReport, { PlayerName, campaignPlayers } from "./CampaignReport";
import Scoreboard from "../components/Scoreboard";
import KillFeed from "../components/KillFeed";
import type { FeedKill } from "../components/killFeedRows";
import { BotTag, VerifySummary } from "../components/bits";
import { Block, Crumbs, Title } from "./Layout";
import Carnage from "./Carnage";
import GametypeIcon from "../components/GametypeIcon";
import { SkullBadges } from "../components/SkullIcon";
import { MapThumb, gameTitle, hostName, resultWords } from "./Side";
import { hms, usDateTime, agoWords } from "./fmt";
import { teamStandings, standings } from "../state/reducer";
import { colourFor, elapsedSeconds, gameSeconds, rosterFor, teamName } from "../lib/game";
import { clock } from "../lib/format";
import { now } from "../lib/now";
import { TICKS_PER_SECOND } from "../state/frames";
import GameViewer from "../viewer/GameViewer";
import Transport, { replayKey } from "../viewer/Transport";
import { createReplaySession } from "../viewer/session";
import { emblemOrigin } from "../lib/emblem";
import { adoptGameHeader, gameHeaderModel, showGameHeader } from "./gameHeader";
import "../styles/stats-game.css";
import { GAME_VIEWER_ID, STATS_ROOT, gameHref, statsPath, theaterHref } from "../routes";
import { readClip, type Clip } from "../api/films";
import type { HostFilm } from "../api/host-film";
import { filmGame } from "../theater/filmGame";
import { createFilmControls } from "../theater/FilmControls";
import "../theater/theater.css";

export default function Game() {
  const params = useParams<{ id: string; clip?: string }>();
  const route = createMemo(() => ({ id: params.id, clip: params.clip, api: live.server().api, reader: identity.me()?.xuid }));
  return <Show when={route()} keyed>{(r) => <GameDetails id={r.id} clipId={r.clip} />}</Show>;
}

function GameDetails(props: { id: string; clipId?: string }) {
  const [hostFilm, setHostFilm] = createSignal<{ game: GameState; film: HostFilm }>();
  const recordedGame = () => live.state.games[props.id];
  const game = () => recordedGame() ?? hostFilm()?.game;
  const film = () => recordedGame() ? undefined : hostFilm()?.film;
  const [failure, setFailure] = createSignal("");
  createEffect(on(live.loadMode, (mode) => {
    if (mode === "snapshot") return;
    const request = new AbortController();
    onCleanup(() => request.abort());
    const api = live.api();
    void live.ensureGame(props.id).then(async (ok) => {
      if (ok || request.signal.aborted || recordedGame()) return;
      const found = await api.hostFilm(props.id, request.signal);
      if (request.signal.aborted || recordedGame()) return;
      if (found) setHostFilm({ game: filmGame(props.id, found.front), film: found });
      else setFailure("This game isn't available on this server.");
    }).catch((e: unknown) => {
      if (!request.signal.aborted) setFailure(e instanceof Error ? e.message : String(e));
    });
  }));

  const clipRead = new AbortController();
  onCleanup(() => clipRead.abort());
  const [clip] = createResource(
    () => live.loadMode() !== "snapshot" && props.clipId,
    async (id): Promise<{ clip: Clip } | { error: string }> => {
      try {
        const found = await readClip(live.server().api, live.api().readToken, id, clipRead.signal);
        return found.game === props.id ? { clip: found } : { error: "This clip belongs to a different film." };
      } catch (e) {
        return { error: e instanceof Error ? e.message : String(e) };
      }
    },
  );
  const openedClip = () => {
    const result = clip();
    return result && "clip" in result ? result.clip : undefined;
  };
  const clipError = () => {
    const result = clip();
    return result && "error" in result ? result.error : undefined;
  };
  const crumb = () => {
    const g = game();
    if (g?.status !== "ended") return "Game Details";
    return isCampaign(g) ? "Mission Report" : "Carnage Report";
  };
  const trail = (): Array<[string, string?]> => [
    ["Home", STATS_ROOT], ["Games", statsPath("/games")], [crumb(), props.clipId ? gameHref(props.id) : undefined],
    ...(props.clipId ? [["Clip"] as [string]] : []),
  ];

  let hero!: HTMLDivElement;
  onMount(() => {
    const source = document.querySelector<HTMLTemplateElement>("#game-header-source");
    if (source) adoptGameHeader(hero, source.content, props.id);
    createEffect(() => {
      const g = game();
      if (g) showGameHeader(hero, gameHeaderModel(g, emblemOrigin(live.server().base, location.origin)));
      else if (failure() || clipError()) showGameHeader(hero, null);
    });
  });

  return (
    <>
      <Crumbs trail={trail()} />
      <div class="s-full s-game">
        <div ref={hero} class="s-game-hero" />
        <Show when={!clipError()} fallback={<><GameTitle /><p class="s-pad s-bad" role="alert">Couldn't open the clip. {clipError()}</p></>}>
          <Show when={game() && (!props.clipId || openedClip()) ? game() : undefined} fallback={
            <>
              <GameTitle />
              <Show when={failure()} fallback={<div class="s-loading">{props.clipId ? "Loading clip…" : "Loading game…"}</div>}>
                <p class="s-pad">{failure()} <A href={statsPath("/games")}>Back to games</A></p>
              </Show>
            </>
          }>
            {(g) => <>
              <GameView game={g()} clip={openedClip()} hostFilm={film()} />
              <Show when={!film() && !props.clipId}>
                <Show when={isCampaign(g())} fallback={<Carnage game={g()} />}><CampaignReport game={g()} /></Show>
              </Show>
            </>}
          </Show>
        </Show>
      </div>
    </>
  );
}

function GameTitle(props: { game?: GameState; clip?: Clip; watch?: () => void }) {
  const at = useLocation();
  const href = () => theaterHref(props.game!.game_id).replace("#", `${at.search}#`);
  return (
    <Title class="s-title--bar" back={props.clip ? ["watch full film", theaterHref(props.clip.game)] : ["see all games", statsPath("/games")]}
      right={
        <Show when={!props.clip && props.game?.status === "ended"}>
          <A href={href()} onClick={(event: MouseEvent) => {
            if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
            props.watch?.();
            if (at.hash === `#${GAME_VIEWER_ID}`) document.getElementById(GAME_VIEWER_ID)?.scrollIntoView();
          }}>Watch in Theater</A>
        </Show>
      }>
      {props.clip?.title ?? "Game Details"}
    </Title>
  );
}

/** What the game was: its map, type, result, length and record. Under
 *  the viewer and the scoreboard, which come first. */
function Overview(props: { game: GameState }) {
  const g = () => props.game;
  const campaign = () => isCampaign(g());
  const result = () => resultWords(g());
  const host = () => hostName(g());
  return (
    <>
      <div class="s-overview">
        <MapThumb game={g()} top />
        <ul class="s-summary">
          <Show
            when={campaign()}
            fallback={<li class="first"><GametypeIcon variant={g().variant} size={28} /><span>{gameTitle(g())}<small>{" "}on {gameMapTitle(g())}</small></span></li>}
          >
            <li class="first"><span>{gameMapTitle(g())}<small>{" "}{gameTitle(g())}</small></span></li>
          </Show>
          <li class="s-match-status"><Show when={g().status === "live"} fallback={<span class="s-final">FINAL</span>}><span class="s-live-tag">LIVE</span></Show>{" "}<span title={g().end_reason === "lost" ? LOST_NOTE : undefined}>{result()}</span></li>
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
        fallback={<For each={standings(props.game)}>{(s) => <span><i style={{ background: colourFor(props.game, s.team, s.index) }} />{s.name}<Show when={s.bot}><BotTag /></Show></span>}</For>}
      >
        <For each={teamStandings(props.game)}>{(t) => <span><i style={{ background: colourFor(props.game, t.team, 0) }} />{teamName(t.team)} Team</span>}</For>
      </Show>
      <span><i class="wedge" />facing</span>
      <span><i class="x" />knockout</span>
    </div>
  );
}

/** The viewer's title: the game type on the map (the mission, in a campaign). */
function viewerTitle(g: GameState): string {
  return isCampaign(g) ? gameMapTitle(g) : `${gameTitle(g)} on ${gameMapTitle(g)}`;
}

/** Beside the viewer: the scoreboard (players, in a campaign) and the field report. */
function Side(props: { game: GameState; scores: GameState; kills: FeedKill[]; at?: string; onHover: (i: number | null) => void; onPick?: (i: number) => void; picked?: number | null; final?: HostFilm["front"]["summary"] }) {
  return (
    <div class="s-viewer s-viewer--below">
      <Show
        when={isCampaign(props.game)}
        fallback={
          <Block title={props.final ? "Final scores" : "Scoreboard"} right={props.at} class="s-sec">
            <Show when={props.final} fallback={<Scoreboard game={props.scores} onHover={props.onHover} onPick={props.onPick} picked={props.picked} />}>
              {(summary) => <Show when={summary().players.length} fallback={<p class="s-empty">No final scores recorded.</p>}>
                <table class="s-table"><thead><tr><th>Player</th><th>Score</th></tr></thead>
                  <tbody><For each={summary().players}>{(p) => <tr><td>{p.name}</td><td>{p.score}</td></tr>}</For></tbody>
                </table>
              </Show>}
            </Show>
          </Block>
        }
      >
        <Block title="Players" class="s-sec"><Fireteam game={props.game} onHover={props.onHover} /></Block>
      </Show>
      <Block title={isCampaign(props.game) ? "Deaths" : "Field Report"} right="newest first" class="s-sec"><KillFeed game={props.game} kills={props.kills} /></Block>
    </div>
  );
}

/** The shared live and recorded game viewer, also embedded in Uplink. */
export function GameView(props: { game: GameState; embedded?: boolean; focus?: () => number | null; clip?: Clip; hostFilm?: HostFilm }) {
  const [search] = useSearchParams<{ t?: string | string[]; view?: string; cam?: string }>();
  const at = useLocation();
  const navigate = useNavigate();
  const [hover, setHover] = createSignal<number | null>(null);
  const roster = createMemo(() => rosterFor(props.game));
  // One session drives the 2D map, the 3D view and the scrubber alike.
  const session = createReplaySession(() => props.game, roster, () => search.t, { range: props.clip, hostFilm: () => props.hostFilm });
  const { timeline, error, tick, isLive } = session;
  const openFilm = () => !props.embedded && (!!props.clip || at.hash === `#${GAME_VIEWER_ID}`);
  const showFilmControls = () => !props.embedded && !isLive();
  const useDirector = () => showFilmControls() && search.cam !== "top" && openFilm();
  const film = createFilmControls({
    game: () => props.game, clip: props.clip, roster, session, active: useDirector,
    mode: () => {
      if (search.view === "3d") return search.cam ?? "orbit";
      if (search.view === "2d") return "top-down";
      return props.clip ? undefined : "orbit";
    },
    setMode: (mode) => {
      const params = new URLSearchParams(at.search);
      params.set("view", mode === "top-down" ? "2d" : "3d");
      if (mode === "top-down") params.delete("cam");
      else params.set("cam", mode);
      navigate(`${at.pathname}?${params}${at.hash}`, { replace: true, scroll: false });
    },
  });
  const watch = () => {
    const tl = session.timeline();
    if (session.isLive() || !tl?.frames.length || session.playing()) return;
    session.toggle();
  };
  const killColour = (k: number | null) => (k !== null ? colourFor(props.game, props.game.players[k]?.team ?? 0, k) : undefined);

  const snapshot = createMemo((): GameState => {
    const tl = timeline();
    if (isLive() || !tl?.scores.length) return props.game;
    const s = tl.scoresAt(tick());
    return { ...props.game, scores: s.players, teams: { ...zeroTeams(props.game), ...s.teams } };
  });
  const kills = createMemo((): FeedKill[] => {
    const tl = timeline();
    if (isLive() || !tl?.frames.length) return props.game.kills;
    return tl.killsBetween(-Infinity, tick());
  });

  const ready = () => !!timeline()?.frames.length && (!props.clip || !!session.range?.());
  const onKey = (e: KeyboardEvent) => ready() && replayKey(session, e);

  const controls = () => (
    <>
      <Show when={error()}>{(message) => <p class="s-pad s-bad">Couldn't load the replay: {message()}</p>}</Show>
      <Show when={ready()} fallback={
        <Show when={!error()}>
          <Show when={isLive()} fallback={
            <Show when={timeline()} fallback={<div class="s-loading">Loading recorded frames…</div>}>
              <p class="s-empty">{props.clip ? "No recorded frames are available for this clip." : "No recorded frames for this game."}</p>
            </Show>
          }>
            <div class="s-live-clock"><span>Waiting for live frames…</span><b>{hms(elapsedSeconds(props.game, now()))}</b></div>
          </Show>
        </Show>
      }>
        <Show when={film.partialClip()}><p class="s-pad" role="status">Only part of this clip is in the recording.</p></Show>
        <div class="s-timeline-head">
          <span>{props.clip ? "Clip Timeline · Looping" : isLive() ? "Live Timeline" : "Match Timeline"}</span>
          <small>Marks show kills, deaths and cuts · click one to watch it · Space to play</small>
          <Show when={showFilmControls() && !props.clip}>{film.editButton()}</Show>
        </div>
        <Transport session={session} matchStartTick={props.game.start_tick} killColour={killColour} selection={film.selection()} />
        <Show when={showFilmControls()}>{film.editor()}</Show>
      </Show>
    </>
  );

  return (
    <>
      <Show when={!props.embedded}><GameTitle game={props.game} clip={props.clip} watch={watch} /></Show>
      <Show when={props.clip}>{(clip) => (
        <div class="theater-byline">
          <span>Clip by{" "}{film.author(clip())}</span>
          <span>{viewerTitle(props.game)}</span>
        </div>
      )}</Show>
      <GameViewer
        id={props.embedded ? undefined : GAME_VIEWER_ID}
        open={openFilm()}
        game={props.game}
        title={viewerTitle(props.game)}
        status={<span class="s-viewer-status"><Show when={isLive()} fallback={props.clip ? "Clip" : "Recorded replay"}><i />Live coverage</Show></span>}
        frame={session.frame}
        focus={props.focus}
        roster={roster}
        highlight={() => hover() ?? (useDirector() ? film.director.selection().follow : null)}
        director={useDirector() ? film.director : undefined}
        viewControls={film.cameras()}
        overlay={useDirector() ? film.overlay(() => snapshot().scores) : undefined}
        label={`${isLive() ? "Live map" : "Replay"} of ${gameMapTitle(props.game)}`}
        soughtAt={session.soughtAt}
        camera={session.camera}
        onKey={onKey}
        keysLabel="Game viewer. Space plays or pauses; arrow keys step five seconds; comma and full stop step a frame; less-than and greater-than jump between kills; k watches the latest kill."
        controls={controls()}
      >
        <Legend game={props.game} />
      </GameViewer>
      <Show when={!props.embedded}>
        <Side
          game={props.game}
          scores={snapshot()}
          kills={kills()}
          at={isLive() || !timeline()?.frames.length ? undefined : `at ${clock((tick() - timeline()!.startTick) / TICKS_PER_SECOND)}`}
          onHover={setHover}
          onPick={useDirector() ? film.pick : undefined}
          picked={useDirector() ? film.director.selection().follow : undefined}
          final={!timeline()?.scores.length ? props.hostFilm?.front.summary : undefined}
        />
        <Show when={!props.clip && !props.hostFilm}><Overview game={props.game} /></Show>
      </Show>
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
