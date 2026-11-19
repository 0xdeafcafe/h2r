// Preset playlists: official Halo 2 Xbox Live lineups across the game's
// online life (2004-2010), written from public knowledge as descriptive
// references only (playlist names, game types, built-in variant names, map
// display names). No game data: a preset resolves in the browser against
// the owner's catalog (./catalog.ts) and their own published file, and
// whatever does not resolve is listed as unavailable instead of written.
//
// What a preset can set, and what it cannot (docs/live/live-admin.md, "Presets"):
// - the title and description the playlist list shows;
// - the first two range fields where the compiled-in defaults show what
//   they mean for that kind of playlist (others are kept);
// - its entries: each a map (the record id) and a weight, with a game
//   variant copied whole from an entry already in the published file that
//   plays the same game type (variant settings are 0x130 bytes the builder
//   cannot author, so "Team Hardcore" rules, say, come from whatever source
//   entry is picked, not from the preset);
// - not the hopper type (its meaning is unconfirmed), nor the matchmaking
//   timeouts or the validated table in the descriptor.

import { engineOf, findMap, normaliseName, playlistText, type Catalog } from "./catalog";
import { MAX_RECORDS, clipUtf8, type EditHopper, type EditRecord, type HopperRecord } from "./validate";

export interface PresetGame {
  /** `slayer`, `ctf`, `oddball`, `king_of_the_hill`, `juggernaut`,
   *  `territories`, `assault`. */
  gameType: string;
  /** A built-in variant name to prefer as the source (e.g. "Team Slayer"). */
  variant?: string;
  /** Team game: prefer a team source variant. */
  team?: boolean;
  /** Map display names; one entry per map. */
  maps: string[];
  /** Relative weight (1..1000); default 1. */
  weight?: number;
}

export interface Preset {
  key: string;
  /** Shown in the picker. */
  label: string;
  era: string;
  /** The catalog playlist whose title and description to use, when the
   *  game ships one; otherwise `title` and `description`. */
  playlist?: string;
  title: string;
  description: string;
  /** The first two range fields; null keeps the slot's. */
  ranges?: [number | null, number | null, number | null];
  games: PresetGame[];
  /** Why parts of it are from memory rather than a record. */
  uncertain?: string;
}

const LAUNCH_SMALL = ["Lockout", "Midship", "Ivory Tower", "Beaver Creek", "Ascension"];
const LAUNCH_BIG = ["Coagulation", "Headlong", "Zanzibar", "Waterworks", "Burial Mounds"];
const PACK_SMALL = ["Warlock", "Sanctuary", "Turf", "Elongation", "Backwash", "Relic"];
const PACK_BIG = ["Containment", "Terminal", "Relic", "Gemini"];

export const PRESETS: Preset[] = [
  {
    key: "rumble-pit-2004",
    label: "Rumble Pit (launch)",
    era: "2004 launch",
    playlist: "ffa",
    title: "Rumble Pit",
    description: "Free for all, up to 8 players. Slayer with some objective games.",
    ranges: [4, 8, null],
    games: [
      { gameType: "slayer", variant: "Slayer", maps: LAUNCH_SMALL, weight: 3 },
      { gameType: "oddball", variant: "Oddball", maps: ["Lockout", "Midship", "Ivory Tower"] },
      { gameType: "king_of_the_hill", variant: "Crazy King", maps: ["Midship", "Ivory Tower", "Ascension"] },
      { gameType: "juggernaut", variant: "Ninjanaut", maps: ["Lockout", "Beaver Creek"] },
    ],
    uncertain: "map rotation and weights are approximate",
  },
  {
    key: "team-skirmish-2004",
    label: "Team Skirmish (launch)",
    era: "2004 launch",
    playlist: "small_team",
    title: "Team Skirmish",
    description: "Two teams of up to 4. Mostly CTF and other objective games.",
    games: [
      { gameType: "ctf", variant: "CTF Classic", team: true, maps: ["Midship", "Beaver Creek", "Zanzibar", "Ivory Tower"] },
      { gameType: "assault", variant: "Single Bomb", team: true, maps: ["Midship", "Zanzibar"] },
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: LAUNCH_SMALL },
      { gameType: "king_of_the_hill", variant: "Team King", team: true, maps: ["Midship", "Ascension"] },
      { gameType: "territories", variant: "3 Plots", team: true, maps: ["Ascension", "Beaver Creek"] },
    ],
    uncertain: "map rotation is approximate",
  },
  {
    key: "head-to-head-2004",
    label: "Head to Head (launch)",
    era: "2004 launch",
    playlist: "head_to_head",
    title: "Head to Head",
    description: "One on one Slayer.",
    games: [{ gameType: "slayer", variant: "Slayer", maps: LAUNCH_SMALL }],
  },
  {
    key: "big-team-battle-2004",
    label: "Big Team Battle (launch)",
    era: "2004 launch",
    playlist: "big_team",
    title: "Big Team Battle",
    description: "Two teams of up to 8. Mostly objective games, with vehicles.",
    games: [
      { gameType: "ctf", variant: "Multi Flag CTF", team: true, maps: LAUNCH_BIG, weight: 2 },
      { gameType: "assault", variant: "Multi Bomb", team: true, maps: ["Coagulation", "Headlong", "Waterworks"] },
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: LAUNCH_BIG },
      { gameType: "territories", variant: "Land Grab", team: true, maps: ["Coagulation", "Burial Mounds"] },
    ],
    uncertain: "map rotation and weights are approximate",
  },
  {
    key: "minor-clanmatch-2004",
    label: "Minor Clanmatch (launch)",
    era: "2004 launch",
    playlist: "small_team_clanmatch",
    title: "Minor Clanmatch",
    description: "Clan parties of 3-4. Plays the Team Skirmish games.",
    ranges: [2, 3, 4],
    games: [
      { gameType: "ctf", variant: "CTF Classic", team: true, maps: ["Midship", "Beaver Creek", "Zanzibar"] },
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: LAUNCH_SMALL },
    ],
    uncertain: "ranges follow the compiled-in default for this playlist; their meaning is unconfirmed",
  },
  {
    key: "major-clanmatch-2004",
    label: "Major Clanmatch (launch)",
    era: "2004 launch",
    playlist: "big_team_clanmatch",
    title: "Major Clanmatch",
    description: "Clan parties of 6-8. Plays the Big Team Battle games.",
    ranges: [null, 6, 8],
    games: [
      { gameType: "ctf", variant: "Multi Flag CTF", team: true, maps: LAUNCH_BIG },
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: LAUNCH_BIG },
    ],
    uncertain: "ranges follow the compiled-in default for this playlist; their meaning is unconfirmed",
  },
  {
    key: "training-ground-2004",
    label: "Training Ground (launch)",
    era: "2004 launch",
    playlist: "small_team_unranked",
    title: "Training Ground",
    description: "Unranked team games; guests allowed.",
    games: [
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: LAUNCH_SMALL },
      { gameType: "ctf", variant: "CTF Classic", team: true, maps: ["Midship", "Beaver Creek"] },
    ],
  },
  {
    key: "team-slayer",
    label: "Team Slayer",
    era: "2005 onward",
    title: "Team Slayer",
    description: "Two teams of 4. Team Slayer only.",
    games: [{ gameType: "slayer", variant: "Team Slayer", team: true, maps: [...LAUNCH_SMALL, ...PACK_SMALL] }],
    uncertain: "added in a 2005 playlist update; exact map list varied",
  },
  {
    key: "double-team",
    label: "Double Team",
    era: "2005 onward",
    title: "Double Team",
    description: "Two teams of 2. Team Slayer, some objective games.",
    games: [
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: ["Lockout", "Midship", "Beaver Creek", "Ivory Tower", "Warlock", "Sanctuary"], weight: 3 },
      { gameType: "ctf", variant: "1 Flag CTF", team: true, maps: ["Midship", "Beaver Creek"] },
    ],
    uncertain: "added in 2005; objective share is approximate",
  },
  {
    key: "team-snipers",
    label: "Team Snipers",
    era: "2005 onward",
    title: "Team Snipers",
    description: "Sniper rifles only, teams of 4.",
    games: [{ gameType: "slayer", variant: "Snipers", team: true, maps: ["Zanzibar", "Headlong", "Burial Mounds", "Ascension", "Coagulation", "Lockout"] }],
    uncertain: "the source variant's settings decide the weapons; a team Snipers entry must already be in the file",
  },
  {
    key: "team-hardcore",
    label: "Team Hardcore",
    era: "2006 onward",
    title: "Team Hardcore",
    description: "Competitive 4 on 4: Slayer and CTF on the tightest maps.",
    games: [
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: ["Lockout", "Midship", "Ivory Tower", "Beaver Creek", "Sanctuary", "Warlock"], weight: 2 },
      { gameType: "ctf", variant: "CTF Classic", team: true, maps: ["Midship", "Beaver Creek", "Sanctuary"] },
      { gameType: "king_of_the_hill", variant: "Team King", team: true, maps: ["Midship", "Lockout"] },
    ],
    uncertain: "hardcore rules lived in custom variants; the builder copies whichever source variant matches",
  },
  {
    key: "big-team-2006",
    label: "Big Team Battle (with the map packs)",
    era: "2005-2007",
    playlist: "big_team",
    title: "Big Team Battle",
    description: "Two teams of up to 8, map pack maps included.",
    games: [
      { gameType: "ctf", variant: "Multi Flag CTF", team: true, maps: [...LAUNCH_BIG, ...PACK_BIG], weight: 2 },
      { gameType: "assault", variant: "Multi Bomb", team: true, maps: ["Coagulation", "Containment", "Terminal"] },
      { gameType: "slayer", variant: "Team Slayer", team: true, maps: [...LAUNCH_BIG, ...PACK_BIG] },
    ],
    uncertain: "map pack maps resolve only when the catalog lists them",
  },
  {
    key: "shotty-snipers",
    label: "Shotty Snipers",
    era: "2006-2007",
    title: "Shotty Snipers",
    description: "Shotguns and sniper rifles, teams of 4.",
    games: [{ gameType: "slayer", variant: "Snipers", team: true, maps: ["Lockout", "Zanzibar", "Ascension", "Midship", "Sanctuary"] }],
    uncertain: "dates and maps from memory; the shotgun loadout needs a source variant that has it",
  },
  {
    key: "rumble-pit-later",
    label: "Rumble Pit (later)",
    era: "2006-2010",
    playlist: "ffa",
    title: "Rumble Pit",
    description: "Free for all, up to 8 players, map pack maps included.",
    ranges: [4, 8, null],
    games: [
      { gameType: "slayer", variant: "Slayer", maps: [...LAUNCH_SMALL, "Warlock", "Sanctuary", "Elongation", "Gemini"], weight: 3 },
      { gameType: "oddball", variant: "Oddball", maps: ["Lockout", "Warlock"] },
      { gameType: "king_of_the_hill", variant: "Crazy King", maps: ["Midship", "Sanctuary"] },
    ],
    uncertain: "map rotation and weights are approximate",
  },
];

/** One preset entry as it resolved. */
export interface ResolvedGame {
  label: string;
  record: EditRecord | null;
  /** Why it is unavailable. */
  missing?: string;
}

export interface Resolution {
  title: string;
  description: string;
  ranges?: [number | null, number | null, number | null];
  games: ResolvedGame[];
  /** Entries that will be written. */
  records: EditRecord[];
}

/** A published entry: the hopper and slot it is in, and its fields. */
interface Source {
  hopper: number;
  slot: number;
  record: HopperRecord;
}

/** The published entry to copy for a game: same engine; then a matching
 *  variant name; then matching team play; then the first. */
function source(published: EditHopper[], game: PresetGame): Source | null {
  const engine = engineOf(game.gameType);
  if (engine === null) return null;
  const all: Source[] = [];
  for (const h of published)
    h.records.forEach((record, slot) => {
      if (record.variant?.engine === engine) all.push({ hopper: h.index, slot, record });
    });
  const score = (s: Source): number =>
    (game.variant && normaliseName(s.record.variant?.name ?? "") === normaliseName(game.variant) ? 2 : 0) +
    (game.team !== undefined && s.record.variant?.team === game.team ? 1 : 0);
  let best: Source | null = null;
  for (const s of all) if (!best || score(s) > score(best)) best = s;
  return best;
}

/** Resolves a preset against the catalog and the published file (the
 *  hoppers as last loaded, not as edited: copies name published slots). */
export function resolvePreset(preset: Preset, catalog: Catalog | null, published: EditHopper[]): Resolution {
  const text = preset.playlist ? playlistText(catalog, preset.playlist) : null;
  const games: ResolvedGame[] = [];
  for (const g of preset.games) {
    const src = source(published, g);
    const typeName = g.variant ?? g.gameType;
    for (const name of g.maps) {
      const label = `${typeName} on ${name}`;
      const map = findMap(catalog, name);
      if (!catalog) {
        games.push({ label, record: null, missing: "no catalog from this server" });
      } else if (!map) {
        games.push({ label, record: null, missing: `${name} is not in this game's maps` });
      } else if (!src) {
        games.push({ label, record: null, missing: `no published entry plays ${g.gameType.replace(/_/g, " ")} to copy` });
      } else {
        const weight = Math.min(1000, Math.max(1, Math.round(g.weight ?? 1)));
        const { variant, string, flags, dwords } = src.record;
        games.push({
          label,
          record: {
            id: map.id,
            string,
            flags,
            dwords: [weight, dwords[1]],
            variant: variant ?? null,
            src: src.slot,
            srcHopper: src.hopper,
          },
        });
      }
    }
  }
  const records = games.flatMap((g) => (g.record ? [g.record] : [])).slice(0, MAX_RECORDS);
  return {
    title: text?.title ?? preset.title,
    description: text?.description ?? preset.description,
    ranges: preset.ranges,
    games,
    records,
  };
}

/** The hopper with a resolution applied. Keeps its entries when nothing
 *  resolved (a hopper needs at least one), and its type, status and the
 *  ranges the preset leaves alone. Titles longer than the slot are cut. */
export function applyPreset(h: EditHopper, r: Resolution): EditHopper {
  const ranges = [...h.ranges] as [number, number, number];
  r.ranges?.forEach((v, i) => {
    if (v !== null) ranges[i] = v;
  });
  return {
    ...h,
    title: clip(r.title, 15),
    description: clip(r.description, 127),
    ranges,
    records: r.records.length > 0 ? r.records.map((x) => ({ ...x, dwords: [...x.dwords] as [number, number] })) : h.records,
  };
}

/** At most `max` UTF-8 bytes, cut on a character boundary. */
export const clip = clipUtf8;
