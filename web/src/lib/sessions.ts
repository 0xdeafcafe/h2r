// Open game sessions: what hosts advertise so others can find them
// (docs/live-api.md "Sessions"). The web only reads them; joining happens
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
  /** `custom`, `hopper` or `ranked`. */
  flags: string;
  hopper_id: number | null;
  map: string | null;
  /** The host's name for the game, usually its gametype. */
  name: string | null;
  slots: SessionSlots;
  /** The roster the host has reported: XUID to a display slot ("" when none). */
  players: Record<string, string>;
}

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
    hopper_id: typeof ad.hopper_id === "number" ? ad.hopper_id : null,
    map: text(ad.map),
    name: text(ad.name),
    slots: { public_filled: count(s.public_filled), public_open: count(s.public_open), private_filled: count(s.private_filled), private_open: count(s.private_open) },
    players,
  };
}

/** Reads a list response's items, dropping any that can't be read. */
export function parseSessions(items: unknown): Session[] {
  return Array.isArray(items) ? items.map(parseSession).filter((s): s is Session => s !== null) : [];
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
  return s.name ?? kind(s);
}

/** How the session was made. */
export function kind(s: Pick<Session, "flags">): string {
  return s.flags === "ranked" ? "ranked match" : s.flags === "hopper" ? "matchmaking" : "custom game";
}

/** The map's name for display: the file name without path or `.map`. */
export function mapName(s: Pick<Session, "map">): string {
  const m = s.map?.split(/[\\/]/).pop()?.replace(/\.map$/i, "").replace(/[_-]+/g, " ").trim();
  return m || "unknown map";
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

/** A list row: the host, then the map and players against the most. */
export function sessionRow(s: Session): { key: string; label: string; meta: string } {
  const n = seats(s);
  return { key: s.session_id, label: s.host_gamertag, meta: `${mapName(s)} ${n.filled}/${n.max}` };
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
