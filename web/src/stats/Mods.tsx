import { For, Show, createEffect, createMemo, createResource, createSignal, on } from "solid-js";
import { A, useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { mapTitle } from "../state/gameart";
import { seedPackages } from "../state/maps";
import { formatBytes, lookupFromResponse, packImageUrl, type MapDetail, type MapImage } from "../lib/maps";
import {
  DOWNLOAD_NOTE, MOD_CATEGORIES, MOD_SORTS, baseOptions, categoryLabel, modMakers, releasedText, MODS_PER_PAGE,
  modHref, modMissingText, modQueryFromSearch, modQueryKey, modSearchParams,
  modsHref, shortSha, downloadsText, type ModQuery,
} from "../lib/mods";
import { mapCount, packAdvisoryText, packHref, packPath, memberName, packCoverSources, packDownloads, memberCoverSources, galleryUrl, parseModPack, filterPacks, type ModPack } from "../lib/modPacks";
import ModPicture, { ModPhoto } from "../components/ModPicture";
import { PackMetadataForm, ScreenshotForm } from "./ModForms";
import { ApiError } from "../api/http";
import { reportPack, reportMap } from "../admin/content";
import MapPicture from "../components/MapPicture";
import { statsPath } from "../routes";
import { HOME_CRUMB } from "./modes";
import { Crumbs, Panel, Tabs, Title, playerHref } from "./Layout";
import { dotDate, longDate, num } from "./fmt";
import "../styles/mods.css";

const CRUMB: [string, string] = ["Mods", statsPath("/mods")];

function NoPicture(props: { map: string }) {
  return <span class="s-mod-nopic">{mapTitle(props.map)}</span>;
}

/** The menu column. The pills are lit by the query, not the path: every
 *  one of them is the mods page. */
function ModSide(props: { query?: ModQuery }) {
  const q = () => props.query ?? { sort: "new" as const };
  const plain = () => !!props.query && !q().base && !q().category && !q().q;
  const pills = (): Array<[string, string, boolean]> => {
    const me = identity.me();
    return [
      ["Newest Packs", modsHref({ sort: "new" }), plain() && !q().author && q().sort === "new"],
      ["Most Downloaded", modsHref({ sort: "popular" }), plain() && !q().author && q().sort === "popular"],
      me ? ["My Packs", modsHref({ sort: "new", author: me.gamertag }), plain() && q().author?.toLowerCase() === me.gamertag.toLowerCase()] : ["Sign Up for h2 live", "/dash/signup", false],
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
  const [number, setNumber] = createSignal(0);
  const [text, setText] = createSignal(query().q ?? "");
  createEffect(on(query, () => { setNumber(0); setText(query().q ?? ""); }));
  const [catalog] = createResource(
    () => [live.server().api, identity.me()?.xuid, live.wellKnown()?.id] as const,
    async () => {
      const result = await live.api().modPacks();
      const packs = result.items.map(parseModPack).filter((p): p is ModPack => p !== null);
      seedPackages(packs.flatMap((p) => p.members));
      return packs;
    },
  );
  const allPacks = () => catalog.error || catalog.loading ? [] : catalog() ?? [];
  const packs = createMemo(() => filterPacks(allPacks(), query()));
  const items = () => packs().slice(number() * MODS_PER_PAGE, (number() + 1) * MODS_PER_PAGE);
  const hasNext = () => (number() + 1) * MODS_PER_PAGE < packs().length;
  const previous = () => setNumber((page) => Math.max(0, page - 1));
  const next = () => { if (hasNext()) setNumber((page) => page + 1); };
  const set = (q: Partial<ModQuery>) => setSp(modSearchParams({ ...query(), ...q }));
  const bases = createMemo(() => baseOptions(allPacks().flatMap((p) => p.members.map((m) => m.base.map)), query().base, mapTitle));
  const filtered = () => !!(query().base || query().category || query().author || query().q);

  return (
    <>
      <Crumbs trail={[HOME_CRUMB, [CRUMB[0]]]} />
      <div class="s-cols">
        <ModSide query={query()} />
        <div class="s-right">
          <Title class="s-title--cap" help="Browse mod packs by author, category or base map. Open a pack to see its maps, screenshots and source.">Mod Repository</Title>
          <Tabs label="Sort" active={query().sort} tabs={MOD_SORTS.map(([id, label]) => ({ id, label, href: modsHref({ ...query(), sort: id }) }))} />
          <div class="s-matte s-mods-wrap">
            <form class="s-toolbar s-mod-filters" role="search" onSubmit={(e) => { e.preventDefault(); set({ q: text().trim() || undefined }); }}>
              <label for="s-mod-base">Base map</label>
              <select id="s-mod-base" onChange={(e) => set({ base: e.currentTarget.value || undefined })}>
                <option value="" selected={!query().base}>All maps</option>
                <For each={bases()}>{(b) => <option value={b.map} selected={query().base === b.map}>{b.title}</option>}</For>
              </select>
              <label for="s-mod-cat">Category</label>
              <select id="s-mod-cat" onChange={(e) => set({ category: (e.currentTarget.value || undefined) as ModQuery["category"] })}>
                <option value="" selected={!query().category}>All categories</option>
                <For each={[...MOD_CATEGORIES]}>{(c) => <option value={c} selected={query().category === c}>{categoryLabel(c)}</option>}</For>
              </select>
              <label for="s-mod-q" class="sr-only">Search mods</label>
              <input id="s-mod-q" type="search" placeholder="Search mods" value={text()} onInput={(e) => setText(e.currentTarget.value)} />
              <button type="submit" class="s-btn">Search</button>
              <Show when={query().author}>{(a) => <span class="s-mod-author">Uploaded by <b>{a()}</b></span>}</Show>
              <Show when={filtered()}><A class="s-mod-clear" href={modsHref({ sort: query().sort })}>clear filters</A></Show>
            </form>
            <div class="s-grid-wrap">
              <div class="s-grid-pager">
                <ModPager page={number()} count={items().length} hasNext={hasNext()}
                  onPrev={previous} onNext={next} />
              </div>
              <Show when={!catalog.error} fallback={<p class="s-empty s-bad">Couldn't load mods: {errorText(catalog.error as unknown)}</p>}>
                <Show when={catalog()} fallback={<p class="s-loading">Loading mods…</p>}>
                  <ul class="s-mods" classList={{ "s-mods--busy": catalog.loading }}>
                    <For each={items()} fallback={<li class="s-empty">{filtered() ? "No packs match. Try fewer filters." : number() ? "No more packs." : "Nobody has published a mod pack on this server yet."}</li>}>
                      {(m) => <PackRow pack={m} />}
                    </For>
                  </ul>
                </Show>
              </Show>
              <Show when={hasNext() || number() > 0}>
                <div class="s-grid-pager s-grid-pager--foot">
                  <ModPager page={number()} count={items().length} hasNext={hasNext()}
                    onPrev={previous} onNext={() => { next(); scrollTo({ top: 0 }); }} />
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
      <span>Page {props.page + 1}{props.count ? `, ${props.count} ${props.count === 1 ? "pack" : "packs"}` : ""}</span>
      <span class="s-pager-sep">|</span>
      <button disabled={!props.hasNext} onClick={props.onNext}>More packs &gt;</button>
    </div>
  );
}

function PackMembers(props: { pack: ModPack }) {
  return <ul class="s-pack-members"><For each={props.pack.members}>{(member) =>
    <li>
      <A href={modHref(member.map_id)}>{memberName(props.pack, member)}</A>
      <span>{mapTitle(member.base.map)} · {formatBytes(member.size)}</span>
    </li>
  }</For></ul>;
}

export function PackRow(props: { pack: ModPack }) {
  const p = () => props.pack;
  return <li class="s-mod">
    <A href={packHref(p().id)} class="s-mod-pic" tabindex="-1" aria-hidden="true">
      <ModPicture sources={packCoverSources(live.server().api, p())} base={p().members[0]?.base.map} name={p().name} />
    </A>
    <div class="s-mod-body">
      <div class="s-mod-head">
        <A href={packHref(p().id)} class="s-mod-name">{p().name}</A>
        <Show when={p().version}>{(v) => <span class="s-mod-tag">{v()}</span>}</Show>
      </div>
      <PackByline pack={p()} />
      <Show when={p().description}>{(d) => <p class="s-mod-desc">{d()}</p>}</Show>
      <ul class="s-mod-meta">
        <li>{mapCount(p().members.length)}</li>
        <Show when={p().year}>{(y) => <li>{y()}</li>}</Show>
        <Show when={p().advisory}>{(a) => <li class="s-pack-advisory">{packAdvisoryText(a())}</li>}</Show>
        <Show when={p().source_url}>{(u) => <li><a href={u()} target="_blank" rel="noopener noreferrer nofollow">Source</a></li>}</Show>
        <li>{formatBytes(p().size)}</li>
        <li>{downloadsText(packDownloads(p()))}</li>
      </ul>
      <details class="s-pack-expand"><summary>View maps</summary><PackMembers pack={p()} /></details>
    </div>
  </li>;
}

function PackByline(props: { pack: ModPack }) {
  return <p class="s-mod-by">
    <Show when={props.pack.made_by}>{(madeBy) => <>Made by <b>{madeBy()}</b>{" · "}</>}</Show>
    Uploaded by <A href={playerHref(props.pack.uploader.gamertag)}>{props.pack.uploader.gamertag}</A>
  </p>;
}

export function ModPackPage() {
  const params = useParams();
  const [pack, { refetch }] = createResource(
    () => ({ id: params.id ?? "", server: live.server().api, reader: identity.me()?.xuid, audience: live.wellKnown()?.id }),
    async ({ id }) => {
      const result = await live.api().modPack(id);
      const p = parseModPack(result.pack);
      if (!p) throw new Error("The server returned an unreadable pack.");
      return p;
    },
  );
  const loaded = () => !pack.error && !pack.loading ? pack() : null;
  return <>
    <Crumbs trail={[HOME_CRUMB, CRUMB, [loaded()?.name ?? "Mod Pack"]]} />
    <div class="s-cols"><ModSide /><div class="s-right">
      <Show when={!pack.error} fallback={<p class="s-empty s-bad">{pack.error instanceof ApiError && pack.error.status === 410 ? "This pack was taken down." : "Couldn’t load this pack."}</p>}>
        <Show when={loaded()} keyed fallback={<p class="s-loading">Loading pack…</p>}>
          {(p) => <PackDetail pack={p} changed={() => void refetch()} />}
        </Show>
      </Show>
    </div></div>
  </>;
}

export function PackDetail(props: { pack: ModPack; changed: () => void }) {
  const p = props.pack;
  const [reason, setReason] = createSignal("");
  const [note, setNote] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [reported, setReported] = createSignal(false);
  async function report(e: SubmitEvent) {
    e.preventDefault();
    const me = identity.me(), audience = live.wellKnown()?.id;
    if (!me || !audience || !reason().trim()) return;
    setBusy(true);
    try {
      await reportPack(live.server().api, audience, me, p.id, reason());
      setReported(true);
      setNote("Report sent.");
    } catch (e) {
      setNote(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  return <>
    <Title class="s-title--cap" back={["see all packs", CRUMB[1]]}>{p.name}</Title>
    <div class="s-matte s-mod-page">
      <div class="s-mod-hero"><ModPicture sources={packCoverSources(live.server().api, p)} base={p.members[0]?.base.map} name={p.name} /></div>
      <div class="s-mod-top">
        <div><PackByline pack={p} />
          <Show when={p.description}>{(d) => <p class="s-mod-desc-full">{d()}</p>}</Show>
          <Show when={p.source_url}>{(url) => <p class="s-mod-source">Source: <a href={url()} target="_blank" rel="noopener noreferrer nofollow">{new URL(url()).host}</a></p>}</Show>
        </div>
        <dl class="s-facts s-mod-facts">
          <Show when={p.version}>{(v) => <div><dt>Version</dt><dd>{v()}</dd></div>}</Show>
          <Show when={p.year}>{(y) => <div><dt>Release year</dt><dd>{y()}</dd></div>}</Show>
          <Show when={p.advisory}>{(a) => <div><dt>Advisory</dt><dd class="s-pack-advisory">{packAdvisoryText(a())}</dd></div>}</Show>
          <div><dt>Maps</dt><dd>{p.members.length}</dd></div>
          <div><dt>Pack size</dt><dd>{formatBytes(p.size)}</dd></div>
        </dl>
      </div>
      <div class="s-mod-get"><b>Install in the game</b><p>Select this pack in the Xbox Live Mods menu. Your disc supplies the base maps.</p></div>
    </div>
    <Title class="s-title--rule">Included maps</Title><PackMembers pack={p} />
    <Show when={p.images.length} fallback={<MemberScreenshots pack={p} />}>
      <Title class="s-title--rule">Pack screenshots</Title>
      <ModShots images={p.images} url={(i) => galleryUrl(live.server().api, packPath(p.id), p.images[i]!, i)} />
    </Show>
    <Show when={identity.me()?.xuid === p.uploader.xuid}>
      <details class="s-pack-tools"><summary>Edit pack</summary><PackMetadataForm pack={p} changed={props.changed} /></details>
      <ScreenshotForm path={packPath(p.id)} changed={props.changed} />
    </Show>
    <div class="s-mod-report">
      <Show when={identity.me()} fallback={<span><A href="/dash/signin">Sign in</A> to report this pack.</span>}>
        <Show when={!reported()}><details><summary>Report this pack</summary>
          <form onSubmit={(e) => void report(e)}><label>Reason <input required maxLength={500} value={reason()} onInput={(e) => setReason(e.currentTarget.value)} /></label>
            {" "}<button type="submit" class="s-btn" disabled={busy() || !reason().trim()}>Send report</button>
          </form>
        </details></Show>
        <span role="status">{note()}</span>
      </Show>
    </div>
  </>;
}

/** A pack without a dedicated gallery can still show its member shots.
 * Show one per map, linking through to the map's complete gallery. */
function MemberScreenshots(props: { pack: ModPack }) {
  const covers = () => props.pack.members.map((member) => ({
    member, sources: memberCoverSources(live.server().api, member),
  })).filter((row) => row.sources.length);
  return <Show when={covers().length}>
    <Title class="s-title--rule">Map screenshots</Title>
    <ul class="s-mod-shots"><For each={covers()}>{({ member, sources }) => <li>
      <A href={modHref(member.map_id)} class="s-member-shot">
        <ModPicture name={member.name} sources={sources} base={member.base.map} />
      </A>
      <p class="s-mod-shot-cap"><A href={modHref(member.map_id)}>{memberName(props.pack, member)}</A></p>
    </li>}</For></ul>
  </Show>;
}

// ---- one mod ----

export function ModPage() {
  const params = useParams();
  const [lookup, { refetch }] = createResource(
    () => ({ id: params.id ?? "", api: live.server().api, reader: identity.me()?.xuid, audience: live.wellKnown()?.id }),
    // Public manifests load anonymously so viewing a page does not count a download.
    async ({ id }) => {
      const r = await live.api().mapPackage(id).catch(() => ({ status: 0, body: undefined }));
      const l = lookupFromResponse(r.status, r.body);
      if (l.state === "ok") seedPackages([l.map]);
      return l;
    },
  );
  const mod = () => { const l = lookup.loading ? undefined : lookup(); return l?.state === "ok" ? l.map : null; };
  return (
    <>
      <Crumbs trail={[HOME_CRUMB, CRUMB, [mod()?.name ?? "Mod"]]} />
      <div class="s-cols">
        <ModSide />
        <div class="s-right">
          <Show when={!lookup.loading && lookup()} fallback={<div class="s-loading">Loading mod…</div>}>
            {(l) => (
              <Show when={mod()} keyed fallback={<ModMissing state={l().state as "unknown" | "gone" | "error"} />}>
                {(m) => <ModDetail m={m} missing={(l() as { missing?: string[] }).missing ?? []} changed={() => void refetch()} />}
              </Show>
            )}
          </Show>
        </div>
      </div>
    </>
  );
}

/** The gallery screenshots, each with its caption and credit; one
 *  that fails to load is left out. */
function ModShots(props: { images: MapImage[]; url: (n: number) => string }) {
  const [broken, setBroken] = createSignal<number[]>([]);
  return (
    <ul class="s-mod-shots">
      <For each={props.images}>
        {(img, i) => (
          <Show when={!broken().includes(i())}>
            <li>
              <ModPhoto src={props.url(i())} alt={img.caption ?? "Mod screenshot"} fullSize failed={() => setBroken((b) => [...b, i()])} />
              <Show when={img.caption || img.credit || img.source_url}>
                <p class="s-mod-shot-cap">
                  {img.caption}
                  <Show when={img.credit || img.source_url}>
                    <span class="s-dim"> {img.caption ? "· " : ""}{img.credit ? `image: ${img.credit}` : "image"}
                      <Show when={img.source_url}>{(u) => <> (<a href={u()} target="_blank" rel="noopener noreferrer nofollow">{new URL(u()).host}</a>)</>}</Show>
                    </span>
                  </Show>
                </p>
              </Show>
            </li>
          </Show>
        )}
      </For>
    </ul>
  );
}

function ModMissing(props: { state: "unknown" | "gone" | "error" }) {
  const t = () => modMissingText(props.state === "unknown" || props.state === "gone" ? props.state : "error");
  return (
    <>
      <Title class="s-title--cap" back={["see all mods", statsPath("/mods")]}>{t().title}</Title>
      <p class="s-empty">{t().text}</p>
    </>
  );
}

function ModDetail(props: { m: MapDetail; missing: string[]; changed: () => void }) {
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
  const makers = () => modMakers(m()) ?? (m().pack_info?.authors.join(", ") || null);
  const credits = () => m().credits;
  return (
    <>
      <Title class="s-title--cap" back={["see all mods", statsPath("/mods")]}>{m().name}</Title>
      <div class="s-matte s-mod-page">
        <div class="s-mod-hero">
          <ModPicture sources={memberCoverSources(live.server().api, m())} base={m().base.map} name={m().name} />
        </div>
        <div class="s-mod-top">
          <div class="s-mod-about">
            <p class="s-mod-by s-mod-by--big">
              <Show when={makers()}>{(c) => <>Made by <b>{c()}</b></>}</Show>
            </p>
            <p class="s-dim">Uploaded by <A href={playerHref(m().author.gamertag)}>{m().author.gamertag}</A></p>
            <Show when={m().description} fallback={<p class="s-dim s-mod-desc-full">No description.</p>}>
              <p class="s-mod-desc-full">{m().description}</p>
            </Show>
            <Show when={m().source_url}>
              {(u) => <p class="s-mod-source">From <a href={u()} target="_blank" rel="noopener noreferrer nofollow">{new URL(u()).host}</a></p>}
            </Show>
            <Show when={credits()?.notes}>{(n) => <p class="s-dim s-mod-credit-notes">{n()}</p>}</Show>
          </div>
          <dl class="s-facts s-mod-facts">
            <div><dt>Category</dt><dd><A href={modsHref({ sort: "new", category: m().category as ModQuery["category"] })}>{categoryLabel(m().category)}</A></dd></div>
            <div><dt>Download</dt><dd>{formatBytes(m().size)}</dd></div>
            <div><dt>Downloads</dt><dd>{num(m().downloads)}</dd></div>
            <div><dt>Published</dt><dd title={longDate(m().published_at)}>{dotDate(m().published_at)}</dd></div>
            <Show when={makers()}>{(c) => <div><dt>{(credits()?.authors.length ?? 0) > 1 ? "Authors" : "Author"}</dt><dd>{c()}</dd></div>}</Show>
            <Show when={m().mod_pack}>{(id) => <div><dt>Pack</dt><dd><A href={packHref(id())}>{m().pack_name ?? "View pack"}</A></dd></div>}</Show>
            <Show when={credits()?.release}>{(r) => <div><dt>Released as</dt><dd>{r()}</dd></div>}</Show>
            <Show when={credits()?.released}>{(d) => <div><dt>Released</dt><dd>{releasedText(d())}</dd></div>}</Show>
            <div><dt>Uploaded by</dt><dd><A href={playerHref(m().author.gamertag)}>{m().author.gamertag}</A></dd></div>
            <Show when={m().geometry}><div><dt>Geometry</dt><dd>changed</dd></div></Show>
          </dl>
        </div>
        <div class="s-mod-get">
          <b>How to get it</b>
          <p>{DOWNLOAD_NOTE}</p>
          <Show when={props.missing.length}><p class="s-bad">The server is missing {props.missing.length} of this mod's files, so it can't be downloaded right now.</p></Show>
        </div>
      </div>
      <Show when={identity.me()?.xuid === m().author.xuid}><ScreenshotForm path={`/maps/${m().map_id}`} changed={props.changed} /></Show>
      <Show when={m().images?.length}>
        <Title class="s-title--rule">Screenshots</Title>
        <ModShots images={m().images ?? []} url={(n) => galleryUrl(live.server().api, `/maps/${m().map_id}`, m().images![n]!, n)} />
      </Show>
      <Show when={(m().report?.lines ?? m().contents ?? []).length}>
        <Title class="s-title--rule" help="Read off the mod's map against the retail map it is built on: the tags it adds or changes, the references it swaps, the stats it changes and what it places.">What's Inside</Title>
        <ul class="s-mod-contents">
          <For each={m().report?.lines ?? m().contents ?? []}>{(l) => <li>{l}</li>}</For>
        </ul>
      </Show>
      <Show when={m().pack_info}>
        {(p) => (
          <>
            <Title class="s-title--rule">From {p().name}</Title>
            <div class="s-mod-pack">
              <Show when={p().authors.length || p().released}>
                <p class="s-dim">
                  {p().authors.length ? `by ${p().authors.join(", ")}` : ""}
                  {p().authors.length && p().released ? " · " : ""}
                  {p().released ? releasedText(p().released!) : ""}
                </p>
              </Show>
              <Show when={p().description}>{(d) => <p class="s-mod-desc-full">{d()}</p>}</Show>
              <Show when={p().source_url}>
                {(u) => <p class="s-mod-source">From <a href={u()} target="_blank" rel="noopener noreferrer nofollow">{new URL(u()).host}</a></p>}
              </Show>
            </div>
            <Show when={p().images.length}>
              <ModShots images={p().images} url={(n) => packImageUrl(live.server().api, p().id, n)} />
            </Show>
          </>
        )}
      </Show>
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
