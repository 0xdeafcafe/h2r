// Where the signed-in account is signed in, and signing the others out
// (docs/live/live-api.md "Sign-ins"). A sign-in is one of the account's
// keys: each browser and game install has its own.
//
//   - GET    /v1/accounts/by-xuid/{xuid}/signins                  lxm signins.read
//   - DELETE /v1/accounts/by-xuid/{xuid}/signins/{kid}            lxm signins.write
//   - POST   /v1/accounts/by-xuid/{xuid}/signins/sign-out-others  lxm signins.write
//
// A signed-out key's next request is refused (401), so the device has to
// sign in again with the gamertag and a sign-in code.

import { issueToken } from "../admin/token";
import { ApiError } from "../api/http";
import type { LocalAccount } from "./account";

export const SIGNINS_READ = "signins.read";
export const SIGNINS_WRITE = "signins.write";

export type SignInKind = "browser" | "game" | "other" | "unknown";

export interface SignIn {
  kid: string;
  kind: SignInKind;
  name: string | null;
  /** signup, code (a sign-in code) or key (added by another key). */
  via: string;
  created_at: string;
  last_seen: string | null;
  /** The key this browser signs with. */
  current: boolean;
}

const KINDS: readonly SignInKind[] = ["browser", "game", "other", "unknown"];

/** The server's list, malformed items dropped. */
export function parseSignIns(v: unknown): SignIn[] {
  const items = (v as { items?: unknown } | null)?.items;
  if (!Array.isArray(items)) return [];
  const out: SignIn[] = [];
  for (const raw of items) {
    if (!raw || typeof raw !== "object") continue;
    const o = raw as Record<string, unknown>;
    if (typeof o.kid !== "string" || !o.kid || typeof o.created_at !== "string") continue;
    out.push({
      kid: o.kid,
      kind: KINDS.includes(o.kind as SignInKind) ? (o.kind as SignInKind) : "unknown",
      name: typeof o.name === "string" && o.name ? o.name : null,
      via: typeof o.via === "string" ? o.via : "",
      created_at: o.created_at,
      last_seen: typeof o.last_seen === "string" ? o.last_seen : null,
      current: o.current === true,
    });
  }
  return out;
}

/** A row's name: "this browser", the device, or what made it. */
export function signInLabel(s: SignIn): string {
  if (s.current) return "this browser";
  if (s.name) return s.name;
  switch (s.kind) {
    case "browser": return "a browser";
    case "game": return "Halo 2";
    default: return "a device";
  }
}

const day = (iso: string) => {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
};

/** "now", "5 min ago", "3 h ago", "2 days ago". */
export function ago(iso: string, nowMs: number): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((nowMs - t) / 1000));
  if (s < 90) return "now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 48 * 3600) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} days ago`;
}

/** The row's right-hand note: when it was last used, else since when. */
export function signInMeta(s: SignIn, nowMs: number): string {
  if (s.current) return "signed in here";
  if (s.last_seen) return `used ${ago(s.last_seen, nowMs)}`;
  const d = day(s.created_at);
  return d ? `since ${d}` : "";
}

/** How the key came, for the plate. */
export function viaText(via: string): string {
  switch (via) {
    case "signup": return "made the account";
    case "code": return "signed in with a code";
    case "key": return "added by another sign-in";
    default: return "";
  }
}

/** The account plate's "signed in" line: this browser and the others. */
export function signInSummary(list: SignIn[]): string {
  const others = list.filter((s) => !s.current);
  const here = list.some((s) => s.current) ? "this browser" : "";
  if (!others.length) return here ? "only this browser" : "nowhere";
  const names = others.slice(0, 2).map(signInLabel);
  const more = others.length - names.length;
  return [here, ...names].filter(Boolean).join(", ") + (more > 0 ? ` and ${more} more` : "");
}

/** The sign-ins list's last row: every sign-in but this one. */
export const OTHERS_KEY = "#others";

export interface SignInRow { key: string; label: string; meta?: string }

/** The list's rows: the sign-ins, then "sign out others" when there is
 *  anywhere else. */
export function signInRows(list: SignIn[], nowMs: number): SignInRow[] {
  const rows: SignInRow[] = list.map((s) => ({ key: s.kid, label: signInLabel(s), meta: signInMeta(s, nowMs) }));
  const others = list.filter((s) => !s.current).length;
  if (others) rows.push({ key: OTHERS_KEY, label: "sign out others", meta: others === 1 ? "1 other" : `all ${others} others` });
  return rows;
}

/** What confirming a row signs out, in words. */
export function confirmText(row: SignInRow, s: SignIn | undefined): string {
  if (row.key === OTHERS_KEY) return `Sign out everywhere but this browser (${row.meta})? Each will need your gamertag and sign-in code to sign in again.`;
  if (s?.current) return "Sign out of this browser? You'll need your gamertag and sign-in code to sign in here again.";
  return `Sign out ${row.label}? It will need your gamertag and sign-in code to sign in again.`;
}

async function fail(r: Response): Promise<never> {
  let code = "http_" + r.status, message = r.statusText;
  try { const b = await r.json(); if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; } } catch { /* not JSON */ }
  throw new ApiError(r.status, code, message);
}

const base = (api: string, me: LocalAccount) => `${api}/accounts/by-xuid/${encodeURIComponent(me.xuid)}/signins`;

export async function fetchSignIns(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch, nowMs: number = Date.now()): Promise<SignIn[]> {
  const token = await issueToken(me, aud, SIGNINS_READ, nowMs);
  const r = await fetchFn(base(api, me), { headers: { accept: "application/json", authorization: `Bearer ${token}` }, cache: "no-store" });
  if (!r.ok) return fail(r);
  return parseSignIns(await r.json());
}

/** Signs one other sign-in out; the kids revoked. */
export async function signOutOne(api: string, aud: string, me: LocalAccount, kid: string, fetchFn: typeof fetch = fetch, nowMs: number = Date.now()): Promise<string[]> {
  const token = await issueToken(me, aud, SIGNINS_WRITE, nowMs);
  const r = await fetchFn(`${base(api, me)}/${encodeURIComponent(kid)}`, { method: "DELETE", headers: { accept: "application/json", authorization: `Bearer ${token}` } });
  if (!r.ok) return fail(r);
  return revokedOf(await r.json());
}

/** Signs every sign-in but this browser out; the kids revoked. */
export async function signOutOthers(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch, nowMs: number = Date.now()): Promise<string[]> {
  const token = await issueToken(me, aud, SIGNINS_WRITE, nowMs);
  const r = await fetchFn(`${base(api, me)}/sign-out-others`, { method: "POST", headers: { accept: "application/json", authorization: `Bearer ${token}` } });
  if (!r.ok) return fail(r);
  return revokedOf(await r.json());
}

function revokedOf(v: unknown): string[] {
  const r = (v as { revoked?: unknown } | null)?.revoked;
  return Array.isArray(r) ? r.filter((k): k is string => typeof k === "string") : [];
}
