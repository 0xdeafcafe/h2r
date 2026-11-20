// The Admin area's "Custom content" tab (docs/game/maps.md §5): map packages and
// stored blobs with their state and report counts, hide / unhide / delete /
// block buttons, the reports, and the per-account upload switch. The calls
// and the parsing are in ./content.ts.

import { For, Show, createSignal, onMount } from "solid-js";
import { Panel } from "../stats/Layout";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { formatBytes } from "../lib/maps";
import ModPackTable, { ModerationButtons } from "./ModPackTable";
import { ago } from "../lib/format";
import { fetchContent, moderate, setUploads, shortHash, type ContentList, type ContentTarget, type ModerateAction } from "./content";

export default function ContentAdmin() {
  const [content, setContent] = createSignal<ContentList | null>(null);
  const [missing, setMissing] = createSignal(false);
  const [expanded, setExpanded] = createSignal<string | null>(null);
  const [showBlobs, setShowBlobs] = createSignal(false);
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
  const act = (target: ContentTarget, id: string, a: ModerateAction) =>
    void run(a, (c) => moderate(c.api, c.aud, c.me, target, id, a));
  const uploads = (gamertag: string, allowed: boolean) =>
    void run(allowed ? "switch uploads on" : "switch uploads off", (c) => setUploads(c.api, c.aud, c.me, gamertag, allowed));

  return (
    <div class="adm-content">
      <Show when={!missing()} fallback={<Panel title="Custom content"><p>This server doesn't store custom content.</p></Panel>}>
        <Panel title="Mod packs" right={<button class="s-btn" disabled={busy()} onClick={() => void load()}>Refresh</button>}>
          <Show when={content()} fallback={<p class="adm-preset-note">Loading…</p>}>
            {(c) => <Show when={c().packs.length || c().maps.length} fallback={<p class="adm-preset-note">No mods published yet.</p>}>
              <ModPackTable content={c()} api={live.server().api} expanded={expanded()} onExpand={setExpanded} busy={busy()} onAction={act} />
            </Show>}
          </Show>
        </Panel>
        <Panel title="Stored content" sub="patches, content packs and images by SHA-256">
          <Show when={content()?.blobs.length} fallback={<p class="adm-preset-note">{content() ? "Nothing stored yet." : "Loading…"}</p>}>
            <details class="adm-blob-details" onToggle={(e) => setShowBlobs(e.currentTarget.open)}>
              <summary>{content()!.blobs.length} {content()!.blobs.length === 1 ? "stored file" : "stored files"}</summary>
              <Show when={showBlobs()}>
                <table class="adm-records adm-mod">
                  <thead><tr><th class="adm-grow">SHA-256</th><th>Uploader</th><th>Size</th><th>Stored</th><th>State</th><th>Reports</th><th /></tr></thead>
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
                          <td><ModerationButtons target="content" id={b.sha256} state={b.state} busy={busy()} onAction={act} /></td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </Show>
            </details>
          </Show>
        </Panel>
        <Panel title="Mod reports" sub="packs, maps and stored content, newest first">
          <Show when={content()?.reports.length} fallback={<p class="adm-preset-note">No reports.</p>}>
            <ul class="adm-reports">
              <For each={content()!.reports}>
                {(r) => {
                  const name = () => {
                    if (r.target === "pack") return content()!.packs.find((pack) => pack.id === r.id)?.name ?? r.id;
                    if (r.target === "map") return content()!.maps.find((m) => m.map_id === r.id)?.name ?? shortHash(r.id);
                    return shortHash(r.id);
                  };
                  return <li><b>{name()}</b> <span>{r.target === "content" ? "blob" : r.target} · by {r.by}<Show when={r.at}> · {ago(r.at)}</Show></span><p>{r.reason || "(no reason given)"}</p></li>;
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
