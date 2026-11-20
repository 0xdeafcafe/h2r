// The game viewer's 2D map: the pre-drawn picture of the level when the
// server has one (with a switcher for its views), else the grid fitted to
// the outline or to where players have been. It fills the box it is in
// (the viewer's stage, full screen too) and fits the map to it as that
// changes. Live games and replays both use it, each with its own frame
// source, so whatever drives that source drives the 2D and 3D views alike.

import { For, Show, createEffect, createSignal, on } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import MapCanvas, { type MapFrame } from "../components/MapCanvas";
import { featuresFor, geometryFor } from "../map/geometry";
import type { RosterEntry } from "../map/render";
import ViewMap, { ViewPicker } from "../map/ViewMap";
import { pickView, viewParam } from "../map/views";
import type { DamageMode } from "../map/vitals";
import { outlineFor } from "../state/outlines";
import type { GameState } from "../state/reducer";

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
  follow?: () => number | null;
  soughtAt?: () => number;
  damage: DamageMode;
  onDamage: (mode: DamageMode) => void;
  label: string;
}) {
  const outline = outlineFor(() => props.game.map);
  const geometry = geometryFor(() => props.game.map, () => props.game.map_id);
  // Teleporters and lifts, from the head of the map's glb; shown unless turned off.
  const features = featuresFor(() => geometry()?.glb);
  const [showFeatures, setShowFeatures] = createSignal(true);
  const drawn = () => (showFeatures() ? features() : null);
  const [layer, setLayer] = createSignal<number | null>(null);
  const [broken, setBroken] = createSignal(false);
  const [params, setParams] = useSearchParams<{ map?: string }>();
  createEffect(on(() => props.game.map, () => setBroken(false), { defer: true }));

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
          <label>
            Damage{" "}
            <select class="s-3d-button" value={props.damage} onChange={(e) => props.onDamage(e.currentTarget.value as DamageMode)}>
              <option value="off">Off</option>
              <option value="bars">Bars</option>
              <option value="numbers">Bars and numbers</option>
            </select>
          </label>
          <Show when={views()} fallback={<LayerPicker count={outline()?.layers.length ?? 0} value={layer()} onChange={setLayer} />}>
            {(v) => <ViewPicker views={v()} value={viewId()} onChange={setView} />}
          </Show>
          <Show when={features()}>
            <button class="s-3d-button" aria-pressed={showFeatures()} classList={{ on: showFeatures() }} onClick={() => setShowFeatures(!showFeatures())} title="Teleporters with their links, and gravity lifts">Lifts &amp; teleporters</button>
          </Show>
        </span>
      </div>
      <div class="s-map" classList={{ "s-map--pic": !!views() }}>
        <Show
          when={views()}
          fallback={
            <>
              <MapCanvas frame={props.frame} roster={props.roster} outline={outline} layer={layer} highlight={props.highlight} label={props.label}
                features={drawn} damage={props.damage} follow={props.follow} soughtAt={props.soughtAt} />
              <Show when={!outline()}><div class="s-map-note">Map outline not available; the grid is fitted to where players have been.</div></Show>
            </>
          }
        >
          {(v) => (
            <ViewMap
              frame={props.frame} roster={props.roster} views={v()} view={viewId()} image={geometry()!.image}
              highlight={props.highlight} label={props.label} onError={() => setBroken(true)} features={drawn}
              damage={props.damage} follow={props.follow} soughtAt={props.soughtAt}
            />
          )}
        </Show>
      </div>
    </>
  );
}
