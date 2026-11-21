// Query fields for Live's open session list (docs/live/live-api.md, "Sessions").
export const SESSION_KINDS = [
  ["multiplayer", "Multiplayer"],
  ["coop", "Campaign Co-op"],
] as const;

export const SESSION_CLANKERS = [
  [true, "With Clankers"],
  [false, "Without Clankers"],
] as const;

export const SESSION_GAME_TYPES = [
  [2, "Slayer"],
  [1, "CTF"],
  [3, "Oddball"],
  [4, "King of the Hill"],
  [7, "Juggernaut"],
  [8, "Territories"],
  [9, "Assault"],
] as const;

export const SESSION_MAP_CATEGORIES = [
  ["halo2", "Halo 2"],
  ["map_pack", "Map Pack"],
  ["mods", "Mods"],
  ["halo_ce", "Halo: CE"],
] as const;

export const SESSION_CAMPAIGNS = [
  ["halo2", "Halo 2"],
  ["halo_ce", "Halo: CE"],
] as const;

export const SESSION_DIFFICULTIES = [
  [0, "Easy"],
  [1, "Normal"],
  [2, "Heroic"],
  [3, "Legendary"],
] as const;

export interface SessionFilters {
  kind?: (typeof SESSION_KINDS)[number][0];
  clankers?: boolean;
  game_type?: (typeof SESSION_GAME_TYPES)[number][0];
  variant?: string;
  map_category?: (typeof SESSION_MAP_CATEGORIES)[number][0];
  has_space?: boolean;
  campaign?: (typeof SESSION_CAMPAIGNS)[number][0];
  difficulty?: (typeof SESSION_DIFFICULTIES)[number][0];
}

function choice<T>(options: ReadonlyArray<readonly [T, string]>, value: unknown): T | undefined {
  return options.find(([key]) => key === value)?.[0];
}

export function sessionVariant(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const name = value.trim();
  return name && !/[\u0000-\u001f\u007f-\u009f]/.test(name) && new TextEncoder().encode(name).length <= 256 ? name : undefined;
}

/** Reads a saved setting, ignoring fields that are invalid or unrecognised. */
export function readSessionFilters(raw: unknown): SessionFilters {
  const x = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  return {
    kind: choice(SESSION_KINDS, x.kind),
    clankers: choice(SESSION_CLANKERS, x.clankers),
    game_type: choice(SESSION_GAME_TYPES, x.game_type),
    variant: sessionVariant(x.variant),
    map_category: choice(SESSION_MAP_CATEGORIES, x.map_category),
    has_space: x.has_space === true ? true : undefined,
    campaign: choice(SESSION_CAMPAIGNS, x.campaign),
    difficulty: choice(SESSION_DIFFICULTIES, x.difficulty),
  };
}

export function sessionFiltersParam(filters: SessionFilters): string | undefined {
  const packed = JSON.stringify(readSessionFilters(filters));
  return packed === "{}" ? undefined : packed;
}
