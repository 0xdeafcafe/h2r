# Firefight kit: what the kit costs on the large multiplayer maps

Evidence for [firefight.md](../firefight.md), the kit's "Sectors the AI
can use" and cost report.

## 2026-11-23: Zanzibar over its headroom, where the bytes go

Read at `0723010bf` (before trimming and the group move), offline, on the
owner's disc. Nothing was built or run for these numbers.

**The refusal.** `h2 firefight kit zanzibar` (kit v2) reported: sectors
161836 B, zones 4480 B, squads 5056 B, characters and palettes 784 B,
scripts 264 B; tag growth 253952 B, available 135168 B. Beaver Creek:
sectors 82788 B, growth 167936 B, available 1409024 B.

**The 81.5 KB the parts did not list.** The parts sum to 172420 B, 81532 B
short of the growth. `h2 tags zanzibar` lists 5023 tags, 5023 index slots
and no null slot. The kit adds one tag; a python reader of the disc image
(below) shows Zanzibar's index region is `0x53000` bytes, the entries end
at `0x13f98` and the `0x3f068` bytes after them are 255431 nonzero (DXT
blocks by the look of them). So the entry table cannot grow in place and
moves to the metadata: 5024 × 16 = 80384 B. The remaining 1148 B are
alignment and the page rounding of `+0x1c`. Beaver Creek: 4575 × 16 =
73200 B, so its growth was sectors + entries + 11.9 KB as well.

**Where the sector bytes go.** Estimated from the bots' nav cache
(`~/Library/Caches/h2recomp/bots/nav-4-7`, the shared cache; polygons
are the generated sectors one for one, with each polygon's out-of-map
flag): links are the distinct polygon sides, vertices the distinct
corners. The cache files are keyed by hash; the one whose total matches
Zanzibar's 161836 B with refs of about 6,700 collision surfaces:

| Kept | Sectors | Links | Vertices | Bytes before refs |
|---|---|---|---|---|
| all generated | 2274 | 5082 | 2936 | 134736 |
| in the map | 1657 | 3444 | 1771 | 89612 |
| the largest walk component | 1514 | 3019 | 1454 | 77864 |

Refs (4 B per collision surface, about 27 KB) stay whatever is kept. The
same estimate for the cache whose total matches Beaver Creek: 67800 B all,
47260 B in the map, 29000 B largest component.

**What fits.** With the group table moved instead of the entries
(1416 B), Zanzibar's growth is about sectors + 12 KB + padding against
132 KiB available. Untrimmed (158 KiB of sectors) it cannot fit; trimmed
to the arena's in-map area (about 105 to 117 KB with refs) it fits with
roughly 4 to 15 KB to spare, by this estimate. The coordinator's kit run
on the disc gives the real figure.

**The other large maps**, from the room in
[docs/halo1/notes/shared-map.md](../../halo1/notes/shared-map.md):
Waterworks is at the multiplayer bound and has no room at any size;
Headlong has 340 KiB and Coagulation 876 KiB. Containment and Terminal
(Map Pack) were not measured; a map whose `+0x1c` is above `0x62c000`
gets no room from the kit's check (Turf is `0x68b000`).

**Not adopted here.** Clearing the index tail (`make_index_room`) would
let the entries grow in place for free, but the tail on retail
multiplayer maps is live-looking data. Raising `max_metadata_size` needs
the pool the loader leaves to be measured
(`[0x4e6440] - [0x4e642c]` after a load, scenario-editor.md §6); Turf
loading on the title update is the only evidence above the bound.

Reproduce: `h2 maps disc/halo2.iso` (tag and slot counts); the index
tail with a short python XDVDFS reader of `maps/zanzibar.map` (header
`+0x10` index offset, `+0x14` size; index `+8` entry pointer, `+0x18`
slot count); the sector estimate by parsing the nav cache files
(`H2BT` after a 4-byte prefix, then `H2NV` polygons with a flag byte,
bit 0 out of the map, `crates/live/bots/src/nav.rs` `NavMesh::to_bytes`).
