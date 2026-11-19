// An installed mod's own art (docs/live/live-art.md, "Mods"): found by the
// game's MapId, never by the base map's name alone.
import { describe, expect, test } from "bun:test";
import { mapFilesFor, parseMapFiles } from "../src/map/views";

const ID_A = "ab".repeat(32);
const ID_B = "cd".repeat(32);

const raw = {
  maps: {
    teacup: { geometry: "map/teacup/map.glb", views: "map/teacup/views.json", kind: "multiplayer" },
    "mod-abababababababab": {
      geometry: "map/mod-abababababababab/map.glb",
      kind: "multiplayer",
      name: "Teacup Hogs",
      map_id: ID_A.toUpperCase(),
      base: "Teacup",
      sha256: "ef".repeat(32),
    },
    "mod-bad": { geometry: "map/mod-bad/map.glb", map_id: "not hex", base: "../x", sha256: 7 },
  },
};

describe("mod geometry", () => {
  test("the manifest's mod fields are read and checked", () => {
    const maps = parseMapFiles(raw);
    expect(maps["mod-abababababababab"]).toEqual({
      geometry: "map/mod-abababababababab/map.glb",
      kind: "multiplayer",
      name: "Teacup Hogs",
      map_id: ID_A,
      base: "teacup",
      sha256: "ef".repeat(32),
    });
    expect(maps["mod-bad"]).toEqual({ geometry: "map/mod-bad/map.glb" });
  });

  test("a game on a mod draws the mod's own entry, by MapId", () => {
    const maps = parseMapFiles(raw);
    expect(mapFilesFor(maps, "teacup", ID_A)?.stem).toBe("mod-abababababababab");
    expect(mapFilesFor(maps, "teacup", ID_A.toUpperCase())?.files.name).toBe("Teacup Hogs");
  });

  test("a retail game, or a mod the art lacks, falls back to the map name's", () => {
    const maps = parseMapFiles(raw);
    expect(mapFilesFor(maps, "teacup")?.stem).toBe("teacup");
    expect(mapFilesFor(maps, "teacup", null)?.stem).toBe("teacup");
    expect(mapFilesFor(maps, "maps\\teacup.map", ID_B)?.stem).toBe("teacup");
    expect(mapFilesFor(maps, "nowhere", ID_B)).toBeNull();
    expect(mapFilesFor(null, "teacup", ID_A)).toBeNull();
  });

  test("the MapId wins over the name", () => {
    const maps = parseMapFiles(raw);
    expect(mapFilesFor(maps, "mod-bad", ID_A)?.stem).toBe("mod-abababababababab");
    // Without a MapId the name lookup is by stem only.
    expect(mapFilesFor(maps, null, null)).toBeNull();
  });
});
