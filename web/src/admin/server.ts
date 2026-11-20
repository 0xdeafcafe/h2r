// The dashboard's server screen (docs/live/live-admin.md §5): the public facts
// every visitor sees (GET /v1/server), the admin status (GET
// /v1/admin/status, an `admin.read` token) and invites made by an admin
// account: a signed `InviteCreated` event POSTed to /v1/invites with an
// `invites.write` token; the server answers the code. The body is the
// event's compact JWS, or JSON {event, code} when the admin chooses the
// code. Generated codes are 25 characters in five groups
// (XXXXX-XXXXX-XXXXX-XXXXX-XXXXX); custom ones are what the admin typed,
// and the list never shows them again.
//
// Parsing is defensive: an older server answers 404 for the new
// endpoints and the screen falls back to what discovery says.

import { ApiError } from "../api/http";
import type { InviteGrant } from "../api/types";
import type { LocalAccount } from "../state/account";
import { eventBody, signEvent } from "../state/sign";
import { issueToken, randomId } from "./token";
import { DASH_PLAYERS, LIVE_ADMIN, STATS_ROOT, statsPath } from "../routes";

export const ADMIN_READ = "admin.read";
export const INVITES_WRITE = "invites.write";
export const INVITES_READ = "invites.read";
export const INVITE_GRANTS_WRITE = "invite-grants.write";
export const INVITE_STREAM = "invites";
/** The longest an admin's invite may last (the server's MAX_DAYS). */
export const INVITE_MAX_DAYS = 30;
export const INVITE_DAY_CHOICES = [1, 7, 30] as const;

/** The next invite length when the "lasts" row is cycled: 1, 7, 30 days
 *  (the server caps an admin's invite at 30 days, so there is no "never"). */
export function nextInviteDays(d: number, step: 1 | -1 = 1): number {
  const c = INVITE_DAY_CHOICES as readonly number[];
  return c[(Math.max(0, c.indexOf(d)) + step + c.length) % c.length]!;
}

export interface ServerCounts { accounts: number; online: number; live_games: number; sessions: number }

export interface ServerFacts {
  id: string;
  software: string | null;
  url: string | null;
  invite_only: boolean;
  private: boolean;
  federation: boolean;
  counts: ServerCounts | null;
  position: number | null;
}

export type TunnelKind = "quick" | "named" | "proxy";

export interface GameArtFacts {
  present: boolean;
  shared: boolean;
  manifest_version: number | null;
  extractor: number | null;
  source: string | null;
  maps: number;
}

export interface AdminEntry { gamertag: string; source: "configured" | "granted" | string }

export interface AdminStatus extends ServerFacts {
  started_at: string | null;
  uptime_s: number;
  public: { url: string | null; tunnel: TunnelKind | null };
  game_art: GameArtFacts;
  admins: AdminEntry[];
  log: { position: number; head: number; store: string };
  invites: { open: number; used: number; revoked: number; expired: number };
  /** Only when the browser is on the server's own machine. */
  data_dir: string | null;
}

export type InviteState = "open" | "used" | "revoked" | "expired";

export interface InviteItem {
  jti: string;
  by: string;
  by_gamertag: string | null;
  at: string;
  exp: number;
  state: InviteState;
  used_by_gamertag: string | null;
  /** The code, for open generated invites only (never for custom ones). */
  invite: string | null;
  /** The admin chose the code. */
  custom: boolean;
}

const num = (v: unknown, d = 0): number => (typeof v === "number" && Number.isFinite(v) ? v : d);
const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});

export function parseServerFacts(body: unknown): ServerFacts {
  const o = obj(body);
  const c = o.counts && typeof o.counts === "object" ? obj(o.counts) : null;
  return {
    id: str(o.id) ?? "",
    software: str(o.software),
    url: str(o.url),
    invite_only: o.invite_only === true,
    private: o.private === true,
    federation: o.federation === true,
    counts: c ? { accounts: num(c.accounts), online: num(c.online), live_games: num(c.live_games), sessions: num(c.sessions) } : null,
    position: typeof o.position === "number" ? o.position : null,
  };
}

export function parseAdminStatus(body: unknown): AdminStatus {
  const o = obj(body);
  const pub = obj(o.public);
  const art = obj(o.game_art);
  const log = obj(o.log);
  const inv = obj(o.invites);
  const tunnel = pub.tunnel === "quick" || pub.tunnel === "named" || pub.tunnel === "proxy" ? pub.tunnel : null;
  return {
    ...parseServerFacts(body),
    started_at: str(o.started_at),
    uptime_s: num(o.uptime_s),
    public: { url: str(pub.url) ?? str(o.url), tunnel },
    game_art: {
      present: art.present === true,
      shared: art.shared === true,
      manifest_version: typeof art.manifest_version === "number" ? art.manifest_version : null,
      extractor: typeof art.extractor === "number" && art.extractor > 0 ? art.extractor : null,
      source: str(art.source),
      maps: num(art.maps),
    },
    admins: Array.isArray(o.admins)
      ? o.admins.map(obj).filter((a) => typeof a.gamertag === "string").map((a) => ({ gamertag: a.gamertag as string, source: str(a.source) ?? "configured" }))
      : [],
    log: { position: num(log.position), head: num(log.head), store: str(log.store) ?? "unknown" },
    invites: { open: num(inv.open), used: num(inv.used), revoked: num(inv.revoked), expired: num(inv.expired) },
    data_dir: str(o.data_dir),
  };
}

export function parseInvites(body: unknown): InviteItem[] {
  const items = obj(body).items;
  if (!Array.isArray(items)) return [];
  return items.map(obj).filter((i) => typeof i.jti === "string").map((i) => ({
    jti: i.jti as string,
    by: str(i.by) ?? "",
    by_gamertag: str(i.by_gamertag),
    at: str(i.at) ?? "",
    exp: num(i.exp),
    state: (["open", "used", "revoked", "expired"].includes(i.state as string) ? i.state : "open") as InviteState,
    used_by_gamertag: str(i.used_by_gamertag),
    invite: str(i.invite),
    custom: i.custom === true,
  }));
}

/** "3d 4h", "2h 05m", "12m", "40s". */
export function formatUptime(s: number): string {
  s = Math.max(0, Math.floor(s));
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m) return `${m}m`;
  return `${s}s`;
}

/** How the server is reached from outside, in words. */
export function reachWords(p: AdminStatus["public"]): string {
  if (!p.url) return "this machine only";
  switch (p.tunnel) {
    case "quick": return "quick tunnel (new address each run)";
    case "named": return "named tunnel";
    case "proxy": return "your proxy";
    default: return "public";
  }
}

export function artWords(a: GameArtFacts): string {
  if (!a.present) return "none extracted";
  const who = a.shared ? "shared with everyone" : "this machine only";
  return a.extractor ? `${who} · extractor v${a.extractor}` : who;
}

/** What an admin signs to make an invite lasting `days` from `nowMs`. */
export function inviteData(days: number, nowMs: number, jti: string = randomId()): { jti: string; exp: number } {
  const d = Math.min(INVITE_MAX_DAYS, Math.max(1, Math.round(days)));
  return { jti, exp: Math.floor(nowMs / 1000) + d * 86400 };
}

async function fail(r: Response): Promise<never> {
  let code = "http_" + r.status, message = r.statusText;
  try {
    const b = await r.json();
    if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
  } catch { /* not JSON */ }
  throw new ApiError(r.status, code, message);
}

/** `GET /v1/server`; null from a server too old to have it. */
export async function fetchServerFacts(api: string, fetchFn: typeof fetch = fetch): Promise<ServerFacts | null> {
  const r = await fetchFn(`${api}/server`, { headers: { accept: "application/json" } });
  if (r.status === 404) return null;
  if (!r.ok) return fail(r);
  return parseServerFacts(await r.json());
}

async function signedGet(api: string, path: string, aud: string, me: LocalAccount, method: string, fetchFn: typeof fetch): Promise<unknown> {
  const token = await issueToken(me, aud, method);
  const r = await fetchFn(`${api}${path}`, { headers: { accept: "application/json", authorization: `Bearer ${token}` } });
  if (!r.ok) return fail(r);
  return r.json();
}

export function adminGet(api: string, path: string, aud: string, me: LocalAccount, fetchFn: typeof fetch): Promise<unknown> {
  return signedGet(api, path, aud, me, ADMIN_READ, fetchFn);
}

export async function fetchAdminStatus(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch): Promise<AdminStatus> {
  return parseAdminStatus(await adminGet(api, "/admin/status", aud, me, fetchFn));
}

export async function fetchInvites(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch): Promise<InviteItem[]> {
  return parseInvites(await adminGet(api, "/invites", aud, me, fetchFn));
}

export async function fetchMyInvites(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch): Promise<InviteItem[]> {
  return parseInvites(await signedGet(api, "/invites/mine", aud, me, INVITES_READ, fetchFn));
}

export async function writeInviteGrant(
  api: string, aud: string, me: LocalAccount, tag: string,
  grant: InviteGrant | null, fetchFn: typeof fetch = fetch,
): Promise<void> {
  const token = await issueToken(me, aud, INVITE_GRANTS_WRITE);
  const headers: Record<string, string> = { authorization: `Bearer ${token}` };
  if (grant) headers["content-type"] = "application/json";
  const r = await fetchFn(`${api}/accounts/${encodeURIComponent(tag)}/invite-grant`, {
    method: grant ? "POST" : "DELETE",
    headers,
    body: grant ? JSON.stringify(grant) : undefined,
  });
  if (!r.ok) await fail(r);
}

/** A custom code's rules (the server's): 4 to 32 printable ASCII
 *  characters. It matches ignoring case, spaces and dashes. */
export const CUSTOM_CODE_MIN = 4;
export const CUSTOM_CODE_MAX = 32;
export function customCodeError(code: string): string | null {
  if (code.length < CUSTOM_CODE_MIN || code.length > CUSTOM_CODE_MAX) return `${CUSTOM_CODE_MIN} to ${CUSTOM_CODE_MAX} characters`;
  if (!/^[\x20-\x7e]+$/.test(code)) return "letters, digits, spaces and punctuation only";
  return null;
}

/** The body of `POST /v1/invites`: the event alone, or with a chosen code. */
export function inviteBody(jws: string, code?: string): { contentType: string; body: string } {
  return code
    ? { contentType: "application/json", body: JSON.stringify({ event: jws, code }) }
    : { contentType: "application/jose", body: jws };
}

async function postSigned(api: string, path: string, aud: string, me: LocalAccount, type: string, data: unknown, fetchFn: typeof fetch, nowMs: number, code?: string): Promise<unknown> {
  const jws = await signEvent(me, eventBody(me, { stream: INVITE_STREAM, type, data }, nowMs));
  const token = await issueToken(me, aud, INVITES_WRITE, nowMs);
  const b = inviteBody(jws, code);
  const r = await fetchFn(`${api}${path}`, {
    method: "POST",
    headers: { "content-type": b.contentType, authorization: `Bearer ${token}` },
    body: b.body,
  });
  if (!r.ok) return fail(r);
  return r.json();
}

/** Makes an invite lasting `days`, with the code `code` if given (trimmed;
 *  empty means the server makes one); resolves to its code and expiry. */
export async function createInvite(
  api: string, aud: string, me: LocalAccount, days: number,
  fetchFn: typeof fetch = fetch, nowMs: number = Date.now(), code?: string,
): Promise<{ invite: string; jti: string; exp: number; custom: boolean }> {
  const want = code?.trim() || undefined;
  if (want) {
    const bad = customCodeError(want);
    if (bad) throw new ApiError(400, "invalid", `that code won't work: ${bad}`);
  }
  const b = obj(await postSigned(api, "/invites", aud, me, "InviteCreated", inviteData(days, nowMs), fetchFn, nowMs, want));
  if (typeof b.invite !== "string") throw new ApiError(500, "invalid", "the server sent no invite");
  return { invite: b.invite, jti: String(b.jti ?? ""), exp: num(b.exp), custom: b.custom === true || !!want };
}

export async function revokeInvite(api: string, aud: string, me: LocalAccount, jti: string, fetchFn: typeof fetch = fetch, nowMs: number = Date.now()): Promise<void> {
  await postSigned(api, `/invites/${encodeURIComponent(jti)}/revoke`, aud, me, "InviteRevoked", { jti }, fetchFn, nowMs);
}

/** One of the four small pods on a server screen: a place to go, or an
 *  action on the invites screen. */
export interface ServerPod { label: string; glyph: "key" | "sign" | "record" | "server" | "history" | "career" | "download" | "globe" | "gear"; to?: string; back?: boolean; run?: "create" | "copy" }

/** The server screens: the overview, and under it invites, accounts,
 *  importing an account, player reports and bans (/dash/server/<view>). */
export type ServerView = "overview" | "invites" | "accounts" | "import" | "reports" | "bans";

/** Where the dashboard's "halo admin" goes: the stats site's admin area,
 *  which keeps only the game's own things (the message of the day,
 *  playlists, custom content). Everything about the service (invites,
 *  accounts and admins, importing an account, the server) is on the
 *  dashboard. */
export const HALO_ADMIN = statsPath("/admin");

/** The Live admin's usage screen (docs/live/web-boundary.md §5), in the dense style. */
export const LIVE_ADMIN_USAGE = `${LIVE_ADMIN}/usage`;

/** The pods of each server screen. Admin tools only for an admin
 *  (`isMeAdmin() === true`); everyone else, and anyone while that is still
 *  unknown, gets the visitor's set. Importing an account is open to
 *  anyone: it is how a browser signs in as an account the CLI holds. */
export function serverPods(view: ServerView, admin: boolean | null): ServerPod[] {
  if (admin !== true) {
    switch (view) {
      case "overview":
        return [
          { label: "another server", glyph: "server", to: "/dash/server/connect" },
          { label: "event log", glyph: "history", to: statsPath("/events") },
          { label: "players", glyph: "career", to: DASH_PLAYERS },
          { label: "stats site", glyph: "record", to: STATS_ROOT },
        ];
      case "import":
        return [
          { label: "sign in", glyph: "key", to: "/dash/signin" },
          { label: "create account", glyph: "sign", to: "/dash/signup" },
          { label: "another server", glyph: "server", to: "/dash/server/connect" },
          { label: "server", glyph: "globe", to: "/dash/server", back: true },
        ];
      default:
        return [];
    }
  }
  switch (view) {
    case "invites":
      return [
        { label: "new invite", glyph: "key", run: "create" },
        { label: "copy code", glyph: "download", run: "copy" },
        { label: "accounts", glyph: "career", to: "/dash/server/accounts" },
        { label: "your server", glyph: "globe", to: "/dash/server", back: true },
      ];
    case "reports":
      return [
        { label: "accounts", glyph: "career", to: "/dash/server/accounts", back: true },
        { label: "bans", glyph: "sign", to: "/dash/server/bans" },
        { label: "players", glyph: "record", to: DASH_PLAYERS },
        { label: "your server", glyph: "globe", to: "/dash/server", back: true },
      ];
    case "bans":
      return [
        { label: "accounts", glyph: "career", to: "/dash/server/accounts", back: true },
        { label: "reports", glyph: "history", to: "/dash/server/reports", back: true },
        { label: "players", glyph: "record", to: DASH_PLAYERS },
        { label: "your server", glyph: "globe", to: "/dash/server", back: true },
      ];
    case "accounts":
      return [
        { label: "invites", glyph: "key", to: "/dash/server/invites" },
        { label: "import account", glyph: "download", to: "/dash/server/import" },
        { label: "usage", glyph: "history", to: LIVE_ADMIN_USAGE },
        { label: "your server", glyph: "globe", to: "/dash/server", back: true },
      ];
    case "import":
      return [
        { label: "accounts", glyph: "career", to: "/dash/server/accounts", back: true },
        { label: "invites", glyph: "key", to: "/dash/server/invites" },
        { label: "halo admin", glyph: "gear", to: HALO_ADMIN },
        { label: "your server", glyph: "globe", to: "/dash/server", back: true },
      ];
    default:
      return [
        { label: "invites", glyph: "key", to: "/dash/server/invites" },
        { label: "accounts", glyph: "career", to: "/dash/server/accounts" },
        { label: "halo admin", glyph: "gear", to: HALO_ADMIN },
        { label: "another server", glyph: "server", to: "/dash/server/connect" },
      ];
  }
}

/** How an admin came to be one, in words (no commands: granting is the
 *  operator's, on the server's own machine). */
export function adminSourceWords(source: string): string {
  switch (source) {
    case "configured": return "set at startup";
    case "granted": return "granted";
    case "session": return "this session";
    default: return source;
  }
}
