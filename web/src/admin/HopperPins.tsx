// The playlist builder's custom maps: the server's published packages (the
// catalog's `custom` section) and which of them one hopper allows. Pins are
// stored on the server beside the hopper file, not in it (docs/maps.md §5:
// ranked playlists allow only retail or admin-pinned map_ids), so each tick
// saves at once.

import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { ApiError } from "../api/http";
import { live } from "../state/live";
import { parseMapSummaries, type MapSummary } from "../lib/maps";
import { seedPackages } from "../state/maps";
import { getPins, putPins } from "./api";
import { customSection, togglePin, type Catalog, type CatalogCustomMap } from "./catalog";

export default function HopperPins(props: { hopper: number; catalog: Catalog | null }) {
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

  return (
    <details class="adm-hint adm-pins" open={mine().length > 0}>
      <summary>Custom maps allowed ({mine().length})</summary>
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
        <p class="adm-preset-note">A pinned map may be played in this playlist; saved at once, apart from Publish.</p>
      </Show>
      <Show when={err()}><p class="adm-error">{err()}</p></Show>
    </details>
  );
}
