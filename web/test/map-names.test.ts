import { describe, expect, test } from "bun:test";
import { gameArt, mapDisplayName, mapKey, parseManifest, resolveMapStem } from "../src/lib/gameart";
import { applyCatalog, artStem, gameImage, isHalo1Map, loadServerArt, mapTitle, setGameArtForTest, setMapTitles, variantType } from "../src/state/gameart";
import { gameMapGroupLabel, gameMapTitle } from "../src/state/maps";
import { variantLabel, variantName, gameTypeLine } from "../src/state/gametype";
import { OUTCOME_WORDS } from "../src/state/derive";
import { campaignLine, levelName, mapName, parseSession } from "../src/lib/sessions";
import { count } from "../src/stats/fmt";

// Invented names and stems in the shapes the server's art and catalog have
// (catalog.json by stem; the manifest's `maps` with a name for converted
// Halo 1 maps). Nothing here is the game's.
const CATALOG = { tower: "Tall Tower", "02a_dock": "The Dock", creek: "Creek Two" };
const MANIFEST = {
  version: 1,
  images: {
    "map/tower/preview": { file: "map/tower/preview.png", width: 4, height: 3 },
    "map/h1_gulch/preview": { file: "map/h1_gulch/preview.png", width: 4, height: 3 },
    "map/h1_creek/preview": { file: "map/h1_creek/preview.png", width: 4, height: 3 },
    "map/x10/preview": { file: "map/x10/preview.png", width: 4, height: 3 },
  },
  maps: {
    h1_gulch: { geometry: "map/h1_gulch/map.glb", kind: "multiplayer", game: "halo1", name: "Gulch of Blood" },
    h1_creek: { geometry: "map/h1_creek/map.glb", kind: "multiplayer", game: "halo1", name: "Creek One" },
    x10: { geometry: "map/x10/map.glb", kind: "campaign", game: "halo1", name: "The First Level" },
  },
};

const withArt = () => {
  setMapTitles(CATALOG);
  setGameArtForTest(gameArt("https://art.example/art/abc/", parseManifest(MANIFEST)!));
};
const noArt = () => {
  setMapTitles({});
  setGameArtForTest(null);
};

describe("map names, pure", () => {
  const src = { names: new Map(Object.entries(CATALOG)), maps: parseManifest(MANIFEST)!.maps };

  test("the catalog first, then the manifest's name, then the stem", () => {
    expect(mapDisplayName("tower", src)).toBe("Tall Tower");
    expect(mapDisplayName("02a_dock", src)).toBe("The Dock");
    expect(mapDisplayName("h1_gulch", src)).toBe("Gulch of Blood");
    expect(mapDisplayName("x10", src)).toBe("The First Level");
    expect(mapDisplayName("unknown_place", src)).toBe("Unknown Place");
    expect(mapDisplayName("03b_new_town", src)).toBe("New Town");
    expect(mapDisplayName("h1_other", src)).toBe("Other");
  });

  test("Halo 1's bare name finds the converted map; a Halo 2 stem of the same name keeps its own", () => {
    expect(resolveMapStem("gulch", src)).toBe("h1_gulch");
    expect(mapDisplayName("gulch", src)).toBe("Gulch of Blood");
    // "creek" is a Halo 2 map in the catalog and a Halo 1 one in the manifest.
    expect(resolveMapStem("creek", src)).toBe("creek");
    expect(mapDisplayName("creek", src)).toBe("Creek Two");
    expect(resolveMapStem("h1_gulch", src)).toBe("h1_gulch");
    expect(resolveMapStem("nowhere", src)).toBe("nowhere");
    expect(resolveMapStem("", src)).toBe("");
  });

  test("a bare stem with only a preview under h1_ still finds it", () => {
    const bare = { names: new Map<string, string>(), maps: {} };
    const has = (s: string) => s === "h1_lonely";
    expect(resolveMapStem("lonely", bare, has)).toBe("h1_lonely");
    expect(resolveMapStem("lonely", bare)).toBe("lonely");
  });
});

describe("map names and pictures on the site", () => {
  test("with the server's art and catalog: names and previews for Halo 2, campaign and Halo 1 maps", () => {
    withArt();
    expect(mapTitle("tower")).toBe("Tall Tower");
    expect(mapTitle("maps/02a_dock.map")).toBe("The Dock");
    expect(mapTitle("h1_gulch")).toBe("Gulch of Blood");
    expect(mapTitle("gulch")).toBe("Gulch of Blood");
    expect(mapTitle("x10")).toBe("The First Level");
    expect(isHalo1Map("gulch")).toBe(true);
    expect(gameImage(mapKey(artStem("tower")))!.url).toBe("https://art.example/art/abc/map/tower/preview.png");
    expect(gameImage(mapKey(artStem("gulch")))!.url).toBe("https://art.example/art/abc/map/h1_gulch/preview.png");
    expect(gameImage(mapKey(artStem("x10")))!.url).toBe("https://art.example/art/abc/map/x10/preview.png");
    // A map the art has no picture of: no URL, so the thumbnail draws its card.
    expect(gameImage(mapKey(artStem("02a_dock")))).toBeNull();
    noArt();
  });

  test("a campaign game on an unpublished package reads as the level, not the package", () => {
    withArt();
    expect(gameMapTitle({ map: "x10", map_id: null })).toBe("The First Level");
    noArt();
  });

  test("without art (a server that has none, or before it loads): title-cased stems and no pictures", () => {
    noArt();
    expect(mapTitle("tower")).toBe("Tower");
    expect(mapTitle("h1_gulch")).toBe("Gulch");
    expect(mapTitle("01b_space_dock")).toBe("Space Dock");
    expect(artStem("gulch")).toBe("gulch");
    expect(gameImage(mapKey(artStem("tower")))).toBeNull();
  });
});

describe("sessions and co-op levels use the lookup they are given", () => {
  const coop = parseSession({
    session_id: "00112233445566aa", host_xuid: "0009000000000001", host_gamertag: "host", map: "02a_dock",
    advertisement: { slots: { public_filled: 1, public_open: 1 }, flags: "coop", campaign: { level: "02a_dock", difficulty: 2 }, visibility: "public" },
    since: "2026-11-16T10:00:00.000Z", state: "open", players: {},
  })!;

  test("the level's catalog name when the game sent none", () => {
    withArt();
    expect(levelName({ level: "02a_dock", name: null }, mapTitle)).toBe("The Dock");
    expect(levelName({ level: "02a_dock", name: "Game's Own" }, mapTitle)).toBe("Game's Own");
    expect(levelName({ level: "02a_dock", name: null })).toBe("dock");
    expect(campaignLine(coop, mapTitle)).toStartWith("The Dock, ");
    expect(mapName({ map: "tower" }, mapTitle)).toBe("Tall Tower");
    noArt();
  });
});

describe("wording", () => {
  test("counts agree with their number", () => {
    expect(count(1, "player")).toBe("1 player");
    expect(count(0, "player")).toBe("0 players");
    expect(count(12, "player")).toBe("12 players");
    expect(count(1, "death")).toBe("1 death");
  });

  test("variant names keep the game's casing; bare keys are title-cased", () => {
    expect(variantName("King of the Hill")).toBe("King of the Hill");
    expect(variantName("1 Flag CTF")).toBe("1 Flag CTF");
    expect(variantName("team_slayer")).toBe("Team Slayer");
    expect(variantName(null)).toBe("");
    const g = { mode: "multiplayer", kind: "custom", playlist: null, online: null, players: {}, difficulty: null };
    expect(gameTypeLine({ ...g, variant: "King of the Hill" })).toBe("King of the Hill · Custom Game");
  });

  test("a game whose console went quiet reads Unfinished, not Connection lost", () => {
    expect(OUTCOME_WORDS.lost).toBe("Unfinished");
  });
});

describe("variants with their game type", () => {
  const types: Record<string, string> = { rockets: "Slayer", "team slayer": "Slayer", "crazy king": "King of the Hill", "2 on 1": "Juggernaut", "ninjanaut": "Juggernaut", "1 flag ctf": "Capture the Flag" };
  const typeOf = (v: string) => types[v.toLowerCase()] ?? null;

  test("the type is added only when the name doesn't say it", () => {
    expect(variantLabel("Rockets", typeOf)).toBe("Rockets (Slayer)");
    expect(variantLabel("2 on 1", typeOf)).toBe("2 on 1 (Juggernaut)");
    expect(variantLabel("Ninjanaut", typeOf)).toBe("Ninjanaut (Juggernaut)");
    expect(variantLabel("Team Slayer", typeOf)).toBe("Team Slayer");
    expect(variantLabel("Crazy King", typeOf)).toBe("Crazy King");
    expect(variantLabel("1 Flag CTF", typeOf)).toBe("1 Flag CTF");
    // A player's own variant the catalog doesn't list: its name alone.
    expect(variantLabel("Pete's Mayhem", typeOf)).toBe("Pete's Mayhem");
    expect(variantLabel(null, typeOf)).toBe("");
  });

  test("by default the types come from the server's catalog, and lines use them", () => {
    applyCatalog({ maps: [], campaign: [], variants: [{ name: "Rockets", game_type: "slayer", engine: 2 }, { name: "Land Grab", game_type: "territories", engine: 8 }] });
    expect(variantType("rockets")).toBe("Slayer");
    expect(variantLabel("Land Grab")).toBe("Land Grab (Territories)");
    const g = { mode: "multiplayer", kind: "custom", playlist: null, online: null, players: {}, difficulty: null };
    expect(gameTypeLine({ ...g, variant: "Rockets" })).toBe("Rockets (Slayer) · Custom Game");
    applyCatalog(null);
    expect(variantLabel("Rockets")).toBe("Rockets");
  });
});

describe("the catalog without the manifest", () => {
  const catalog = { version: 1, maps: [{ id: 1, name: "Tall Tower", description: "", key: "tower" }], campaign: [], playlists: [], variants: [], skipped: {}, custom: [] };

  test("names come from the catalog even when the manifest is missing", async () => {
    noArt();
    await loadServerArt("https://a.example", { art: async () => null, catalog: async () => catalog, current: () => "https://a.example" });
    expect(gameImage(mapKey("tower"))).toBeNull();
    expect(mapTitle("tower")).toBe("Tall Tower");
    noArt();
  });

  test("an answer for a server the user has left is dropped", async () => {
    noArt();
    await loadServerArt("https://b.example", { art: async () => null, catalog: async () => catalog, current: () => "https://elsewhere.example" });
    expect(mapTitle("tower")).toBe("Tower");
  });
});

describe("Halo 1 levels without art", () => {
  test("a Halo 1 level played from its unpublished package is not 'modded'", () => {
    noArt();
    expect(isHalo1Map("a10")).toBe(true);
    expect(isHalo1Map("maps/D40.map")).toBe(true);
    expect(isHalo1Map("01b_spacestation")).toBe(false);
    expect(gameMapGroupLabel({ map: "a10", map_id: "ab".repeat(32) })).toBe("A10");
    // A Halo 2 level's mod still reads as modded.
    expect(gameMapGroupLabel({ map: "05a_deep_water", map_id: "ab".repeat(32) })).toBe("Deep Water (modded)");
  });
});
