import { A } from "@solidjs/router";
import { For, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import type { Clip, ClipInput } from "../api/films";
import type { PlayerScore } from "../api/types";
import { clock } from "../lib/format";
import { GEOMETRY_CHANGED_NOTE } from "../lib/maps";
import type { RosterEntry } from "../map/render";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { geometryChanged } from "../state/maps";
import type { GameState } from "../state/reducer";
import { playerHref } from "../stats/Layout";
import type { DirectorMode, ViewerDirector } from "../viewer/camera";
import type { ReplaySession } from "../viewer/session";
import ClipEditor from "./ClipEditor";
import type { ClipMarks } from "./clipRange";

const CAMERAS = [
  ["eyes", "First person"],
  ["follow", "Follow"],
  ["orbit", "Orbit"],
  ["free", "Free"],
  ["top-down", "Top-down"],
] as const satisfies readonly [DirectorMode, string][];

const emptyMarks: ClipMarks = { from_tick: null, to_tick: null };

export function createFilmControls(props: {
  game: () => GameState;
  clip?: Clip;
  roster: () => Map<number, RosterEntry>;
  session: ReplaySession;
  active: () => boolean;
  mode: () => string | undefined;
  setMode: (mode: DirectorMode) => void;
}) {
  const { clip, session } = props;
  const [follow, setFollow] = createSignal<number | null>(clip?.pov ?? null);
  const [hud, setHud] = createSignal(true);
  const [editing, setEditing] = createSignal(false);
  const [marks, setMarks] = createSignal(emptyMarks);
  const has3d = () => !geometryChanged(props.game());
  const selection = createMemo(() => {
    const requested = props.mode() ?? clip?.camera;
    const mode = CAMERAS.find(([mode]) => mode === requested)?.[0] ?? (clip ? "orbit" : "top-down");
    const wanted = follow();
    const roster = props.roster();
    return {
      mode: has3d() ? mode : "top-down" as const,
      follow: wanted !== null && roster.has(wanted) ? wanted : roster.keys().next().value ?? null,
    };
  });
  const director: ViewerDirector = {
    selection,
    pick(mode, follow) {
      setFollow(follow);
      if (selection().mode !== mode) props.setMode(mode);
    },
  };
  const picked = () => director.selection().follow;
  const pick = (index: number) => {
    const { mode } = director.selection();
    director.pick(mode === "eyes" || mode === "top-down" ? mode : "follow", index);
  };
  createEffect(on(session.camera, (camera) => { if (camera && props.active()) director.pick(camera.mode, camera.follow); }));

  const recording = session.timeline;
  const bounds = () => ({ from_tick: recording()!.startTick, to_tick: recording()!.recordedEndTick });
  const partialClip = () => {
    const range = session.range?.();
    return range && clip && (session.clipTick(range.from_tick) !== clip.from_tick || session.clipTick(range.to_tick) !== clip.to_tick);
  };
  const clipView = (): Pick<ClipInput, "pov" | "camera"> => {
    const { mode, follow } = director.selection();
    return { camera: mode, ...(follow === null ? {} : { pov: follow }) };
  };
  const closeEditor = () => { setEditing(false); setMarks(emptyMarks); };

  return {
    director, pick, partialClip,
    selection: () => editing() ? marks() : undefined,
    author(clip: Clip) {
      const tag = () => live.state.accounts[clip.by.toLowerCase()]?.gamertag;
      return <Show when={tag()} fallback={clip.by}>{(name) => <A href={playerHref(name())}>{name()}</A>}</Show>;
    },
    editButton: () => (
      <Show when={identity.me()} fallback={<A href="/dash/signin">Sign in to make a clip</A>}>
        <button type="button" class="s-btn" aria-expanded={editing()} onClick={() => setEditing(true)}>Clip</button>
      </Show>
    ),
    editor: () => (
      <Show when={editing() && identity.me() && recording()}>
        {(tl) => <ClipEditor game={props.game().game_id} bounds={bounds()} tick={session.tick} clipTick={session.clipTick} hz={tl().ticksPerSecond} marks={marks()} setMarks={setMarks} view={clipView} close={closeEditor} />}
      </Show>
    ),
    cameras: () => (
      <span class="theater-cameras" role="group" aria-label="Camera">
        <For each={CAMERAS}>{([mode, label]) => (
          <button type="button" aria-pressed={director.selection().mode === mode} disabled={mode !== "top-down" && !has3d()}
            title={mode !== "top-down" && !has3d() ? GEOMETRY_CHANGED_NOTE : undefined}
            onClick={() => director.pick(mode, picked())}>{label}</button>
        )}</For>
        <button type="button" aria-pressed={hud()} onClick={() => setHud(!hud())}>HUD</button>
      </span>
    ),
    overlay(scores: () => Record<number, PlayerScore>) {
      const person = () => props.roster().get(picked() ?? -1);
      const playerScore = () => scores()[picked() ?? -1];
      return (
        <Show when={hud() && recording()?.frames.length && (!clip || session.range?.())}>
          <div class="theater-hud">
            <Show when={person()}>{(p) => (
              <div>
                <strong>{p().name}</strong>
                <Show when={playerScore()}>{(s) => <small>{s().kills} K · {s().deaths} D</small>}</Show>
              </div>
            )}</Show>
            <Show when={director.selection().mode === "eyes"}><span class="theater-reticle" aria-hidden="true" /></Show>
            <time>{clock((session.tick() - (session.range?.()?.from_tick ?? recording()!.startTick)) / recording()!.ticksPerSecond)}</time>
          </div>
        </Show>
      );
    },
  };
}
