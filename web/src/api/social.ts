// Friends and teams on the wire (docs/live/live-api.md, "Friends" and "Teams"):
// the read documents, defensive mapping of what the server sends, and the
// event payloads a signed-in account writes. Signing and sending are in
// state/sign.ts; the reactive store is state/social.ts.

import type { ServerTarget } from "./server";
import { ApiError } from "./http";
import type { Xuid } from "./types";

// ---------- friends ----------

export interface FriendPresence { state: string; game_id: string | null; activity?: string | null; since: string | null }
export interface FriendEntry { xuid: Xuid; gamertag: string; friends_since: string; presence: FriendPresence | null }
export interface FriendRequest { xuid: Xuid; gamertag: string; at: string }
/** `GET /v1/accounts/{gamertag}/friends`. */
export interface FriendsDoc { friends: FriendEntry[]; requests_in: FriendRequest[]; requests_out: FriendRequest[] }

export const emptyFriends = (): FriendsDoc => ({ friends: [], requests_in: [], requests_out: [] });

const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
export const byTag = <T extends { gamertag: string }>(a: T, b: T) => a.gamertag.localeCompare(b.gamertag, undefined, { sensitivity: "base" });

function presenceOf(v: unknown): FriendPresence | null {
  const o = obj(v);
  if (!o || typeof o.state !== "string") return null;
  const p: FriendPresence = { state: o.state, game_id: typeof o.game_id === "string" ? o.game_id : null, since: typeof o.since === "string" ? o.since : null };
  if (typeof o.activity === "string" && o.activity) p.activity = o.activity;
  return p;
}

function requestOf(v: unknown): FriendRequest | null {
  const o = obj(v);
  if (!o || typeof o.xuid !== "string") return null;
  return { xuid: o.xuid.toLowerCase(), gamertag: str(o.gamertag, o.xuid), at: str(o.at) };
}

/** The friends document, whatever shape it arrives in: missing lists are
 *  empty, junk entries are dropped, lists come back sorted by gamertag. */
export function normalizeFriends(body: unknown): FriendsDoc {
  const o = obj(body) ?? {};
  const friends: FriendEntry[] = [];
  for (const f of arr(o.friends)) {
    const e = obj(f);
    if (!e || typeof e.xuid !== "string") continue;
    friends.push({ xuid: e.xuid.toLowerCase(), gamertag: str(e.gamertag, e.xuid), friends_since: str(e.friends_since), presence: presenceOf(e.presence) });
  }
  const reqs = (v: unknown) => arr(v).map(requestOf).filter((r): r is FriendRequest => !!r).sort(byTag);
  return { friends: friends.sort(byTag), requests_in: reqs(o.requests_in), requests_out: reqs(o.requests_out) };
}

/** Where `other` stands with the signed-in account. */
export type FriendStatus = "self" | "friend" | "incoming" | "outgoing" | "none";

export function friendStatus(doc: FriendsDoc | null, me: Xuid | null, other: Xuid): FriendStatus {
  if (me && me === other) return "self";
  if (!doc) return "none";
  if (doc.friends.some((f) => f.xuid === other)) return "friend";
  if (doc.requests_in.some((r) => r.xuid === other)) return "incoming";
  if (doc.requests_out.some((r) => r.xuid === other)) return "outgoing";
  return "none";
}

// ---------- teams ----------

export interface TeamIdentity { xuid: Xuid; gamertag: string }
export interface TeamProperties { name: string; description: string; motto: string; url: string; data: number[] }
export interface MemberProperties { privileges: number; data: number[] }
export interface TeamMember { identity: TeamIdentity; properties: MemberProperties; joined_at: string }
export interface Team {
  id: string;
  creator: Xuid;
  properties: TeamProperties;
  max_members: number;
  created_at: string;
  members: Record<Xuid, TeamMember>;
  recruits: Record<Xuid, TeamMember>;
  deleted: boolean;
}

/** Permission bits (live-proto team rules). The creator holds them all. */
export const PRIV = { delete: 1, modifyTeam: 2, modifyMember: 4, removeMember: 8, recruit: 16 } as const;
export const LEADER = 0xffffffff;
/** What Halo 2 gives officers and ordinary members. */
export const OFFICER = 0x1c;
export const MEMBER = 0x10;
export const TEAM_NAME_MAX = 15;
export const TEAM_TEXT_MAX = 255;

const bytes = (v: unknown): number[] => arr(v).filter((n): n is number => typeof n === "number" && n >= 0 && n <= 255 && Number.isInteger(n));
const u32 = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v >>> 0 : 0);

export function teamProperties(v: unknown): TeamProperties {
  const o = obj(v) ?? {};
  return { name: str(o.name), description: str(o.description), motto: str(o.motto), url: str(o.url), data: bytes(o.data) };
}

export function memberProperties(v: unknown): MemberProperties {
  const o = obj(v) ?? {};
  return { privileges: u32(o.privileges), data: bytes(o.data) };
}

function roster(v: unknown): Record<Xuid, TeamMember> {
  const out: Record<Xuid, TeamMember> = {};
  for (const [k, m] of Object.entries(obj(v) ?? {})) {
    const e = obj(m);
    if (!e) continue;
    const id = obj(e.identity);
    const xuid = (typeof id?.xuid === "string" ? id.xuid : k).toLowerCase();
    out[xuid] = { identity: { xuid, gamertag: str(id?.gamertag, xuid) }, properties: memberProperties(e.properties), joined_at: str(e.joined_at) };
  }
  return out;
}

/** A team as the server sends it, or null when it isn't one. */
export function normalizeTeam(v: unknown): Team | null {
  const o = obj(v);
  if (!o || typeof o.id !== "string") return null;
  return {
    id: o.id.toLowerCase(),
    creator: str(o.creator).toLowerCase(),
    properties: teamProperties(o.properties),
    max_members: typeof o.max_members === "number" ? o.max_members : 100,
    created_at: str(o.created_at),
    members: roster(o.members),
    recruits: roster(o.recruits),
    deleted: o.deleted === true,
  };
}

export type TeamRole = "leader" | "officer" | "member" | "recruit" | null;

export function roleOf(t: Team, xuid: Xuid | null | undefined): TeamRole {
  if (!xuid) return null;
  const m = t.members[xuid];
  if (m) {
    if (xuid === t.creator || m.properties.privileges === LEADER) return "leader";
    return (m.properties.privileges & (PRIV.removeMember | PRIV.modifyMember)) !== 0 ? "officer" : "member";
  }
  return t.recruits[xuid] ? "recruit" : null;
}

export const ROLE_LABEL: Record<Exclude<TeamRole, null>, string> = { leader: "Leader", officer: "Officer", member: "Member", recruit: "Recruit" };

/** Whether the member `xuid` holds permission `bit` (a recruit holds none). */
export function can(t: Team, xuid: Xuid | null | undefined, bit: number): boolean {
  if (!xuid) return false;
  const m = t.members[xuid];
  if (!m) return false;
  return xuid === t.creator || (m.properties.privileges & bit) === bit;
}

/** Members leader first, then officers, then by gamertag. */
export function sortedMembers(t: Team): TeamMember[] {
  const rank = (m: TeamMember) => ({ leader: 0, officer: 1, member: 2, recruit: 3 } as const)[roleOf(t, m.identity.xuid) ?? "member"];
  return Object.values(t.members).sort((a, b) => rank(a) - rank(b) || byTag(a.identity, b.identity));
}

export const leaderOf = (t: Team): TeamMember | undefined => t.members[t.creator];

/** UTF-16 length checks, as live-proto counts them. An error or null. */
export function teamPropsError(p: Pick<TeamProperties, "name" | "description" | "motto" | "url">): string | null {
  if (!p.name.trim()) return "a team needs a name";
  if (p.name.length > TEAM_NAME_MAX) return `team names fit ${TEAM_NAME_MAX} characters`;
  for (const k of ["description", "motto", "url"] as const) if (p[k].length > TEAM_TEXT_MAX) return `the ${k} fits ${TEAM_TEXT_MAX} characters`;
  return null;
}

/** A new team id: 48 random bits under `000a`, sixteen hex digits. */
export function randomTeamId(rand: (n: number) => Uint8Array = (n) => crypto.getRandomValues(new Uint8Array(n))): string {
  const b = rand(6);
  return "000a" + Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

// ---------- event payloads ----------

/** One event to write: the stream, type and data of a signed body. */
export interface EventWrite { stream: string; type: string; data: unknown }

const acct = (x: Xuid) => `account-${x}`;
const teamStream = (id: string) => `team-${id}`;

/** Friend events go on the acting account's own stream. */
export const friendWrites = {
  request: (me: Xuid, to: Xuid): EventWrite => ({ stream: acct(me), type: "FriendRequested", data: { from: me, to } }),
  accept: (me: Xuid, from: Xuid): EventWrite => ({ stream: acct(me), type: "FriendAccepted", data: { from, to: me } }),
  decline: (me: Xuid, from: Xuid): EventWrite => ({ stream: acct(me), type: "FriendDeclined", data: { from, to: me } }),
  /** Ends a friendship, or cancels a pending request either way. */
  remove: (me: Xuid, friend: Xuid): EventWrite => ({ stream: acct(me), type: "FriendRemoved", data: { by: me, friend } }),
};

const fullProps = (p: Partial<TeamProperties>): TeamProperties => ({ name: p.name ?? "", description: p.description ?? "", motto: p.motto ?? "", url: p.url ?? "", data: p.data ?? [] });

/** Team events go on `team-<id>`; the author is the acting member. */
export const teamWrites = {
  create: (id: string, props: Partial<TeamProperties>, maxMembers = 100): EventWrite => ({
    stream: teamStream(id), type: "TeamCreated",
    data: { operation: "create", id, properties: fullProps(props), member: { privileges: LEADER, data: [] }, max_members: maxMembers },
  }),
  properties: (id: string, props: TeamProperties): EventWrite => ({ stream: teamStream(id), type: "TeamPropertiesChanged", data: { operation: "properties", id, properties: fullProps(props) } }),
  delete: (id: string): EventWrite => ({ stream: teamStream(id), type: "TeamDeleted", data: { operation: "delete", id } }),
  recruit: (id: string, target: TeamIdentity, privileges = MEMBER): EventWrite => ({
    stream: teamStream(id), type: "TeamMemberRecruited", data: { operation: "recruit", id, target, properties: { privileges, data: [] } },
  }),
  answer: (id: string, accept: boolean): EventWrite => ({ stream: teamStream(id), type: accept ? "TeamRecruitAccepted" : "TeamRecruitDeclined", data: { operation: "answer", id, accept } }),
  member: (id: string, user: Xuid, properties: MemberProperties): EventWrite => ({
    stream: teamStream(id), type: "TeamMemberPropertiesChanged", data: { operation: "member_properties", id, user, properties },
  }),
  /** Removes a member or cancels a recruit; with your own xuid, leaves. */
  remove: (id: string, user: Xuid): EventWrite => ({ stream: teamStream(id), type: "TeamMemberRemoved", data: { operation: "remove", id, user } }),
};

export const TEAM_EVENT_TYPES = new Set([
  "TeamCreated", "TeamPropertiesChanged", "TeamDeleted", "TeamMemberRecruited", "TeamRecruitAccepted", "TeamRecruitDeclined",
  "TeamMemberPropertiesChanged", "TeamMemberRemoved",
]);
export const FRIEND_EVENT_TYPES = new Set(["FriendRequested", "FriendAccepted", "FriendDeclined", "FriendRemoved"]);

/** A failed team write's `team_error` enum, in words. */
export function teamErrorText(code: string | undefined, fallback: string): string {
  switch (code) {
    case "Invalid": return "the server refused that as invalid";
    case "NotFound": return "that team no longer exists";
    case "UserNotFound": return "no account has that gamertag";
    case "AlreadyExists": return "a team already has that name";
    case "AlreadyMember": return "they are already on the team";
    case "Pending": return "they already have an invitation";
    case "NotMember": return "you are not on that team";
    case "Privileges": return "you don't have the rank for that";
    case "NoAdmin": return "the leader can't leave; delete the team instead";
    case "Full": return "the team is full";
    case "UserTeamsFull": return "that account is already on eight teams";
    default: return fallback;
  }
}

// ---------- reads ----------

async function read(url: string, signal?: AbortSignal, bearer?: string | null): Promise<unknown> {
  const headers: Record<string, string> = { accept: "application/json" };
  if (bearer) headers.authorization = `Bearer ${bearer}`;
  const r = await fetch(url, { signal, headers });
  if (!r.ok) {
    let code = "http_" + r.status, message = r.statusText;
    try { const b = await r.json(); if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; } } catch { /* not JSON */ }
    throw new ApiError(r.status, code, message);
  }
  return r.json();
}

const items = (b: unknown) => arr(obj(b)?.items);

/** `reader` gives a `read` token for the signed-in account: a friend's
 *  presence in a friends-only co-op game shows its details only to readers
 *  allowed to see that game. */
export function socialApi(t: ServerTarget, reader: () => Promise<string | null> = async () => null) {
  const u = (p: string) => t.api + p;
  const tag = (s: string) => encodeURIComponent(s);
  return {
    friends: async (gamertag: string, s?: AbortSignal) =>
      normalizeFriends(await read(u(`/accounts/${tag(gamertag)}/friends`), s, await reader().catch(() => null))),
    /** Every active team, following cursors (bounded). */
    teams: async (s?: AbortSignal): Promise<Team[]> => {
      const out: Team[] = [];
      let cursor: string | null = null;
      for (let i = 0; i < 50; i++) {
        const b = await read(u(`/teams?limit=100${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`), s);
        const page = items(b).map(normalizeTeam).filter((x): x is Team => !!x);
        out.push(...page);
        const next = obj(b)?.next;
        cursor = typeof next === "string" && page.length ? next : null;
        if (!cursor) break;
      }
      return out.filter((x) => !x.deleted);
    },
    /** One team, or null when it is missing or deleted. */
    team: async (id: string, s?: AbortSignal): Promise<Team | null> => {
      try {
        const b = obj(await read(u(`/teams/${encodeURIComponent(id)}`), s));
        return normalizeTeam(b?.team);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    /** An account's teams, pending recruits included. */
    teamsOf: async (gamertag: string, s?: AbortSignal): Promise<Team[]> =>
      items(await read(u(`/accounts/${tag(gamertag)}/teams`), s)).map(normalizeTeam).filter((x): x is Team => !!x && !x.deleted),
    /** `{xuid, gamertag}` for an account by name, for recruiting. */
    identity: async (gamertag: string, s?: AbortSignal): Promise<TeamIdentity | null> => {
      try {
        const b = obj(await read(u(`/accounts/${tag(gamertag)}`), s));
        return b && typeof b.xuid === "string" ? { xuid: b.xuid.toLowerCase(), gamertag: str(b.gamertag, gamertag) } : null;
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
  };
}
