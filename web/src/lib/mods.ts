// The mod repository's browser (`statsPath("/mods")`) and mod pages: the list
// query (`GET /v1/maps?sort=&base=&category=&q=&limit=&cursor=`), its state
// in the page URL, the client-side stack of `next` cursors for paging back,
// category labels and the small rules a mod page shows. Pure; the pages are
// in stats/Mods.tsx.

import type { MapSummary } from "./maps";
import { statsPath } from "../routes";

export { safeSourceUrl } from "./maps";

export const MOD_CATEGORIES = ["gameplay", "weapons", "vehicles", "visuals", "maps", "other"] as const;
export type ModCategory = (typeof MOD_CATEGORIES)[number];

const LABELS: Record<ModCategory, string> = {
  gameplay: "Gameplay",
  weapons: "Weapons",
  vehicles: "Vehicles",
  visuals: "Visuals",
  maps: "Map Changes",
  other: "Other",
};

export function isModCategory(v: unknown): v is ModCategory {
  return typeof v === "string" && (MOD_CATEGORIES as readonly string[]).includes(v);
}

/** A category's label; "Uncategorized" for none or one this page doesn't know. */
export function categoryLabel(c: string | null | undefined): string {
  return isModCategory(c) ? LABELS[c] : "Uncategorized";
}

export type ModSort = "new" | "popular";
export const MOD_SORTS: Array<[ModSort, string]> = [["new", "Newest"], ["popular", "Most Downloaded"]];

/** What the browser shows: the tab and the filters. */
export interface ModQuery { sort: ModSort; base?: string; category?: ModCategory; author?: string; q?: string }

type Params = Record<string, string | string[] | undefined> | URLSearchParams;

const one = (p: Params, k: string): string | undefined => {
  const v = p instanceof URLSearchParams ? p.get(k) ?? undefined : p[k];
  const s = Array.isArray(v) ? v[0] : v;
  return typeof s === "string" && s.trim() ? s.trim() : undefined;
};

/** The page URL's `?sort=&base=&category=&author=&q=`, read leniently: an unknown
 *  sort is "new", an unknown category is dropped, the base is lower-cased. */
export function modQueryFromSearch(p: Params): ModQuery {
  const sort = one(p, "sort") === "popular" ? "popular" : "new";
  const out: ModQuery = { sort };
  const base = one(p, "base");
  if (base) out.base = base.toLowerCase();
  const category = one(p, "category")?.toLowerCase();
  if (isModCategory(category)) out.category = category;
  const author = one(p, "author");
  if (author) out.author = author;
  const q = one(p, "q");
  if (q) out.q = q.slice(0, 100);
  return out;
}

/** The page URL's search params for a query (for `setSearchParams`): every
 *  key present, undefined clears it; the default sort is left out. */
export function modSearchParams(q: ModQuery): Record<string, string | undefined> {
  return { sort: q.sort === "new" ? undefined : q.sort, base: q.base || undefined, category: q.category || undefined, author: q.author || undefined, q: q.q || undefined };
}

/** A link to the browser with this query. */
export function modsHref(q: ModQuery): string {
  const s = new URLSearchParams();
  for (const [k, v] of Object.entries(modSearchParams(q))) if (v) s.set(k, v);
  const t = s.toString();
  return t ? `${statsPath("/mods")}?${t}` : statsPath("/mods");
}

export const MODS_PER_PAGE = 20;

/** The API's query for one page. */
export function modListParams(q: ModQuery, cursor: string | null, limit = MODS_PER_PAGE): Record<string, string | number | undefined> {
  return { sort: q.sort, base: q.base, category: q.category, author: q.author, q: q.q, limit, cursor: cursor ?? undefined };
}

/** Same query, ignoring cursors: when it changes the browser starts over. */
export function modQueryKey(q: ModQuery): string {
  return JSON.stringify([q.sort, q.base ?? "", q.category ?? "", q.author ?? "", q.q ?? ""]);
}

// ---- paging: the server only gives `next`, so earlier pages' cursors are
// kept here. `stack[i]` is the cursor that fetched page i (null for the first).

export interface CursorStack { stack: Array<string | null> }

export const cursorStart = (): CursorStack => ({ stack: [null] });
export const cursorCurrent = (c: CursorStack): string | null => c.stack[c.stack.length - 1] ?? null;
export const cursorPage = (c: CursorStack): number => c.stack.length - 1;

/** Forward to the page `next` fetches; no-op without one. */
export function cursorNext(c: CursorStack, next: string | null): CursorStack {
  return next === null ? c : { stack: [...c.stack, next] };
}

/** Back one page; the first page stays put. */
export function cursorPrev(c: CursorStack): CursorStack {
  return c.stack.length > 1 ? { stack: c.stack.slice(0, -1) } : c;
}

// ---- what a mod shows ----

/** The base maps the filter offers: the ones seen so far plus the one
 *  chosen, unique, in title order. */
export function baseOptions(seen: Iterable<string>, chosen: string | undefined, title: (m: string) => string): Array<{ map: string; title: string }> {
  const all = new Set<string>();
  for (const m of seen) if (m) all.add(m.toLowerCase());
  if (chosen) all.add(chosen.toLowerCase());
  return [...all].map((map) => ({ map, title: title(map) })).sort((a, b) => a.title.localeCompare(b.title) || a.map.localeCompare(b.map));
}

/** Who made the mod, on one line: the credited authors ("A", "A & B",
 *  "A, B & C"), else the one-line credit; null when nobody is credited. Never
 *  the uploader: a mod with no credit is by an unknown author, not by
 *  whoever published it. */
export function modMakers(m: Pick<MapSummary, "credit" | "credits">): string | null {
  const a = m.credits?.authors ?? [];
  if (a.length === 1) return a[0]!;
  if (a.length > 1) return `${a.slice(0, -1).join(", ")} & ${a[a.length - 1]}`;
  return m.credit?.trim() || null;
}

/** "2005", "March 2005", "14 March 2005" from a credit's `released`; the
 *  text as given when it isn't one of those shapes. */
export function releasedText(d: string): string {
  const m = /^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/.exec(d.trim());
  if (!m) return d;
  const months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
  const month = m[2] ? months[Number(m[2]) - 1] : undefined;
  if (!month) return m[1]!;
  return m[3] ? `${Number(m[3])} ${month} ${m[1]}` : `${month} ${m[1]}`;
}

/** "1 download", "1,204 downloads". */
export function downloadsText(n: number): string {
  return `${n.toLocaleString("en-US")} ${n === 1 ? "download" : "downloads"}`;
}

/** A layer's sha256, short: its first 12 hex. */
export const shortSha = (h: string) => h.slice(0, 12);

export const modHref = (id: string) => statsPath(`/mods/${encodeURIComponent(id)}`);

/** What the mod page says when the package can't be shown. */
export function modMissingText(state: "unknown" | "gone" | "error"): { title: string; text: string } {
  if (state === "unknown") return { title: "No Such Mod", text: "There's no mod with that id on this server." };
  if (state === "gone") return { title: "Mod Unavailable", text: "This mod was taken down by an admin." };
  return { title: "Mod Unavailable", text: "Couldn't reach the server for this mod. Try again in a moment." };
}

export const DOWNLOAD_NOTE =
  "Mods download in the game, from the Xbox Live menu. Only the changes are downloaded; your own disc's copy of the base map is the rest, so you need the base map installed.";
