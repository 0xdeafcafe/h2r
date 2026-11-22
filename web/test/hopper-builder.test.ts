// The playlist builder's working copy: unused slots, new playlists from a
// template, clearing, and picking an entry's game variant (validate.ts), with
// the PUT body each makes (the server side is live-server's apply_patch tests).

import { describe, expect, test } from "bun:test";
import { ENGINES, baseMapId } from "../src/admin/catalog";
import {
  duplicatePlaylist, entryShares, groupEntries, moveGroupEntry, playlistDiff, shareLabel,
} from "../src/admin/playlistDraft";
import {
  buildPatch,
  choiceKey,
  clearPlaylist,
  createPlaylist,
  editErrors,
  isUnused,
  toEditable,
  variantChoices,
  withVariant,
  type BuiltinVariant,
  type ClearHopper,
  type EditRecord,
  type HopperView,
  type PatchHopper,
  type PlaylistTemplate,
} from "../src/admin/validate";

const BUILTINS: BuiltinVariant[] = [
  { key: "rumble_slayer", name: "Rumble Slayer", engine: 2, team: false },
  { key: "team_slayer", name: "Team Slayer", engine: 2, team: true },
  { key: "classic_ctf", name: "Classic CTF", engine: 1, team: true },
];

const TEAM_SLAYER: PlaylistTemplate = {
  id: 2,
  type: 4,
  title: "Team Slayer",
  description: "Four against four.",
  min_players: 6,
  max_players: 8,
  ranges: [2, 3, 4],
  dwords: Array(18).fill(7),
  entry: { id: 50, variant: "team_slayer" },
};

/** Two playlists, then unused slots, as the default set leaves 12..16. */
function file(): HopperView[] {
  const used = (index: number, title: string): HopperView => ({
    index,
    type: 4,
    status: 0,
    record_count: 2,
    name: "",
    title,
    description: "",
    ranges: [2, 3, 4],
    dwords: Array(18).fill(1),
    records: [
      { id: 50, string: "", flags: 1, dwords: [10, 0], variant: { name: "Team Slayer", engine: 2, team: true }, builtin: "team_slayer" },
      { id: 20, string: "", flags: 3, dwords: [4, 9], variant: { name: "Team Hardcore", engine: 2, team: true } },
    ],
  });
  const unused = (index: number): HopperView => ({
    index,
    type: 0,
    unused: true,
    status: 0,
    record_count: 0,
    name: "",
    title: "",
    description: "",
    ranges: [0, 0, 0],
    dwords: Array(18).fill(0),
    records: [],
  });
  return [used(0, "Team Slayer"), used(1, "Big Team"), unused(2), unused(3), unused(4)];
}

describe("unused slots", () => {
  test("untouched unused slots never block a publish", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    expect(isUnused(edited[2]!)).toBe(true);
    expect(isUnused(edited[0]!)).toBe(false);
    edited[0]!.title = "Slayer";
    expect([...editErrors(original, edited)]).toEqual([]);
    expect(buildPatch(original, edited).hoppers.map((h) => h.index)).toEqual([0]);
  });

  test("a title typed into an unused slot says to make a playlist there", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    edited[2]!.title = "Modtacular";
    const errs = editErrors(original, edited);
    expect([...errs.keys()]).toEqual([2]);
    expect(errs.get(2)![0]).toContain("make a playlist here");
    // The other unused slots say nothing.
    expect(errs.has(3)).toBe(false);
  });
});

describe("a new playlist", () => {
  test("starts from a template with a valid type and one entry", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    edited[2] = createPlaylist(edited[2]!, TEAM_SLAYER, BUILTINS);
    const h = edited[2]!;
    expect(isUnused(h)).toBe(false);
    expect(h).toMatchObject({ template: 2, type: 4, status: 0, title: "Team Slayer", ranges: [2, 3, 4] });
    expect(h.records).toHaveLength(1);
    expect(h.records[0]).toMatchObject({ id: 50, pick: "team_slayer", src: null, dwords: [1, 0] });
    expect([...editErrors(original, edited)]).toEqual([]);
    edited[2]!.title = "Modtacular";
    const patch = buildPatch(original, edited);
    expect(patch.hoppers).toHaveLength(1);
    const p = patch.hoppers[0] as PatchHopper;
    expect(p).toMatchObject({ index: 2, template: 2, type: 4, title: "Modtacular", record_count: 1, dwords: TEAM_SLAYER.dwords });
    // A built-in pick is built by the server: a key, no source slot.
    expect(p.records[0]).toEqual({ id: 50, string: "", flags: 0, dwords: [1, 0], variant: "team_slayer" });
  });

  test("an emptied new playlist is caught at the playlist", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    edited[2] = { ...createPlaylist(edited[2]!, TEAM_SLAYER, BUILTINS), records: [] };
    expect(editErrors(original, edited).get(2)!.join()).toContain("1 to 100 entries, got 0");
  });

  test("another kind for a published playlist keeps its entries", () => {
    const original = toEditable(file());
    const h = createPlaylist(original[0]!, TEAM_SLAYER, BUILTINS);
    expect(h.records).toEqual(original[0]!.records);
    expect(h.template).toBe(2);
  });
});

describe("removing a playlist", () => {
  test("clears a published one; nothing for an unused one", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    edited[1] = clearPlaylist(edited[1]!);
    edited[3] = clearPlaylist(edited[3]!);
    expect(isUnused(edited[1]!)).toBe(true);
    expect([...editErrors(original, edited)]).toEqual([]);
    const patch = buildPatch(original, edited);
    expect(patch.hoppers).toEqual([{ index: 1, clear: true } satisfies ClearHopper]);
  });
});

describe("an entry's game variant", () => {
  test("choices: the built-ins, then each other variant in the file once", () => {
    const original = toEditable(file());
    const c = variantChoices(BUILTINS, original, ENGINES);
    expect(c.map((x) => x.key)).toEqual(["b:rumble_slayer", "b:team_slayer", "b:classic_ctf", "f:0:1"]);
    expect(c[1]!.label).toBe("Team Slayer · Slayer · teams");
    expect(c[3]!.label).toBe("Team Hardcore · Slayer · teams");
    expect(choiceKey(original[0]!.records[0]!, 0)).toBe("b:team_slayer");
    expect(choiceKey(original[1]!.records[1]!, 1)).toBe("f:1:1");
  });

  test("picking a built-in keeps the map and weight and sends its key", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    const c = variantChoices(BUILTINS, original, ENGINES);
    const r = withVariant(edited[0]!.records[1]!, c.find((x) => x.key === "b:classic_ctf")!, 0);
    expect(r).toMatchObject({ id: 20, dwords: [4, 9], pick: "classic_ctf", src: null });
    expect(r.variant).toEqual({ name: "Classic CTF", engine: 1, team: true });
    edited[0]!.records[1] = r;
    const p = buildPatch(original, edited).hoppers[0] as PatchHopper;
    expect(p.records[1]).toMatchObject({ id: 20, variant: "classic_ctf" });
    expect(p.records[1]!.from).toBeUndefined();
    expect(p.records[0]!.variant).toBeUndefined();
  });

  test("picking one from another playlist copies that slot", () => {
    const original = toEditable(file());
    const edited = toEditable(file());
    const c = variantChoices(BUILTINS, original, ENGINES);
    edited[1]!.records[0] = withVariant(edited[1]!.records[0]!, c.find((x) => x.key === "f:0:1")!, 1);
    const p = buildPatch(original, edited).hoppers[0] as PatchHopper;
    expect(p.index).toBe(1);
    expect(p.records[0]).toEqual({ id: 50, string: "", flags: 3, dwords: [10, 9], from: 1, from_hopper: 0 });
  });
});

describe("custom maps' base maps", () => {
  const maps = [
    { id: 10, name: "Ivory Tower", key: "cyclotron" },
    { id: 50, name: "Lockout" },
    { id: 100, name: "Beaver Creek" },
  ];

  test("by catalog key, else by name", () => {
    expect(baseMapId(maps, "cyclotron")).toBe(10);
    expect(baseMapId(maps, "maps/lockout.map")).toBe(50);
    expect(baseMapId(maps, "beavercreek")).toBe(100);
    expect(baseMapId(maps, "zanzibar")).toBeNull();
  });
});

describe("playlist rotation", () => {
  test("groups nonadjacent entries by variant while leaving published slot order intact", () => {
    const [hopper] = toEditable(file());
    const [slayer, hardcore] = hopper!.records;
    const alternate = { ...hardcore!, src: 2, variant: { name: "Team Hardcore", engine: 2, team: false } };
    const anotherMap = { ...slayer!, id: 10, src: 3 };
    const records = [slayer!, hardcore!, alternate, anotherMap];
    const groups = groupEntries(records);
    expect(groups.map((group) => group.slots)).toEqual([[0, 3], [1], [2]]);
    expect(groups.map((group) => group.label)).toEqual([
      "Team Slayer · Slayer · teams", "Team Hardcore · Slayer · teams", "Team Hardcore · Slayer",
    ]);
    expect(records.map((record) => record.src)).toEqual([0, 1, 2, 3]);
    expect(moveGroupEntry(records, groups[0]!.slots, 0, 1).map((record) => record.src)).toEqual([3, 1, 2, 0]);
  });

  test("uses the game's signed, clamped weights across all variant groups", () => {
    const [hopper] = toEditable(file());
    const [slayer, hardcore] = hopper!.records;
    expect(entryShares([slayer!, { ...hardcore!, dwords: [70, 0] }]).map(shareLabel)).toEqual(["12.5%", "87.5%"]);
    expect(entryShares([slayer!])).toEqual([100]);
    expect(entryShares([])).toEqual([]);
    const shares = entryShares([0, 1100, 0xffff_ffff].map((weight): EditRecord => ({ ...slayer!, dwords: [weight, 0] })));
    expect(shares[0]).toBeCloseTo(100 / 1002);
    expect(shares[1]).toBeCloseTo(100000 / 1002);
    expect(shares[2]).toEqual(shares[0]);
    expect(shareLabel(shares[0]!)).toBe("<0.1%");
  });

  test("duplicates edit data and uses the original source hopper for every copied record", () => {
    const original = toEditable(file());
    const draft = toEditable(file());
    const source = draft[0]!;
    source.title = "Équipe Hardcore";
    source.records[1] = { ...source.records[1]!, srcHopper: 1 };
    draft[2] = duplicatePlaylist(source, draft[2]!, TEAM_SLAYER);
    const patches = buildPatch(original, draft).hoppers as PatchHopper[];
    expect(patches[1]).toMatchObject({
      index: 2, template: TEAM_SLAYER.id, title: "Équipe Ha copy",
      records: [{ from: 0, from_hopper: 0 }, { from: 1, from_hopper: 1 }],
    });
  });

  test("diffs edits, reorders, additions and removals against the published slots", () => {
    const original = toEditable(file());
    const draft = toEditable(file());
    const name = (id: number) => id === 50 ? "Lockout" : "Ascension";
    expect(playlistDiff(original, draft, name)).toEqual([]);
    const [first, second] = draft[0]!.records;
    draft[0] = {
      ...draft[0]!, title: "New Slayer", records: [{ ...second!, dwords: [8, 9] }, first!],
    };
    draft[1] = clearPlaylist(draft[1]!);
    draft[2] = createPlaylist(draft[2]!, TEAM_SLAYER, BUILTINS);
    const review = playlistDiff(original, draft, name);
    expect(review.map(({ index, action }) => [index, action])).toEqual([[0, "Edited"], [1, "Removed"], [2, "Added"]]);
    expect(review[0]!.lines).toContainEqual({ field: "Title", before: "Team Slayer", after: "New Slayer" });
    const entry = review[0]!.lines.find((line) => line.field === "Entry 1");
    expect(entry?.before).toContain("Lockout · Team Slayer · Slayer · teams · Weight 10");
    expect(entry?.after).toContain("Ascension · Team Hardcore · Slayer · teams · Weight 8");
    expect(review[0]!.lines).toContainEqual({ field: "Entry 1 source", before: "Playlist 1, entry 1", after: "Playlist 1, entry 2" });
    expect(review[0]!.lines).toContainEqual({ field: "Entry 1 flags", before: "1", after: "3" });
    expect(review[2]!.lines).toContainEqual({ field: "Descriptor template", before: null, after: "2" });
  });
});


test("a Map Pack rotation keeps a disc-map entry", () => {
  const original = toEditable(file());
  const first = original[0]!;
  const addon = { ...first.records[0]!, id: 0x12345 };
  const disc = [{ id: first.records[0]!.id, name: "A disc map" }];
  const edit = (records: EditRecord[]) => [ { ...first, records }, ...original.slice(1) ];
  expect(editErrors(original, edit([addon, first.records[0]!]), disc).size).toBe(0);
  expect(editErrors(original, edit([addon]), disc).get(0)).toContain("Keep a Halo 2 disc map for players without the Map Pack.");
});
