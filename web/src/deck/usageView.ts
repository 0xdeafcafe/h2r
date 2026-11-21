// Usage numbers shaped for the deck's panels: the range's days filled in,
// the series for each chart, the tiles with their change against the
// period before, and the ranked lists with their "other". The admin usage
// screen (/dash/admin/usage) and the style page (fixed data) both draw
// from this.

import { compact, dayRange, delta, fillDays, periods, topN, type Day, type Delta, type RankRow, type Series } from "./chart";
export type { RankRow } from "./chart";
import { MESSAGE_KINDS, STORED_KINDS, UPLOAD_KINDS, type AccountSort, type UsageDay, type UsageOverview } from "../admin/usage";

export interface Tile { key: string; label: string; value: number; delta: Delta | null; trend: number[]; bytes?: boolean; upIsGood: boolean }

export interface UsageView {
  days: Day[];
  upload: Series[];
  messages: Series[];
  people: Series[];
  events: Series[];
  tiles: Tile[];
  eventTypes: RankRow[];
  stored: RankRow[];
  storedTotal: number;
  requests: { total: number | null; measuredFrom: Day | null; days: Day[]; daily: Series };
  hasActivity: boolean;
}

export const zeroDay = (day: Day): UsageDay => ({
  day,
  up: { events: 0, frames: 0, content: 0, saves: 0 },
  events: 0,
  messages: { friend_requests: 0, invites: 0 },
  games: 0, sessions_hosted: 0, sign_ins: 0, reports: 0, active_accounts: 0,
});

export const uploadOf = (d: UsageDay) => UPLOAD_KINDS.reduce((a, k) => a + d.up[k.key], 0);
export const messagesOf = (d: UsageDay) => MESSAGE_KINDS.reduce((a, k) => a + d.messages[k.key], 0);

/** The `range` days ending on `last`. `wide`, the server's answer for
 *  twice the range, gives the tiles their change against the range
 *  before; without it they show none. */
export function usageView(cur: UsageOverview, wide: UsageOverview | null, last: Day, range: number, topTypes = 7): UsageView {
  const days = dayRange(last, range);
  const rows = fillDays(cur.days, days, zeroDay);
  const both = wide ? fillDays(wide.days, dayRange(last, range * 2), zeroDay) : null;
  const col = (f: (d: UsageDay) => number) => rows.map(f);
  const tile = (key: string, label: string, f: (d: UsageDay) => number, extra: Partial<Tile> = {}): Tile => {
    const now = rows.reduce((a, d) => a + f(d), 0);
    const d = both ? delta(now, periods(both.map(f), range).before) : null;
    return { key, label, value: now, delta: d, trend: col(f), upIsGood: true, ...extra };
  };
  const requestRows = rows.flatMap((row) => row.requests == null ? [] : [{ day: row.day, requests: row.requests }]);
  const stored = STORED_KINDS.map((k) => ({ key: k.key, label: k.label, value: cur.stored[k.key] }));
  return {
    days,
    requests: {
      total: cur.totals.requests ?? null,
      measuredFrom: cur.request_count_from ?? null,
      days: requestRows.map((row) => row.day),
      daily: { key: "requests", label: "Live API requests", values: requestRows.map((row) => row.requests) },
    },
    hasActivity: cur.days.some((row) => uploadOf(row) || messagesOf(row) || row.events || row.games
      || row.sessions_hosted || row.sign_ins || row.reports || row.active_accounts),
    upload: UPLOAD_KINDS.map((k) => ({ key: k.key, label: k.label, values: col((d) => d.up[k.key]) })),
    messages: MESSAGE_KINDS.map((k) => ({ key: k.key, label: k.label, values: col((d) => d.messages[k.key]) })),
    people: [
      { key: "active", label: "active accounts", values: col((d) => d.active_accounts) },
      { key: "sign_ins", label: "sign-ins", values: col((d) => d.sign_ins) },
    ],
    events: [{ key: "events", label: "events written", values: col((d) => d.events) }],
    tiles: [
      tile("uploaded", "data uploaded", uploadOf, { bytes: true }),
      tile("events", "events written", (d) => d.events),
      tile("messages", "messages sent", messagesOf),
      tile("games", "games played", (d) => d.games),
      tile("sign_ins", "sign-ins", (d) => d.sign_ins),
      // More reports is not good news.
      tile("reports", "player reports", (d) => d.reports, { upIsGood: false }),
    ],
    eventTypes: topN<RankRow>(
      cur.event_types.map((t) => ({ key: t.type, label: t.type, value: t.count })),
      topTypes, (r) => r.value,
      (sum, count) => ({ key: "_other", label: `${count} other ${count === 1 ? "type" : "types"}`, value: sum, other: true }),
    ),
    stored: topN<RankRow>(stored.filter((r) => r.value > 0), stored.length, (r) => r.value),
    storedTotal: stored.reduce((a, r) => a + r.value, 0),
  };
}

/** A tile's value as text: bytes or a compact count. */
export const tileValue = (t: Tile, bytes: (n: number) => string) => (t.bytes ? bytes(t.value) : compact(t.value));

/** The server sorts the usage screen offers for its top accounts. */
export const USAGE_SORTS: readonly { value: AccountSort; label: string }[] = [
  { value: "uploaded", label: "uploaded" },
  { value: "stored", label: "stored" },
  { value: "events", label: "events" },
  { value: "messages", label: "messages" },
  { value: "reports_received", label: "reported" },
];

/** The top accounts table's column (UsageBoard) that a server sort fills. */
export function sortCol(sort: string): string {
  switch (sort) {
    case "stored": return "stored";
    case "events": return "events";
    case "messages": return "msgs";
    case "reports_received": return "reports";
    case "games": return "games";
    case "active_days": return "days";
    default: return "up";
  }
}
