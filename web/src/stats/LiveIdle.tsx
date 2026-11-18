// The stats home's "Live Now" panel when nothing is live: the game that
// finished last as a small looping top-down replay (the game page's
// recording, map and drawing, on a loop over its closing stretch), what it
// was and how it ended, and a few highlights from the last day. With no
// games on record at all, a montage of maps instead of an empty box.

import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { A } from "@solidjs/router";
import { live } from "../state/live";
import { endedGames, isCampaign, type GameState } from "../state/reducer";
import { gameMapTitle, packageLookup } from "../state/maps";
import { gameTypeLine } from "../state/gametype";
import { outlineFor } from "../state/outlines";
import { TICKS_PER_SECOND } from "../state/frames";
import { ReplayTimeline, fetchRecording } from "../state/replay";
import { LoopClock, highlights, loopSpan, montageMaps, pickLastGame, type Highlight } from "../lib/lastGame";
import { geometryChoice } from "../lib/maps";
import { rosterFor } from "../lib/game";
import { now } from "../lib/now";
import type { StatsSummary } from "../lib/statsHome";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import MapPicture from "../components/MapPicture";
import GametypeIcon from "../components/GametypeIcon";
import ViewMap from "../map/ViewMap";
import { geometryFor } from "../map/geometry";
import { onAnimationFrame } from "../map/scheduler";
import { topView } from "../map/views";
import { eventsFromState, replayFrame } from "../viewer/session";
import { ScoreLine } from "./Games";
import { MapThumb, Who, fireteam, gameTitle, resultWords } from "./Side";
import { agoWords, count } from "./fmt";

const EMPTY: MapFrame = { players: [], markers: [], seen: null };

/** What the panel shows in place of live games. */
export default function LiveIdle(props: { week: StatsSummary | null }) {
  const last = createMemo(() => pickLastGame(endedGames(live.state, 12)));
  return (
    <div class="s-idle">
      <Show when={last()} fallback={<Montage week={props.week} />}>
        {(g) => <LastGame game={g()} />}
      </Show>
      <p class="s-idle-next">No games in progress right now. The next one shows up here as it starts.</p>
    </div>
  );
}

function LastGame(props: { game: GameState }) {
  const g = () => props.game;
  const href = () => `/stats/games/${g().game_id}`;
  const hl = createMemo(() => highlights(endedGames(live.state, 200), now()));
  const report = () => (isCampaign(g()) ? "Mission report" : "Carnage report");
  return (
    <>
      <A href={href()} class="s-idle-replay" aria-label={`Replay of the last game, ${gameTitle(g())} on ${gameMapTitle(g())}`}>
        <LoopMap game={g()} />
        <span class="s-idle-tag">LAST GAME</span>
      </A>
      <div class="s-idle-info">
        <h3><GametypeIcon variant={isCampaign(g()) ? null : g().variant} />{isCampaign(g()) ? gameMapTitle(g()) : gameTitle(g())}</h3>
        <span class="s-idle-map">{isCampaign(g()) ? gameTitle(g()) : `on ${gameMapTitle(g())}`}</span>
        <span class="s-idle-result"><span class="s-final">FINAL</span>{resultWords(g())}</span>
        <span class="s-idle-score">
          <Show when={!isCampaign(g())} fallback={<>{fireteam(g())}</>}><ScoreLine game={g()} /></Show>
        </span>
        <small>{gameTypeLine(g())} · {count(Object.keys(g().players).length, "player")}</small>
        <small>ended {agoWords(g().ended_at ?? g().started_at, now())}</small>
        <A href={href()} class="s-idle-link">{report()} ›</A>
      </div>
      <Show when={hl().length}>
        <aside class="s-idle-hl" aria-label="Since yesterday">
          <h4>Since Yesterday</h4>
          <ul>
            <For each={hl().slice(0, 3)}>{(h) => <HighlightRow h={h} />}</For>
          </ul>
        </aside>
      </Show>
    </>
  );
}

function HighlightRow(props: { h: Highlight }) {
  return (
    <li>
      <span class="s-idle-hl-label">{props.h.label}</span>
      <span class="s-idle-hl-who">
        <Show when={props.h.who} fallback={
          <Show when={props.h.map} fallback={<b>{props.h.value}</b>}>
            {(m) => <span>{gameMapTitle(m())}</span>}
          </Show>
        }>
          {(w) => <Who xuid={w().xuid ?? ""} name={w().name} size={14} />}
        </Show>
      </span>
      <Show when={props.h.who || props.h.map}><span class="s-idle-hl-val">{props.h.value}</span></Show>
    </li>
  );
}

/** The game's recording on a loop: the map's top view when the server has
 *  one, else the outline grid. Plays only while on screen, in a visible tab
 *  and without a reduced-motion preference (then it holds the final moment).
 *  No recording: the game's thumbnail. */
function LoopMap(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const outline = outlineFor(() => props.game.map);
  const sameGeometry = () => geometryChoice(packageLookup(props.game.map_id)) === "base";
  const geometry = geometryFor(() => (sameGeometry() ? props.game.map : undefined));
  const [broken, setBroken] = createSignal(false);
  const top = () => (broken() ? null : topView(geometry()?.views));
  const [tl, setTl] = createSignal<ReplayTimeline | null>(null);
  const [failed, setFailed] = createSignal(false);
  const [progress, setProgress] = createSignal(1);
  const still = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  let clock: LoopClock | null = null;
  let onScreen = true;
  let wrap!: HTMLSpanElement;

  const sync = () => {
    if (!clock) return;
    const t = performance.now();
    const go = !still && onScreen && (typeof document === "undefined" || !document.hidden);
    if (go) clock.play(t); else clock.pause(t);
  };

  createEffect(on(() => props.game.game_id, (id) => {
    const ac = new AbortController();
    setTl(null);
    setFailed(false);
    clock = null;
    setBroken(false);
    fetchRecording(live.api(), id, ac.signal).then(({ frames, events }) => {
      if (!frames.length) { setFailed(true); return; }
      const t = new ReplayTimeline(frames, events?.length ? events : eventsFromState(props.game));
      const span = loopSpan(t.startTick, t.endTick, TICKS_PER_SECOND);
      if (!span) { setFailed(true); return; }
      clock = new LoopClock(span, TICKS_PER_SECOND);
      setTl(t);
      sync();
    }, (e: Error) => { if (e.name !== "AbortError") setFailed(true); });
    onCleanup(() => ac.abort());
  }));

  onMount(() => {
    const io = new IntersectionObserver((es) => { onScreen = es.some((e) => e.isIntersecting); sync(); });
    io.observe(wrap);
    document.addEventListener("visibilitychange", sync);
    let lastUi = 0;
    const stop = onAnimationFrame((t) => {
      if (!clock || t - lastUi < 200) return;
      lastUi = t;
      const { from, to } = clock.span;
      setProgress((clock.tickAt(t) - from) / Math.max(1, to - from));
    });
    onCleanup(() => { io.disconnect(); document.removeEventListener("visibilitychange", sync); stop(); });
  });

  const frame = (t: number): MapFrame => {
    const r = tl();
    if (!r || !clock) return EMPTY;
    return replayFrame(r, still ? clock.span.to : clock.tickAt(t), roster());
  };

  return (
    <span class="s-idle-map-wrap" ref={wrap}>
      <Show when={tl() && !failed()} fallback={<MapThumb game={props.game} large class="s-idle-thumb" />}>
        <Show
          when={top()}
          fallback={<MapCanvas mini frame={frame} roster={roster} outline={outline} label={`Replay of ${gameMapTitle(props.game)}`} />}
        >
          {(v) => (
            <span class="s-thumb s-thumb--top s-idle-thumb">
              <ViewMap mini frame={frame} roster={roster} views={geometry()!.views!} view={v().id} image={geometry()!.image} label={`Replay of ${gameMapTitle(props.game)}`} onError={() => setBroken(true)} />
            </span>
          )}
        </Show>
        <span class="s-idle-bar" aria-hidden="true"><i style={{ width: `${Math.round(progress() * 100)}%` }} /></span>
      </Show>
    </span>
  );
}

/** No games on record: the maps the next one might be on. */
function Montage(props: { week: StatsSummary | null }) {
  const maps = createMemo(() => montageMaps(props.week?.maps));
  return (
    <div class="s-idle-montage">
      <h3>Pick a map, start a game</h3>
      <ul>
        <For each={maps()}>
          {(m) => (
            <li>
              <MapPicture map={m.map} mapId={m.map_id} class="s-idle-montage-pic" fallback={<span class="s-idle-montage-none" />} />
              <span>{gameMapTitle(m)}</span>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}
