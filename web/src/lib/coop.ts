// Finished co-op campaign games, as `GET /v1/coop/games` lists them
// (docs/live/live-api.md "Co-op games"). The server derives them from the
// co-op sessions that closed after a guest joined. Everything here is pure.

import { difficultyName, levelName, parseCampaign, type Campaign, type Visibility } from "./sessions";

export interface CoopPlayer {
  xuid: string;
  /** Null when the account is gone. */
  gamertag: string | null;
}

export interface CoopGame {
  session_id: string;
  host: CoopPlayer;
  /** The host first, then every guest that ever joined. */
  players: CoopPlayer[];
  campaign: Campaign;
  /** When the first guest joined (ISO time). */
  started: string;
  ended: string;
  /** `closed`, `host_left` or `expired`. */
  end_reason: string;
  /** How the session was listed when it closed; the server shows a
   *  friends-only or invite-only game only to who may see it. Older servers
   *  don't say: public. */
  visibility: Visibility;
}

/** The token method a reader signs to say who it is (live-security.md §3). */
export const READ = "read";

const str = (v: unknown): string | null => (typeof v === "string" && v ? v : null);

function player(raw: unknown): CoopPlayer | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const xuid = str(r.xuid);
  return xuid ? { xuid: xuid.toLowerCase(), gamertag: str(r.gamertag) } : null;
}

/** Reads one item; null for anything without a session, host, level and times. */
export function parseCoopGame(raw: unknown): CoopGame | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const id = str(r.session_id), host = player(r.host), campaign = parseCampaign(r.campaign);
  const started = str(r.started), ended = str(r.ended);
  if (!id || !host || !campaign || !started || !ended) return null;
  const players = Array.isArray(r.players) ? r.players.map(player).filter((p): p is CoopPlayer => p !== null) : [];
  return { session_id: id, host, players: players.length ? players : [host], campaign, started, ended, end_reason: str(r.end_reason) ?? "closed",
    visibility: r.visibility === "friends" || r.visibility === "invite" ? r.visibility : "public" };
}

export function parseCoopGames(items: unknown): CoopGame[] {
  return Array.isArray(items) ? items.map(parseCoopGame).filter((g): g is CoopGame => g !== null) : [];
}

/** "Outskirts": the level's name for display. */
export const coopLevel = (g: Pick<CoopGame, "campaign">): string => levelName(g.campaign);

/** "heroic". */
export const coopDifficulty = (g: Pick<CoopGame, "campaign">): string => difficultyName(g.campaign.difficulty);

/** Gamertags, host first; an account the server no longer knows reads "guest". */
export const coopNames = (g: Pick<CoopGame, "players">): string[] => g.players.map((p) => p.gamertag ?? "guest");

/** How long the game ran, in whole minutes (at least 1), or null when the times don't read. */
export function coopMinutes(g: Pick<CoopGame, "started" | "ended">): number | null {
  const a = Date.parse(g.started), b = Date.parse(g.ended);
  return Number.isFinite(a) && Number.isFinite(b) && b >= a ? Math.max(1, Math.round((b - a) / 60000)) : null;
}

/** Newest first, by the end time. */
export const newestCoop = (list: readonly CoopGame[]): CoopGame[] =>
  [...list].sort((a, b) => Date.parse(b.ended) - Date.parse(a.ended) || b.session_id.localeCompare(a.session_id));
