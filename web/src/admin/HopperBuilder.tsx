// The playlist editor's working copy of multiplayer_hopper.dat. Record
// sources refer to the file as it was loaded, including across playlists.
import { For, Index, Show, createMemo, createSignal, onMount } from "solid-js";
import { Panel } from "../stats/Layout";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { parseClankerMarks } from "../lib/ranks";
import MapPicture from "../components/MapPicture";
import { getHopper, putHopper } from "./api";
import { ENGINES, loadCatalog, type Catalog } from "./catalog";
import { PRESETS, applyPreset, resolvePreset } from "./presets";
import HopperFields, { TYPE_NAMES } from "./HopperFields";
import HopperPins from "./HopperPins";
import {
  duplicatePlaylist, effectiveWeight, entryShares, groupEntries, moveGroupEntry,
  playlistDiff, playlistKind, playlistSize, playlistTitle, shareLabel,
} from "./playlistDraft";
import {
  HOPPER_COUNT, MAX_DESCRIPTION, MAX_RECORDS, MAX_TITLE, MAX_WEIGHT,
  buildPatch, builtinEntry, clearPlaylist, createPlaylist, editErrors,
  isUnused, sameHopper, toEditable, variantChoices, withVariant,
  type BuiltinVariant, type EditHopper, type EditRecord, type PlaylistTemplate, type RetailMap,
} from "./validate";

type MapOption = RetailMap & { key?: string };
type Dialog =
  | { kind: "add" | "rules" | "preset" | "publish" }
  | { kind: "map"; slot?: number; groupSlot?: number; variantKey?: string }
  | { kind: "variant"; slot?: number };

function MapArt(props: { map?: MapOption }) {
  return (
    <div class="adm-map-art" aria-hidden="true">
      <Show when={props.map}>
        {(map) => <MapPicture map={map().key ?? map().name} />}
      </Show>
    </div>
  );
}

function templateLabel(t: PlaylistTemplate): string {
  const players = t.min_players != null && t.max_players != null ? ` · ${t.min_players}–${t.max_players} players` : "";
  return `${TYPE_NAMES[t.type] ?? `Type ${t.type}`}${players}`;
}

export default function HopperBuilder() {
  const [original, setOriginal] = createSignal<EditHopper[]>([]);
  const [hoppers, setHoppers] = createSignal<EditHopper[]>([]);
  const [selected, setSelected] = createSignal<number>();
  const [error, setError] = createSignal("");
  const [status, setStatus] = createSignal("");
  const [busy, setBusy] = createSignal(true);
  const [stale, setStale] = createSignal(false);
  const [catalog, setCatalog] = createSignal<Catalog | null>(null);
  const [marks, setMarks] = createSignal<ReadonlySet<number>>(new Set());
  const [builtins, setBuiltins] = createSignal<BuiltinVariant[]>([]);
  const [templates, setTemplates] = createSignal<PlaylistTemplate[]>([]);
  const [retailMaps, setRetailMaps] = createSignal<RetailMap[]>([]);
  const [presetKey, setPresetKey] = createSignal("");
  const [dialog, setDialog] = createSignal<Dialog>();
  const [drag, setDrag] = createSignal<{ group: string; position: number }>();
  const [drop, setDrop] = createSignal<number>();
  let modal!: HTMLDialogElement;

  const maps = (): readonly MapOption[] => catalog()?.maps ?? retailMaps();
  // The extracted catalog includes disc maps and any Map Pack maps found
  // alongside the disc. The builder API's fallback list is disc-only.
  const isMapPack = (id: number) => retailMaps().length > 0 && !retailMaps().some(map => map.id === id);
  const mapAt = (id: number) => maps().find((map) => map.id === id);
  const mapName = (id: number) => mapAt(id)?.name ?? `Map ${id}`;
  const choices = createMemo(() => variantChoices(builtins(), original(), ENGINES));
  const problems = createMemo(() => editErrors(original(), hoppers(), retailMaps()));
  const patch = createMemo(() => buildPatch(original(), hoppers()));
  const changed = () => patch().hoppers.length > 0;
  const playlistIds = createMemo(() => hoppers().filter((h) => !isUnused(h)).map((h) => h.index));
  const current = () => hoppers().find((h) => h.index === selected() && !isUnused(h)) ??
    hoppers().find((h) => !isUnused(h));
  const available = () => hoppers().find((h) => isUnused(h) && !h.clear) ??
    hoppers().find((h) => h.clear);
  const groups = createMemo(() => groupEntries(current()?.records ?? []));
  const shares = createMemo(() => entryShares(current()?.records ?? []));
  const preset = () => PRESETS.find((p) => p.key === presetKey());
  const resolution = createMemo(() => {
    const p = preset();
    return p ? resolvePreset(p, catalog(), original()) : null;
  });

  const kind = (h: EditHopper) => playlistKind(h.type, marks().has(h.index) && h.template == null);
  const wasUnused = (h: EditHopper) => !original()[h.index] || isUnused(original()[h.index]!);
  const copyTemplate = (h: EditHopper) =>
    templates().find((t) => t.id === h.template) ??
    templates().find((t) => t.type === h.type && t.ranges.every((n, i) => n === h.ranges[i])) ??
    templates().find((t) => t.type === h.type);
  const canAddMap = () => (current()?.records.length ?? MAX_RECORDS) < MAX_RECORDS;

  function loadMarks(): void {
    void live.api().clankerHoppers().then((body) => setMarks(parseClankerMarks(body))).catch(() => setMarks(new Set()));
  }

  onMount(() => {
    void load();
    void loadCatalog(live.server().base).then(setCatalog);
    loadMarks();
  });

  function showDialog(next: Dialog): void {
    setError("");
    setPresetKey("");
    setDialog(next);
    modal.showModal();
  }

  function closeDialog(): void {
    modal.close();
    setDialog(undefined);
  }

  async function load(): Promise<void> {
    setBusy(true);
    setError("");
    setStatus("");
    try {
      const reply = await getHopper();
      if (!reply) {
        setError("No multiplayer_hopper.dat is published. Import a hopper file on the server to edit it here.");
        setOriginal([]);
        setHoppers([]);
        return;
      }
      setOriginal(toEditable(reply.hoppers));
      setHoppers(toEditable(reply.hoppers));
      setBuiltins(reply.variants ?? []);
      setTemplates(reply.templates ?? []);
      setRetailMaps(reply.maps ?? []);
      setStale(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function replace(h: EditHopper): void {
    setHoppers((list) => list.map((item) => item.index === h.index ? h : item));
  }

  function edit(fields: Partial<EditHopper>): void {
    const h = current();
    if (h) replace({ ...h, ...fields });
  }

  function revert(index: number): void {
    const h = original()[index];
    if (h) replace(h);
  }

  function applyTemplate(template: PlaylistTemplate, add: boolean): void {
    const h = add ? available() : current();
    if (!h) return;
    const next = createPlaylist(h, template, builtins());
    replace(add ? next : { ...next, title: h.title, description: h.description });
    setSelected(h.index);
    closeDialog();
  }

  function duplicate(): void {
    const h = current();
    const slot = available();
    const template = h && copyTemplate(h);
    if (!h || !slot || !template) return;
    replace(duplicatePlaylist(h, slot, template));
    setSelected(slot.index);
  }

  function remove(h: EditHopper): void {
    if (wasUnused(h)) revert(h.index);
    else replace(clearPlaylist(h));
    setSelected(undefined);
  }

  function editRecord(slot: number, fields: Partial<EditRecord>): void {
    const h = current();
    if (h) edit({ records: h.records.map((r, i) => i === slot ? { ...r, ...fields } : r) });
  }

  function moveRecord(slot: number, by: -1 | 1): void {
    const h = current();
    if (!h || slot + by < 0 || slot + by >= h.records.length) return;
    const records = [...h.records];
    [records[slot], records[slot + by]] = [records[slot + by]!, records[slot]!];
    edit({ records });
  }

  function copyRecord(slot: number): void {
    const h = current();
    const r = h?.records[slot];
    if (!h || !r || !canAddMap()) return;
    const records = [...h.records];
    records.splice(slot + 1, 0, { ...r });
    edit({ records });
  }

  function removeRecord(slot: number): void {
    const h = current();
    if (h && h.records.length > 1) edit({ records: h.records.filter((_, i) => i !== slot) });
  }

  function addRecord(id: number, groupSlot?: number): void {
    const h = current();
    if (!h || !canAddMap()) return;
    const source = h.records[groupSlot ?? h.records.length - 1];
    const defaultKey = h.type >= 3 ? "team_slayer" : "rumble_slayer";
    const variant = builtins().find((b) => b.key === defaultKey) ?? builtins().find((b) => b.team === (h.type >= 3)) ?? builtins()[0];
    const r = source ? { ...source, id } : variant ? builtinEntry(id, variant) : null;
    if (!r) return;
    edit({ records: [...h.records, r] });
  }

  function chooseVariant(key: string): void {
    const picker = dialog();
    const h = current();
    const choice = choices().find((item) => item.key === key);
    if (!h || picker?.kind !== "variant" || !choice) return;
    if (picker.slot !== undefined) {
      const r = h.records[picker.slot];
      if (r) editRecord(picker.slot, withVariant(r, choice, h.index));
      closeDialog();
    } else {
      setDialog({ kind: "map", variantKey: key });
      modal.querySelector<HTMLButtonElement>(".adm-picker-maps button")?.focus();
    }
  }

  function chooseMap(id: number): void {
    const picker = dialog();
    const h = current();
    if (picker?.kind !== "map" || !h) return;
    if (picker.slot !== undefined) {
      editRecord(picker.slot, { id });
    } else if (picker.variantKey) {
      const choice = choices().find((item) => item.key === picker.variantKey);
      if (!choice || !canAddMap()) return;
      const record: EditRecord = { id, string: "", flags: 0, dwords: [1, 0], src: null };
      edit({ records: [...h.records, withVariant(record, choice, h.index)] });
    } else {
      addRecord(id, picker.groupSlot);
    }
    closeDialog();
  }

  function applySelectedPreset(): void {
    const h = current();
    const resolved = resolution();
    if (h && resolved?.records.length) {
      replace(applyPreset(h, resolved));
      closeDialog();
    }
  }

  function moveMap(slots: number[], from: number, to: number): void {
    const h = current();
    if (!h || to < 0 || to >= slots.length || from === to) return;
    edit({ records: moveGroupEntry(h.records, slots, from, to) });
  }

  function endDrag(): void {
    setDrag(undefined);
    setDrop(undefined);
  }

  async function save(): Promise<void> {
    if (busy() || stale() || !changed() || problems().size) return;
    const body = patch();
    const server = live.server().base;
    const account = identity.me()?.xuid;
    setBusy(true);
    setError("");
    setStatus("");
    try {
      const latest = await getHopper();
      if (live.server().base !== server || identity.me()?.xuid !== account) {
        throw new Error("The selected server or account changed. Reopen the editor to continue.");
      }
      const loaded = original();
      if (!latest || latest.hoppers.length !== loaded.length ||
          toEditable(latest.hoppers).some((h) => !loaded[h.index] || !sameHopper(h, loaded[h.index]!))) {
        setStale(true);
        setError("The published hopper changed while you were editing. Close this review and reload the published file to start a fresh draft.");
        return;
      }
      const reply = await putHopper(body);
      setOriginal(toEditable(reply.hoppers));
      setHoppers(toEditable(reply.hoppers));
      setStatus(`Published ${reply.published.size.toLocaleString()} bytes · SHA-256 ${reply.published.sha256.slice(0, 12)}…`);
      loadMarks();
      closeDialog();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="adm-hopper">
      <Panel title="Playlists" sub="multiplayer_hopper.dat · Downloaded at sign-in" right={
        <>
          <Show when={changed()}><span class="adm-dirty">Unpublished changes</span></Show>
          <Show when={problems().size}><span class="adm-error">Fix {problems().size === 1 ? "1 playlist" : `${problems().size} playlists`} to publish</span></Show>
          <button class="s-btn" disabled={busy() || stale() || !changed() || problems().size > 0}
            onClick={() => showDialog({ kind: "publish" })}>Review &amp; publish</button>
        </>
      }>
        <Show when={hoppers().length} fallback={<Show when={busy()}><p>Loading playlists…</p></Show>}>
          <fieldset class="adm-hopper-cols" disabled={busy()}>
            <aside class="adm-playlist-rail" aria-label="Playlists">
              <div class="adm-playlist-capacity">
                <span>{playlistIds().length} of {HOPPER_COUNT}</span>
                <button class="s-btn" disabled={!available() || !templates().length} onClick={() => showDialog({ kind: "add" })}>Add playlist</button>
              </div>
              <Show when={!templates().length}><p class="adm-preset-note">This server provides no playlist templates.</p></Show>
              <ul class="adm-list">
                <For each={playlistIds()}>{(id) => {
                  const h = () => hoppers()[id]!;
                  return (
                    <li>
                      <button classList={{ on: current()?.index === id, bad: problems().has(id) }}
                        aria-current={current()?.index === id ? "true" : undefined} onClick={() => setSelected(id)}>
                        <span class="adm-list-name">{playlistTitle(h())}</span>
                        <span class="adm-list-meta">{kind(h())}</span>
                      </button>
                    </li>
                  );
                }}</For>
              </ul>
            </aside>
            <Show when={current()} fallback={<p class="adm-note">Add a playlist to start building a lineup.</p>}>
              {(h) => (
                <div class="adm-editor">
                  <header class="adm-playlist-head">
                    <div>
                      <h3>{playlistTitle(h())}</h3>
                      <p><strong>{kind(h())}</strong> · {playlistSize(h())}</p>
                    </div>
                    <div class="adm-toolbar">
                      <button class="s-btn" disabled={!available() || !copyTemplate(h())} onClick={duplicate}
                        title={kind(h()) === "Clanker" ? "New copies use human matchmaking rules" : undefined}>Duplicate</button>
                      <button class="s-btn" onClick={() => remove(h())}>{wasUnused(h()) ? "Discard" : "Remove"}</button>
                      <button class="s-btn" onClick={() => showDialog({ kind: "preset" })}>Reset to preset</button>
                    </div>
                  </header>
                  <Show when={problems().get(h().index)}>{(errs) => (
                    <div class="adm-inline-errors" role="alert">
                      <ul class="adm-errors"><For each={errs()}>{(e) => <li>{e}</li>}</For></ul>
                    </div>
                  )}</Show>
                  <div class="adm-row">
                    <label class="adm-field">
                      <span>Title</span>
                      <input value={h().title} maxLength={MAX_TITLE} onInput={(e) => edit({ title: e.currentTarget.value })} />
                    </label>
                    <label class="adm-field adm-grow">
                      <span>Description</span>
                      <input value={h().description} maxLength={MAX_DESCRIPTION} onInput={(e) => edit({ description: e.currentTarget.value })} />
                    </label>
                    <button class="s-btn" disabled={!templates().length} onClick={() => showDialog({ kind: "rules" })}>Change rules</button>
                    <Show when={!wasUnused(h()) && patch().hoppers.some((item) => item.index === h().index)}>
                      <button class="s-btn" onClick={() => revert(h().index)}>Revert edits</button>
                    </Show>
                  </div>
                  <div class="adm-lineup-head">
                    <h4>Map rotation <span>{h().records.length} {h().records.length === 1 ? "entry" : "entries"}</span></h4>
                    <button class="s-btn" disabled={!canAddMap() || !choices().length || !maps().length}
                      onClick={() => showDialog({ kind: "variant" })}>Add variant</button>
                  </div>
                  <Show when={h().records.some(entry => isMapPack(entry.id))}>
                    <p class="adm-note">Map Pack maps are available when every console in the match has the pack.</p>
                  </Show>
                  <Show when={groups().length === 0}>
                    <p class="adm-note">Add a game variant and choose its first map.</p>
                  </Show>
                  <For each={groups().map((group) => group.key)}>{(key) => {
                    const group = () => groups().find((item) => item.key === key)!;
                    return (
                      <section class="adm-variant-group">
                        <header>
                          <h4>{group().label}</h4>
                          <button class="s-btn" disabled={!canAddMap() || !maps().length} aria-label={`Add map to ${group().label}`}
                            onClick={() => showDialog({ kind: "map", groupSlot: group().slots.at(-1) })}>Add map</button>
                        </header>
                        <ul class="adm-map-cards">
                          <Index each={group().slots}>{(slot, position) => {
                            const record = () => h().records[slot()]!;
                            const label = () => mapName(record().id);
                            const odds = () => shares()[slot()] ?? 0;
                            const activeDrag = () => drag()?.group === key;
                            function keyboardMove(event: KeyboardEvent): void {
                              const by = event.key === "ArrowUp" ? -1 : event.key === "ArrowDown" ? 1 : 0;
                              if (!by) return;
                              event.preventDefault();
                              const handle = event.currentTarget as HTMLButtonElement;
                              const card = handle.closest("li");
                              const target = by < 0 ? card?.previousElementSibling : card?.nextElementSibling;
                              moveMap(group().slots, position, position + by);
                              target?.querySelector<HTMLButtonElement>(".adm-drag-handle")?.focus();
                            }
                            return (
                              <li classList={{ "adm-map-card": true, "adm-map-drop": activeDrag() && drop() === position }}
                                onDragOver={(e) => {
                                  if (!activeDrag()) return;
                                  e.preventDefault();
                                  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
                                  setDrop(position);
                                }}
                                onDrop={(e) => {
                                  const source = drag();
                                  if (source?.group !== key) return;
                                  e.preventDefault();
                                  moveMap(group().slots, source.position, position);
                                  endDrag();
                                }}>
                                <button class="adm-drag-handle" draggable={group().slots.length > 1}
                                  disabled={group().slots.length < 2}
                                  aria-label={`Reorder ${label()} in ${group().label} with the Up or Down arrow`}
                                  title="Drag or press ↑ or ↓ to reorder"
                                  onKeyDown={keyboardMove}
                                  onDragStart={(e) => {
                                    if (e.dataTransfer) {
                                      e.dataTransfer.effectAllowed = "move";
                                      e.dataTransfer.setData("text/plain", label());
                                    }
                                    setDrag({ group: key, position });
                                  }}
                                  onDragEnd={endDrag}>⠿</button>
                                <button class="adm-map-pick" disabled={!maps().length}
                                  aria-label={`Change map for ${label()}`}
                                  onClick={() => showDialog({ kind: "map", slot: slot() })}>
                                  <MapArt map={mapAt(record().id)} />
                                  <span>{label()}</span>
                                </button>
                                <div class="adm-map-weight">
                                  <div class="adm-weight-value">
                                    <input type="number" min={1} max={MAX_WEIGHT} step={1}
                                      aria-label={`Weight for ${label()}`} title="Selection weight"
                                      value={effectiveWeight(record().dwords[0])} onChange={(e) => {
                                        const input = e.currentTarget;
                                        const weight = input.valueAsNumber;
                                        if (Number.isInteger(weight) && weight >= 1 && weight <= MAX_WEIGHT) {
                                          editRecord(slot(), { dwords: [weight, record().dwords[1]] });
                                        } else {
                                          input.value = String(effectiveWeight(record().dwords[0]));
                                        }
                                      }} />
                                    <span> · {shareLabel(odds())}</span>
                                  </div>
                                  <progress max={100} value={odds()} aria-label={`Selection chance for ${label()}`} />
                                </div>
                                <div class="adm-map-actions">
                                  <button class="adm-link" disabled={!choices().length} aria-label={`Change variant for ${label()}`}
                                    onClick={() => showDialog({ kind: "variant", slot: slot() })}>Change variant</button>
                                  <button class="adm-link" disabled={!canAddMap()} aria-label={`Copy ${label()}`} onClick={() => copyRecord(slot())}>Copy map</button>
                                  <button class="adm-link" disabled={h().records.length <= 1} aria-label={`Remove ${label()}`} onClick={() => removeRecord(slot())}>Remove map</button>
                                </div>
                              </li>
                            );
                          }}</Index>
                        </ul>
                      </section>
                    );
                  }}</For>
                  <HopperFields hopper={h()} edit={edit} editRecord={editRecord}
                    moveRecord={moveRecord} copyRecord={copyRecord} removeRecord={removeRecord} />
                  <Show when={!wasUnused(h())} fallback={<p class="adm-preset-note">Publish this playlist to allow custom maps.</p>}>
                    <HopperPins hopper={h().index} catalog={catalog()} maps={maps()}
                      entryMaps={h().records.map((r) => r.id)} onAddEntry={addRecord} />
                  </Show>
                </div>
              )}
            </Show>
          </fieldset>
        </Show>
        <For each={hoppers().filter((h) => h.clear)}>{(h) => (
          <div class="adm-removed-playlist">
            <span>{playlistTitle(original()[h.index] ?? h)} will be removed</span>
            <button class="adm-link" disabled={busy()} onClick={() => { revert(h.index); setSelected(h.index); }}>Undo</button>
          </div>
        )}</For>
        <Show when={problems().size > 1}>
          <ul class="adm-errors">
            <For each={[...problems().keys()]}>{(index) => (
              <li><button class="adm-link" onClick={() => setSelected(index)}>{playlistTitle(hoppers()[index]!)}</button></li>
            )}</For>
          </ul>
        </Show>
        <Show when={error() && !dialog()}><p class="adm-error" role="alert">{error()}</p></Show>
        <Show when={stale() && !dialog()}>
          <button class="s-btn" disabled={busy()} onClick={() => void load()}>Discard draft and reload</button>
        </Show>
        <Show when={status()}><p class="adm-ok" role="status">{status()}</p></Show>
      </Panel>

      <dialog ref={modal} class="adm-dialog" aria-labelledby="adm-hopper-dialog-title"
        onCancel={(e) => { if (busy()) e.preventDefault(); }}
        onClose={() => { if (!modal.open) setDialog(undefined); }}>
        <Show when={dialog()}>{(d) => (
          <>
            <h3 id="adm-hopper-dialog-title">{dialogTitle(d())}</h3>
            <div class="adm-dialog-body">
              <Show when={d().kind === "add" || d().kind === "rules"}>
                <ul class="adm-picker">
                  <For each={templates()}>{(t) => (
                    <li><button class="s-btn" onClick={() => applyTemplate(t, d().kind === "add")}>
                      <strong>{t.title}</strong><small>{templateLabel(t)}</small>
                    </button></li>
                  )}</For>
                </ul>
              </Show>
              <Show when={d().kind === "preset"}>
                <label class="adm-field">
                  <span>Historical lineup</span>
                  <select value={presetKey()} onChange={(e) => setPresetKey(e.currentTarget.value)}>
                    <option value="">Choose a preset…</option>
                    <For each={PRESETS}>{(p) => <option value={p.key}>{p.label} · {p.era}</option>}</For>
                  </select>
                </label>
                <Show when={resolution()}>{(r) => (
                  <div class="adm-preset-preview">
                    <p class="adm-preset-title">{r().title}{" "}<span>{r().description}</span></p>
                    <Show when={preset()?.uncertain}>{(note) => <p class="adm-preset-note">From memory · {note()}</p>}</Show>
                    <ul class="adm-preset-games">
                      <For each={r().games}>{(g) => (
                        <li classList={{ off: !g.record }}>{g.label}<Show when={g.missing}><span> · Unavailable: {g.missing}</span></Show></li>
                      )}</For>
                    </ul>
                  </div>
                )}</Show>
                <Show when={!catalog()}><p class="adm-preset-note">Extract the server’s map catalog to resolve historical lineups.</p></Show>
              </Show>
              <Show when={d().kind === "variant"}>
                <For each={["built-in", "in this file"] as const}>{(group) => {
                  const options = () => choices().filter((choice) => choice.group === group);
                  return (
                    <Show when={options().length}>
                      <h4>{group === "built-in" ? "Built-in variants" : "Variants in the published file"}</h4>
                      <ul class="adm-picker">
                        <For each={options()}>{(choice) => (
                          <li><button class="s-btn" onClick={() => chooseVariant(choice.key)}>{choice.label}</button></li>
                        )}</For>
                      </ul>
                    </Show>
                  );
                }}</For>
              </Show>
              <Show when={d().kind === "map"}>
                <ul class="adm-picker adm-picker-maps">
                  <For each={maps()}>{(map) => (
                    <li>
                      <button class="s-btn" onClick={() => chooseMap(map.id)}>
                        <MapArt map={map} />
                        <strong>{map.name}</strong>
                        <Show when={isMapPack(map.id)}><small>Map Pack</small></Show>
                      </button>
                    </li>
                  )}</For>
                </ul>
              </Show>
              <Show when={d().kind === "publish"}>
                <p>Review the draft against the published hopper. The game downloads the updated file at sign-in.</p>
                <For each={playlistDiff(original(), hoppers(), mapName)}>{(item) => (
                  <section class="adm-playlist-diff">
                    <h4>{item.action} · {item.title} <small>Slot {item.index + 1}</small></h4>
                    <Show when={item.lines.length}>
                      <table class="s-stats">
                        <thead><tr><th scope="col">Field</th><th scope="col">Published</th><th scope="col">Draft</th></tr></thead>
                        <tbody><For each={item.lines}>{(line) => (
                          <tr><th scope="row">{line.field}</th><td>{line.before || "—"}</td><td>{line.after || "—"}</td></tr>
                        )}</For></tbody>
                      </table>
                    </Show>
                  </section>
                )}</For>
                <Show when={error()}><p class="adm-error" role="alert">{error()}</p></Show>
              </Show>
            </div>
            <footer class="adm-toolbar">
              <button class="s-btn" disabled={busy()} onClick={closeDialog}>Cancel</button>
              <Show when={d().kind === "preset"}>
                <button class="s-btn" disabled={!resolution()?.records.length} onClick={applySelectedPreset}>Apply preset</button>
              </Show>
              <Show when={d().kind === "publish"}>
                <button class="s-btn" disabled={busy() || stale()} onClick={() => void save()}>{busy() ? "Publishing…" : "Publish changes"}</button>
              </Show>
            </footer>
          </>
        )}</Show>
      </dialog>
    </div>
  );
}

function dialogTitle(dialog: Dialog): string {
  switch (dialog.kind) {
    case "add": return "Add playlist";
    case "rules": return "Change matchmaking rules";
    case "preset": return "Reset to preset";
    case "publish": return "Review playlist changes";
    case "variant": return dialog.slot === undefined ? "Add game variant" : "Change game variant";
    case "map": return dialog.slot !== undefined ? "Change map" : dialog.variantKey ? "Choose the first map" : "Add map";
  }
}
