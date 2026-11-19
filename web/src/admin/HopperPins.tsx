// The playlist builder's custom maps: the server's published packages (the
// catalog's `custom` section) and which of them one hopper allows. Pins are
// stored on the server beside the hopper file, not in it (docs/game/maps.md §5:
// ranked playlists allow only retail or admin-pinned map_ids), so each tick
// saves at once.
//
// A hopper entry names a disc map by id; the file has no room for a custom
// map's MapId. So a pinned map is played when a game of this playlist lands
// on an entry for its base map and the host has the map installed for that
// base: the server then accepts the session (it is pinned) and players
// without it download it as they join. Each pinned map says whether the
// playlist has an entry on its base, with a button to add one
// (docs/live/live-matchmaking.md §6.10).

import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { ApiError } from "../api/http";
import { live } from "../state/live";
import { parseMapSummaries, type MapSummary } from "../lib/maps";
import { seedPackages } from "../state/maps";
import { getPins, putPins } from "./api";
import { baseMapId, customSection, togglePin, type Catalog, type CatalogCustomMap } from "./catalog";

export default function HopperPins(props: {
  hopper: number;
  catalog: Catalog | null;
  /** The maps entries can name (the catalog's, or the server's retail list). */
  maps: ReadonlyArray<{ id: number; name: string; key?: string }>;
  /** The map ids the playlist's entries name now (as edited). */
  entryMaps: readonly number[];
  /** Adds an entry on this map to the working copy. */
  onAddEntry: (mapId: number) => void;
}) {
  const [maps, setMaps] = createSignal<MapSummary[]>([]);
  const custom = createMemo((): CatalogCustomMap[] => customSection(props.catalog, maps()));
  const [pins, setPins] = createSignal<Map<number, string[]>>(new Map());
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal("");
  const [loaded, setLoaded] = createSignal(false);

  onMount(async () => {
    try {
      const [page, p] = await Promise.all([
        // An older server has no packages (404): nothing to pin.
        live.api().allMaps().catch((e) => (e instanceof ApiError && e.status === 404 ? { items: [] } : Promise.reject(e))),
        getPins(),
      ]);
      const maps = parseMapSummaries(page.items);
      seedPackages(maps);
      setMaps(maps);
      setPins(p);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setLoaded(true);
    }
  });

  const mine = () => pins().get(props.hopper) ?? [];
  // A pin whose package is no longer listed (hidden, deleted, blocked) still shows, by id.
  const unlisted = () => mine().filter((id) => !custom().some((c) => c.map_id === id));

  async function set(mapId: string, on: boolean) {
    if (busy()) return;
    const next = togglePin(mine(), mapId, on);
    setBusy(true);
    setErr("");
    try {
      await putPins(props.hopper, next);
      const all = new Map(pins());
      all.set(props.hopper, next);
      setPins(all);
    } catch (e) {
      setErr(`couldn't save pins: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  const base = (c: CatalogCustomMap) => baseMapId(props.maps, c.base);

  return (
    <details class="adm-hint adm-pins" open={mine().length > 0}>
      <summary>Custom maps allowed in this playlist ({mine().length})</summary>
      <p class="adm-preset-note">
        Entries name disc maps. Allowing a custom map here lets a game of this playlist be played on it instead of its base map, when the host has it installed: the server accepts the game, and players without the map download it as they join. The playlist file cannot name a custom map, so it needs an entry on the base map, and a host without the custom map plays the base.
      </p>
      <Show when={loaded()} fallback={<p class="adm-preset-note">Loading the server's custom maps…</p>}>
        <Show when={custom().length || unlisted().length} fallback={<p class="adm-preset-note">No custom maps are published on this server.</p>}>
          <ul class="adm-pin-list">
            <For each={custom()}>
              {(c) => (
                <li>
                  <label>
                    <input type="checkbox" checked={mine().includes(c.map_id)} disabled={busy()} onChange={(e) => void set(c.map_id, e.currentTarget.checked)} />
                    <b>{c.name}</b> <span>by {c.author} · on {c.base_title}</span>
                  </label>
                  <Show when={mine().includes(c.map_id)}>
                    <Show
                      when={base(c)}
                      fallback={<span class="adm-pin-warn"> · base map not in the map list</span>}
                    >
                      {(id) => (
                        <Show
                          when={props.entryMaps.includes(id())}
                          fallback={
                            <>
                              <span class="adm-pin-warn"> · no entry on {c.base_title} yet</span>{" "}
                              <button class="adm-link" onClick={() => props.onAddEntry(id())}>
                                Add an entry on {c.base_title}
                              </button>
                            </>
                          }
                        >
                          <span class="adm-pin-ok"> · plays on this playlist's {c.base_title} entries</span>
                        </Show>
                      )}
                    </Show>
                  </Show>
                </li>
              )}
            </For>
            <For each={unlisted()}>
              {(id) => (
                <li class="off">
                  <label>
                    <input type="checkbox" checked disabled={busy()} onChange={(e) => void set(id, e.currentTarget.checked)} />
                    <span class="adm-mono">{id.slice(0, 12)}…</span> <span>no longer published</span>
                  </label>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <p class="adm-preset-note">Ticks save at once, apart from Publish.</p>
      </Show>
      <Show when={err()}><p class="adm-error">{err()}</p></Show>
    </details>
  );
}
