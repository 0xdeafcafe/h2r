// Fixed data for the dense dashboard's style page (/dash/admin/style) and
// the dev mock's usage endpoints: a plausible server's usage, in the shapes
// GET /v1/admin/usage and /v1/admin/usage/accounts answer (src/admin/usage.ts).
// Seeded, so every load (and every test) sees the same numbers.

import { dayRange, type Day } from "./chart";
import type { AccountUsage, UsageDay, UsageOverview } from "../admin/usage";
import { MESSAGE_KINDS, STORED_KINDS, UPLOAD_KINDS } from "../admin/usage";

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
const TYPES: [string, number][] = [
  ["GameFrame", 0.55], ["PlayerKilled", 0.17], ["ScoreChanged", 0.11], ["PresenceChanged", 0.05],
  ["PlayerJoined", 0.025], ["PlayerLeft", 0.022], ["GameStarted", 0.008], ["GameEnded", 0.008], ["RankChanged", 0.006],
  ["SignedIn", 0.004], ["GameInviteSent", 0.003], ["FriendRequested", 0.001], ["ContentStored", 0.001], ["PlayerReported", 0.0004],
];

export interface UsageSample { days: UsageDay[]; accounts: AccountUsage[]; stored: UsageOverview["stored"] }

/** `n` days of usage ending on `last`, and the accounts behind them. */
export function usageMock(last: Day = "2026-11-18", n = 60, seed = 7): UsageSample {
  const r = rng(seed);
  const days = dayRange(last, n).map((day, i): UsageDay => {
    const weekend = [0, 6].includes(new Date(`${day}T00:00:00Z`).getUTCDay());
    const busy = (weekend ? 1.6 : 1) * (0.6 + (0.4 * i) / n) * (0.75 + r() * 0.5);
    const games = Math.round(14 * busy);
    return {
      day,
      up: {
        events: Math.round(games * (0.9 + r() * 0.3) * MB),
        frames: Math.round(games * (5.5 + r() * 2.5) * MB),
        content: Math.round((r() < 0.25 ? 40 + r() * 120 : r() * 8) * MB),
        saves: Math.round(busy * (2 + r() * 3) * MB),
      },
      events: Math.round(games * (1150 + r() * 400)),
      messages: { friend_requests: Math.round(3 * busy * r()), invites: Math.round(9 * busy * (0.6 + r() * 0.6)) },
      games,
      sessions_hosted: Math.round(games * (0.55 + r() * 0.2)),
      sign_ins: Math.round(26 * busy),
      reports: Math.round(r() * r() * 3),
      active_accounts: Math.round(11 * busy + r() * 3),
    };
  });
  const sum = (f: (d: UsageDay) => number) => days.slice(-30).reduce((a, d) => a + f(d), 0);
  // A long tail: a few accounts carry most of it, as on any real server.
  const accounts = TAGS.map((gamertag, i): AccountUsage => {
    const w = (1 / (i + 1.2) ** 1.15) * (0.8 + r() * 0.4);
    const heavy = i === 3 ? 3.5 : 1;
    const up = {
      events: Math.round(sum((d) => d.up.events) * w * 0.4),
      frames: Math.round(sum((d) => d.up.frames) * w * 0.45 * (i % 4 === 0 ? 1.4 : 1)),
      content: Math.round(sum((d) => d.up.content) * w * 0.3 * heavy),
      saves: Math.round(sum((d) => d.up.saves) * w * 0.4),
    };
    const stored = { events: up.events * 3, content: Math.round(up.content * 2.2), saves: Math.round((8 + 40 * w) * MB) };
    const messages = { friend_requests: Math.round(20 * w * r()), invites: Math.round(200 * w * (0.4 + r())) };
    return {
      xuid: `0009${(0xa000 + i * 0x111).toString(16)}00c0ffee`,
      gamertag,
      up, uploaded: up.events + up.frames + up.content + up.saves,
      stored, stored_total: stored.events + stored.content + stored.saves,
      events: Math.round(sum((d) => d.events) * w * 0.4),
      messages, messages_total: messages.friend_requests + messages.invites,
      games: Math.round(sum((d) => d.games) * w * 0.9),
      sessions_hosted: Math.round(sum((d) => d.sessions_hosted) * w * 0.7),
      sign_ins: Math.round(160 * w + 3),
      reports_sent: Math.round(r() * 2),
      reports_received: i === 6 ? 4 : Math.round(r() * r()),
      active_days: Math.min(30, Math.round(4 + 30 * w * 1.6)),
      last_active: days[days.length - 1 - Math.floor(r() * Math.min(n, 3 + i))]!.day,
    };
  });
  const stored = Object.fromEntries(STORED_KINDS.map((k) => [k.key, accounts.reduce((a, x) => a + x.stored[k.key], 0)])) as UsageOverview["stored"];
  return { days, accounts, stored };
}

/** The overview the server would answer for the `days` days ending on the sample's last. */
export function mockOverview(s: UsageSample, days: number): UsageOverview {
  const ds = s.days.slice(-days);
  const totals: UsageDay = {
    day: "",
    up: Object.fromEntries(UPLOAD_KINDS.map((k) => [k.key, ds.reduce((a, d) => a + d.up[k.key], 0)])) as UsageDay["up"],
    events: ds.reduce((a, d) => a + d.events, 0),
    messages: Object.fromEntries(MESSAGE_KINDS.map((k) => [k.key, ds.reduce((a, d) => a + d.messages[k.key], 0)])) as UsageDay["messages"],
    games: ds.reduce((a, d) => a + d.games, 0),
    sessions_hosted: ds.reduce((a, d) => a + d.sessions_hosted, 0),
    sign_ins: ds.reduce((a, d) => a + d.sign_ins, 0),
    reports: ds.reduce((a, d) => a + d.reports, 0),
    active_accounts: ds.reduce((a, d) => a + d.active_accounts, 0),
  };
  return {
    from: ds[0]?.day ?? "",
    to: ds[ds.length - 1]?.day ?? "",
    keep_days: 400,
    days: ds,
    totals,
    accounts_active: Math.min(s.accounts.length, Math.round(Math.sqrt(days) * 4)),
    event_types: TYPES.map(([type, share]) => ({ type, count: Math.round(totals.events * share) })).sort((a, b) => b.count - a.count),
    stored: s.stored,
  };
}
