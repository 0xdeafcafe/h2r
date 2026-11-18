// The game page's map: the pre-drawn picture of the level when the server
// has one (with a switcher for its views), else the grid fitted to the
// outline or to where players have been; and the button that opens the 3D
// view in a window. Live games and replays both use it, each with its own
// frame source, so whatever drives that source drives both views.

import { gameMapTitle, geometryChanged } from "../state/maps";
import { GEOMETRY_CHANGED_NOTE } from "../lib/maps";
import { For, Show, createEffect, createSignal, on, type JSX } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import { featuresFor, geometryFor } from "../map/geometry";
import type { RosterEntry } from "../map/render";
import ViewMap, { ViewPicker } from "../map/ViewMap";
import { pickView, viewParam } from "../map/views";
import { outlineFor } from "../state/outlines";
import type { GameState } from "../state/reducer";
import ViewerWindow from "./ViewerWindow";
import type { CameraRequest } from "./sync";

function LayerPicker(props: { count: number; value: number | null; onChange: (v: number | null) => void }) {
  return (
    <Show when={props.count > 1}>
      <div class="layers" role="group" aria-label="Map floor">
        <button aria-pressed={props.value === null} classList={{ on: props.value === null }} onClick={() => props.onChange(null)}>All floors</button>
        <For each={Array.from({ length: props.count }, (_, i) => i)}>
          {(i) => <button aria-pressed={props.value === i} classList={{ on: props.value === i }} onClick={() => props.onChange(i)}>Floor {i + 1}</button>}
        </For>
      </div>
    </Show>
  );
}

export default function TacticalMap(props: {
  game: GameState;
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  highlight: () => number | null;
  label: string;
  /** Replays: keys on the map (space, arrows) and what they do, for screen readers. */
  onKeyDown?: (e: KeyboardEvent) => void;
  keysLabel?: string;
  soughtAt?: () => number;
  /** Controls shown under the 3D view (the replay's transport). */
  transport?: () => JSX.Element;
  /** The replay's camera requests (a killcam): the 3D view opens for one. */
  camera?: () => CameraRequest | null;
}) {
  const outline = outlineFor(() => props.game.map);
  const geometry = geometryFor(() => props.game.map);
  // Teleporters and lifts, from the head of the map's glb; shown unless turned off.
  const features = featuresFor(() => geometry()?.glb);
  const [showFeatures, setShowFeatures] = createSignal(true);
  const drawn = () => (showFeatures() ? features() : null);
  const [layer, setLayer] = createSignal<number | null>(null);
  const [broken, setBroken] = createSignal(false);
  const [params, setParams] = useSearchParams<{ view?: string; map?: string }>();
  const open = () => params.view === "3d";
  const setOpen = (on: boolean) => setParams({ view: on ? "3d" : undefined }, { replace: true });
  createEffect(on(() => props.game.map, () => setBroken(false), { defer: true }));
  // A killcam wants the 3D view: open it (it takes the camera as it opens).
  createEffect(on(() => props.camera?.() ?? null, (c) => { if (c && !open()) setOpen(true); }, { defer: true }));

  const views = () => (broken() ? null : geometry()?.views ?? null);
  // The chosen view lives in the URL (`?map=iso-ne`), so it can be linked
  // to; changing it replaces the entry rather than adding history.
  const viewId = () => { const v = views(); return v ? pickView(v, params.map).id : ""; };
  const setView = (id: string) => { const v = views(); if (v) setParams({ map: viewParam(v, id) }, { replace: true }); };

  return (
    <>
      <div class="s-map-tools">
        <span>Player positions</span>
        <span class="s-map-tools-right">
          <Show when={views()} fallback={<LayerPicker count={outline()?.layers.length ?? 0} value={layer()} onChange={setLayer} />}>
            {(v) => <ViewPicker views={v()} value={viewId()} onChange={setView} />}
          </Show>
          <Show when={features()}>
            <button class="s-3d-button" aria-pressed={showFeatures()} classList={{ on: showFeatures() }} onClick={() => setShowFeatures(!showFeatures())} title="Teleporters with their links, and gravity lifts">Lifts &amp; teleporters</button>
          </Show>
          <Show when={!geometryChanged(props.game)} fallback={<span class="s-map-note-inline" title="The mod changed the level's geometry; the base map's would be wrong">{GEOMETRY_CHANGED_NOTE}</span>}>
            <button class="s-3d-button" aria-pressed={open()} classList={{ on: open() }} onClick={() => setOpen(!open())} title="Open the 3D view in a window">3D view</button>
          </Show>
        </span>
      </div>
      <div
        class="s-map"
        classList={{ "s-map--pic": !!views() }}
        tabindex={props.onKeyDown ? "0" : undefined}
        onKeyDown={props.onKeyDown}
        role={props.onKeyDown ? "group" : undefined}
        aria-label={props.keysLabel}
      >
        <Show
          when={views()}
          fallback={
            <>
              <MapCanvas frame={props.frame} roster={props.roster} outline={outline} layer={layer} highlight={props.highlight} label={props.label} features={drawn} />
              <Show when={!outline()}><div class="s-map-note">Map outline not available; the grid is fitted to where players have been.</div></Show>
            </>
          }
        >
          {(v) => (
            <ViewMap
              frame={props.frame} roster={props.roster} views={v()} view={viewId()} image={geometry()!.image}
              highlight={props.highlight} label={props.label} onError={() => setBroken(true)} features={drawn}
            />
          )}
        </Show>
      </div>
      <Show when={open() && !geometryChanged(props.game)}>
        <ViewerWindow
          title={`3D view · ${gameMapTitle(props.game)}`}
          popout={`/stats/games/${encodeURIComponent(props.game.game_id)}/3d`}
          onClose={() => setOpen(false)}
          frame={props.frame}
          roster={props.roster}
          glb={() => geometry()?.glb ?? null}
          highlight={props.highlight}
          soughtAt={props.soughtAt}
          camera={props.camera}
        >
          {props.transport?.()}
        </ViewerWindow>
      </Show>
    </>
  );
}
