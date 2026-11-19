// Per-playlist ranks (docs/live/live-matchmaking.md §8): a player's level
// 1..50 and experience in each ranked playlist, as the server works them
// out after every ranked game. Parsers for the three rank reads and the
// small sums the pages show.

export interface Standing { level: number; experience: number; highest: number; games: number; wins: number }
export interface RankEntry { gameId: string; at: string; before: Standing; after: Standing }
export interface BoardRow extends Standing { xuid: string; gamertag: string }

export const MIN_LEVEL = 1, MAX_LEVEL = 50;

/** The server's built-in playlists, by hopper index, for when the hopper
 *  file hasn't been read: the same names its default set uses. */
export const DEFAULT_PLAYLISTS: Readonly<Record<string, string>> = {
  "0": "Rumble Pit", "1": "Team Slayer", "2": "Team Skirmish", "3": "Team Snipers",
  "4": "Big Team Battle", "5": "Team Objective", "6": "Free for All", "7": "Unranked Teams",
  "8": "Clanker Slayer", "9": "Clanker Teams", "10": "Clanker Doubles",
};

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

/** `GET /v1/ranks/{playlist}`: the leaderboard, in the server's order. */
export function parseBoard(body: unknown): BoardRow[] {
  const items = (body as { items?: unknown } | null)?.items;
  const out: BoardRow[] = [];
  for (const it of Array.isArray(items) ? items : []) {
    const r = it as Record<string, unknown>;
    const s = parseStanding(r);
    if (s && typeof r.xuid === "string" && typeof r.gamertag === "string") out.push({ ...s, xuid: r.xuid, gamertag: r.gamertag });
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
