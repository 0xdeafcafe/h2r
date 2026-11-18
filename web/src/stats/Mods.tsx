// The mod repository: the browser (`/stats/mods`, newest or most
// downloaded, filtered by base map, category and text, paged with the
// server's cursors) and one mod's page (`/stats/mods/:id`). Read-only but
// for Report; mods are downloaded in the game. The rules are in lib/mods.ts.

import { For, Show, createEffect, createMemo, createResource, createSignal, on } from "solid-js";
import { A, useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { mapTitle } from "../state/gameart";
import { seedPackages } from "../state/maps";
import { formatBytes, lookupFromResponse, parseMapSummaries, type MapDetail, type MapSummary } from "../lib/maps";
import {
  DOWNLOAD_NOTE, MOD_CATEGORIES, MOD_SORTS, baseOptions, categoryLabel, creditBeyondAuthor, cursorCurrent, cursorNext, cursorPage,
  cursorPrev, cursorStart, downloadsText, type CursorStack, modHref, modListParams, modMissingText, modQueryFromSearch, modQueryKey, modSearchParams,
  modsHref, shortSha, type ModQuery,
} from "../lib/mods";
import { reportMap } from "../admin/content";
import MapPicture from "../components/MapPicture";
import { Crumbs, Panel, Tabs, Title, playerHref } from "./Layout";
import { dotDate, longDate, num } from "./fmt";
import "../styles/mods.css";

const CRUMB: [string, string] = ["Mods", "/stats/mods"];

function NoPicture(props: { map: string }) {
  return <span class="s-mod-nopic">{mapTitle(props.map)}</span>;
}

/** The menu column. The pills are lit by the query, not the path: every
 *  one of them is /stats/mods. */
function ModSide(props: { query?: ModQuery }) {
  const q = () => props.query ?? { sort: "new" as const };
  const plain = () => !!props.query && !q().base && !q().category && !q().q;
  const pills = (): Array<[string, string, boolean]> => {
    const me = identity.me();
    return [
      ["Newest Mods", modsHref({ sort: "new" }), plain() && !q().author && q().sort === "new"],
      ["Most Downloaded", modsHref({ sort: "popular" }), plain() && !q().author && q().sort === "popular"],
      me ? ["My Mods", modsHref({ sort: "new", author: me.gamertag }), plain() && q().author?.toLowerCase() === me.gamertag.toLowerCase()] : ["Sign Up for h2 live", "/dash/signup", false],
    ];
  };
  return (
    <aside class="s-left">
      <Title>Mods</Title>
      <nav class="s-pills" aria-label="Mods">
        <For each={pills()}>{([label, href, on]) => <A href={href} activeClass="" classList={{ active: on }} aria-current={on ? "page" : undefined}>{label}</A>}</For>
      </nav>
      <Panel title="Categories" sub="browse by what a mod changes">
        <ul class="s-mini-list s-mod-cats">
          <For each={[...MOD_CATEGORIES]}>
            {(c) => <li classList={{ on: q().category === c }}><A href={modsHref({ ...q(), category: c })}>{categoryLabel(c)}</A></li>}
          </For>
        </ul>
      </Panel>
      <Panel title="Getting Mods" sub="downloads happen on your console">
        <p class="s-pad s-mod-note">{DOWNLOAD_NOTE}</p>
      </Panel>
    </aside>
  );
}

export default function Mods() {
  const [sp, setSp] = useSearchParams();
  const query = createMemo(() => modQueryFromSearch(sp as Record<string, string | string[] | undefined>), undefined, { equals: (a, b) => modQueryKey(a) === modQueryKey(b) });
  // The cursor stack belongs to one query; a new query starts on page one.
  const [stack, setStack] = createSignal({ key: modQueryKey(query()), c: cursorStart() });
  const cursors = () => (stack().key === modQueryKey(query()) ? stack().c : cursorStart());
  const setCursors = (f: (c: CursorStack) => CursorStack) => setStack({ key: modQueryKey(query()), c: f(cursors()) });
  const [seen, setSeen] = createSignal<string[]>([]);
  const [text, setText] = createSignal(query().q ?? "");
  createEffect(on(() => query().q, (q) => setText(q ?? "")));

  const [page] = createResource(
    () => ({ q: query(), cursor: cursorCurrent(cursors()), api: live.server().api }),
    async ({ q, cursor }) => {
      const p = await live.api().maps(modListParams(q, cursor));
      const items = parseMapSummaries(p.items);
      seedPackages(items);
      setSeen((s) => [...new Set([...s, ...items.map((m) => m.base.map.toLowerCase())])]);
      return { items, next: p.next };
    },
  );
  const items = () => page.latest?.items ?? [];
  const set = (q: Partial<ModQuery>) => setSp(modSearchParams({ ...query(), ...q }));
  const bases = createMemo(() => baseOptions(seen(), query().base, mapTitle));
  const filtered = () => !!(query().base || query().category || query().author || query().q);

  return (
    <>
      <Crumbs trail={[["Home", "/stats"], [CRUMB[0]]]} />
      <div class="s-cols">
        <ModSide query={query()} />
        <div class="s-right">
          <Title class="s-title--cap" help="Packages players have published on this server. Each one is the changes to one retail map; your own disc supplies the rest.">Mod Repository</Title>
          <Tabs label="Sort" active={query().sort} tabs={MOD_SORTS.map(([id, label]) => ({ id, label, href: modsHref({ ...query(), sort: id }) }))} />
          <div class="s-matte s-mods-wrap">
            <form class="s-toolbar s-mod-filters" role="search" onSubmit={(e) => { e.preventDefault(); set({ q: text().trim() || undefined }); }}>
              <label for="s-mod-base">base map</label>
              <select id="s-mod-base" value={query().base ?? ""} onChange={(e) => set({ base: e.currentTarget.value || undefined })}>
                <option value="">All maps</option>
                <For each={bases()}>{(b) => <option value={b.map}>{b.title}</option>}</For>
              </select>
              <label for="s-mod-cat">category</label>
              <select id="s-mod-cat" value={query().category ?? ""} onChange={(e) => set({ category: (e.currentTarget.value || undefined) as ModQuery["category"] })}>
                <option value="">All categories</option>
                <For each={[...MOD_CATEGORIES]}>{(c) => <option value={c}>{categoryLabel(c)}</option>}</For>
              </select>
              <label for="s-mod-q" class="sr-only">Search mods</label>
              <input id="s-mod-q" type="search" placeholder="Search mods" value={text()} onInput={(e) => setText(e.currentTarget.value)} />
              <button type="submit" class="s-btn">search</button>
              <Show when={query().author}>{(a) => <span class="s-mod-author">by <b>{a()}</b></span>}</Show>
              <Show when={filtered()}><A class="s-mod-clear" href={modsHref({ sort: query().sort })}>clear filters</A></Show>
            </form>
            <div class="s-grid-wrap">
              <div class="s-grid-pager">
                <ModPager page={cursorPage(cursors())} count={items().length} hasNext={!!page.latest?.next}
                  onPrev={() => setCursors(cursorPrev)} onNext={() => setCursors((c) => cursorNext(c, page.latest?.next ?? null))} />
              </div>
              <Show when={!page.error} fallback={<p class="s-empty s-bad">Couldn't load mods: {errorText(page.error as unknown)}</p>}>
                <Show when={page.latest} fallback={<p class="s-loading">Loading mods…</p>}>
                  <ul class="s-mods" classList={{ "s-mods--busy": page.loading }}>
                    <For each={items()} fallback={<li class="s-empty">{filtered() ? "No mods match. Try fewer filters." : cursorPage(cursors()) ? "No more mods." : "Nobody has published a mod on this server yet."}</li>}>
                      {(m) => <ModRow m={m} query={query()} />}
                    </For>
                  </ul>
                </Show>
              </Show>
              <Show when={page.latest?.next || cursorPage(cursors()) > 0}>
                <div class="s-grid-pager s-grid-pager--foot">
                  <ModPager page={cursorPage(cursors())} count={items().length} hasNext={!!page.latest?.next}
                    onPrev={() => setCursors(cursorPrev)} onNext={() => { setCursors((c) => cursorNext(c, page.latest?.next ?? null)); scrollTo({ top: 0 }); }} />
                </div>
              </Show>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

function ModPager(props: { page: number; count: number; hasNext: boolean; onPrev: () => void; onNext: () => void }) {
  return (
    <div class="s-pager s-mod-pager">
      <button disabled={props.page === 0} onClick={props.onPrev}>&lt; Previous</button>
      <span class="s-pager-sep">|</span>
      <span>Page {props.page + 1}{props.count ? `, ${props.count} ${props.count === 1 ? "mod" : "mods"}` : ""}</span>
      <span class="s-pager-sep">|</span>
      <button disabled={!props.hasNext} onClick={props.onNext}>More mods &gt;</button>
    </div>
  );
}

function ModRow(props: { m: MapSummary; query: ModQuery }) {
  const m = () => props.m;
  return (
    <li class="s-mod">
      <A href={modHref(m().map_id)} class="s-mod-pic" tabIndex={-1} aria-hidden="true">
        <MapPicture map={m().base.map} mapId={m().map_id} fallback={<NoPicture map={m().base.map} />} />
      </A>
      <div class="s-mod-body">
        <div class="s-mod-head">
          <A href={modHref(m().map_id)} class="s-mod-name">{m().name}</A>
          <A href={modsHref({ ...props.query, category: m().category as ModQuery["category"] })} class="s-mod-tag">{categoryLabel(m().category)}</A>
        </div>
        <p class="s-mod-by">
          by <A href={playerHref(m().author.gamertag)}>{m().author.gamertag}</A>
          <Show when={creditBeyondAuthor(m())}>{(c) => <span class="s-dim">, made by {c()}</span>}</Show>
        </p>
        <Show when={m().description}><p class="s-mod-desc">{m().description}</p></Show>
        <ul class="s-mod-meta">
          <li>on <A href={modsHref({ ...props.query, base: m().base.map.toLowerCase() })}>{mapTitle(m().base.map)}</A></li>
          <li>{formatBytes(m().size)}</li>
          <li>{downloadsText(m().downloads)}</li>
          <li title={longDate(m().published_at)}>{dotDate(m().published_at)}</li>
        </ul>
      </div>
    </li>
  );
}

// ---- one mod ----

export function ModPage() {
  const params = useParams();
  const [lookup] = createResource(
    () => ({ id: params.id ?? "", api: live.server().api }),
    // No token: a token here counts a download, which is the console's job.
    async ({ id }) => {
      const r = await live.api().mapPackage(id).catch(() => ({ status: 0, body: undefined }));
      const l = lookupFromResponse(r.status, r.body);
      if (l.state === "ok") seedPackages([l.map]);
      return l;
    },
  );
  const mod = () => { const l = lookup.latest; return l?.state === "ok" ? l.map : null; };
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], CRUMB, [mod()?.name ?? "Mod"]]} />
      <div class="s-cols">
        <ModSide />
        <div class="s-right">
          <Show when={lookup.latest} fallback={<div class="s-loading">Loading mod…</div>}>
            {(l) => (
              <Show when={mod()} fallback={<ModMissing state={l().state as "unknown" | "gone" | "error"} />}>
                {(m) => <ModDetail m={m()} missing={(l() as { missing?: string[] }).missing ?? []} />}
              </Show>
            )}
          </Show>
        </div>
      </div>
    </>
  );
}

function ModMissing(props: { state: "unknown" | "gone" | "error" }) {
  const t = () => modMissingText(props.state === "unknown" || props.state === "gone" ? props.state : "error");
  return (
    <>
      <Title class="s-title--cap" back={["see all mods", "/stats/mods"]}>{t().title}</Title>
      <p class="s-empty">{t().text}</p>
    </>
  );
}

function ModDetail(props: { m: MapDetail; missing: string[] }) {
  const m = () => props.m;
  const navigate = useNavigate();
  const [said, setSaid] = createSignal<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = createSignal(false);
  async function report() {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk) return;
    const reason = typeof prompt === "function" ? prompt(`Why should the admins look at “${m().name}”?`) : null;
    if (reason === null || !reason.trim()) return;
    setBusy(true);
    try {
      await reportMap(live.server().api, wk.id, me, m().map_id, reason);
      setSaid({ ok: true, text: "Reported. Thanks, the admins will take a look." });
    } catch (e) {
      setSaid({ ok: false, text: `Couldn't report: ${e instanceof Error ? e.message : String(e)}` });
    } finally {
      setBusy(false);
    }
  }
  const credit = () => creditBeyondAuthor(m());
  return (
    <>
      <Title class="s-title--cap" back={["see all mods", "/stats/mods"]}>{m().name}</Title>
      <div class="s-matte s-mod-page">
        <div class="s-mod-hero">
          <MapPicture map={m().base.map} mapId={m().map_id} class="s-mod-hero-img" fallback={<NoPicture map={m().base.map} />} />
        </div>
        <div class="s-mod-top">
          <div class="s-mod-about">
            <p class="s-mod-by s-mod-by--big">
              by <A href={playerHref(m().author.gamertag)}>{m().author.gamertag}</A>
              <Show when={credit()}>{(c) => <>, made by <b>{c()}</b></>}</Show>
            </p>
            <Show when={m().description} fallback={<p class="s-dim s-mod-desc-full">No description.</p>}>
              <p class="s-mod-desc-full">{m().description}</p>
            </Show>
            <Show when={m().source_url}>
              {(u) => <p class="s-mod-source">From <a href={u()} target="_blank" rel="noopener noreferrer nofollow">{new URL(u()).host}</a></p>}
            </Show>
          </div>
          <dl class="s-facts s-mod-facts">
            <div><dt>Category</dt><dd><A href={modsHref({ sort: "new", category: m().category as ModQuery["category"] })}>{categoryLabel(m().category)}</A></dd></div>
            <div><dt>Download</dt><dd>{formatBytes(m().size)}</dd></div>
            <div><dt>Downloads</dt><dd>{num(m().downloads)}</dd></div>
            <div><dt>Published</dt><dd title={longDate(m().published_at)}>{dotDate(m().published_at)}</dd></div>
            <Show when={credit()}><div><dt>Original author</dt><dd>{credit()}</dd></div></Show>
            <Show when={m().geometry}><div><dt>Geometry</dt><dd>changed</dd></div></Show>
          </dl>
        </div>
        <div class="s-mod-get">
          <b>How to get it</b>
          <p>{DOWNLOAD_NOTE}</p>
          <Show when={props.missing.length}><p class="s-bad">The server is missing {props.missing.length} of this mod's files, so it can't be downloaded right now.</p></Show>
        </div>
      </div>
      <Title class="s-title--rule">Base Map</Title>
      <div class="s-mod-base">
        <A href={modsHref({ sort: "new", base: m().base.map.toLowerCase() })} class="s-mod-pic">
          <MapPicture map={m().base.map} fallback={<NoPicture map={m().base.map} />} />
        </A>
        <div>
          <A href={modsHref({ sort: "new", base: m().base.map.toLowerCase() })} class="s-mod-name">{mapTitle(m().base.map)}</A>
          <p class="s-dim">{m().base.derived ? "Built on another package of this map." : "Built on the retail map from your disc."}</p>
          <Show when={m().base.sha256}>{(h) => <p class="s-dim">base <code title={h()}>{shortSha(h())}</code></p>}</Show>
        </div>
      </div>
      <Title class="s-title--rule">Files</Title>
      <div class="s-grid-wrap s-mod-layers">
        <table class="s-grid s-grid--roomy">
          <thead><tr><th>Kind</th><th>SHA-256</th><th class="num">Size</th></tr></thead>
          <tbody>
            <For each={m().layers} fallback={<tr><td colspan="3" class="s-dim">No files listed.</td></tr>}>
              {(l) => (
                <tr classList={{ gone: props.missing.includes(l.sha256) }}>
                  <td>{l.kind === "pack" ? "Pack" : "Patch"}</td>
                  <td><code title={l.sha256}>{shortSha(l.sha256)}</code>{props.missing.includes(l.sha256) ? " (missing)" : ""}</td>
                  <td class="num">{formatBytes(l.size)}</td>
                </tr>
              )}
            </For>
            <tr class="s-mod-total"><td colspan="2"><b>To download</b></td><td class="num"><b>{formatBytes(m().size)}</b></td></tr>
          </tbody>
        </table>
      </div>
      <div class="s-mod-report">
        <Show when={identity.me()} fallback={<span class="s-dim">Something wrong with this mod? <A href="/dash/signin">Sign in</A> to report it.</span>}>
          <Show when={!said()?.ok}>
            <button type="button" class="s-btn" disabled={busy()} onClick={() => void report()}>Report this mod</button>
          </Show>
          <Show when={said()}>{(s) => <span classList={{ "s-bad": !s().ok, "s-dim": s().ok }}>{s().text}</span>}</Show>
        </Show>
        <button type="button" class="s-linkbtn" onClick={() => navigate(modsHref({ sort: "new", base: m().base.map.toLowerCase() }))}>More mods on {mapTitle(m().base.map)}</button>
      </div>
    </>
  );
}
