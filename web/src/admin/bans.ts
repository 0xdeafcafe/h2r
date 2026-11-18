// Account bans (docs/live/live-admin.md, "Bans"): an admin bans a player,
// for a while or until lifted, often from a report in the queue, and lifts
// it again; a banned player can still read their own notice. While a ban
// is in force the server refuses the player's every write as `banned`
// (403), signing in from the game included. Admin calls carry a
// `reports.moderate` token, accepted only for admins; the notice a `read`
// token. Also the public commendation counts the stats site shows. Parsing
// is defensive like ./reports.ts.

import type { LocalAccount } from "../state/account";
import { adminCall } from "./content";
import { REPORTS_MODERATE, type Who } from "./reports";

export type BanListState = "active" | "all";
export type BanState = "active" | "expired" | "lifted" | "replaced";

export interface Ban {
  id: number;
  target: Who;
  reason: string;
  /** RFC 3339; null for a ban until lifted. */
  until: string | null;
  /** The player report it answers. */
  report: number | null;
  /** The admin's gamertag; null for the operator. */
  by: string | null;
  at: string;
  state: BanState;
  lifted: { note: string; by: string | null; at: string } | null;
}

export interface BanList { items: Ban[]; position: number | null }

/** What the banned player reads. */
export interface BanNotice { id: number; reason: string; until: string | null; at: string }

export interface Commendations { good_attitude: number; good_session: number }

/** The lengths an admin picks from; null is until lifted. */
export const BAN_LENGTHS: { label: string; days: number | null }[] = [
  { label: "1 day", days: 1 },
  { label: "1 week", days: 7 },
  { label: "30 days", days: 30 },
  { label: "until lifted", days: null },
];

const STATES: BanState[] = ["active", "expired", "lifted", "replaced"];
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const count = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);
const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** `GET /v1/admin/bans`, dropping entries it can't read; newest first. */
export function parseBans(body: unknown): BanList {
  const o = obj(body);
  const items: Ban[] = [];
  for (const raw of Array.isArray(o.items) ? o.items : []) {
    const b = obj(raw);
    const t = obj(b.target);
    if (!count(b.id) || !str(t.xuid)) continue;
    const l = b.lifted === undefined || b.lifted === null ? null : obj(b.lifted);
    items.push({
      id: count(b.id),
      target: { xuid: str(t.xuid), gamertag: str(t.gamertag) || "unknown" },
      reason: str(b.reason),
      until: str(b.until) || null,
      report: count(b.report) || null,
      by: str(b.by) || null,
      at: str(b.at),
      state: STATES.includes(b.state as BanState) ? (b.state as BanState) : "active",
      lifted: l ? { note: str(l.note), by: str(l.by) || null, at: str(l.at) } : null,
    });
  }
  items.sort((a, b) => b.id - a.id);
  return { items, position: typeof o.position === "number" ? o.position : null };
}

/** `GET /v1/accounts/by-xuid/{xuid}/ban`: the ban in force, or null. */
export function parseBanStatus(body: unknown): BanNotice | null {
  const o = obj(body);
  if (o.banned !== true) return null;
  const b = obj(o.ban);
  return { id: count(b.id), reason: str(b.reason), until: str(b.until) || null, at: str(b.at) };
}

/** `GET /v1/accounts/{who}/commendations`. */
export function parseCommendations(body: unknown): Commendations {
  const o = obj(body);
  return { good_attitude: count(o.good_attitude), good_session: count(o.good_session) };
}

/** When a ban of `days` made at `nowMs` ends (RFC 3339), or null. */
export function banUntil(days: number | null, nowMs: number = Date.now()): string | null {
  return days === null ? null : new Date(nowMs + days * 86_400_000).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/** What a ban needs before it can be sent. Null when it is fine. */
export function banProblem(reason: string): string | null {
  const t = reason.trim();
  if (!t) return "say why, for the player";
  if ([...t].length > 500) return "a reason is at most 500 characters";
  return null;
}

/** How a ban's end reads: "until lifted" or "until 18 Nov 2026, 15:00". */
export function untilText(until: string | null): string {
  if (!until) return "until lifted";
  const d = new Date(until);
  return Number.isNaN(d.getTime()) ? `until ${until}` : `until ${d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`;
}

export async function fetchBans(api: string, aud: string, me: LocalAccount, state: BanListState = "active", fetchFn: typeof fetch = fetch): Promise<BanList> {
  return parseBans(await adminCall(api, `/admin/bans?state=${state}`, aud, me, REPORTS_MODERATE, {}, fetchFn));
}

export async function banAccount(
  api: string, aud: string, me: LocalAccount, xuid: string,
  ban: { reason: string; days: number | null; report?: number | null },
  fetchFn: typeof fetch = fetch, nowMs: number = Date.now(),
): Promise<number> {
  const problem = banProblem(ban.reason);
  if (problem) throw new Error(problem);
  const until = banUntil(ban.days, nowMs);
  const body = { reason: ban.reason.trim(), ...(until ? { until } : {}), ...(ban.report ? { report: ban.report } : {}) };
  const r = obj(await adminCall(api, `/admin/bans/${encodeURIComponent(xuid)}`, aud, me, REPORTS_MODERATE, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  }, fetchFn));
  return count(r.position);
}

export async function liftBan(api: string, aud: string, me: LocalAccount, xuid: string, note: string, fetchFn: typeof fetch = fetch): Promise<void> {
  if ([...note.trim()].length > 500) throw new Error("a note is at most 500 characters");
  await adminCall(api, `/admin/bans/${encodeURIComponent(xuid)}/lift`, aud, me, REPORTS_MODERATE, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ note: note.trim() }),
  }, fetchFn);
}

/** The ban in force on `xuid`, read with `me`'s `read` token (the player
 *  themselves or an admin). */
export async function fetchBan(api: string, aud: string, me: LocalAccount, xuid: string, fetchFn: typeof fetch = fetch): Promise<BanNotice | null> {
  return parseBanStatus(await adminCall(api, `/accounts/by-xuid/${encodeURIComponent(xuid)}/ban`, aud, me, "read", {}, fetchFn));
}
