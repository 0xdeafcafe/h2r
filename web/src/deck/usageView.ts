// Usage numbers shaped for the deck's panels: the range's days filled in,
// the series for each chart, the tiles with their change against the
// period before, and the ranked lists with their "other". Used by the style
// page with fixed data (./mock.ts); the admin usage screen reads the same
// shapes from the server.

import { compact, dayRange, delta, fillDays, periods, topN, type Day, type Delta, type Series } from "./chart";
import { MESSAGE_KINDS, UPLOAD_KINDS, messagesOf, type EventTypeCount, type UsageDay, type UsageMock } from "./mock";
import type { RankRow } from "./Deck";

export interface Tile { key: string; label: string; value: number; delta: Delta; trend: number[]; bytes?: boolean; upIsGood: boolean }

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
}

const zeroDay = (day: Day): UsageDay => ({
  day,
  bytes_up: { events: 0, frames: 0, content: 0, relay: 0 },
  events: 0,
  messages: { friend_requests: 0, invites: 0, text: 0 },
  games: 0, sessions_hosted: 0, sign_ins: 0, active_accounts: 0,
});

const uploadOf = (d: UsageDay) => UPLOAD_KINDS.reduce((a, k) => a + d.bytes_up[k.key], 0);

/** The last `range` days ending on `last`, against the `range` before for the tiles' change. */
export function usageView(m: UsageMock, last: Day, range: number, topTypes = 7): UsageView {
  const all = fillDays(m.days, dayRange(last, range * 2), zeroDay);
  const cur = all.slice(range);
  const days = cur.map((d) => d.day);
  const col = (f: (d: UsageDay) => number) => cur.map(f);
  const tile = (key: string, label: string, f: (d: UsageDay) => number, extra: Partial<Tile> = {}): Tile => {
    const p = periods(all.map(f), range);
    return { key, label, value: p.now, delta: delta(p.now, p.before), trend: col(f), upIsGood: true, ...extra };
  };
  const storedRows = (Object.entries(m.stored) as [string, number][]).map(([k, v]) => ({ key: k, label: storedLabel(k), value: v }));
  return {
    days,
    upload: UPLOAD_KINDS.map((k) => ({ key: k.key, label: k.label, values: col((d) => d.bytes_up[k.key]) })),
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
      tile("hosted", "sessions hosted", (d) => d.sessions_hosted),
      tile("sign_ins", "sign-ins", (d) => d.sign_ins),
    ],
    eventTypes: topN<RankRow>(
      m.event_types.map((t: EventTypeCount) => ({ key: t.type, label: t.type, value: t.count })),
      topTypes, (r) => r.value,
      (sum, count) => ({ key: "_other", label: `${count} other ${count === 1 ? "type" : "types"}`, value: sum, other: true }),
    ),
    stored: topN<RankRow>(storedRows.filter((r) => r.value > 0), storedRows.length, (r) => r.value),
    storedTotal: storedRows.reduce((a, r) => a + r.value, 0),
  };
}

function storedLabel(k: string): string {
  return UPLOAD_KINDS.find((u) => u.key === k)?.label ?? (k === "saves" ? "cloud saves" : k);
}

/** A tile's value as text: bytes or a compact count. */
export const tileValue = (t: Tile, bytes: (n: number) => string) => (t.bytes ? bytes(t.value) : compact(t.value));
