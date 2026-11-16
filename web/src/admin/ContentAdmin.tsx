// The Admin area's "Custom content" tab (docs/maps.md §5): map packages and
// stored blobs with their state and report counts, hide / unhide / delete /
// block buttons, the reports, and the per-account upload switch. The calls
// and the parsing are in ./content.ts.

import { For, Show, createSignal, onMount } from "solid-js";
import { Panel } from "../stats/Layout";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { mapTitle } from "../state/gameart";
import { formatBytes } from "../lib/maps";
import { ago } from "../lib/format";
import { actionsFor, fetchContent, moderate, reportsFor, setUploads, shortHash, type ContentList, type ModerateAction } from "./content";

export default function ContentAdmin() {
  const [content, setContent] = createSignal<ContentList | null>(null);
  const [missing, setMissing] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal("");
  const [tag, setTag] = createSignal("");

  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };

  async function load() {
    const c = ctx();
    if (!c) return;
    try {
      const got = await fetchContent(c.api, c.aud, c.me);
      setMissing(got === null);
      setContent(got);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }
  onMount(() => void load());

  async function run(what: string, fn: (c: NonNullable<ReturnType<typeof ctx>>) => Promise<void>) {
    const c = ctx();
    if (!c || busy()) return;
    setBusy(true);
    setErr("");
    try {
      await fn(c);
      await load();
    } catch (e) {
      setErr(`couldn't ${what}: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(false);
    }
  }
  const act = (target: "map" | "content", id: string, a: ModerateAction) =>
    void run(a, (c) => moderate(c.api, c.aud, c.me, target, id, a));
  const uploads = (gamertag: string, allowed: boolean) =>
    void run(allowed ? "switch uploads on" : "switch uploads off", (c) => setUploads(c.api, c.aud, c.me, gamertag, allowed));

  const Buttons = (p: { target: "map" | "content"; id: string; state: Parameters<typeof actionsFor>[0] }) => (
    <span class="adm-rec-btns adm-mod-btns">
      <For each={actionsFor(p.state)}>{(a) => <button class="s-btn" disabled={busy()} onClick={() => act(p.target, p.id, a)}>{a}</button>}</For>
    </span>
  );

  return (
    <div class="adm-content">
      <Show when={!missing()} fallback={<Panel title="Custom content"><p>This server doesn't store custom content.</p></Panel>}>
        <Panel title="Map packages" sub="published custom and modded maps" right={<button class="s-btn" disabled={busy()} onClick={() => void load()}>Refresh</button>}>
          <Show when={content()?.maps.length} fallback={<p class="adm-preset-note">{content() ? "No packages yet." : "Loading…"}</p>}>
            <table class="adm-records adm-mod">
              <thead><tr><th class="adm-grow">Package</th><th>Author</th><th>Base</th><th>Size</th><th>State</th><th>Reports</th><th /></tr></thead>
              <tbody>
                <For each={content()!.maps}>
                  {(m) => (
                    <tr classList={{ off: m.state !== "ok" }}>
                      <td title={m.description ?? m.map_id}>{m.name} <span class="adm-mono">{shortHash(m.map_id)}</span></td>
                      <td>{m.author.gamertag}</td>
                      <td>{mapTitle(m.base.map)}</td>
                      <td>{formatBytes(m.size)}</td>
                      <td class={`adm-state adm-state--${m.state}`}>{m.state}</td>
                      <td title={reportsFor(content()!, "map", m.map_id).map((r) => `${r.by}: ${r.reason}`).join("\n")}>{m.reports}</td>
                      <td><Buttons target="map" id={m.map_id} state={m.state} /></td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </Panel>
        <Panel title="Stored content" sub="patch and pack blobs by sha256">
          <Show when={content()?.blobs.length} fallback={<p class="adm-preset-note">{content() ? "Nothing stored yet." : "Loading…"}</p>}>
            <table class="adm-records adm-mod">
              <thead><tr><th class="adm-grow">sha256</th><th>Uploader</th><th>Size</th><th>Stored</th><th>State</th><th>Reports</th><th /></tr></thead>
              <tbody>
                <For each={content()!.blobs}>
                  {(b) => (
                    <tr classList={{ off: b.state !== "ok" }}>
                      <td class="adm-mono" title={b.sha256}>{shortHash(b.sha256)}</td>
                      <td>{b.uploader.gamertag}</td>
                      <td>{formatBytes(b.size)}</td>
                      <td>{b.stored_at ? ago(b.stored_at) : "–"}</td>
                      <td class={`adm-state adm-state--${b.state}`}>{b.state}</td>
                      <td>{b.reports}</td>
                      <td><Buttons target="content" id={b.sha256} state={b.state} /></td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </Panel>
        <Panel title="Reports" sub="newest first">
          <Show when={content()?.reports.length} fallback={<p class="adm-preset-note">No reports.</p>}>
            <ul class="adm-reports">
              <For each={content()!.reports}>
                {(r) => {
                  const name = () => (r.target === "map" ? content()!.maps.find((m) => m.map_id === r.id)?.name : null) ?? shortHash(r.id);
                  return <li><b>{name()}</b> <span>{r.target === "map" ? "package" : "blob"} · by {r.by} · {r.at ? ago(r.at) : ""}</span><p>{r.reason || "(no reason given)"}</p></li>;
                }}
              </For>
            </ul>
          </Show>
        </Panel>
        <Panel title="Uploads" sub="accounts whose uploads are switched off">
          <Show when={content()?.uploads_off.length} fallback={<p class="adm-preset-note">Everyone may upload.</p>}>
            <ul class="adm-reports">
              <For each={content()!.uploads_off}>
                {(t) => <li><b>{t}</b> <button class="s-btn" disabled={busy()} onClick={() => uploads(t, true)}>allow uploads</button></li>}
              </For>
            </ul>
          </Show>
          <form class="adm-row" onSubmit={(e) => { e.preventDefault(); const t = tag().trim(); if (t) { uploads(t, false); setTag(""); } }}>
            <label class="adm-field">
              <span>Gamertag</span>
              <input value={tag()} onInput={(e) => setTag(e.currentTarget.value)} list="adm-upload-tags" />
            </label>
            <datalist id="adm-upload-tags"><For each={Object.values(live.state.accounts)}>{(a) => <option value={a.gamertag} />}</For></datalist>
            <button class="s-btn" type="submit" disabled={busy() || !tag().trim()}>Switch uploads off</button>
          </form>
        </Panel>
      </Show>
      <Show when={err()}><p class="adm-error">{err()}</p></Show>
    </div>
  );
}
