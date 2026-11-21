// Per-playlist ranks (docs/live/live-matchmaking.md §8): a player's level
// 1..50 and experience in each ranked playlist, as the server works them
// out after every ranked game. Parsers for the rank reads, the playlist
// menu (which playlists the served hopper file holds, in three groups),
// and the small sums the pages show.

export interface Standing { level: number; experience: number; highest: number; games: number; wins: number }
export interface RankEntry { gameId: string; at: string; before: Standing; after: Standing }
export interface BoardRow extends Standing { xuid: string; gamertag: string; bot?: true }

export const MIN_LEVEL = 1, MAX_LEVEL = 50;

/** The server's built-in lineup, hopper by hopper: what it serves and
 *  ranks when no hopper file was published (live-proto's
 *  hopper::defaults, kept in step by hand). Each row is
 *  `[name, description, hopper type, type params, game types, clanker?]`;
 *  type params are the three at +0x5ec — players for free for all
 *  (types 1, 2), teams then team size for team types (3, 4, 5). */
const DEFAULT_LINEUP: Array<[string, string, number, [number, number, number], string[], boolean?]> = [
  ["Rumble Pit", "Every player for themselves, up to 8. Slayer, with some Oddball and King of the Hill.", 2, [4, 8, 0], ["Rumble Slayer", "Oddball", "King of the Hill"]],
  ["Team Slayer", "Four against four Slayer on the small and medium maps.", 4, [2, 3, 4], ["Team Slayer"]],
  ["Team Skirmish", "Two against two. Bring a partner or find one here.", 4, [2, 2, 2], ["Team Slayer", "Teamball"]],
  ["Team Snipers", "Four against four with sniper rifles and no vehicles.", 4, [2, 3, 4], ["Team Snipers"]],
  ["Big Team Battle", "Up to eight a side on the big maps: Slayer, CTF, Assault and Territories.", 4, [2, 5, 8], ["Team Slayer", "Classic CTF", "Assault", "Territories"]],
  ["Team Objective", "Four against four: Capture the Flag, Assault, Territories, King and Oddball.", 4, [2, 3, 4], ["Classic CTF", "Assault", "Territories", "Team King", "Teamball"]],
  ["Free for All", "Unranked free for all, 2 to 8 players. Parties welcome.", 1, [2, 8, 0], ["Rumble Slayer", "Oddball", "King of the Hill"]],
  ["Unranked Teams", "Unranked team games for parties of any size, 2 to 4 a side.", 3, [2, 2, 4], ["Team Slayer", "Classic CTF", "Team King", "Teamball"]],
  ["Clanker Slayer", "Free for all Slayer, up to 8 players, where every other player is a bot.", 1, [2, 8, 0], ["Rumble Slayer"], true],
  ["Clanker Teams", "Four against four Team Slayer. Bots fill every seat your party does not.", 3, [2, 1, 4], ["Team Slayer"], true],
  ["Clanker Doubles", "Two against two Team Slayer. Bots fill every seat your party does not.", 3, [2, 1, 2], ["Team Slayer"], true],
];

/** The server's built-in playlists, by hopper index, for when the hopper
 *  file hasn't been read: the same names its default set uses. */
export const DEFAULT_PLAYLISTS: Readonly<Record<string, string>> = Object.fromEntries(
  DEFAULT_LINEUP.map(([name], i) => [String(i), name]),
);

export function playlistName(id: string, names: Readonly<Record<string, string>> = DEFAULT_PLAYLISTS): string {
  return names[id] ?? `Playlist ${id}`;
}

const int = (v: unknown, lo: number, hi: number): number | null =>
  typeof v === "number" && Number.isInteger(v) && v >= lo && v <= hi ? v : null;

export function parseStanding(v: unknown): Standing | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  const level = int(o.level, MIN_LEVEL, MAX_LEVEL), highest = int(o.highest, MIN_LEVEL, MAX_LEVEL);
  const experience = int(o.experience, 0, 0x3fffffff), games = int(o.games, 0, 2 ** 32), wins = int(o.wins, 0, 2 ** 32);
  if (level === null || highest === null || experience === null || games === null || wins === null) return null;
  return { level, experience, highest, games, wins };
}

/** `GET /v1/accounts/{who}/ranks`: playlist id -> standing, best level first. */
export function parseRanks(body: unknown): Array<[string, Standing]> {
  const ranks = (body as { ranks?: unknown } | null)?.ranks;
  if (!ranks || typeof ranks !== "object") return [];
  const out: Array<[string, Standing]> = [];
  for (const [id, v] of Object.entries(ranks)) {
    const s = parseStanding(v);
    if (s) out.push([id, s]);
  }
  return out.sort((a, b) => b[1].level - a[1].level || b[1].experience - a[1].experience || a[0].localeCompare(b[0], undefined, { numeric: true }));
}

/** `GET /v1/accounts/{who}/ranks/{playlist}`: the standing and its history, newest first. */
export function parseHistory(body: unknown): { standing: Standing | null; history: RankEntry[] } {
  const o = (body ?? {}) as { standing?: unknown; history?: unknown };
  const history: RankEntry[] = [];
  for (const h of Array.isArray(o.history) ? o.history : []) {
    const r = h as Record<string, unknown>;
    const before = parseStanding(r.before), after = parseStanding(r.after);
    if (typeof r.game_id === "string" && typeof r.at === "string" && before && after) history.push({ gameId: r.game_id, at: r.at, before, after });
  }
  return { standing: parseStanding(o.standing), history };
}

/** `GET /v1/ranks/{playlist}`: the leaderboard, in the server's order. A
 *  row the server marks `bot` (a persistent bot account, ranked with
 *  everyone) keeps the mark. */
export function parseBoard(body: unknown): BoardRow[] {
  const items = (body as { items?: unknown } | null)?.items;
  const out: BoardRow[] = [];
  for (const it of Array.isArray(items) ? items : []) {
    const r = it as Record<string, unknown>;
    const s = parseStanding(r);
    if (s && typeof r.xuid === "string" && typeof r.gamertag === "string") {
      out.push(r.bot === true ? { ...s, xuid: r.xuid, gamertag: r.gamertag, bot: true } : { ...s, xuid: r.xuid, gamertag: r.gamertag });
    }
  }
  return out;
}

/** Experience needed for a level: the server's table (live_proto::rank::threshold). */
export function threshold(level: number): number {
  const k = Math.min(Math.max(level, MIN_LEVEL), MAX_LEVEL) - 1;
  return 25 * k * k + 75 * k;
}

/** How far a standing is from its level to the next, 0..1 (1 at the top). */
export function progress(s: Standing): number {
  if (s.level >= MAX_LEVEL) return 1;
  const from = threshold(s.level), to = threshold(s.level + 1);
  return Math.min(1, Math.max(0, (s.experience - from) / (to - from)));
}

/** One history row as words: "+100 XP, level 1 → 2". */
export function changeText(e: RankEntry): string {
  const d = e.after.experience - e.before.experience;
  const xp = `${d >= 0 ? "+" : "−"}${Math.abs(d)} XP`;
  return e.after.level === e.before.level ? `${xp}, level ${e.after.level}` : `${xp}, level ${e.before.level} → ${e.after.level}`;
}

export function winRate(s: Standing): string {
  return s.games ? `${Math.round((100 * s.wins) / s.games)}%` : "–";
}

// ---- The playlist menu (docs/live/live-matchmaking.md §6): which
// playlists the served hopper file holds, in the side menu's three
// groups. ----

/** Which side-menu group a playlist sits in. Clankermatch hoppers are
 *  their own group whatever their hopper type, as ranking treats them. */
export type PlaylistGroup = "ranked" | "unranked" | "clanker";

/** The groups in menu order, with their headings. */
export const PLAYLIST_GROUPS: ReadonlyArray<[PlaylistGroup, string]> = [
  ["ranked", "Ranked"], ["unranked", "Unranked"], ["clanker", "Clankermatch"],
];

export interface PlaylistInfo {
  /** The hopper's descriptor index: what a board's URL and the standings name. */
  id: string;
  name: string;
  description: string;
  group: PlaylistGroup;
  /** The hopper's rules in words ("4 vs 4", "2–8 players"); null when the
   *  file's type params don't read. */
  size: string | null;
  /** The game types its entries carry, in file order, once each. */
  games: string[];
  hasMapPackMaps?: boolean;
}

/** `GET /v1/hoppers/clankers`: the descriptor indices marked clanker. */
export function parseClankerMarks(body: unknown): ReadonlySet<number> {
  const out = new Set<number>();
  for (const h of Array.isArray((body as { hoppers?: unknown } | null)?.hoppers) ? (body as { hoppers: unknown[] }).hoppers : []) {
    const id = (h as Record<string, unknown> | null)?.hopper_id;
    if (typeof id === "number" && Number.isInteger(id) && id >= 0) out.add(id);
  }
  return out;
}

/** A hopper type's rules in words, from the three type params at +0x5ec:
 *  players for free for all, teams then team size for team types. */
function sizeText(type: number, r: unknown): string | null {
  if (!Array.isArray(r) || r.length < 3 || !r.every((n) => typeof n === "number" && Number.isInteger(n) && n >= 0)) return null;
  if (type === 1 || type === 2) {
    const [lo, hi] = [r[0]!, r[1]!];
    if (!lo || !hi || lo > hi) return null;
    return `${lo === hi ? lo : `${lo}–${hi}`} players, free for all`;
  }
  const [teams, lo, hi] = [r[0]!, r[1]!, r[2]!];
  if (!teams || !lo || !hi || lo > hi) return null;
  return teams === 2 && lo === hi ? `${lo} vs ${lo}` : `${teams} teams of ${lo === hi ? lo : `${lo}–${hi}`}`;
}

/** One hopper of `GET /v1/titles/4D530064/hopper` (the editors' view of
 *  the published file) as a menu entry; unused slots (type 0) are null. */
function hopperToPlaylist(h: unknown, marked: ReadonlySet<number>): PlaylistInfo | null {
  const o = (h ?? {}) as Record<string, unknown>;
  const index = o.index, type = o.type;
  if (typeof index !== "number" || !Number.isInteger(index) || index < 0) return null;
  if (typeof type !== "number" || type < 1 || type > 5) return null;
  const name = typeof o.title === "string" && o.title.trim() ? o.title.trim() : `Playlist ${index}`;
  const description = typeof o.description === "string" ? o.description.trim() : "";
  const games: string[] = [];
  for (const r of Array.isArray(o.records) ? o.records : []) {
    const v = (r as Record<string, unknown> | null)?.variant as Record<string, unknown> | null | undefined;
    const n = typeof v?.name === "string" ? v.name.trim() : "";
    if (n && !games.includes(n)) games.push(n);
  }
  return {
    id: String(index),
    name,
    description,
    group: marked.has(index) ? "clanker" : type === 2 || type === 4 || type === 5 ? "ranked" : "unranked",
    size: sizeText(type, o.ranges),
    games,
    ...(o.has_map_pack_maps === true ? { hasMapPackMaps: true } : {}),
  };
}

/** The playlists the server serves and ranks, in hopper order: the
 *  published file's (`doc`, the editors' read) when there is one, else the
 *  built-in lineup the server then serves. Clanker marks (`marks`) move a
 *  hopper to the Clankermatch group whatever its type, as ranking does. */
export function playlistMenu(doc: unknown, marks: unknown): PlaylistInfo[] {
  const marked = parseClankerMarks(marks);
  const hoppers = (doc as { hoppers?: unknown } | null | undefined)?.hoppers;
  const fromDoc = Array.isArray(hoppers) ? hoppers.map((h) => hopperToPlaylist(h, marked)) : null;
  return fromDoc && fromDoc.length
    ? fromDoc.filter((p): p is PlaylistInfo => p !== null)
    : DEFAULT_LINEUP.map(([name, description, type, ranges, games, clanker], i) => ({
      id: String(i),
      name,
      description,
      // The marks read sees the served file — the built-in lineup when
      // nothing was published — so it outvotes the table's own mark only
      // by adding one (a default hopper stays clanker either way).
      group: clanker || marked.has(i)
        ? "clanker" as const
        : type === 2 || type === 4 || type === 5 ? "ranked" as const : "unranked" as const,
      size: sizeText(type, ranges),
      games: [...games],
    }));
}

/** Which playlist bare `/ranks` opens: the ranked one with the most
 *  players on it, the lowest hopper on a tie; the first ranked one (or
 *  Team Slayer) until the counts are in. */
export function defaultPlaylist(menu: PlaylistInfo[] | null, counts: Readonly<Record<string, number>>): string {
  const ranked = (menu ?? []).filter((p) => p.group === "ranked");
  const pool = ranked.length ? ranked : menu ?? [];
  let best: PlaylistInfo | null = null, bestN = -1;
  for (const p of pool) {
    const n = counts[p.id] ?? 0;
    if (n > bestN) { best = p; bestN = n; }
  }
  return bestN > 0 && best ? best.id : pool[0]?.id ?? "1";
}

/** What a board's header panel says: how many ranked players it holds,
 *  the games they add up to, and the highest level anyone reached. */
export function boardStats(rows: BoardRow[]): { players: number; games: number; highest: number } {
  let games = 0, highest = MIN_LEVEL;
  for (const r of rows) { games += r.games; highest = Math.max(highest, r.highest); }
  return { players: rows.length, games, highest };
}

/** Where `xuid` sits on the board, 1-based; null when off it (or nobody
 *  signed in). */
export function yourRank(rows: BoardRow[], xuid: string | null | undefined): number | null {
  if (!xuid) return null;
  const i = rows.findIndex((r) => r.xuid === xuid);
  return i < 0 ? null : i + 1;
}

/** XP from a standing to its next level, never negative; null at the top. */
export function toNext(s: Standing): number | null {
  return s.level >= MAX_LEVEL ? null : Math.max(0, threshold(s.level + 1) - s.experience);
}

/** The level badge's colour grade: steel, bronze, silver, gold, commander
 *  for the forties, and 50 apart. */
export function levelTier(level: number): number {
  return level >= MAX_LEVEL ? 5 : Math.min(4, Math.floor(Math.max(MIN_LEVEL, level) / 10));
}
