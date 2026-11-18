// Pure folds of friend and team events, so a page can follow the stream
// between reads without reloading. The server's read documents stay the
// truth: the store (social.ts) applies these at once, then re-reads.
// The mock server uses the same folds as its projection.

import type { WireEvent, Xuid } from "../api/types";
import {
  byTag, memberProperties, normalizeTeam, teamProperties, type FriendsDoc, type Team, type TeamIdentity,
} from "../api/social";
import { countMedals, gameMedals, type MedalId } from "./derive";
import { authorXuid } from "./verify";
import { isCampaign, isWinner, type GameState, type LiveState } from "./reducer";

/** A gamertag for an XUID, from whatever the caller knows. */
export type TagOf = (xuid: Xuid) => string;

interface FriendPair { from?: string; to?: string; by?: string; friend?: string }

/** The other account a friend event is about, seen from `me`, or null when
 *  the event isn't about `me`. */
export function friendCounterpart(me: Xuid, ev: Pick<WireEvent, "type" | "data">): Xuid | null {
  const d = (ev.data ?? {}) as FriendPair;
  if (ev.type === "FriendRemoved") {
    if (d.by === me) return d.friend ?? null;
    if (d.friend === me) return d.by ?? null;
    return null;
  }
  if (d.from === me) return d.to ?? null;
  if (d.to === me) return d.from ?? null;
  return null;
}

/** `me`'s friends document after one friend event. Returns `doc` itself
 *  when the event changes nothing for `me`. */
export function applyFriendEvent(doc: FriendsDoc, me: Xuid, ev: Pick<WireEvent, "type" | "data" | "at">, tagOf: TagOf): FriendsDoc {
  const other = friendCounterpart(me, ev);
  if (!other) return doc;
  const d = ev.data as FriendPair;
  const drop = <T extends { xuid: Xuid }>(xs: T[]) => xs.filter((x) => x.xuid !== other);
  const gamertag = tagOf(other);
  switch (ev.type) {
    case "FriendRequested": {
      if (doc.friends.some((f) => f.xuid === other)) return doc;
      const req = { xuid: other, gamertag, at: ev.at };
      return d.from === me
        ? { ...doc, requests_out: [...drop(doc.requests_out), req].sort(byTag) }
        : { ...doc, requests_in: [...drop(doc.requests_in), req].sort(byTag) };
    }
    case "FriendAccepted": {
      const friends = doc.friends.some((f) => f.xuid === other)
        ? doc.friends
        : [...doc.friends, { xuid: other, gamertag, friends_since: ev.at, presence: null }].sort(byTag);
      return { friends, requests_in: drop(doc.requests_in), requests_out: drop(doc.requests_out) };
    }
    case "FriendDeclined":
      return { ...doc, requests_in: drop(doc.requests_in), requests_out: drop(doc.requests_out) };
    case "FriendRemoved":
      return { friends: drop(doc.friends), requests_in: drop(doc.requests_in), requests_out: drop(doc.requests_out) };
    default:
      return doc;
  }
}

/** The team id a team event is for (from its stream). */
export function teamIdOf(ev: Pick<WireEvent, "stream">): string | null {
  return ev.stream.startsWith("team-") ? ev.stream.slice(5).toLowerCase() : null;
}

/** One team event folded into a team (null before its creation). Returns a
 *  new object; `prev` is left alone. Unknown events return `prev`. */
export function applyTeamEvent(prev: Team | null, ev: Pick<WireEvent, "type" | "data" | "at" | "author" | "stream">, tagOf: TagOf): Team | null {
  const id = teamIdOf(ev);
  const d = (ev.data ?? {}) as Record<string, unknown>;
  const actor = ev.author ? authorXuid(ev.author) : null;
  if (ev.type === "TeamCreated") {
    if (!id || !actor) return prev;
    return {
      id, creator: actor, properties: teamProperties(d.properties), max_members: typeof d.max_members === "number" ? d.max_members : 100,
      created_at: ev.at, deleted: false, recruits: {},
      members: { [actor]: { identity: { xuid: actor, gamertag: tagOf(actor) }, properties: memberProperties(d.member), joined_at: ev.at } },
    };
  }
  if (!prev) return prev;
  const t: Team = { ...prev, members: { ...prev.members }, recruits: { ...prev.recruits } };
  const user = typeof d.user === "string" ? d.user.toLowerCase() : null;
  switch (ev.type) {
    case "TeamPropertiesChanged": t.properties = teamProperties(d.properties); break;
    case "TeamDeleted": t.deleted = true; t.members = {}; t.recruits = {}; break;
    case "TeamMemberRecruited": {
      const target = d.target as Partial<TeamIdentity> | undefined;
      if (!target || typeof target.xuid !== "string") return prev;
      const x = target.xuid.toLowerCase();
      t.recruits[x] = { identity: { xuid: x, gamertag: target.gamertag ?? tagOf(x) }, properties: memberProperties(d.properties), joined_at: ev.at };
      break;
    }
    case "TeamRecruitAccepted": {
      const r = actor ? t.recruits[actor] : undefined;
      if (!actor || !r) return prev;
      delete t.recruits[actor];
      t.members[actor] = { ...r, joined_at: ev.at };
      break;
    }
    case "TeamRecruitDeclined":
      if (!actor || !t.recruits[actor]) return prev;
      delete t.recruits[actor];
      break;
    case "TeamMemberPropertiesChanged": {
      const m = user ? t.members[user] : undefined;
      if (!user || !m) return prev;
      t.members[user] = { ...m, properties: memberProperties(d.properties) };
      break;
    }
    case "TeamMemberRemoved":
      if (!user) return prev;
      delete t.members[user];
      delete t.recruits[user];
      break;
    default:
      return prev;
  }
  return t;
}

/** Re-checks a team read off the wire, for callers holding raw JSON. */
export const teamFrom = normalizeTeam;

// ---------- combined stats ----------

export interface TeamGame {
  game: GameState;
  /** Slots of the team's members in it. */
  slots: number[];
  won: boolean;
  kills: number;
  deaths: number;
  score: number;
}

export interface TeamRecord {
  games: TeamGame[]; together: number; wins: number; kills: number; deaths: number;
  medals: Map<MedalId, number>;
  /** Campaign games two or more members played in co-op, newest first.
   *  They count for nothing above: wins and K/D are multiplayer only. */
  campaign: GameState[];
}

/** Multiplayer games two or more members played together (on one side in
 *  team games), newest first, with the members' combined numbers, plus the
 *  campaign games they shared. Derived in the browser from the games this
 *  page has loaded; the API serves no team stats. */
export function teamRecord(s: LiveState, members: Iterable<Xuid>): TeamRecord {
  const ids = new Set(members);
  const out: TeamGame[] = [];
  const medals = new Map<MedalId, number>();
  const campaign: GameState[] = [];
  for (const gid of s.gameOrder) {
    const g = s.games[gid];
    if (!g) continue;
    const players = Object.values(g.players).filter((p) => p.xuid && ids.has(p.xuid));
    if (players.length < 2) continue;
    if (isCampaign(g)) { campaign.push(g); continue; }
    let side = players;
    if (g.team_play) {
      const counts = new Map<number, number>();
      for (const p of players) counts.set(p.team, (counts.get(p.team) ?? 0) + 1);
      const best = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]!;
      if (best[1] < 2) continue;
      side = players.filter((p) => p.team === best[0]);
    }
    let kills = 0, deaths = 0, score = 0;
    for (const p of side) { const sc = g.scores[p.index]; kills += sc?.kills ?? 0; deaths += sc?.deaths ?? 0; score += sc?.score ?? 0; }
    const won = g.status === "ended" && side.some((p) => isWinner(g, p));
    const slots = side.map((p) => p.index);
    if (g.status === "ended") {
      for (const [medal, count] of countMedals(gameMedals(g.kills), new Set(slots))) {
        medals.set(medal, (medals.get(medal) ?? 0) + count);
      }
    }
    out.push({ game: g, slots, won, kills, deaths, score });
  }
  return {
    games: out,
    together: out.length,
    wins: out.filter((x) => x.won).length,
    kills: out.reduce((n, x) => n + x.kills, 0),
    deaths: out.reduce((n, x) => n + x.deaths, 0),
    medals,
    campaign,
  };
}

/** The latest appearance a player's games carried, for a team emblem. */
export function latestAppearance(s: LiveState, xuid: Xuid) {
  for (const gid of s.gameOrder) {
    const g = s.games[gid];
    if (!g) continue;
    for (const p of Object.values(g.players)) if (p.xuid === xuid && p.appearance) return p.appearance;
  }
  return null;
}
