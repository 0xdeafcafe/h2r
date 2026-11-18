import { describe, expect, test } from "bun:test";
import { isHalo1, parseMapFiles, worldToPixel, type Mat2x4 } from "../src/map/views";
import { gameArt, parseManifest } from "../src/lib/gameart";
import { isHalo1Map, mapTitle, setGameArtForTest, setMapTitles } from "../src/state/gameart";
import { gameMapGroupLabel, geometryChanged } from "../src/state/maps";

// An invented manifest in the shape `h2 live assets extract` writes for
// converted Halo 1 maps (docs/live/live-art.md, "Halo 1 maps").
const MANIFEST = {
  version: 1,
  images: { "map/h1_alpha/preview": { file: "map/h1_alpha/preview.png", width: 3, height: 2 } },
  maps: {
    h1_alpha: { geometry: "map/h1_alpha/map.glb", views: "map/h1_alpha/views.json", kind: "multiplayer", bsps: 1, game: "halo1", name: "Alpha Canyon" },
    x10: { geometry: "map/x10/map.glb", views: "map/x10/views.json", kind: "campaign", bsps: 3, game: "halo1", name: "First Level" },
    teacup: { geometry: "map/teacup/map.glb", views: "map/teacup/views.json", kind: "multiplayer", bsps: 1 },
  },
};

describe("Halo 1 maps in the art manifest", () => {
  test("the manifest's game, kind and name are kept", () => {
    const m = parseMapFiles(MANIFEST);
    expect(m.h1_alpha).toEqual({ geometry: "map/h1_alpha/map.glb", views: "map/h1_alpha/views.json", kind: "multiplayer", game: "halo1", name: "Alpha Canyon" });
    expect(m.x10?.kind).toBe("campaign");
    expect(m.x10?.game).toBe("halo1");
    expect(m.teacup?.game).toBeUndefined();
  });

  test("odd values are dropped, not trusted", () => {
    const m = parseMapFiles({ maps: { a: { geometry: "map/a/map.glb", kind: "other", game: "<b>", name: "  " } } });
    expect(m.a).toEqual({ geometry: "map/a/map.glb" });
  });

  test("a Halo 1 map is told apart by the manifest, or by its h1_ stem without one", () => {
    const m = parseMapFiles(MANIFEST);
    expect(isHalo1("x10", m.x10)).toBe(true);
    expect(isHalo1("x10", null)).toBe(false);
    expect(isHalo1("h1_alpha", null)).toBe(true);
    expect(isHalo1("maps\\H1_Alpha.map")).toBe(true);
    expect(isHalo1("teacup", m.teacup)).toBe(false);
    expect(isHalo1("h1_")).toBe(false);
  });
});

describe("Halo 1 maps on the site", () => {
  test("titles come from the manifest when the catalog lacks them", () => {
    setMapTitles({});
    setGameArtForTest(null);
    expect(mapTitle("h1_alpha")).toBe("Alpha");
    expect(isHalo1Map("h1_alpha")).toBe(true);
    expect(isHalo1Map("x10")).toBe(false);
    setGameArtForTest(gameArt("http://h/art/game/", parseManifest(MANIFEST)!));
    expect(mapTitle("h1_alpha")).toBe("Alpha Canyon");
    expect(mapTitle("x10")).toBe("First Level");
    expect(isHalo1Map("x10")).toBe(true);
    expect(isHalo1Map("teacup")).toBe(false);
    setMapTitles({ x10: "Catalog Name" });
    expect(mapTitle("x10")).toBe("Catalog Name");
    setMapTitles({});
    setGameArtForTest(null);
  });

  test("a Halo 1 package's games are neither 'modded' nor 'geometry changed'", () => {
    setGameArtForTest(gameArt("http://h/art/game/", parseManifest(MANIFEST)!));
    const g = { map: "h1_alpha", map_id: null };
    expect(gameMapGroupLabel(g)).toBe("Alpha Canyon");
    expect(geometryChanged(g)).toBe(false);
    setGameArtForTest(null);
  });

  test("positions plot with the view's matrix, game axes as telemetry gives them", () => {
    // A top view as the extractor writes one: north (+y) up, east (+x) right.
    const m: Mat2x4 = [[2, 0, 0, 100], [0, -2, 0, 100]];
    expect(worldToPixel(m, 10, 0, 5)).toEqual([120, 100]);
    expect(worldToPixel(m, 0, 10, -5)).toEqual([100, 80]);
  });
});
