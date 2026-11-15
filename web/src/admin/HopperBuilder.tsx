// The playlist builder: the 16 hoppers of multiplayer_hopper.dat. Edits the
// fields the format documents (name, type, the party-size ranges, each
// entry's id/string/flags/dwords) and reorders, copies, adds and removes
// entries — each kept entry remembering which published slot its unknown
// bytes (the game's template blobs) come from, so nothing the builder
// cannot edit is ever lost. 404 until a base image is imported.

import { For, Show, createSignal, onMount } from "solid-js";
import { Panel } from "../stats/Layout";
import { getHopper, putHopper } from "./api";
import {
  MAX_NAME,
  MAX_RECORDS,
  MAX_STRING,
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

  onMount(() => void load());

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
                      <span class="adm-list-name">{h.name || "(unnamed)"}</span>
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
                  <div class="adm-row">
                    <label class="adm-field adm-grow">
                      <span>Name (ASCII)</span>
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
                    <span class="adm-field-label">Party size ranges</span>
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
                        <th>id</th>
                        <th class="adm-grow">string</th>
                        <th>flags</th>
                        <th>dword 1</th>
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
                  <button class="s-btn" disabled={h().records.length >= MAX_RECORDS} onClick={addRecord}>
                    Add an entry (copies the last)
                  </button>
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
