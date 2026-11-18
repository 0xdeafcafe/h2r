// Fixed data for the dense dashboard's style page (/dash/admin/style): a
// plausible server's usage over 30 days and its top accounts. The shapes
// are a draft of the admin usage endpoints (slice 2 settles them in
// docs/live/live-api.md); nothing here is fetched. Seeded, so every load
// (and every test) sees the same numbers.

import { dayRange, type Day } from "./chart";

export const UPLOAD_KINDS = [
  { key: "events", label: "events" },
  { key: "frames", label: "live frames" },
  { key: "content", label: "blobs and content" },
  { key: "relay", label: "co-op relay" },
] as const;
export type UploadKind = (typeof UPLOAD_KINDS)[number]["key"];

export const MESSAGE_KINDS = [
  { key: "friend_requests", label: "friend requests" },
  { key: "invites", label: "game invites" },
  { key: "text", label: "text" },
] as const;
export type MessageKind = (typeof MESSAGE_KINDS)[number]["key"];

export interface UsageDay {
  day: Day;
  bytes_up: Record<UploadKind, number>;
  events: number;
  messages: Record<MessageKind, number>;
  games: number;
  sessions_hosted: number;
  sign_ins: number;
  active_accounts: number;
}

export interface AccountUsage {
  xuid: string;
  gamertag: string;
  bytes_up: Record<UploadKind, number>;
  bytes_stored: number;
  events: number;
  messages: number;
  games: number;
  sessions_hosted: number;
  sign_ins: number;
  active_days: number;
  last_seen: Day;
}

export interface EventTypeCount { type: string; count: number; bytes: number }

export interface UsageMock {
  days: UsageDay[];
  accounts: AccountUsage[];
  event_types: EventTypeCount[];
  stored: Record<UploadKind | "saves", number>;
}

/** A small deterministic generator (mulberry32). */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const MB = 1024 * 1024;
const TAGS = [
  "Sgt Marmalade", "Quiet Lantern", "Ferrous", "Moth Rally", "Kilowatt Kid", "Ribbonfish", "Halcyon Mule",
  "Ostrich Ops", "Tin Cardinal", "Velvet Rover", "Gravel Saint", "Pocket Orbit", "Mister Dune", "Lowtide",
  "Cobalt Wren", "Nimbus Ate", "Brass Otter", "Juniper Six",
];

export function usageMock(last: Day = "2026-11-18", n = 30, seed = 7): UsageMock {
  const r = rng(seed);
  const days = dayRange(last, n).map((day, i): UsageDay => {
    const weekend = [0, 6].includes(new Date(`${day}T00:00:00Z`).getUTCDay());
    const grow = 0.6 + (0.4 * i) / n;
    const busy = (weekend ? 1.6 : 1) * grow * (0.75 + r() * 0.5);
    const games = Math.round(14 * busy);
    return {
      day,
      bytes_up: {
        events: Math.round(games * (0.9 + r() * 0.3) * MB),
        frames: Math.round(games * (5.5 + r() * 2.5) * MB),
        content: Math.round((r() < 0.25 ? 40 + r() * 120 : r() * 8) * MB),
        relay: Math.round(games * 0.35 * (1.6 + r()) * MB),
      },
      events: Math.round(games * (1150 + r() * 400)),
      messages: {
        friend_requests: Math.round(3 * busy * r()),
        invites: Math.round(9 * busy * (0.6 + r() * 0.6)),
        text: Math.round(22 * busy * (0.5 + r())),
      },
      games,
      sessions_hosted: Math.round(games * (0.55 + r() * 0.2)),
      sign_ins: Math.round(26 * busy),
      active_accounts: Math.round(11 * busy + r() * 3),
    };
  });
  // A long tail: a few accounts carry most of it, as on any real server.
  const accounts = TAGS.map((gamertag, i): AccountUsage => {
    const w = 1 / (i + 1.2) ** 1.15 * (0.8 + r() * 0.4);
    const total = (k: UploadKind) => days.reduce((a, d) => a + d.bytes_up[k], 0);
    const heavyContent = i === 3 ? 3.5 : 1;
    return {
      xuid: `0009${(0xa000 + i * 0x111).toString(16)}00c0ffee`,
      gamertag,
      bytes_up: {
        events: Math.round(total("events") * w * 0.4),
        frames: Math.round(total("frames") * w * 0.45 * (i % 4 === 0 ? 1.4 : 1)),
        content: Math.round(total("content") * w * 0.3 * heavyContent),
        relay: Math.round(total("relay") * w * 0.4),
      },
      bytes_stored: Math.round((40 + 900 * w * heavyContent) * MB),
      events: Math.round(days.reduce((a, d) => a + d.events, 0) * w * 0.4),
      messages: Math.round(240 * w * (0.4 + r())),
      games: Math.round(days.reduce((a, d) => a + d.games, 0) * w * 0.9),
      sessions_hosted: Math.round(days.reduce((a, d) => a + d.sessions_hosted, 0) * w * 0.7),
      sign_ins: Math.round(160 * w + 3),
      active_days: Math.min(n, Math.round(4 + 30 * w * 1.6)),
      last_seen: days[days.length - 1 - Math.floor(r() * Math.min(n, 3 + i))]!.day,
    };
  });
  const event_types: EventTypeCount[] = [
    ["GameFrame", 0.58, 900], ["PlayerKilled", 0.17, 260], ["ScoreChanged", 0.11, 140], ["PresenceChanged", 0.05, 180],
    ["PlayerJoined", 0.025, 220], ["PlayerLeft", 0.022, 200], ["GameStarted", 0.008, 640], ["GameEnded", 0.008, 1200],
    ["SignedIn", 0.004, 300], ["GameInviteSent", 0.003, 420], ["FriendRequested", 0.001, 260], ["ContentStored", 0.001, 380],
  ].map(([type, share, size]) => {
    const total = days.reduce((a, d) => a + d.events, 0);
    const count = Math.round(total * (share as number));
    return { type: type as string, count, bytes: count * (size as number) };
  });
  const sum = (k: UploadKind) => accounts.reduce((a, x) => a + x.bytes_up[k], 0);
  const stored = { events: sum("events") * 3, frames: sum("frames") * 0.8, content: sum("content") * 2.2, relay: 0, saves: 310 * MB };
  return { days, accounts, event_types, stored: Object.fromEntries(Object.entries(stored).map(([k, v]) => [k, Math.round(v)])) as UsageMock["stored"] };
}

/** An account's upload over every kind. */
export const uploaded = (a: { bytes_up: Record<UploadKind, number> }) => UPLOAD_KINDS.reduce((s, k) => s + a.bytes_up[k.key], 0);
/** A day's messages over every kind. */
export const messagesOf = (d: UsageDay) => MESSAGE_KINDS.reduce((s, k) => s + d.messages[k.key], 0);
