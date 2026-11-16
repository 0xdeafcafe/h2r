// The playlist builder: the 16 hoppers of multiplayer_hopper.dat. Edits the
// fields the format documents (title, description, the range fields, each
// entry's map and weight; the raw words behind an "Advanced" disclosure)
// and reorders, copies, adds and removes entries — each kept entry
// remembering which published slot its unknown bytes (its game variant)
// come from, so nothing the builder cannot edit is ever lost. A preset
// fills a playlist from a historical lineup, resolved against the server's
// catalog of the owner's maps and variants (./catalog.ts, ./presets.ts).
// 404 until a base image is imported.

import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { Panel } from "../stats/Layout";
import { live } from "../state/live";
import { getHopper, putHopper } from "./api";
import { ENGINES, loadCatalog, mapById, type Catalog } from "./catalog";
import { PRESETS, applyPreset, resolvePreset } from "./presets";
import HopperPins from "./HopperPins";
import {
  MAX_DESCRIPTION,
  MAX_NAME,
  MAX_RECORDS,
  MAX_STRING,
  MAX_TITLE,
  MAX_WEIGHT,
  buildPatch,
  hopperErrors,
  toEditable,
  type EditHopper,
  type EditRecord,
  type HopperView,
} from "./validate";

const TYPES: Array<[number, string]> = [
  [1, "1 · hidden"],
  [2, "2 · playlist"],
  [3, "3 · hidden"],
  [4, "4 · playlist"],
  [5, "5 · playlist"],
];

const STATUSES: Array<[number, string]> = [
  [0, "0 · not loaded"],
  [1, "1 · ok"],
  [2, "2 · error"],
];

/** What the validation helpers want, from a working copy. */
function view(h: EditHopper): HopperView {
  return { ...h, record_count: h.records.length };
}

export default function HopperBuilder() {
  const [original, setOriginal] = createSignal<EditHopper[]>([]);
  const [hoppers, setHoppers] = createSignal<EditHopper[]>([]);
  const [sel, setSel] = createSignal(0);
  const [error, setError] = createSignal("");
  const [status, setStatus] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [catalog, setCatalog] = createSignal<Catalog | null>(null);
  const [presetKey, setPresetKey] = createSignal("");

  onMount(() => {
    void load();
    void loadCatalog(live.server().base).then(setCatalog);
  });

  const preset = () => PRESETS.find((p) => p.key === presetKey()) ?? null;
  const resolution = createMemo(() => {
    const p = preset();
    return p ? resolvePreset(p, catalog(), original()) : null;
  });

  function usePreset(): void {
    const h = current();
    const r = resolution();
    if (!h || !r || r.records.length === 0) return;
    const next = [...hoppers()];
    next[sel()] = applyPreset(h, r);
    setHoppers(next);
    setPresetKey("");
  }

  const variantLabel = (r: EditRecord): string => {
    const v = r.variant;
    if (!v) return "—";
    const type = ENGINES[v.engine]?.name ?? `engine ${v.engine}`;
    return `${v.name || "(unnamed)"} · ${type}${v.team ? " · teams" : ""}`;
  };

  async function load(): Promise<void> {
    setBusy(true);
    setError("");
    try {
      const reply = await getHopper();
      if (!reply) {
        setError("no multiplayer_hopper.dat published for this server yet");
        setStatus("import the game's own file once: h2 live hopper import <path>, on the server's machine");
        setHoppers([]);
        setOriginal([]);
        return;
      }
      setOriginal(toEditable(reply.hoppers));
      setHoppers(toEditable(reply.hoppers));
      setStatus("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const current = (): EditHopper | null => hoppers()[sel()] ?? null;
  const errors = (): string[] =>
    hoppers().flatMap((h) => hopperErrors(view(h)).map((e) => `Playlist ${h.index + 1}: ${e}`));
  const changed = (): boolean =>
    hoppers().length > 0 && buildPatch(original(), hoppers()).hoppers.length > 0;

  function edit(patch: Partial<EditHopper>): void {
    const h = current();
    if (!h) return;
    const next = [...hoppers()];
    next[sel()] = { ...h, ...patch };
    setHoppers(next);
  }

  function editRecord(i: number, patch: Partial<EditRecord>): void {
    const h = current();
    if (!h) return;
    const records = [...h.records];
    const rec = records[i];
    if (!rec) return;
    records[i] = { ...rec, ...patch };
    edit({ records });
  }

  function moveRecord(i: number, by: -1 | 1): void {
    const h = current();
    if (!h) return;
    const j = i + by;
    if (j < 0 || j >= h.records.length) return;
    const records = [...h.records];
    const a = records[i];
    const b = records[j];
    if (!a || !b) return;
    records[i] = b;
    records[j] = a;
    edit({ records });
  }

  function copyRecord(i: number): void {
    const h = current();
    if (!h || h.records.length >= MAX_RECORDS) return;
    const from = h.records[i];
    if (!from) return;
    const records = [...h.records];
    records.splice(i + 1, 0, { ...from, src: from.src ?? i });
    edit({ records });
  }

  function removeRecord(i: number): void {
    const h = current();
    if (!h || h.records.length <= 1) return;
    edit({ records: h.records.filter((_, j) => j !== i) });
  }

  function addRecord(): void {
    const h = current();
    if (!h || h.records.length >= MAX_RECORDS) return;
    // A new entry copies the last one (its template blob included).
    const last = h.records[h.records.length - 1];
    if (!last) return;
    edit({ records: [...h.records, { ...last, src: last.src ?? h.records.length - 1 }] });
  }

  async function save(): Promise<void> {
    if (!changed() || errors().length > 0) return;
    setBusy(true);
    setError("");
    setStatus("");
    try {
      const reply = await putHopper(buildPatch(original(), hoppers()));
      setOriginal(toEditable(reply.hoppers));
      setHoppers(toEditable(reply.hoppers));
      setStatus(`published ${reply.published.size} bytes (sha256 ${reply.published.sha256.slice(0, 12)}…)`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const num = (v: string): number | null => {
    const n = Number(v);
    return Number.isInteger(n) && n >= 0 ? n : null;
  };

  return (
    <div class="adm-hopper">
      <Panel
        title="Playlists"
        sub="multiplayer_hopper.dat · the game downloads this after sign-in"
        right={
          <>
            <Show when={changed()}>
              <span class="adm-dirty">unsaved changes</span>
            </Show>
            <button class="s-btn" disabled={busy() || !changed() || errors().length > 0} onClick={() => void save()}>
              {busy() ? "…" : "Publish"}
            </button>
          </>
        }
      >
        <Show when={hoppers().length > 0} fallback={
          <Show when={error()}>
            <p class="adm-error">{error()}</p>
            <p class="adm-ok">{status()}</p>
          </Show>
        }>
          <div class="adm-hopper-cols">
            <ol class="adm-list">
              <For each={hoppers()}>
                {(h) => (
                  <li>
                    <button
                      classList={{ on: h.index === sel() }}
                      onClick={() => setSel(h.index)}
                    >
                      <span class="adm-list-idx">{h.index + 1}</span>
                      <span class="adm-list-name">{h.title || h.name || "(untitled)"}</span>
                      <span class="adm-list-meta">
                        t{h.type} · {h.records.length}
                      </span>
                    </button>
                  </li>
                )}
              </For>
            </ol>
            <Show when={current()}>
              {(h) => (
                <div class="adm-editor">
                  <div class="adm-preset">
                    <label class="adm-field adm-grow">
                      <span>Start from preset</span>
                      <select value={presetKey()} onChange={(e) => setPresetKey(e.currentTarget.value)}>
                        <option value="">Choose a historical playlist…</option>
                        <For each={PRESETS}>
                          {(p) => (
                            <option value={p.key} selected={p.key === presetKey()}>
                              {p.label} · {p.era}
                            </option>
                          )}
                        </For>
                      </select>
                    </label>
                    <Show when={resolution()}>
                      {(r) => (
                        <div class="adm-preset-preview">
                          <p class="adm-preset-title">
                            {r().title} <span>{r().description}</span>
                          </p>
                          <Show when={preset()?.uncertain}>
                            <p class="adm-preset-note">From memory: {preset()?.uncertain}.</p>
                          </Show>
                          <ul class="adm-preset-games">
                            <For each={r().games}>
                              {(g) => (
                                <li classList={{ off: !g.record }}>
                                  {g.label}
                                  <Show when={g.missing}>
                                    <span> · unavailable: {g.missing}</span>
                                  </Show>
                                </li>
                              )}
                            </For>
                          </ul>
                          <button class="s-btn" disabled={r().records.length === 0} onClick={usePreset}>
                            Fill playlist {h().index + 1} with {r().records.length} entries
                          </button>
                        </div>
                      )}
                    </Show>
                    <Show when={!catalog()}>
                      <p class="adm-preset-note">
                        No catalog from this server (run h2 live assets extract on its machine): maps show as numbers and presets cannot resolve.
                      </p>
                    </Show>
                  </div>
                  <div class="adm-row">
                    <label class="adm-field">
                      <span>Title</span>
                      <input
                        value={h().title}
                        maxLength={MAX_TITLE}
                        onInput={(e) => edit({ title: e.currentTarget.value })}
                      />
                    </label>
                    <label class="adm-field adm-grow">
                      <span>Description</span>
                      <input
                        value={h().description}
                        maxLength={MAX_DESCRIPTION}
                        onInput={(e) => edit({ description: e.currentTarget.value })}
                      />
                    </label>
                  </div>
                  <table class="adm-records">
                    <thead>
                      <tr>
                        <th />
                        <th class="adm-grow">Map</th>
                        <th>Game variant</th>
                        <th>Weight</th>
                        <th />
                      </tr>
                    </thead>
                    <tbody>
                      <For each={h().records}>
                        {(r, i) => (
                          <tr>
                            <td class="adm-rec-idx">{i() + 1}</td>
                            <td>
                              <Show
                                when={catalog()}
                                fallback={<span class="adm-mono">map id {r.id}</span>}
                              >
                                {(c) => (
                                  <select
                                    value={String(r.id)}
                                    onChange={(e) => editRecord(i(), { id: Number(e.currentTarget.value) })}
                                  >
                                    <Show when={!mapById(c(), r.id)}>
                                      <option value={r.id} selected>
                                        id {r.id} (not a map in this game)
                                      </option>
                                    </Show>
                                    <For each={c().maps}>
                                      {(m) => (
                                        <option value={m.id} selected={m.id === r.id}>
                                          {m.name}
                                        </option>
                                      )}
                                    </For>
                                  </select>
                                )}
                              </Show>
                            </td>
                            <td class="adm-variant">{variantLabel(r)}</td>
                            <td>
                              <input
                                class="adm-num"
                                type="number"
                                min={1}
                                max={MAX_WEIGHT}
                                value={r.dwords[0]}
                                onInput={(e) => {
                                  const n = num(e.currentTarget.value);
                                  if (n !== null) editRecord(i(), { dwords: [n, r.dwords[1]] });
                                }}
                              />
                            </td>
                            <td class="adm-rec-btns">
                              <button aria-label="Move up" disabled={i() === 0} onClick={() => moveRecord(i(), -1)}>
                                ↑
                              </button>
                              <button
                                aria-label="Move down"
                                disabled={i() === h().records.length - 1}
                                onClick={() => moveRecord(i(), 1)}
                              >
                                ↓
                              </button>
                              <button aria-label="Duplicate" onClick={() => copyRecord(i())}>
                                ⧉
                              </button>
                              <button
                                aria-label="Remove"
                                disabled={h().records.length <= 1}
                                onClick={() => removeRecord(i())}
                              >
                                ×
                              </button>
                            </td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                  <details class="adm-hint adm-advanced">
                    <summary>Advanced: raw fields</summary>
                    <p class="adm-preset-note">
                      The entry string, flags and second dword lie inside the entry's game variant, and the name field overlaps a table the game validates: change them only if you know the bytes.
                    </p>
                  <div class="adm-row">
                    <label class="adm-field adm-grow">
                      <span>Name (raw, ASCII)</span>
                      <input
                        value={h().name}
                        maxLength={MAX_NAME}
                        onInput={(e) => edit({ name: e.currentTarget.value })}
                      />
                    </label>
                    <label class="adm-field">
                      <span>Type</span>
                      <select
                        value={String(h().type)}
                        onChange={(e) => edit({ type: Number(e.currentTarget.value) })}
                      >
                        {TYPES.map(([v, label]) => (
                          <option value={v} selected={v === h().type}>
                            {label}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label class="adm-field">
                      <span>Status</span>
                      <select
                        value={String(h().status)}
                        onChange={(e) => edit({ status: Number(e.currentTarget.value) })}
                      >
                        {STATUSES.map(([v, label]) => (
                          <option value={v} selected={v === h().status}>
                            {label}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                  <div class="adm-row">
                    <span class="adm-field-label">Range fields</span>
                    <For each={h().ranges}>
                      {(r, i) => (
                        <input
                          class="adm-num"
                          type="number"
                          min={0}
                          value={r}
                          onInput={(e) => {
                            const n = num(e.currentTarget.value);
                            if (n !== null) {
                              const ranges = [...h().ranges] as [number, number, number];
                              ranges[i()] = n;
                              edit({ ranges });
                            }
                          }}
                        />
                      )}
                    </For>
                  </div>
                  <table class="adm-records">
                    <thead>
                      <tr>
                        <th />
                        <th>map id</th>
                        <th class="adm-grow">string</th>
                        <th>flags</th>
                        <th>weight</th>
                        <th>dword 2</th>
                        <th />
                      </tr>
                    </thead>
                    <tbody>
                      <For each={h().records}>
                        {(r, i) => (
                          <tr>
                            <td class="adm-rec-idx">{i() + 1}</td>
                            <td>
                              <input
                                class="adm-num"
                                type="number"
                                min={0}
                                value={r.id}
                                onInput={(e) => {
                                  const n = num(e.currentTarget.value);
                                  if (n !== null) editRecord(i(), { id: n });
                                }}
                              />
                            </td>
                            <td>
                              <input
                                value={r.string}
                                maxLength={MAX_STRING}
                                onInput={(e) => editRecord(i(), { string: e.currentTarget.value })}
                              />
                            </td>
                            <td>
                              <input
                                class="adm-num"
                                type="number"
                                min={0}
                                value={r.flags}
                                onInput={(e) => {
                                  const n = num(e.currentTarget.value);
                                  if (n !== null) editRecord(i(), { flags: n });
                                }}
                              />
                            </td>
                            <For each={r.dwords}>
                              {(d, j) => (
                                <td>
                                  <input
                                    class="adm-num"
                                    type="number"
                                    min={0}
                                    value={d}
                                    onInput={(e) => {
                                      const n = num(e.currentTarget.value);
                                      if (n !== null) {
                                        const dwords = [...r.dwords] as [number, number];
                                        dwords[j()] = n;
                                        editRecord(i(), { dwords });
                                      }
                                    }}
                                  />
                                </td>
                              )}
                            </For>
                            <td class="adm-rec-btns">
                              <button aria-label="Move up" disabled={i() === 0} onClick={() => moveRecord(i(), -1)}>
                                ↑
                              </button>
                              <button
                                aria-label="Move down"
                                disabled={i() === h().records.length - 1}
                                onClick={() => moveRecord(i(), 1)}
                              >
                                ↓
                              </button>
                              <button aria-label="Duplicate" onClick={() => copyRecord(i())}>
                                ⧉
                              </button>
                              <button
                                aria-label="Remove"
                                disabled={h().records.length <= 1}
                                onClick={() => removeRecord(i())}
                              >
                                ×
                              </button>
                            </td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                  </details>
                  <button class="s-btn" disabled={h().records.length >= MAX_RECORDS} onClick={addRecord}>
                    Add an entry (copies the last)
                  </button>
                  <HopperPins hopper={h().index} catalog={catalog()} />
                </div>
              )}
            </Show>
          </div>
          <Show when={errors().length > 0}>
            <ul class="adm-errors">
              <For each={errors()}>{(e) => <li>{e}</li>}</For>
            </ul>
          </Show>
          <Show when={error()}>
            <p class="adm-error">{error()}</p>
          </Show>
          <Show when={status()}>
            <p class="adm-ok">{status()}</p>
          </Show>
        </Show>
      </Panel>
    </div>
  );
}
