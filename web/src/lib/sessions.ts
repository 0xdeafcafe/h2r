// Open game sessions: what hosts advertise so others can find them
// (docs/live/live-api.md "Sessions"). The web only reads them; joining happens
// in the game. Everything here is pure.

export interface SessionSlots {
  public_filled: number;
  public_open: number;
  private_filled: number;
  private_open: number;
}

export interface Session {
  session_id: string;
  host_xuid: string;
  host_gamertag: string;
  /** `open`, `deleted` or `expired`. */
  state: string;
  /** When it was opened (ISO time), if the server said. */
  since: string | null;
  /** `custom`, `hopper`, `ranked` or `coop`. */
  flags: string;
  /** What a co-op session is playing; null for any other kind. */
  campaign: Campaign | null;
  hopper_id: number | null;
  map: string | null;
  /** The map package the host is playing (docs/game/maps.md §5), when it named
   *  one; `map` is then its base map. Lower-case hex, else null. */
  map_id: string | null;
  /** The host's name for the game, usually its gametype. */
  name: string | null;
  /** Who finds it in lists: everyone, the host's friends, or nobody
   *  (invitations only). Older servers don't say: public. */
  visibility: Visibility;
  slots: SessionSlots;
  /** The roster the host has reported: XUID to a display slot ("" when none). */
  players: Record<string, string>;
  /** A co-op host's measured round trip to its guest, in milliseconds; null
   *  when the host hasn't measured one (or a server doesn't say). */
  ping_ms: number | null;
}

/** The campaign level and difficulty a co-op session is playing. */
export interface Campaign {
  /** The level's map file stem, e.g. `03a_oldmombasa`. */
  level: string;
  /** The level's name as the game shows it, when the host knew it. */
  name: string | null;
  /** 0 easy, 1 normal, 2 heroic, 3 legendary. */
  difficulty: number;
}

const DIFFICULTIES = ["easy", "normal", "heroic", "legendary"];

/** The difficulty in words ("heroic"); "unknown" for a number out of range. */
export function difficultyName(d: number): string {
  return DIFFICULTIES[d] ?? "unknown";
}

/** Reads a campaign as the server sends it; null without a level. */
export function parseCampaign(raw: unknown): Campaign | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const level = text(r.level);
  if (!level) return null;
  const d = typeof r.difficulty === "number" && Number.isInteger(r.difficulty) ? r.difficulty : 1;
  return { level, name: text(r.name), difficulty: d };
}

/** The level's name for display: the game's name for it, else the map stem
 *  without its ordering prefix ("03a_oldmombasa" reads "oldmombasa"). */
export function levelName(c: Pick<Campaign, "level" | "name">): string {
  return c.name ?? (c.level.replace(/^\d+[a-z]?_/i, "").replace(/[_-]+/g, " ").trim() || c.level);
}

export const isCoop = (s: Pick<Session, "flags">): boolean => s.flags === "coop";

/** "Outskirts, heroic" for a co-op session; null for any other. */
export function campaignLine(s: Pick<Session, "flags" | "campaign">): string | null {
  return isCoop(s) && s.campaign ? `${levelName(s.campaign)}, ${difficultyName(s.campaign.difficulty)}` : null;
}

/** A co-op session's players by gamertag: the host first, then those who
 *  joined in XUID order. `tagOf` names an XUID the web may know. */
export function coopPlayers(s: Pick<Session, "host_xuid" | "host_gamertag" | "players">, tagOf: (xuid: string) => string | undefined = () => undefined): string[] {
  const guests = Object.keys(s.players).filter((x) => x !== s.host_xuid).sort().map((x) => tagOf(x) ?? "guest");
  return [s.host_gamertag, ...guests];
}

export type Visibility = "public" | "friends" | "invite";

const visibility = (v: unknown): Visibility => (v === "friends" || v === "invite" ? v : "public");

const count = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);
const text = (v: unknown): string | null => (typeof v === "string" && v.trim() ? v.trim() : null);

/** Reads one session as the server sends it (the advertisement nested
 *  inside); null for anything without an id and a host. */
export function parseSession(raw: unknown): Session | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const id = text(r.session_id), tag = text(r.host_gamertag);
  if (!id || !tag) return null;
  const ad = (r.advertisement && typeof r.advertisement === "object" ? r.advertisement : {}) as Record<string, unknown>;
  const s = (ad.slots && typeof ad.slots === "object" ? ad.slots : {}) as Record<string, unknown>;
  const players: Record<string, string> = {};
  if (r.players && typeof r.players === "object") {
    for (const [xuid, slot] of Object.entries(r.players as Record<string, unknown>)) players[xuid] = typeof slot === "string" ? slot : "";
  }
  return {
    session_id: id,
    host_xuid: text(r.host_xuid) ?? "",
    host_gamertag: tag,
    state: text(r.state) ?? "open",
    since: text(r.since),
    flags: text(ad.flags) ?? "custom",
    campaign: parseCampaign(ad.campaign),
    hopper_id: typeof ad.hopper_id === "number" ? ad.hopper_id : null,
    map: text(ad.map),
    map_id: typeof ad.map_id === "string" && /^[0-9a-f]{64}$/i.test(ad.map_id) ? ad.map_id.toLowerCase() : null,
    name: text(ad.name),
    visibility: visibility(ad.visibility),
    slots: { public_filled: count(s.public_filled), public_open: count(s.public_open), private_filled: count(s.private_filled), private_open: count(s.private_open) },
    players,
    ping_ms: typeof ad.ping_ms === "number" && Number.isFinite(ad.ping_ms) && ad.ping_ms > 0 ? Math.round(ad.ping_ms) : null,
  };
}

/** "42 ms", or null when the session has no ping. */
export function pingText(s: Pick<Session, "ping_ms">): string | null {
  return s.ping_ms ? `${s.ping_ms} ms` : null;
}

/** Reads a list response's items, dropping any that can't be read. */
export function parseSessions(items: unknown): Session[] {
  return Array.isArray(items) ? items.map(parseSession).filter((s): s is Session => s !== null) : [];
}

/** Whether a list should show it. The server already filters by who is
 *  reading; this keeps an invite-only one out even if a server lists it. */
export function listed(s: Pick<Session, "visibility">): boolean {
  return s.visibility !== "invite";
}

/** Who the host lets find the game, in words. */
export function visibilityText(s: Pick<Session, "visibility">): string {
  return s.visibility === "friends" ? "friends only" : s.visibility === "invite" ? "invite only" : "everyone";
}

/** Players in the session and the most it holds. The slots are the host's
 *  own count; a longer reported roster wins, up to the room there is. */
export function seats(s: Session): { filled: number; max: number; open: number } {
  const { public_filled: a, public_open: b, private_filled: c, private_open: d } = s.slots;
  const max = a + b + c + d;
  const filled = Math.min(Math.max(a + c, Object.keys(s.players).length), Math.max(max, a + c));
  return { filled, max: Math.max(max, filled), open: Math.max(0, Math.max(max, filled) - filled) };
}

/** What kind of game it is: the host's name for it, else the kind of session. */
export function gametype(s: Session): string {
  if (isCoop(s)) return "Co-op campaign";
  return s.name ?? kind(s);
}

/** How the session was made. */
export function kind(s: Pick<Session, "flags">): string {
  return s.flags === "coop" ? "co-op campaign" : s.flags === "ranked" ? "ranked match" : s.flags === "hopper" ? "matchmaking" : "custom game";
}

/** The map's name for display. `title` turns a file stem into a name (the
 *  app passes the disc catalog's lookup); by default the stem title-cased. */
export function mapName(s: Pick<Session, "map">, title: (stem: string) => string = stemTitle): string {
  const m = s.map?.split(/[\\/]/).pop()?.replace(/\.map$/i, "").trim();
  return m ? title(m) : "unknown map";
}

function stemTitle(stem: string): string {
  return stem.replace(/[_-]+/g, " ").trim();
}

/** Sessions in list order: ones with room first, fuller ones before emptier
 *  ones, then by host. */
export function sortSessions(list: readonly Session[]): Session[] {
  return [...list].sort((x, y) => {
    const a = seats(x), b = seats(y);
    return Number(a.open === 0) - Number(b.open === 0) || b.filled - a.filled
      || x.host_gamertag.localeCompare(y.host_gamertag) || x.session_id.localeCompare(y.session_id);
  });
}

/** A list row: the host, then the map and players against the most.
 *  `custom` is a custom map's package name, which replaces the map's. */
export function sessionRow(s: Session, title?: (stem: string) => string, custom?: string | null): { key: string; label: string; meta: string } {
  const n = seats(s);
  if (isCoop(s) && s.campaign) {
    const ping = pingText(s);
    return { key: s.session_id, label: s.host_gamertag, meta: `co-op ${levelName(s.campaign)} ${n.filled}/${n.max}${ping ? ` ${ping}` : ""}` };
  }
  return { key: s.session_id, label: s.host_gamertag, meta: `${custom ?? mapName(s, title)} ${n.filled}/${n.max}` };
}

/** "3 open", "full", or "closed" for a session no longer open. */
export function roomText(s: Session): string {
  if (s.state !== "open") return s.state === "expired" ? "expired" : "closed";
  const n = seats(s);
  return n.open ? `${n.open} open` : "full";
}

/** How long ago something started, in words: "just opened", "4 min", "2 h". */
export function openFor(since: string | null, now: number): string {
  const t = since ? Date.parse(since) : NaN;
  if (!Number.isFinite(t)) return "unknown";
  const s = Math.max(0, (now - t) / 1000);
  if (s < 60) return "just opened";
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  return `${Math.floor(s / 3600)} h`;
}

/** Keeps the selection on the same session when the list refreshes: its
 *  new index, else the nearest row that still exists. */
export function keepSelection(rows: readonly { key: string }[], key: string | null, index: number): number {
  const i = key ? rows.findIndex((r) => r.key === key) : -1;
  return i >= 0 ? i : Math.max(0, Math.min(index, rows.length - 1));
}
