// The 3D view in a browser window of its own (`/stats/games/:id/3d`): the
// same viewer filling the window, with the replay's transport under it. A
// replay here keeps in step with the game page it was opened from (and the
// other way round) through the session's channel.

import { mapTitle } from "../state/gameart";
import { Show, Suspense, createEffect, createMemo, createSignal, lazy, on } from "solid-js";
import { A, useParams, useSearchParams } from "@solidjs/router";
import { colourFor, liveFrameSource, rosterFor } from "../lib/game";
import { geometryFor } from "../map/geometry";
import { live } from "../state/live";
import type { GameState } from "../state/reducer";
import { createReplaySession } from "./session";
import Transport from "./Transport";
import type { ViewMode } from "./scene";
import "../styles/stats.css";
import "./viewer.css";

const Viewer3D = lazy(() => import("./Viewer3D"));

export default function Popout() {
  const params = useParams<{ id: string }>();
  const game = () => live.state.games[params.id];
  const [missing, setMissing] = createSignal(false);
  createEffect(on(() => [params.id, live.loadMode()] as const, ([id, mode]) => {
    setMissing(false);
    if (mode === "snapshot") return;
    void live.ensureGame(id).then((ok) => setMissing(!ok && !live.state.games[id]));
  }));
  return (
    <div class="v3-page stats">
      <Show when={game()} fallback={<p class="v3-note v3-note--mid">{missing() ? "This game isn't available on this server." : "Loading game…"}</p>}>
        {(g) => <Show when={g().status === "live"} fallback={<Recorded game={g()} />}><Live game={g()} /></Show>}
      </Show>
    </div>
  );
}

function Frame(props: { game: GameState; live: boolean; children: (geo: { glb: () => string | null; mode?: ViewMode; ao?: boolean; cut?: number | null; seeThrough?: boolean }) => import("solid-js").JSX.Element }) {
  const geometry = geometryFor(() => props.game.map);
  const [params] = useSearchParams<{ cam?: string; ao?: string; cut?: string; see?: string }>();
  createEffect(() => { document.title = `3D view · ${mapTitle(props.game.map)} · h2 live`; });
  return (
    <>
      <header class="v3-bar v3-bar--page">
        <h3>3D view · {mapTitle(props.game.map)}</h3>
        <span class="v3-bar-state">{props.live ? "Live" : "Recorded replay"}</span>
        <span class="v3-bar-buttons"><A href={`/stats/games/${props.game.game_id}`} target="_blank" rel="noopener">Game details</A></span>
      </header>
      <div class="v3-body">
        <Suspense fallback={<div class="v3-note v3-note--mid">Loading the 3D view…</div>}>
          {props.children({
            glb: () => geometry()?.glb ?? null,
            mode: params.cam === "top" || params.cam === "follow" ? params.cam : undefined,
            ao: params.ao !== "0",
            // `?cut=<height>` opens with the height cut there; `?see=0` without the see-through.
            cut: params.cut !== undefined && Number.isFinite(Number(params.cut)) ? Number(params.cut) : null,
            seeThrough: params.see !== "0",
          })}
        </Suspense>
      </div>
    </>
  );
}

function Live(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  return (
    <Frame game={props.game} live>
      {(geo) => <Viewer3D frame={liveFrameSource(props.game.game_id, roster)} roster={roster} {...geo} />}
    </Frame>
  );
}

function Recorded(props: { game: GameState }) {
  const roster = createMemo(() => rosterFor(props.game));
  const session = createReplaySession(() => props.game, roster);
  const onKey = (e: KeyboardEvent) => {
    if ((e.target as HTMLElement).closest("input, select, button, a")) return;
    if (e.key === " ") { e.preventDefault(); session.toggle(); }
  };
  return (
    <div class="v3-page-inner" onKeyDown={onKey} tabindex="-1">
      <Frame game={props.game} live={false}>
        {(geo) => <Viewer3D frame={session.frame} roster={roster} soughtAt={session.soughtAt} {...geo} />}
      </Frame>
      <Show when={session.error()}><p class="v3-note">Couldn't load the replay: {session.error()}</p></Show>
      <Transport session={session} killColour={(k) => (k !== null ? colourFor(props.game, props.game.players[k]?.team ?? 0, k) : undefined)} />
    </div>
  );
}
