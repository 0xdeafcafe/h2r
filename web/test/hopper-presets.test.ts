import { describe, expect, test } from "bun:test";
import { findMap, parseCatalog, type Catalog } from "../src/admin/catalog";
import { PRESETS, applyPreset, clip, resolvePreset, type Preset } from "../src/admin/presets";
import { buildPatch, hopperErrors, toEditable, type HopperView, type PatchHopper } from "../src/admin/validate";

// Everything here is invented: map names, ids and variants are stand-ins,
// not the game's.
const catalog: Catalog = parseCatalog({
  version: 1,
  source: "test",
  maps: [
    { id: 10, name: "Alpha Yard", description: "" },
    { id: 20, name: "Beta Deck", description: "", key: "beta" },
    { id: "bad", name: "Broken" },
    { id: 5, name: "Gamma Level", description: "", key: "01x_gamma", kind: "campaign" },
  ],
  playlists: [{ key: "solo", title: "Solo Pit", description: "Alone." }],
  variants: [{ name: "Team Fight", game_type: "slayer", engine: 2 }],
  skipped: {},
})!;

const published = (): HopperView[] => [
  {
    index: 0, type: 2, status: 1, record_count: 2, name: "", title: "Old", description: "",
    ranges: [1, 8, 4], dwords: Array(18).fill(0),
    records: [
      { id: 10, string: "", flags: 1, dwords: [5, 0], variant: { name: "Free Fight", engine: 2, team: false } },
      { id: 20, string: "", flags: 0, dwords: [5, 2], variant: { name: "Flags", engine: 1, team: true } },
    ],
  },
  {
    index: 1, type: 4, status: 1, record_count: 1, name: "", title: "Teams", description: "",
    ranges: [2, 8, 4], dwords: Array(18).fill(0),
    records: [{ id: 20, string: "", flags: 1, dwords: [9, 1], variant: { name: "Team Fight", engine: 2, team: true } }],
  },
];

const preset: Preset = {
  key: "test",
  label: "Test",
  era: "now",
  playlist: "solo",
  title: "Fallback",
  description: "Fallback text.",
  ranges: [4, 8, null],
  games: [
    { gameType: "slayer", variant: "Team Fight", team: true, maps: ["alpha yard", "Gamma Ridge"], weight: 3 },
    { gameType: "assault", maps: ["Beta Deck"] },
    { gameType: "ctf", maps: ["Beta Deck"] },
  ],
};

describe("the catalog", () => {
  test("drops malformed entries and refuses other versions", () => {
    expect(catalog.maps.map((m) => m.id)).toEqual([10, 20]);
    expect(catalog.maps[1]!.key).toBe("beta");
    expect(parseCatalog({ version: 2, maps: [] })).toBeNull();
    expect(parseCatalog(null)).toBeNull();
    expect(findMap(catalog, "ALPHA-yard")?.id).toBe(10);
  });

  test("campaign levels are kept apart from playlist maps", () => {
    expect(catalog.maps.map((m) => m.id)).toEqual([10, 20]);
    expect(catalog.campaign).toEqual([{ id: 5, name: "Gamma Level", description: "", key: "01x_gamma" }]);
    expect(findMap(catalog, "Gamma Level")).toBeNull();
  });
});

describe("presets", () => {
  test("resolve maps by name and copy a matching published variant", () => {
    const r = resolvePreset(preset, catalog, toEditable(published()));
    // The catalog's playlist text wins over the preset's own.
    expect(r.title).toBe("Solo Pit");
    expect(r.description).toBe("Alone.");
    expect(r.games.map((g) => [g.label, g.missing ?? null])).toEqual([
      ["Team Fight on alpha yard", null],
      ["Team Fight on Gamma Ridge", "Gamma Ridge is not in this game's maps"],
      ["assault on Beta Deck", "no published entry plays assault to copy"],
      ["ctf on Beta Deck", null],
    ]);
    // The named team variant in hopper 1 beats hopper 0's free-for-all one.
    expect(r.records[0]).toMatchObject({ id: 10, src: 0, srcHopper: 1, flags: 1, dwords: [3, 1] });
    expect(r.records[1]).toMatchObject({ id: 20, src: 1, srcHopper: 0, dwords: [1, 2] });
  });

  test("the picker's patch: copies across hoppers, keeps unset ranges", () => {
    const original = toEditable(published());
    const edited = toEditable(published());
    const r = resolvePreset(preset, catalog, original);
    edited[0] = applyPreset(edited[0]!, r);
    expect(hopperErrors({ ...edited[0]!, record_count: edited[0]!.records.length })).toEqual([]);
    const patch = buildPatch(original, edited);
    expect(patch.hoppers.length).toBe(1);
    const h = patch.hoppers[0] as PatchHopper;
    expect(h).toMatchObject({ index: 0, title: "Solo Pit", description: "Alone.", ranges: [4, 8, 4], record_count: 2 });
    expect(h.records).toEqual([
      { id: 10, string: "", flags: 1, dwords: [3, 1], from: 0, from_hopper: 1 },
      // From this hopper's own slot 1, now at slot 1: nothing to say.
      { id: 20, string: "", flags: 0, dwords: [1, 2] },
    ]);
  });

  test("unresolvable presets change nothing they cannot fill", () => {
    const original = toEditable(published());
    const none = resolvePreset(preset, null, original);
    expect(none.records).toEqual([]);
    expect(none.games.every((g) => g.missing === "no catalog from this server")).toBe(true);
    expect(none.title).toBe("Fallback");
    const kept = applyPreset(original[0]!, none);
    expect(kept.records).toEqual(original[0]!.records);
  });

  test("the checked-in presets are well formed", () => {
    const keys = new Set<string>();
    const types = new Set(["slayer", "ctf", "oddball", "king_of_the_hill", "juggernaut", "territories", "assault"]);
    for (const p of PRESETS) {
      expect(keys.has(p.key)).toBe(false);
      keys.add(p.key);
      expect(clip(p.title, 15)).toBe(p.title);
      expect(new TextEncoder().encode(p.description).length).toBeLessThanOrEqual(127);
      expect(p.games.length).toBeGreaterThan(0);
      for (const g of p.games) {
        expect(types.has(g.gameType)).toBe(true);
        expect(g.maps.length).toBeGreaterThan(0);
      }
      const total = p.games.reduce((n, g) => n + g.maps.length, 0);
      expect(total).toBeLessThanOrEqual(100);
    }
  });

  test("clip cuts on characters within the byte budget", () => {
    expect(clip("Big Team Battle!", 15)).toBe("Big Team Battle");
    expect(clip("ééééééééé", 15)).toBe("ééééééé");
  });
});
