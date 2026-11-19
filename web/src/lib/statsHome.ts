// The stats home's numbers: the server's summary (`GET /v1/stats/summary`,
// docs/live/live-api.md "Stats summary") read defensively, and the boards, picks
// and grids the page draws from it. Pure, so tests share them.

export type StatsWindow = "today" | "7d" | "30d" | "all";
export const STATS_WINDOWS: ReadonlyArray<{ id: StatsWindow; label: string; short: string }> = [
  { id: "today", label: "Today", short: "today" },
  { id: "7d", label: "This Week", short: "this week" },
  { id: "30d", label: "This Month", short: "in the last 30 days" },
  { id: "all", label: "All Time", short: "all time" },
];

export function statsWindow(asked: string | null | undefined): StatsWindow {
  return STATS_WINDOWS.some((w) => w.id === asked) ? (asked as StatsWindow) : "7d";
}

export interface Totals {
  games: number; multiplayer: number; campaign: number; matchmaking: number; clankermatch: number; custom: number; coop: number;
  kills: number; deaths: number; betrayals: number; suicides: number; campaign_deaths: number;
  player_seconds: number; legendary: number;
}

export interface PlayerRow {
  xuid: string; name: string; games: number; multiplayer: number; campaign: number; wins: number;
  kills: number; deaths: number; betrayals: number; best_spree: number; seconds: number; legendary: number;
}

export interface MapRow { map: string; map_id?: string; campaign: boolean; games: number; kills: number }
export interface WeaponRow { weapon: string; kills: number }
export interface PlaylistRow { playlist: string; games: number }
export interface SkullRow { skull: number; games: number }
export interface ActivityRow { hour: string; games: number; kills: number }

export interface StatsSummary {
  window: StatsWindow;
  from: string | null;
  to: string;
  position: number;
  totals: Totals;
  last_24h: Totals;
  all_time: Totals;
  players: PlayerRow[];
  maps: MapRow[];
  weapons: WeaponRow[];
  playlists: PlaylistRow[];
  skulls: SkullRow[];
  activity: ActivityRow[];
}

const TOTAL_KEYS: Array<keyof Totals> = [
  "games", "multiplayer", "campaign", "matchmaking", "clankermatch", "custom", "coop", "kills", "deaths", "betrayals",
  "suicides", "campaign_deaths", "player_seconds", "legendary",
];

const n = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : 0);
const s = (v: unknown) => (typeof v === "string" ? v : "");
const arr = (v: unknown): Record<string, unknown>[] =>
  Array.isArray(v) ? v.filter((x): x is Record<string, unknown> => !!x && typeof x === "object") : [];

export function emptyTotals(): Totals {
  return Object.fromEntries(TOTAL_KEYS.map((k) => [k, 0])) as unknown as Totals;
}

function totals(v: unknown): Totals {
  const o = (v && typeof v === "object" ? v : {}) as Record<string, unknown>;
  return Object.fromEntries(TOTAL_KEYS.map((k) => [k, n(o[k])])) as unknown as Totals;
}

/** Reads a summary body; anything missing or malformed reads as nothing. */
export function parseSummary(raw: unknown): StatsSummary {
  const o = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  return {
    window: statsWindow(s(o.window)),
    from: typeof o.from === "string" ? o.from : null,
    to: s(o.to),
    position: n(o.position),
    totals: totals(o.totals),
    last_24h: totals(o.last_24h),
    all_time: totals(o.all_time),
    players: arr(o.players).filter((p) => typeof p.xuid === "string").map((p) => ({
      xuid: s(p.xuid), name: s(p.name), games: n(p.games), multiplayer: n(p.multiplayer), campaign: n(p.campaign),
      wins: n(p.wins), kills: n(p.kills), deaths: n(p.deaths), betrayals: n(p.betrayals), best_spree: n(p.best_spree),
      seconds: n(p.seconds), legendary: n(p.legendary),
    })),
    maps: arr(o.maps).filter((m) => typeof m.map === "string").map((m) => ({
      map: s(m.map), ...(typeof m.map_id === "string" ? { map_id: m.map_id } : {}), campaign: m.campaign === true,
      games: n(m.games), kills: n(m.kills),
    })),
    weapons: arr(o.weapons).filter((w) => typeof w.weapon === "string").map((w) => ({ weapon: s(w.weapon), kills: n(w.kills) })),
    playlists: arr(o.playlists).filter((p) => typeof p.playlist === "string").map((p) => ({ playlist: s(p.playlist), games: n(p.games) })),
    skulls: arr(o.skulls).filter((k) => typeof k.skull === "number").map((k) => ({ skull: n(k.skull), games: n(k.games) })),
    activity: arr(o.activity).filter((a) => typeof a.hour === "string" && !Number.isNaN(Date.parse(s(a.hour))))
      .map((a) => ({ hour: s(a.hour), games: n(a.games), kills: n(a.kills) })),
  };
}

/** Kills per death; kills alone when there are no deaths. */
export function kd(kills: number, deaths: number): number {
  return deaths > 0 ? kills / deaths : kills;
}

/** Whole hours, rounded down past the first. */
export function hoursOf(seconds: number): number {
  return seconds >= 3600 ? Math.floor(seconds / 3600) : Math.round((seconds / 3600) * 10) / 10;
}

export interface BoardRow { xuid: string; name: string; value: number; text: string }
export interface Board { id: string; title: string; unit: string; note?: string; rows: BoardRow[] }

/** Fewest multiplayer games for the K/D board. */
export const KD_MIN_GAMES = 3;

function board(id: string, title: string, unit: string, players: PlayerRow[], value: (p: PlayerRow) => number,
  text: (v: number) => string = String, keep: (p: PlayerRow) => boolean = () => true, note?: string, top = 5): Board {
  const rows = players.filter(keep).map((p) => ({ p, v: value(p) })).filter((r) => r.v > 0)
    // Ties: more games first, then by name, so the order is stable.
    .sort((a, b) => b.v - a.v || b.p.games - a.p.games || a.p.name.localeCompare(b.p.name))
    .slice(0, top)
    .map(({ p, v }) => ({ xuid: p.xuid, name: p.name, value: v, text: text(v) }));
  return { id, title, unit, note, rows };
}

/** The leaderboards the home shows, from the window's player rows. */
export function leaderboards(players: PlayerRow[], minGames = KD_MIN_GAMES): Board[] {
  return [
    board("kills", "Most Kills", "kills", players, (p) => p.kills),
    board("kd", "Best K/D Ratio", "K/D", players, (p) => kd(p.kills, p.deaths), (v) => v.toFixed(2),
      (p) => p.multiplayer >= minGames, `${minGames}+ multiplayer games`),
    board("wins", "Most Wins", "wins", players, (p) => p.wins),
    board("spree", "Longest Killing Spree", "kills", players, (p) => p.best_spree, String, () => true, "kills in a row without dying"),
    board("legendary", "Legendary Levels", "levels", players, (p) => p.legendary, String, () => true, "levels beaten on Legendary"),
    board("time", "Most Time Played", "hours", players, (p) => p.seconds, (v) => `${hoursOf(v)} h`),
  ];
}

/** The window's most used weapon and most played multiplayer map (then any
 *  map), or null when there are none. */
export function ofTheWeek(sum: Pick<StatsSummary, "weapons" | "maps">): { weapon: WeaponRow | null; map: MapRow | null } {
  const weapon = sum.weapons.find((w) => w.kills > 0) ?? null;
  const mp = sum.maps.filter((m) => !m.campaign && m.games > 0);
  const map = mp[0] ?? sum.maps.find((m) => m.games > 0) ?? null;
  return { weapon, map };
}

/** Share of a list's total, as a whole percent. */
export function shares<T>(rows: T[], value: (r: T) => number, top = 5): Array<{ row: T; value: number; pct: number }> {
  const total = rows.reduce((a, r) => a + value(r), 0);
  return rows.slice(0, top).map((row) => ({ row, value: value(row), pct: total ? Math.round((value(row) / total) * 100) : 0 }));
}

export const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;

/** Where an instant falls in the viewer's week: day 0 = Sunday, hour 0-23. */
export type Slot = (d: Date) => [day: number, hour: number];
export const localSlot: Slot = (d) => [d.getDay(), d.getHours()];
export const utcSlot: Slot = (d) => [d.getUTCDay(), d.getUTCHours()];

/** Games started per day of the week and hour, in the viewer's time:
 *  `grid[day][hour]`, plus the busiest cell's count. */
export function heatmap(activity: ActivityRow[], slot: Slot = localSlot, value: (a: ActivityRow) => number = (a) => a.games):
  { grid: number[][]; max: number; total: number } {
  const grid = DAYS.map(() => new Array<number>(24).fill(0));
  let max = 0, total = 0;
  for (const a of activity) {
    const [d, h] = slot(new Date(a.hour));
    const v = value(a);
    grid[d]![h]! += v;
    max = Math.max(max, grid[d]![h]!);
    total += v;
  }
  return { grid, max, total };
}

/** The busiest day and hour of a heatmap, or null when it is empty. */
export function peak(grid: number[][]): { day: number; hour: number; value: number } | null {
  let best: { day: number; hour: number; value: number } | null = null;
  grid.forEach((row, day) => row.forEach((value, hour) => {
    if (value > 0 && (!best || value > best.value)) best = { day, hour, value };
  }));
  return best;
}

/** "9 pm", "12 am". */
export function hourName(h: number): string {
  const n = h % 12 === 0 ? 12 : h % 12;
  return `${n} ${h < 12 ? "am" : "pm"}`;
}

/** Five steps of a sequential scale for a heatmap cell: 0 empty, 1-4 by
 *  share of the busiest cell. */
export function level(v: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (v <= 0 || max <= 0) return 0;
  return Math.min(4, Math.max(1, Math.ceil((v / max) * 4))) as 1 | 2 | 3 | 4;
}

export interface ClanLike { id: string; name: string; members: string[] }
export interface ClanStanding { id: string; name: string; members: number; active: number; games: number; kills: number; deaths: number; wins: number }

/** Clans ranked by their members' combined multiplayer kills in the window
 *  (each member's own record, wherever they played). Clans with no active
 *  member are left out. */
export function clanStandings(clans: ClanLike[], players: PlayerRow[], top = 5): ClanStanding[] {
  const by = new Map(players.map((p) => [p.xuid, p]));
  return clans.map((c) => {
    const rows = c.members.map((x) => by.get(x)).filter((p): p is PlayerRow => !!p && p.multiplayer > 0);
    return {
      id: c.id, name: c.name, members: c.members.length, active: rows.length,
      games: rows.reduce((a, p) => a + p.multiplayer, 0), kills: rows.reduce((a, p) => a + p.kills, 0),
      deaths: rows.reduce((a, p) => a + p.deaths, 0), wins: rows.reduce((a, p) => a + p.wins, 0),
    };
  }).filter((c) => c.active > 0)
    .sort((a, b) => b.kills - a.kills || b.wins - a.wins || a.name.localeCompare(b.name))
    .slice(0, top);
}

/** Games started on each of the last `days` days (the viewer's days),
 *  oldest first, today last. From the summary's hourly activity, which a
 *  `30d` summary carries for the whole span. */
export function dailyGames(activity: ActivityRow[], now: number, days = 14): Array<{ day: Date; games: number }> {
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const out = Array.from({ length: days }, (_, i) => {
    const d = new Date(today);
    d.setDate(d.getDate() - (days - 1 - i));
    return { day: d, games: 0 };
  });
  const first = out[0]!.day.getTime();
  for (const a of activity) {
    const t = Date.parse(a.hour);
    if (!(t >= first) || t > now) continue;
    const d = new Date(t);
    d.setHours(0, 0, 0, 0);
    const i = out.findIndex((o) => o.day.getTime() === d.getTime());
    if (i >= 0) out[i]!.games += a.games;
  }
  return out;
}

export interface FactRow { label: string; value: string }

/** The rows worth showing out of `rows`: the zeros dropped (a young
 *  server's Kills 0 and Legendary 0 say nothing), keeping any marked
 *  `always` (they count towards `max` and are never the ones dropped). */
export function nonZeroRows(rows: Array<{ label: string; n: number; text?: string; always?: boolean }>, max: number): FactRow[] {
  let room = max - rows.filter((r) => r.always).length;
  return rows
    .filter((r) => r.always || (r.n > 0 && room-- > 0))
    .map((r) => ({ label: r.label, value: r.text ?? r.n.toLocaleString("en-US") }));
}

/** The week's most played campaign level, for when the week has no kills
 *  to name a weapon by. */
export function missionOfTheWeek(sum: Pick<StatsSummary, "maps">): MapRow | null {
  return sum.maps.filter((m) => m.campaign && m.games > 0).sort((a, b) => b.games - a.games)[0] ?? null;
}

/** Multiplayer maps after the map of the week, most played first. */
export function alsoPopular(sum: Pick<StatsSummary, "maps">, pick: MapRow | null, n = 3): MapRow[] {
  return sum.maps.filter((m) => m !== pick && !m.campaign && m.games > 0).slice(0, n);
}
