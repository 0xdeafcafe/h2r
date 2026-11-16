// Moderating custom content (docs/game/maps.md §5): an admin lists the server's
// map packages and stored blobs with their state and report counts, hides,
// deletes or blocks them, reads the reports, and switches uploads off per
// account. Every call carries a `content.moderate` token (./token.ts),
// accepted only for admins. Parsing is defensive like ./server.ts.

import { ApiError } from "../api/http";
import type { LocalAccount } from "../state/account";
import { parseMapSummary, type MapSummary } from "../lib/maps";
import { issueToken } from "./token";

export const CONTENT_MODERATE = "content.moderate";

export type ContentState = "ok" | "hidden" | "deleted" | "blocked";
export type ModerateAction = "hide" | "unhide" | "delete" | "block" | "unblock";

export interface ModeratedMap extends MapSummary { state: ContentState; reports: number }

export interface ModeratedBlob {
  sha256: string;
  size: number;
  uploader: { xuid: string; gamertag: string };
  state: ContentState;
  reports: number;
  stored_at: string;
}

export interface ContentReport { target: "map" | "content"; id: string; by: string; reason: string; at: string }

export interface ContentList {
  maps: ModeratedMap[];
  blobs: ModeratedBlob[];
  reports: ContentReport[];
  /** Gamertags whose uploads are switched off. */
  uploads_off: string[];
  position: number | null;
}

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const list = (v: unknown): Record<string, unknown>[] => (Array.isArray(v) ? v.map(obj) : []);
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const state = (v: unknown): ContentState => (v === "hidden" || v === "deleted" || v === "blocked" ? v : "ok");

/** `GET /v1/admin/content`, dropping entries it can't read. */
export function parseContent(body: unknown): ContentList {
  const o = obj(body);
  const maps: ModeratedMap[] = [];
  for (const m of list(o.maps)) {
    const s = parseMapSummary(m);
    if (s) maps.push({ ...s, state: state(m.state), reports: num(m.reports) });
  }
  const blobs = list(o.blobs).filter((b) => /^[0-9a-f]{64}$/.test(str(b.sha256))).map((b) => ({
    sha256: str(b.sha256),
    size: num(b.size),
    uploader: { xuid: str(obj(b.uploader).xuid), gamertag: str(obj(b.uploader).gamertag) || "unknown" },
    state: state(b.state),
    reports: num(b.reports),
    stored_at: str(b.stored_at),
  }));
  const reports = list(o.reports).filter((r) => str(r.id)).map((r) => ({
    target: r.target === "content" ? "content" as const : "map" as const,
    id: str(r.id),
    by: str(r.by) || "unknown",
    reason: str(r.reason),
    at: str(r.at),
  })).sort((a, b) => b.at.localeCompare(a.at));
  const uploads_off = Array.isArray(o.uploads_off) ? o.uploads_off.filter((t): t is string => typeof t === "string") : [];
  return { maps, blobs, reports, uploads_off, position: typeof o.position === "number" ? o.position : null };
}

/** The actions that make sense from a state: hidden can be unhidden,
 *  blocked unblocked; deleted is final but may still be blocked so it
 *  can't come back. */
export function actionsFor(s: ContentState): ModerateAction[] {
  switch (s) {
    case "ok": return ["hide", "delete", "block"];
    case "hidden": return ["unhide", "delete", "block"];
    case "deleted": return ["block"];
    case "blocked": return ["unblock"];
  }
}

/** `/admin/maps/{id}/{action}` or `/admin/content/{sha256}/{action}`. */
export function moderatePath(target: "map" | "content", id: string, action: ModerateAction): string {
  return `/admin/${target === "map" ? "maps" : "content"}/${encodeURIComponent(id)}/${action}`;
}

/** Reports on one package or blob. */
export function reportsFor(c: Pick<ContentList, "reports">, target: "map" | "content", id: string): ContentReport[] {
  return c.reports.filter((r) => r.target === target && r.id === id);
}

/** A short form of a hash for a table: "a1b2c3d4…". */
export const shortHash = (h: string) => (h.length > 12 ? h.slice(0, 8) + "…" : h);

async function fail(r: Response): Promise<never> {
  let code = "http_" + r.status, message = r.statusText;
  try {
    const b = await r.json();
    if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
  } catch { /* not JSON */ }
  throw new ApiError(r.status, code, message);
}

async function call(api: string, path: string, aud: string, me: LocalAccount, init: RequestInit, fetchFn: typeof fetch): Promise<unknown> {
  const token = await issueToken(me, aud, CONTENT_MODERATE);
  const r = await fetchFn(`${api}${path}`, { ...init, headers: { accept: "application/json", authorization: `Bearer ${token}`, ...(init.headers as Record<string, string> | undefined) } });
  if (!r.ok) return fail(r);
  return r.json();
}

/** Everything to moderate. Null from a server without content (404). */
export async function fetchContent(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch): Promise<ContentList | null> {
  try {
    return parseContent(await call(api, "/admin/content", aud, me, {}, fetchFn));
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

export async function moderate(api: string, aud: string, me: LocalAccount, target: "map" | "content", id: string, action: ModerateAction, fetchFn: typeof fetch = fetch): Promise<void> {
  await call(api, moderatePath(target, id, action), aud, me, { method: "POST" }, fetchFn);
}

/** Switches an account's uploads on or off. */
export async function setUploads(api: string, aud: string, me: LocalAccount, gamertag: string, allowed: boolean, fetchFn: typeof fetch = fetch): Promise<void> {
  await call(api, `/admin/uploads/${encodeURIComponent(gamertag)}`, aud, me, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ allowed }),
  }, fetchFn);
}

export const CONTENT_WRITE = "content.write";

/** Flags a package for the admins: any signed-in account, a
 *  `content.write` token. The reason is trimmed and kept short. */
export async function reportMap(api: string, aud: string, me: LocalAccount, mapId: string, reason: string, fetchFn: typeof fetch = fetch): Promise<void> {
  const token = await issueToken(me, aud, CONTENT_WRITE);
  const r = await fetchFn(`${api}/maps/${encodeURIComponent(mapId)}/report`, {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ reason: reason.trim().slice(0, 500) }),
  });
  if (!r.ok) return fail(r);
}
