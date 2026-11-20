import { A, useParams, useSearchParams } from "@solidjs/router";
import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup } from "solid-js";
import { readClip, type Clip, type ClipInput } from "../api/films";
import Scoreboard from "../components/Scoreboard";
import { GEOMETRY_CHANGED_NOTE } from "../lib/maps";
import { colourFor, rosterFor } from "../lib/game";
import { clock } from "../lib/format";
import { STATS_ROOT, statsPath, theaterHref } from "../routes";
import { TICKS_PER_SECOND } from "../state/frames";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { gameMapTitle, geometryChanged } from "../state/maps";
import { isCampaign, type GameState } from "../state/reducer";
import { Block, Crumbs, Title, playerHref } from "../stats/Layout";
import { gameTitle } from "../stats/Side";
import type { CameraSelection, DirectorMode, ViewerDirector } from "../viewer/camera";
import GameViewer from "../viewer/GameViewer";
import Transport, { replayKey } from "../viewer/Transport";
import { createReplaySession } from "../viewer/session";
import ClipEditor from "./ClipEditor";
import type { ClipMarks } from "./clipRange";
import "../styles/stats-game.css";
import "./theater.css";

const CAMERAS = [
  ["eyes", "First person"],
  ["follow", "Follow"],
  ["orbit", "Orbit"],
  ["free", "Free"],
  ["top-down", "Top-down"],
] as const satisfies readonly [DirectorMode, string][];

const emptyMarks: ClipMarks = { from_tick: null, to_tick: null };

function cameraOf(clip: Clip | undefined): DirectorMode {
  return CAMERAS.find(([mode]) => mode === clip?.camera)?.[0] ?? "orbit";
}

export default function Theater() {
  const params = useParams<{ game: string; clip?: string }>();
  const route = createMemo(() => ({
    game: params.game,
    clip: params.clip,
    api: live.server().api,
    reader: identity.me()?.xuid,
  }));
  return <Show when={route()} keyed>{(r) => <TheaterRoute game={r.game} clip={r.clip} />}</Show>;
}

function TheaterRoute(props: { game: string; clip?: string }) {
  const game = () => live.state.games[props.game];
  const [missing, setMissing] = createSignal(false);
  createEffect(on(live.loadMode, (mode) => {
    if (mode === "snapshot") return;
    let current = true;
    onCleanup(() => { current = false; });
    void live.ensureGame(props.game).then((ok) => { if (current) setMissing(!ok); });
  }));

  const clipRead = new AbortController();
  onCleanup(() => clipRead.abort());
  const [clip] = createResource(
    () => live.loadMode() !== "snapshot" && props.clip,
    async (id): Promise<{ clip: Clip } | { error: string }> => {
      try {
        const found = await readClip(live.server().api, live.api().readToken, id, clipRead.signal);
        return found.game === props.game ? { clip: found } : { error: "This clip belongs to a different film." };
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
  const gameLink = () => statsPath(`/games/${encodeURIComponent(props.game)}`);

  return (
    <>
      <Crumbs trail={[["Home", STATS_ROOT], ["Games", statsPath("/games")], ["Theater", props.clip ? theaterHref(props.game) : undefined], ...(props.clip ? [["Clip"] as [string]] : [])]} />
      <div class="s-full s-game theater">
        <Title class="s-title--bar" back={props.clip ? ["watch full film", theaterHref(props.game)] : ["game details", gameLink()]}>
          {openedClip()?.title ?? "Theater"}
        </Title>
        <Show when={clipError()}>{(message) => <p class="s-pad s-bad" role="alert">Couldn't open the clip. {message()}</p>}</Show>
        <Show when={!clipError()}>
          <Show when={!missing()} fallback={<p class="s-pad">This film isn't available on this server.</p>}>
            <Show when={game() && (!props.clip || openedClip()) ? game() : undefined} fallback={<div class="s-loading">{props.clip ? "Loading clip…" : "Loading film…"}</div>}>
              {(g) => (
                <Show when={g().status === "ended"} fallback={<p class="s-pad">Theater is available after the match. <A href={gameLink()}>Watch live on the game page</A></p>}>
                  <Film game={g()} clip={openedClip()} />
                </Show>
              )}
            </Show>
          </Show>
        </Show>
      </div>
    </>
  );
}

function Film(props: { game: GameState; clip?: Clip }) {
  const [search] = useSearchParams<{ t?: string | string[] }>();
  const [view, setView] = createSignal<CameraSelection>({ mode: cameraOf(props.clip), follow: props.clip?.pov ?? null });
  const [hud, setHud] = createSignal(true);
  const [editing, setEditing] = createSignal(false);
  const [marks, setMarks] = createSignal(emptyMarks);
  const roster = createMemo(() => rosterFor(props.game));
  const has3d = () => !geometryChanged(props.game);
  const selection = createMemo((): CameraSelection => {
    const { mode, follow } = view();
    const selected = follow !== null && roster().has(follow) ? follow : roster().keys().next().value;
    return { mode: has3d() ? mode : "top-down", follow: selected ?? null };
  });
  const director: ViewerDirector = { selection, pick: (mode, follow) => setView({ mode, follow }) };
  const session = createReplaySession(() => props.game, roster, () => search.t, { range: props.clip, sync: false });
  const recording = () => session.timeline();
  const bounds = () => ({ from_tick: recording()!.startTick, to_tick: recording()!.recordedEndTick });
  const ready = () => !!recording()?.frames.length && (!props.clip || !!session.range?.());
  const partialClip = () => {
    const range = session.range?.();
    const clip = props.clip;
    return range && clip && (range.from_tick !== clip.from_tick || range.to_tick !== clip.to_tick);
  };
  const title = () => isCampaign(props.game) ? gameMapTitle(props.game) : `${gameTitle(props.game)} on ${gameMapTitle(props.game)}`;
  const author = () => props.clip ? live.state.accounts[props.clip.by.toLowerCase()]?.gamertag : undefined;
  const clipView = (): Pick<ClipInput, "pov" | "camera"> => {
    const { mode, follow } = director.selection();
    return { camera: mode, ...(follow === null ? {} : { pov: follow }) };
  };
  const onPick = (index: number) => {
    const { mode } = director.selection();
    director.pick(mode === "eyes" || mode === "top-down" ? mode : "follow", index);
  };
  createEffect(on(session.camera, (camera) => { if (camera) director.pick(camera.mode, camera.follow); }));

  const score = createMemo(() => {
    const tl = recording();
    return tl?.scores.length ? tl.scoresAt(session.tick()) : null;
  });
  const snapshot = createMemo((): GameState => {
    const game = props.game;
    const s = score();
    if (!s) return game;
    const teams = Object.fromEntries(Object.keys(game.teams).map((team) => [team, 0]));
    return { ...game, scores: s.players, teams: { ...teams, ...s.teams } };
  });
  const picked = () => director.selection().follow;
  const closeEditor = () => { setEditing(false); setMarks(emptyMarks); };
  const onKey = (e: KeyboardEvent) => ready() && replayKey(session, e);
  const controls = () => (
    <>
      <Show when={session.error()}>{(message) => <p class="s-pad s-bad" role="alert">Couldn't load the recording. {message()}</p>}</Show>
      <Show when={ready()} fallback={
        <Show when={!session.error()}>
          <Show when={recording()} fallback={<div class="s-loading">Loading recorded frames…</div>}>
            <p class="s-empty">{props.clip ? "No recorded frames are available for this clip." : "No recorded frames for this game."}</p>
          </Show>
        </Show>
      }>
        <Show when={partialClip()}><p class="s-pad" role="status">Only part of this clip is in the recording.</p></Show>
        <div class="s-timeline-head">
          <span>{props.clip ? "Clip Timeline · Looping" : "Film Timeline"}</span>
          <Show when={!props.clip}>
            <Show when={identity.me()} fallback={<A href="/dash/signin">Sign in to make a clip</A>}>
              <button type="button" class="s-btn" aria-expanded={editing()} onClick={() => setEditing(true)}>Clip</button>
            </Show>
          </Show>
        </div>
        <Transport session={session} matchStartTick={props.game.start_tick} selection={editing() ? marks() : undefined}
          killColour={(index) => index === null ? undefined : colourFor(props.game, props.game.players[index]?.team ?? 0, index)} />
        <Show when={editing() && identity.me()}>
          <ClipEditor game={props.game.game_id} bounds={bounds()} tick={session.tick} marks={marks()} setMarks={setMarks} view={clipView} close={closeEditor} />
        </Show>
      </Show>
    </>
  );
  const cameras = () => (
    <span class="theater-cameras" role="group" aria-label="Camera">
      <For each={CAMERAS}>{([mode, label]) => (
        <button type="button" aria-pressed={director.selection().mode === mode} disabled={mode !== "top-down" && !has3d()}
          title={mode !== "top-down" && !has3d() ? GEOMETRY_CHANGED_NOTE : undefined}
          onClick={() => director.pick(mode, picked())}>{label}</button>
      )}</For>
      <button type="button" aria-pressed={hud()} onClick={() => setHud(!hud())}>HUD</button>
    </span>
  );
  const overlay = () => {
    const person = () => roster().get(picked() ?? -1);
    const playerScore = () => snapshot().scores[picked() ?? -1];
    return (
      <Show when={hud() && ready()}>
        <div class="theater-hud">
          <Show when={person()}>{(p) => (
            <div>
              <strong>{p().name}</strong>
              <Show when={playerScore()}>{(s) => <small>{s().kills} K · {s().deaths} D</small>}</Show>
            </div>
          )}</Show>
          <Show when={director.selection().mode === "eyes"}><span class="theater-reticle" aria-hidden="true" /></Show>
          <time>{clock((session.tick() - (session.range?.()?.from_tick ?? recording()!.startTick)) / TICKS_PER_SECOND)}</time>
        </div>
      </Show>
    );
  };

  return (
    <>
      <Show when={props.clip}>{(clip) => (
        <div class="theater-byline">
          <span>Clip by <Show when={author()} fallback={clip().by}>{(tag) => <A href={playerHref(tag())}>{tag()}</A>}</Show></span>
          <span>{title()}</span>
        </div>
      )}</Show>
      <div class="theater-grid">
        <GameViewer game={props.game} title={title()} status={props.clip ? "Clip" : "Recorded film"} frame={session.frame}
          roster={roster} highlight={picked} label={`Replay of ${gameMapTitle(props.game)}`}
          director={director} viewControls={cameras()} overlay={overlay()} soughtAt={session.soughtAt} onKey={onKey} controls={controls()}
          keysLabel="Theater viewer. Space plays or pauses. Arrows seek five seconds. Comma and period step frames. K watches a kill." />
        <Block title="Scoreboard" class="theater-scoreboard">
          <Show when={roster().size} fallback={<p class="s-empty">No players recorded.</p>}>
            <Scoreboard game={snapshot()} picked={picked()} onPick={onPick} />
          </Show>
        </Block>
      </div>
    </>
  );
}
