// The mock's game types: now and then a campaign game (one to four players
// on an invented level, solo or co-op, at a difficulty, no scores), and
// for multiplayer whether it was a custom game or matchmaking (with an
// invented playlist). Kept apart from world.ts so the simulation there only
// asks "campaign or not" and copies the fields into its events. The AI in
// campaign frames is mock/campaign.ts's frame hook.

import type { GamePlayer } from "../src/api/types";
import { MAPS, type MockMap } from "./maps";

/** Share of new games that are campaign. */
export const CAMPAIGN_SHARE = 0.14;

/** Invented level stems, with a level-select style ordering prefix, on
 *  the mock maps' walkable ground. */
export const LEVELS: MockMap[] = [
  ["01a_long_dock", 0], ["02b_glass_canyon", 1], ["03a_harbor_dawn", 2], ["04b_quiet_relay", 0], ["05a_ember_vault", 1],
].map(([name, i]) => ({ ...MAPS[(i as number) % MAPS.length]!, name: name as string }));

const DIFFICULTIES = ["easy", "normal", "normal", "heroic", "heroic", "legendary"];
const PLAYLISTS: Array<[string, string]> = [
  ["warm_up_lane", "Warm-Up Lane"], ["quad_squad", "Quad Squad"], ["solo_scramble", "Solo Scramble"], ["night_owls", "Night Owls"],
];

export interface CampaignPlan {
  map: MockMap;
  players: GamePlayer[];
  /** Fields for GameStarted and the game's summary. */
  fields: { kind: "campaign"; difficulty: string; online?: boolean };
  /** How long the level runs, in seconds, and how it ends. */
  seconds: number;
  reason: "finished" | "quit";
}

type Who = { gamertag: string; xuid: string };

/** A campaign game for some of the free accounts, or null with none free. */
export function planCampaign(free: Who[], rand: () => number): CampaignPlan | null {
  if (!free.length) return null;
  const n = Math.min(free.length, 1 + Math.floor(rand() * rand() * 4.5));
  const who = free.slice(0, n);
  const map = LEVELS[Math.floor(rand() * LEVELS.length)]!;
  const fields: CampaignPlan["fields"] = { kind: "campaign", difficulty: DIFFICULTIES[Math.floor(rand() * DIFFICULTIES.length)]! };
  if (n > 1 && rand() < 0.35) fields.online = true;
  return {
    map,
    players: who.map((a, i) => ({ index: i, name: a.gamertag, xuid: a.xuid, team: 0, local: i === 0 || !fields.online })),
    fields,
    seconds: 150 + Math.floor(rand() * 240),
    reason: rand() < 0.75 ? "finished" : "quit",
  };
}

/** A multiplayer game's type: a custom game, or matchmaking from a playlist. */
export function multiplayerKind(rand: () => number): { kind: "custom" } | { kind: "matchmaking"; playlist: string; playlist_id: string } {
  if (rand() < 0.5) return { kind: "custom" };
  const [id, name] = PLAYLISTS[Math.floor(rand() * PLAYLISTS.length)]!;
  return { kind: "matchmaking", playlist: name, playlist_id: id };
}

/** Skulls for a mock game, picked from its id so the same game always has
 *  the same ones and the simulation's random numbers are not touched. About
 *  a third of games have some: campaign any of the fifteen, multiplayer
 *  only Famine (4) and Blind (6), the ones that work there. None is empty
 *  (the field is left out, as the server does). */
export function mockSkulls(id: string, campaign: boolean): number[] {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619) >>> 0;
  const next = () => { h = (Math.imul(h, 1664525) + 1013904223) >>> 0; return h / 0x100000000; };
  if (next() > 0.4) return [];
  const pool = campaign ? Array.from({ length: 15 }, (_, i) => i) : [4, 6];
  const out = new Set<number>();
  const n = campaign ? 1 + Math.floor(next() * next() * 4) : 1 + Math.floor(next() * 2);
  while (out.size < n && out.size < pool.length) out.add(pool[Math.floor(next() * pool.length)]!);
  return [...out].sort((a, b) => a - b);
}

/** What an AI kills a campaign player with. */
export const AI_WEAPONS = ["plasma_pistol", "plasma_rifle", "needler", "carbine"];
