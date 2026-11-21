// The Live admin's usage endpoints (docs/live/live-api.md "Usage"):
// GET /v1/admin/usage and GET /v1/admin/usage/accounts, each with an
// `admin.read` token signed by the signed-in admin account. Parsing is
// compatible with sparse counters. Request coverage and storage measurement
// are tracked separately from measured zeroes.

import type { LocalAccount } from "../state/account";
import { adminGet } from "./server";

export const UPLOAD_KINDS = [
  { key: "events", label: "events" },
  { key: "frames", label: "live frames" },
  { key: "content", label: "content" },
  { key: "saves", label: "cloud saves" },
] as const;
export type UploadKind = (typeof UPLOAD_KINDS)[number]["key"];

/** Messages by kind; text joins as `text` when there is a messaging feature. */
export const MESSAGE_KINDS = [
  { key: "friend_requests", label: "friend requests" },
  { key: "invites", label: "game invites" },
] as const;
export type MessageKind = (typeof MESSAGE_KINDS)[number]["key"];

export const STORED_KINDS = [
  { key: "events", label: "event log" },
  { key: "content", label: "map content" },
  { key: "saves", label: "cloud saves" },
] as const;
export type StoredKind = (typeof STORED_KINDS)[number]["key"];

export interface UsageDay {
  day: string;
  up: Record<UploadKind, number>;
  events: number;
  messages: Record<MessageKind, number>;
  games: number;
  sessions_hosted: number;
  sign_ins: number;
  reports: number;
  active_accounts: number;
  requests?: number | null;
}

export interface UsageOverview {
  from: string;
  to: string;
  keep_days: number;
  days: UsageDay[];
  totals: UsageDay;
  accounts_active: number;
  event_types: { type: string; count: number }[];
  stored: Record<StoredKind, number>;
  request_count_from?: string | null;
  saves_storage_measured?: boolean;
}

export interface AccountUsage {
  xuid: string;
  gamertag: string | null;
  up: Record<UploadKind, number>;
  uploaded: number;
  stored: Record<StoredKind, number>;
  stored_total: number;
  events: number;
  messages: Record<MessageKind, number>;
  messages_total: number;
  games: number;
  sessions_hosted: number;
  sign_ins: number;
  reports_sent: number;
  reports_received: number;
  active_days: number;
  last_active: string | null;
}

export interface AccountsPage { days: number; sort: string; total: number; items: AccountUsage[]; saves_storage_measured?: boolean }

export const ACCOUNT_SORTS = [
  "uploaded", "stored", "events", "messages", "games", "sessions_hosted", "sign_ins", "reports_sent", "reports_received", "active_days",
] as const;
export type AccountSort = (typeof ACCOUNT_SORTS)[number];

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (v && typeof v === "object" && !Array.isArray(v) ? (v as Obj) : {});
const n = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const isDay = (s: string) => /^\d{4}-\d{2}-\d{2}$/.test(s) && Number.isFinite(Date.parse(s));
const count = (v: unknown): number | null => typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : null;

function kinds<K extends string>(v: unknown, ks: readonly { key: K }[]): Record<K, number> {
  const o = obj(v);
  return Object.fromEntries(ks.map((k) => [k.key, n(o[k.key])])) as Record<K, number>;
}

export function parseDay(v: unknown): UsageDay {
  const o = obj(v);
  return {
    day: str(o.day),
    up: kinds(o.up, UPLOAD_KINDS),
    events: n(o.events),
    messages: kinds(o.messages, MESSAGE_KINDS),
    games: n(o.games),
    sessions_hosted: n(o.sessions_hosted),
    sign_ins: n(o.sign_ins),
    reports: n(o.reports),
    active_accounts: n(o.active_accounts),
    requests: count(o.requests),
  };
}

export function parseOverview(v: unknown): UsageOverview {
  const o = obj(v);
  return {
    from: str(o.from),
    to: str(o.to),
    keep_days: n(o.keep_days) || 400,
    days: (Array.isArray(o.days) ? o.days : []).map(parseDay).filter((d) => isDay(d.day)),
    totals: parseDay(o.totals),
    accounts_active: n(o.accounts_active),
    event_types: (Array.isArray(o.event_types) ? o.event_types : [])
      .map((t) => ({ type: str(obj(t).type), count: n(obj(t).count) }))
      .filter((t) => t.type),
    stored: kinds(o.stored, STORED_KINDS),
    request_count_from: isDay(str(o.request_count_from)) ? str(o.request_count_from) : null,
    saves_storage_measured: typeof o.saves_storage_measured === "boolean" ? o.saves_storage_measured : undefined,
  };
}

export function parseAccount(v: unknown): AccountUsage {
  const o = obj(v);
  const up = kinds(o.up, UPLOAD_KINDS), stored = kinds(o.stored, STORED_KINDS), messages = kinds(o.messages, MESSAGE_KINDS);
  const sum = (r: Record<string, number>) => Object.values(r).reduce((a, b) => a + b, 0);
  return {
    xuid: str(o.xuid),
    gamertag: typeof o.gamertag === "string" ? o.gamertag : null,
    up, uploaded: n(o.uploaded) || sum(up),
    stored, stored_total: n(o.stored_total) || sum(stored),
    events: n(o.events),
    messages, messages_total: n(o.messages_total) || sum(messages),
    games: n(o.games),
    sessions_hosted: n(o.sessions_hosted),
    sign_ins: n(o.sign_ins),
    reports_sent: n(o.reports_sent),
    reports_received: n(o.reports_received),
    active_days: n(o.active_days),
    last_active: isDay(str(o.last_active)) ? str(o.last_active) : null,
  };
}

export function parseAccounts(v: unknown): AccountsPage {
  const o = obj(v);
  return {
    days: n(o.days),
    sort: str(o.sort),
    total: n(o.total),
    items: (Array.isArray(o.items) ? o.items : []).map(parseAccount).filter((a) => a.xuid),
    saves_storage_measured: typeof o.saves_storage_measured === "boolean" ? o.saves_storage_measured : undefined,
  };
}

export async function fetchUsage(api: string, aud: string, me: LocalAccount, days: number, fetchFn: typeof fetch = fetch): Promise<UsageOverview> {
  const body = obj(await adminGet(api, `/admin/usage?days=${days}`, aud, me, fetchFn));
  if (!isDay(str(body.from)) || !isDay(str(body.to)) || str(body.from) > str(body.to)
    || !Array.isArray(body.days) || !Array.isArray(body.event_types)
    || typeof obj(body.totals).events !== "number" || Object.keys(obj(body.stored)).length === 0) {
    throw new Error("The server returned an invalid usage overview.");
  }
  return parseOverview(body);
}

export async function fetchUsageAccounts(api: string, aud: string, me: LocalAccount, days: number, sort: AccountSort, limit: number, fetchFn: typeof fetch = fetch): Promise<AccountsPage> {
  const body = obj(await adminGet(api, `/admin/usage/accounts?days=${days}&sort=${sort}&limit=${limit}`, aud, me, fetchFn));
  if (!Array.isArray(body.items) || count(body.total) === null) {
    throw new Error("The server returned an invalid account usage list.");
  }
  return parseAccounts(body);
}
