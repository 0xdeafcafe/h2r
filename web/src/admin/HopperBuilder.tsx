// The playlist builder: the 16 hoppers of multiplayer_hopper.dat. Edits the
// fields the format documents (title, description, the range fields, each
// entry's map and weight; the raw words behind an "Advanced" disclosure)
// and reorders, copies, adds and removes entries — each kept entry
// remembering which published slot its unknown bytes (its game variant)
// come from, so nothing the builder cannot edit is ever lost. A preset
// fills a playlist from a historical lineup, resolved against the server's
// catalog of the owner's maps and variants (./catalog.ts, ./presets.ts).
// 404 until a base image is imported.
//
// The file always has 16 slots; the default set fills 11 and leaves the rest
// unused (type 0, no entries), which the game reads as no playlist. An
// unused slot is made a playlist from a template (the server's default
// playlist of that kind: the descriptor the game's validator wants), and a
// playlist can be cleared back to unused. Only playlists that changed are
// checked, so untouched unused slots never block a publish. Each entry's
// game variant is a choice: a built-in one (written whole by the server) or
// one already in the file (its slot copied).

import { For, Index, Show, createMemo, createSignal, onMount } from "solid-js";
import { Panel } from "../stats/Layout";
import { live } from "../state/live";
import { getHopper, putHopper } from "./api";
import { ENGINES, loadCatalog, type Catalog } from "./catalog";
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
  builtinEntry,
  choiceKey,
  clearPlaylist,
  createPlaylist,
  editErrors,
  isUnused,
  toEditable,
  variantChoices,
  variantLabel as describeVariant,
  withVariant,
  type BuiltinVariant,
  type EditHopper,
  type EditRecord,
  type PlaylistTemplate,
  type RetailMap,
} from "./validate";

/** Hopper types as the game sorts them (live-proto `HopperType`). */
const TYPE_NAMES: Record<number, string> = {
  1: "unranked free for all",
  2: "ranked free for all",
  3: "unranked teams",
  4: "ranked teams",
  5: "clan match",
};

const TYPES: Array<[number, string]> = [1, 2, 3, 4, 5].map((t) => [t, `${t} · ${TYPE_NAMES[t]}`]);

/** What the three range fields (the first type parameters) mean. */
function rangeLabels(type: number): [string, string, string] {
  return type >= 3 ? ["Teams", "Smallest team", "Largest team"] : ["Fewest players", "Most players", "(unused)"];
}

function templateLabel(t: PlaylistTemplate): string {
  const players =
    t.min_players != null && t.max_players != null ? ` · ${t.min_players}–${t.max_players} players` : "";
  return `${t.title} · ${TYPE_NAMES[t.type] ?? `type ${t.type}`}${players}`;
}

const STATUSES: Array<[number, string]> = [
  [0, "0 · not loaded"],
  [1, "1 · ok"],
  [2, "2 · error"],
];

export default function HopperBuilder() {
  const [original, setOriginal] = createSignal<EditHopper[]>([]);
  const [hoppers, setHoppers] = createSignal<EditHopper[]>([]);
  const [sel, setSel] = createSignal(0);
  const [error, setError] = createSignal("");
  const [status, setStatus] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [catalog, setCatalog] = createSignal<Catalog | null>(null);
  const [presetKey, setPresetKey] = createSignal("");
  const [builtins, setBuiltins] = createSignal<BuiltinVariant[]>([]);
  const [templates, setTemplates] = createSignal<PlaylistTemplate[]>([]);
  const [retailMaps, setRetailMaps] = createSignal<RetailMap[]>([]);
  const [templateKey, setTemplateKey] = createSignal("");

  /** The maps an entry can name: the owner's catalog, else the server's
   *  retail list. */
  const maps = (): ReadonlyArray<{ id: number; name: string; key?: string }> =>
    catalog()?.maps ?? retailMaps();
  const mapName = (id: number): string | null => maps().find((m) => m.id === id)?.name ?? null;
  const choices = createMemo(() => variantChoices(builtins(), original(), ENGINES));
  const problems = createMemo(() => editErrors(original(), hoppers()));
  const wasUnused = (i: number): boolean => {
    const o = original()[i];
    return !o || isUnused(o);
  };

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

  const variantLabel = (r: EditRecord): string => describeVariant(r.variant, ENGINES);

  async function load(): Promise<void> {
    setBusy(true);
    setError("");
    try {
      const reply = await getHopper();
      if (!reply) {
        setError("no multiplayer_hopper.dat published for this server yet");
        setStatus("the server takes the game's own playlist file once, on its own machine; then it can be edited here");
        setHoppers([]);
        setOriginal([]);
        return;
      }
      setOriginal(toEditable(reply.hoppers));
      setHoppers(toEditable(reply.hoppers));
      setBuiltins(reply.variants ?? []);
      setTemplates(reply.templates ?? []);
      setRetailMaps(reply.maps ?? []);
      setStatus("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const current = (): EditHopper | null => hoppers()[sel()] ?? null;
  const errors = (): Array<{ index: number; text: string }> =>
    [...problems()].flatMap(([index, errs]) => errs.map((e) => ({ index, text: `Playlist ${index + 1}: ${e}` })));
  const changed = (): boolean =>
    hoppers().length > 0 && buildPatch(original(), hoppers()).hoppers.length > 0;

  function edit(patch: Partial<EditHopper>): void {
    const h = current();
    if (!h) return;
    replace({ ...h, ...patch });
  }

  function replace(h: EditHopper): void {
    const next = [...hoppers()];
    next[h.index] = h;
    setHoppers(next);
  }

  /** Back to the playlist as published. */
  function revert(i: number): void {
    const o = original()[i];
    if (o) replace({ ...o, records: o.records.map((r) => ({ ...r })) });
  }

  function chosenTemplate(): PlaylistTemplate | null {
    return templates().find((t) => String(t.id) === templateKey()) ?? templates()[0] ?? null;
  }

  /** Makes the selected unused slot a playlist of the chosen kind. */
  function makePlaylist(): void {
    const h = current();
    const t = chosenTemplate();
    if (!h || !t) return;
    replace(createPlaylist(h, t, builtins()));
    setTemplateKey("");
  }

  /** Gives the selected playlist another kind, keeping its text and entries. */
  function changeKind(id: string): void {
    const h = current();
    const t = templates().find((x) => String(x.id) === id);
    if (!h || !t) return;
    replace({ ...createPlaylist(h, t, builtins()), title: h.title, description: h.description });
  }

  /** Removes the selected playlist: a draft goes back to unused at once;
   *  a published one is cleared on publish. */
  function removePlaylist(): void {
    const h = current();
    if (!h) return;
    if (wasUnused(h.index)) revert(h.index);
    else replace(clearPlaylist(h));
  }

  /** A first entry for a playlist with none: the built-in Slayer that suits
   *  its type, on the first map. */
  function defaultEntry(h: EditHopper, mapId?: number): EditRecord | null {
    const teams = h.type >= 3;
    const v =
      builtins().find((b) => b.key === (teams ? "team_slayer" : "rumble_slayer")) ??
      builtins().find((b) => b.team === teams) ??
      builtins()[0];
    const id = mapId ?? maps()[0]?.id;
    return v && id !== undefined ? builtinEntry(id, v) : null;
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
    // A built-in pick is rebuilt by the server; anything else keeps its slot.
    records.splice(i + 1, 0, { ...from, src: from.pick ? null : (from.src ?? i) });
    edit({ records });
  }

  function removeRecord(i: number): void {
    const h = current();
    if (!h || h.records.length <= 1) return;
    edit({ records: h.records.filter((_, j) => j !== i) });
  }

  function addRecord(mapId?: number): void {
    const h = current();
    if (!h || h.records.length >= MAX_RECORDS) return;
    // A new entry copies the last one (its game variant included), on
    // `mapId` when given; then its map and variant are picked in its row.
    const last = h.records[h.records.length - 1];
    const next = last
      ? { ...last, id: mapId ?? last.id, src: last.pick ? null : (last.src ?? h.records.length - 1) }
      : defaultEntry(h, mapId);
    if (!next) return;
    edit({ records: [...h.records, next] });
  }

  function setVariant(i: number, key: string): void {
    const h = current();
    const r = h?.records[i];
    const c = choices().find((x) => x.key === key);
    if (!h || !r || !c) return;
    const records = [...h.records];
    records[i] = withVariant(r, c, h.index);
    edit({ records });
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
            <Show when={errors().length > 0}>
              <span class="adm-error">fix {problems().size === 1 ? "1 playlist" : `${problems().size} playlists`} to publish</span>
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
                      classList={{ on: h.index === sel(), off: isUnused(h), bad: problems().has(h.index) }}
                      onClick={() => setSel(h.index)}
                    >
                      <span class="adm-list-idx">{h.index + 1}</span>
                      <span class="adm-list-name">
                        {isUnused(h) ? (h.clear ? "(removed on publish)" : "(unused)") : h.title || h.name || "(untitled)"}
                      </span>
                      <span class="adm-list-meta">
                        {problems().has(h.index) ? "fix · " : ""}
                        {isUnused(h) ? "empty" : `t${h.type} · ${h.records.length}`}
                      </span>
                    </button>
                  </li>
                )}
              </For>
            </ol>
            <Show when={current()}>
              {(h) => (
                <Show
                  when={!isUnused(h())}
                  fallback={
                    <div class="adm-editor">
                      <Show
                        when={h().clear}
                        fallback={
                          <>
                            <p class="adm-preset-note">
                              Slot {h().index + 1} is unused: the game shows no playlist here. Choose what kind of playlist to make; it starts with that kind's matchmaking rules and one entry, which you can then change.
                            </p>
                            <Show
                              when={templates().length > 0}
                              fallback={<p class="adm-error">This server does not offer playlist templates yet; update it to add playlists.</p>}
                            >
                              <div class="adm-row">
                                <label class="adm-field adm-grow">
                                  <span>Kind of playlist</span>
                                  <select value={templateKey()} onChange={(e) => setTemplateKey(e.currentTarget.value)}>
                                    <For each={templates()}>
                                      {(t) => (
                                        <option value={t.id} selected={String(t.id) === templateKey()}>
                                          {templateLabel(t)}
                                        </option>
                                      )}
                                    </For>
                                  </select>
                                </label>
                                <button class="s-btn" onClick={makePlaylist}>
                                  Make playlist {h().index + 1}
                                </button>
                              </div>
                            </Show>
                          </>
                        }
                      >
                        <p class="adm-preset-note">Playlist {h().index + 1} will be removed when you publish: its slot becomes unused.</p>
                        <button class="s-btn" onClick={() => revert(h().index)}>
                          Keep it
                        </button>
                      </Show>
                      <Show when={problems().get(h().index)}>
                        {(errs) => (
                          <div class="adm-inline-errors">
                            <ul class="adm-errors">
                              <For each={errs()}>{(e) => <li>{e}</li>}</For>
                            </ul>
                            <button class="s-btn" onClick={() => revert(h().index)}>
                              Undo the changes to this slot
                            </button>
                          </div>
                        )}
                      </Show>
                    </div>
                  }
                >
                <div class="adm-editor">
                  <Show when={problems().get(h().index)}>
                    {(errs) => (
                      <div class="adm-inline-errors">
                        <ul class="adm-errors">
                          <For each={errs()}>{(e) => <li>{e}</li>}</For>
                        </ul>
                        <button class="s-btn" onClick={() => revert(h().index)}>
                          {wasUnused(h().index) ? "Discard this new playlist" : "Revert to the published playlist"}
                        </button>
                      </div>
                    )}
                  </Show>
                  <div class="adm-row">
                    <span class="adm-field-label">
                      {wasUnused(h().index) ? "New playlist" : "Playlist"} {h().index + 1} · {TYPE_NAMES[h().type] ?? `type ${h().type}`}
                    </span>
                    <Show when={templates().length > 0}>
                      <label class="adm-field">
                        <span>Change kind</span>
                        <select
                          value=""
                          onChange={(e) => {
                            changeKind(e.currentTarget.value);
                            e.currentTarget.value = "";
                          }}
                        >
                          <option value="" selected>
                            Keep its rules…
                          </option>
                          <For each={templates()}>{(t) => <option value={t.id}>{templateLabel(t)}</option>}</For>
                        </select>
                      </label>
                    </Show>
                    <button class="s-btn" onClick={removePlaylist}>
                      {wasUnused(h().index) ? "Discard" : "Remove playlist"}
                    </button>
                  </div>
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
                        No map catalog from this server yet (its game art has not been extracted): maps show as numbers and presets cannot resolve.
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
                      <Index each={h().records}>
                        {(r, i) => (
                          <tr>
                            <td class="adm-rec-idx">{i + 1}</td>
                            <td>
                              <Show
                                when={maps().length > 0}
                                fallback={<span class="adm-mono">map id {r().id}</span>}
                              >
                                <select
                                  value={String(r().id)}
                                  onChange={(e) => editRecord(i, { id: Number(e.currentTarget.value) })}
                                >
                                  <Show when={!mapName(r().id)}>
                                    <option value={r().id} selected>
                                      id {r().id} (not a map in this game)
                                    </option>
                                  </Show>
                                  <For each={maps()}>
                                    {(m) => (
                                      <option value={m.id} selected={m.id === r().id}>
                                        {m.name}
                                      </option>
                                    )}
                                  </For>
                                </select>
                              </Show>
                            </td>
                            <td class="adm-variant">
                              <select value={choiceKey(r(), h().index)} onChange={(e) => setVariant(i, e.currentTarget.value)}>
                                <Show when={!choices().some((c) => c.key === choiceKey(r(), h().index))}>
                                  <option value={choiceKey(r(), h().index)} selected>
                                    {variantLabel(r())}
                                  </option>
                                </Show>
                                <For each={["built-in", "in this file"] as const}>
                                  {(group) => (
                                    <Show when={choices().some((c) => c.group === group)}>
                                      <optgroup label={group === "built-in" ? "Built-in variants" : "Variants in this file"}>
                                        <For each={choices().filter((c) => c.group === group)}>
                                          {(c) => (
                                            <option value={c.key} selected={c.key === choiceKey(r(), h().index)}>
                                              {c.label}
                                            </option>
                                          )}
                                        </For>
                                      </optgroup>
                                    </Show>
                                  )}
                                </For>
                              </select>
                            </td>
                            <td>
                              <input
                                class="adm-num"
                                type="number"
                                min={1}
                                max={MAX_WEIGHT}
                                value={r().dwords[0]}
                                onInput={(e) => {
                                  const n = num(e.currentTarget.value);
                                  if (n !== null) editRecord(i, { dwords: [n, r().dwords[1]] });
                                }}
                              />
                            </td>
                            <td class="adm-rec-btns">
                              <button aria-label="Move up" disabled={i === 0} onClick={() => moveRecord(i, -1)}>
                                ↑
                              </button>
                              <button
                                aria-label="Move down"
                                disabled={i === h().records.length - 1}
                                onClick={() => moveRecord(i, 1)}
                              >
                                ↓
                              </button>
                              <button aria-label="Duplicate" onClick={() => copyRecord(i)}>
                                ⧉
                              </button>
                              <button
                                aria-label="Remove"
                                disabled={h().records.length <= 1}
                                onClick={() => removeRecord(i)}
                              >
                                ×
                              </button>
                            </td>
                          </tr>
                        )}
                      </Index>
                    </tbody>
                  </table>
                  <button class="s-btn" disabled={h().records.length >= MAX_RECORDS} onClick={() => addRecord()}>
                    Add an entry
                  </button>
                  <details class="adm-hint adm-advanced">
                    <summary>Advanced: raw fields</summary>
                    <p class="adm-preset-note">
                      The entry string, flags and second dword lie inside the entry's game variant, and the name field overlaps a table the game validates: change them only if you know the bytes. Type and the range fields are the matchmaking rules; the server refuses a combination the game would not load (use Change kind instead).
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
                    <Index each={h().ranges}>
                      {(r, i) => (
                        <input
                          class="adm-num"
                          type="number"
                          min={0}
                          title={rangeLabels(h().type)[i]}
                          aria-label={rangeLabels(h().type)[i]}
                          value={r()}
                          onInput={(e) => {
                            const n = num(e.currentTarget.value);
                            if (n !== null) {
                              const ranges = [...h().ranges] as [number, number, number];
                              ranges[i] = n;
                              edit({ ranges });
                            }
                          }}
                        />
                      )}
                    </Index>
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
                      <Index each={h().records}>
                        {(r, i) => (
                          <tr>
                            <td class="adm-rec-idx">{i + 1}</td>
                            <td>
                              <input
                                class="adm-num"
                                type="number"
                                min={0}
                                value={r().id}
                                onInput={(e) => {
                                  const n = num(e.currentTarget.value);
                                  if (n !== null) editRecord(i, { id: n });
                                }}
                              />
                            </td>
                            <td>
                              <input
                                value={r().string}
                                maxLength={MAX_STRING}
                                onInput={(e) => editRecord(i, { string: e.currentTarget.value })}
                              />
                            </td>
                            <td>
                              <input
                                class="adm-num"
                                type="number"
                                min={0}
                                value={r().flags}
                                onInput={(e) => {
                                  const n = num(e.currentTarget.value);
                                  if (n !== null) editRecord(i, { flags: n });
                                }}
                              />
                            </td>
                            <Index each={r().dwords}>
                              {(d, j) => (
                                <td>
                                  <input
                                    class="adm-num"
                                    type="number"
                                    min={0}
                                    value={d()}
                                    onInput={(e) => {
                                      const n = num(e.currentTarget.value);
                                      if (n !== null) {
                                        const dwords = [...r().dwords] as [number, number];
                                        dwords[j] = n;
                                        editRecord(i, { dwords });
                                      }
                                    }}
                                  />
                                </td>
                              )}
                            </Index>
                            <td class="adm-rec-btns">
                              <button aria-label="Move up" disabled={i === 0} onClick={() => moveRecord(i, -1)}>
                                ↑
                              </button>
                              <button
                                aria-label="Move down"
                                disabled={i === h().records.length - 1}
                                onClick={() => moveRecord(i, 1)}
                              >
                                ↓
                              </button>
                              <button aria-label="Duplicate" onClick={() => copyRecord(i)}>
                                ⧉
                              </button>
                              <button
                                aria-label="Remove"
                                disabled={h().records.length <= 1}
                                onClick={() => removeRecord(i)}
                              >
                                ×
                              </button>
                            </td>
                          </tr>
                        )}
                      </Index>
                    </tbody>
                  </table>
                  </details>
                  <Show
                    when={!wasUnused(h().index)}
                    fallback={<p class="adm-preset-note">Custom maps can be allowed once this playlist is published.</p>}
                  >
                    <HopperPins
                      hopper={h().index}
                      catalog={catalog()}
                      maps={maps()}
                      entryMaps={h().records.map((r) => r.id)}
                      onAddEntry={(id) => addRecord(id)}
                    />
                  </Show>
                </div>
                </Show>
              )}
            </Show>
          </div>
          <Show when={errors().length > 0}>
            <ul class="adm-errors">
              <For each={errors()}>
                {(e) => (
                  <li>
                    <button class="adm-link" onClick={() => setSel(e.index)}>
                      {e.text}
                    </button>
                  </li>
                )}
              </For>
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
