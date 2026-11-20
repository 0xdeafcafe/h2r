# A shared Halo 1 cache, and the Halo 1 sandbox on every map

Status: Design. Nothing here is built; the measurements behind it are in [notes/shared-map.md](notes/shared-map.md).
Code: none yet. It changes `crates/tools/h1objects`, `crates/tools/h1convert`, `crates/formats/content`, `crates/runtime/vfs` and `crates/extensions/game`; each slice below names its files and tests.
Switch: none new. The choice stays the setting `multiplayer.halo1_sandbox` ([multiplayer.md](multiplayer.md), "Halo 1 options").

The question: should Halo 1's assets live in one shared map, so that the
Halo 1 sandbox works on every map without building every map twice? The
answer is yes, in two parts that Halo 2's format keeps apart. Halo 2's
shared caches share **raw resource bytes only**, never tags, so a shared
Halo 1 cache removes the repeated bytes and nothing else. What removes
the second build is making the sandbox a **layer**: the Halo 1 sandbox is
built once from the disc as loose tags, and added to any map as a small
patch over the map's one build. The rule that must hold: a map's Halo 2
view is the file it is today, byte for byte, and its Halo 1 view is that
file plus the layer.

## How Halo 2's shared caches work

- **Tags are never shared.** Every tag a retail map uses has its
  metadata in that map; no tag on the disc lacks local data (**V**,
  `crates/formats/map/tests`, reported in
  [map-builder.md](../formats/map-builder.md)). The loader reads one tag
  index and one metadata region, the current map's
  ([map-format.md](../formats/map-format.md), "Index, root tags and
  address translation"). `shared.map` and `single_player_shared.map`
  have tag tables of their own, which a level does not load.
- **Raw resources are shared.** A raw pointer's top two bits pick a file
  and the low 30 bits are an offset in it: the current map,
  `mainmenu.map`, `shared.map` or `single_player_shared.map`
  (`map::RawPointer`, `map::ResourceFile`; map-format.md, "Raw/shared
  resources"). Bitmap pixels, model geometry and sound samples are
  reached this way. All four values are in use (**V**), so there is no
  free selector for a fifth file.
- **The engine reads them from cache slots.** Each shared cache is
  copied whole from the disc to its own slot on the utility partition
  (shared slot 0, campaign shared slot 1, menu slot 2) and read from
  there by offset; the route function picks the slot handle from the
  selector (**V**, `crates/runtime/kernel/tests/cache_slots.rs`: all
  four selectors read from the right slot;
  [cache-slots.md](../runtime/cache-slots.md)).
- **Each file has two limits.** The header validator looks up a
  per-type file limit (`engine::points::CACHE_TYPE_LIMIT`) under an
  absolute 520 MiB, and each slot has a capacity in a table the
  allocator and the slot selector read. Converted campaign levels
  already raise both for slots 4 and 5
  (`crates/extensions/game/src/campaign_cache.rs`;
  [sounds.md](sounds.md), "Campaign loops and cache slots").
- **Dependents check identity.** A map's header carries the identities
  of the caches it was built against, compared with the loaded caches'
  when it is copied (map-format.md, "Lossless writing and bounded
  edits"). A shared cache's header fields must not change under the
  maps that name it.
- **Tag memory is per map.** The loader takes the map's whole metadata
  (header `+0x1c`) from one physical arena; what is left is the pool
  everything else allocates from. No constant bounds it. Our bound is
  the largest retail map of each type (`map::writer::max_metadata_size`).

Consequences: a shared file cannot carry a weapon. It can carry the
weapon's pixels, geometry and samples; the `weap`, `mode`, `bitm`,
`shad` and `snd!` tags, and the map's sound gestalt entries, must be in
every map that uses it. That per-map part is small: 0.4 to 0.6 MiB of
tag metadata against 15 to 20 MiB of raw for the Halo 1 multiplayer
sandbox ([note](notes/shared-map.md)).

## The options

| | Option | What it shares | Cost and risk |
|---|---|---|---|
| A | A fifth resource file with its own selector | raw | Not possible as data: two selector bits, four values used. Would need every raw reader in the engine hooked. |
| B | **The shared cache's offset space, extended**: the VFS serves `shared.map` as the disc's bytes followed by a Halo 1 appendix, and Halo 1 raw pointers use the shared selector with offsets in the appendix | raw | Slot 0's capacity and the shared type's file limit grow (the campaign slots' precedent). One more thing every peer, host and tool must hold. The engine's own copy, checksum and paging run unchanged. |
| C | Our own rebuilt `shared.map` on disk, Halo 1 assets added | raw | The same bytes as B, but it replaces a disc file: its hash, and the identity dependents check, are the disc's no longer. Adding tags to it gains nothing, since no level loads them. |
| D | Per-map copies, as today | nothing | No engine work. 187 MiB of repeats over the 13 converted maps, growing by about 16 MiB for each further map. |
| E | Tags loaded at run time from a second cache into tag memory | tags and raw | The loader has no such path. It needs datums, addresses and string ids relocated per map, the sound gestalt merged in guest memory, and the same result on every peer: `map::build` running inside the loader. |
| F | One file holding both sandboxes, swapped by a hook at game start (as the HUD is, [hud.md](hud.md), "The switch: one map, both HUDs") | none | Both tag sets in tag memory always, on maps with no room (below). The retail file changes, so a retail map is retail no longer. The choice becomes hidden state that every peer must replicate. |
| G | **The sandbox as a layer**: built once as loose tags, added to a map by a patch over its one build | tags are still copied into each Halo 1 view, by a patch, not a second conversion | The patch is made per map (about a second, no Halo 1 conversion in it). The Halo 2 view is untouched. The view is part of the map's identity, which Live already carries. |

What each concern says:

- **Tag memory.** Only the Halo 1 view pays (G), about 0.4 to 0.6 MiB
  plus the gestalt's growth. Eight of the disc's twelve multiplayer
  maps have `0xdb000` or more under the bound; `headlong` has
  `0x55000`, `zanzibar` `0x21000` and `waterworks` none
  ([note](notes/shared-map.md)). The bound is empirical, and a Map Pack
  map `0x5f000` over it loads (`crates/formats/map/src/build.rs`, the
  pinned source's allowance), so the real room is unknown until
  measured (slice 0).
- **Resource paging.** Halo 1 raw in the appendix pages exactly as
  Halo 2's shared raw does: read on demand from slot 0 into the
  engine's texture, geometry and sound caches. Nothing new is resident.
- **Load time.** Slot 0 is copied once, when it is invalid, at about
  209 MiB in place of 175 (the base at 180 MiB, then an appendix near
  29 MiB for multiplayer). A
  level load copies a smaller Halo 1 view than today's, since its raw
  is not in it.
- **Peers.** Two players are on the same map when their `MapId`s match
  ([maps.md](../game/maps.md), "Identity: `MapId`"). A Halo 1 view's
  identity is its base, its layer and the appendix it points into, so
  the pack, the appendix and the layers must build to the same bytes
  from the same disc and converter version on every machine.
- **The Map Pack.** Its maps use the disc's shared caches, so they
  reach the appendix the same way. They are the largest multiplayer
  maps in tag memory, and the first to run out.
- **The editor and tools.** Everything that resolves a shared raw
  pointer learns one rule (an offset at or past the appendix base is in
  the appendix): `crates/tools/editor-core/src/source.rs`,
  `crates/apps/studio/src/app/shared_refs.rs`,
  `crates/apps/cli/src/bitmap.rs`, `crates/apps/cli/src/sound.rs`,
  `crates/extensions/game/src/pad_speaker.rs`,
  `crates/formats/tags/src/repo/extract.rs`, the viewers and
  `h2 live assets extract`.

## Recommendation

Do G, then B. Not A, C, E or F.

1. **G first.** It is what stops the double build, it needs no engine
   work, and it is the only option that reaches Halo 2's own maps
   without changing them. It also settles the open point in
   multiplayer.md that a guest loads its own setting's variant: the
   view is in the `MapId` the host advertises.
2. **B second.** It turns each Halo 1 view from about 16 MiB of
   repeated raw into a patch of tag metadata, which is what makes
   "every map" cheap to store, send and host. It reuses mechanisms that
   are built: the content resolver's overlay with its checksum repair
   ([maps.md](../game/maps.md), "Applying layers at load"), and the slot
   growth of `campaign_cache.rs`.
3. **Campaign last.** The two asset variants of a level share 94 to 98%
   of their raw, 1.57 GiB over the ten levels, and the ten Halo 1
   levels repeat 634 MiB among themselves ([note](notes/shared-map.md)).
   The twin becomes a layer over the level (slice 7). Campaign raw goes
   to the appendix only for what fits in the 345 MiB the absolute limit
   leaves above `shared.map`.

## How it works

**The Halo 1 pack.** The import builds the Halo 1 sandbox once, from
the union of the disc's maps, with `h1objects` as it does per map: every
weapon, projectile, powerup, grenade, vehicle, the cyborg and its
first-person arms, with their models, collision, animations, effects,
shaders, bitmaps, sounds and HUDs. It is written as a loose tag
repository ([tag-repo.md](../formats/tag-repo.md)) under
`<data>/halo1/sandbox/`, keyed by group and name, with nothing in it
that depends on a map. A tag that needs a retail tag (a shader stem, an
object template's children) names it by group and name; applying the
pack to a map that lacks one is a named error
([multiplayer.md](multiplayer.md) and the role audit keep the rule that
nothing falls back quietly).

**The layer.** `h1convert` applies the pack to a built map through a
pinned `map::build::TagSet` (`TagSet::from_map`, `TagSet::add_tag`): it
adds the pack's tags after the map's own slots, merges their sounds
into the map's gestalt (`h1_assets::sounds::merge_sound`), and makes the
edits `convert::halo1` makes today: the player representation and arms
(`h1convert::globals`), the multiplayer globals' weapon list, one item
or vehicle collection per spawn, the starting equipment. The difference
between the map and the result is written as a file patch, and the
package lists the Halo 1 view as the map plus that patch. A converted
Halo 1 map's structure, sky, scenery and placements are converted once,
into its Halo 2 view; its Halo 1 view takes its item plan from the
Halo 1 scenario, as today. A retail Halo 2 map's Halo 1 view takes its
plan from the reverse sandbox table (below). The VFS serves the base or
the base with its layer by the game's choice, as
`vfs::ContentVariants` does for the two files today
(`crates/runtime/vfs/src/variants.rs`).

**The appendix.** The pack's raw is written once to
`<data>/halo1/halo1_shared.raw`, and the pack's tags point at it with
the shared selector and an offset from a fixed appendix base,
`0x0b400000`: the retail limit for the shared type, which no retail
`shared.map` reaches. The VFS serves `maps\shared.map` as the disc's
file, zeros up to the base, then the appendix, with the declared length
and the copy checksum repaired in the header and every other header
byte the disc's. The shared type's file limit and slot 0's capacity are
raised to the served size at boot, bounded by the absolute 520 MiB, and
the utility partition grows by the same amount. Slot 0 is dropped at
boot when the appendix it was copied with is not the installed one.
Without an installed pack the VFS serves the disc's file and nothing is
raised.

## Beyond assets: what the Halo 1 sandbox on every map needs

- **A reverse sandbox table.** `h1convert::sandbox` maps Halo 1 items
  to Halo 2's, and `halo1_weapon_for` maps the weapon list back. A
  Halo 2 map's spawns need the whole reverse: each Halo 2 item and
  vehicle collection to a Halo 1 collection, and a decision for each
  thing Halo 1 has no counterpart of (the sword, dual wielding, the
  Covenant vehicles Halo 1's multiplayer lacks, turrets): a nearest
  equivalent, as the weapon list already picks, or left out and listed.
- **Game types and variants.** Halo 2's game types run unchanged
  (decided, [campaign.md](campaign.md) "Decisions"). A variant's
  starting weapons index the multiplayer globals' weapon list, which
  the layer rewrites ([multiplayer.md](multiplayer.md), "Starting
  weapons"); "map default" reads the scenario's starting equipment,
  which the layer also writes. A variant that overrides vehicles or
  the weapon set reads other lists in the globals; each is found and
  rewritten, or the override keeps Halo 2's object and is listed.
  Grenade types and counts come from the globals' grenade block.
- **The player.** Both multiplayer representations become the cyborg,
  with Halo 1's arms, so the Elite choice draws the cyborg.
- **Health, precision and the HUD follow the sandbox, not the map
  name.** `health::model_for_map` and `hud::style_for_map` key on the
  `h1_` prefix today (`crates/extensions/game/src/health.rs`,
  `crates/extensions/game/src/hud.rs`); they take the served view. The
  HUD pairs `hud_swap` reads come from the layer's manifest entry. The
  health model, Halo 2's physics and the health pack's powerup type are
  as decided in [health.md](health.md) and campaign.md §6.
- **The options row.** "Halo 1 Options" shows on every multiplayer map
  that has a Halo 1 view, not only on converted maps
  (`crates/extensions/game/src/h1_options_row.rs`).
- **The session.** The host's view is the map the session advertises;
  a guest resolves the same `MapId`, builds the layer if it has the
  pack, and otherwise is refused with the reason.
- **Hosts.** A dedicated host's data bundle carries the appendix and
  the layers of the maps it serves
  ([dedicated-host-build.md](../live/dedicated-host-build.md)).
- **Stats and the editor.** The stats site names Halo 1 weapons from
  the tags a view holds; the editor's palettes list the pack's objects
  on a Halo 1 view.

## Slices

In order. Slices 3 and 4 are independent of each other; both need 2.

| # | Slice | Acceptance |
|---|---|---|
| 0 | **Evidence.** (a) Every reader of a shared slot: the callers of the route function and any other use of the slot handles, read with `h2 disasm`. (b) Which header fields the copy and the validator read for the shared type, and the shared row of the slot table. (c) Tag memory room in a 16-player multiplayer game: a new agent API method reporting the tag arena's cursors and the pool's low mark, run on the largest disc and Map Pack maps. | A dated entry in [notes/shared-map.md](notes/shared-map.md) for each. (c) either raises `map::writer::max_metadata_size` for multiplayer with its evidence or names the maps that get no Halo 1 view. |
| 1 | **The pack.** `h1objects` builds the sandbox from the union of the disc's maps into a tag repository, with its raw local to the repository. | Synthetic: the same input gives the same bytes; every reference resolves in the pack or names a required retail tag. Disc (`#[ignore]`): the pack holds every tag the 13 per-map builds add, each decoding to the same fields. |
| 2 | **The layer on converted maps.** `h1convert` applies the pack to a built map; the manifest's Halo 1 variant is the map plus a patch; the import converts each map once. | Disc: every check of `crates/tools/h1objects/tests/sandbox_disc.rs` passes on the layered view of all 13 maps. `cli` `h1_import::tests`: 13 conversions, not 26. `content`: the Halo 2 view's `MapId` does not depend on the layer; the same inputs give the same layer bytes. |
| 3 | **The appendix.** The pack's raw in `halo1_shared.raw`; the VFS view of `shared.map`; the limit and capacity growth; slot 0 dropped on change; the tools' rule. | `vfs` synthetic: disc bytes below the disc's length, zeros in the gap, appendix bytes above the base, a header that passes `Map::validate_checksum`, the disc's file when no pack is installed. `cache_slots.rs` (`#[ignore]`): slot 0 fills with the grown file, a shared-selector read in the appendix returns its bytes, the menu and a multiplayer map still validate against it. Disc: every raw pointer of a layered view is in range and every sound and bitmap reads back equal to the per-map build's. |
| 4 | **Halo 2 and Map Pack maps.** The reverse table; layers made at import for every multiplayer map on the disc and in the Map Pack; health, precision, HUD and the options row keyed on the view. | Disc: every such map's Halo 1 view is under the tag memory bound of slice 0, every item and vehicle spawn names a pack collection or is in the printed left-out list, and every built-in variant's starting weapons are held by the cyborg's graph (`every_starting_weapon_is_held_by_the_halo1_player_graph`, over these maps). The unlayered map is byte-identical to the disc's. `game` tests: the row shows on a retail map with a view; `model_for_map` follows the view. |
| 5 | **The session.** The view in the advertised `MapId`; a guest builds or is refused; the host bundle. | `live-hle` and `content` synthetic: a guest with the pack resolves the host's view; one without is refused by name. `host-data` tests: the bundle lists the appendix and layers. |
| 6 | **Editor.** Open and save a Halo 1 view; palettes; shared references counted into the appendix. | `editor-core` and `studio` tests on synthetic maps with appendix pointers. |
| 7 | **Campaign.** The Halo 2 assets level as a layer over the Halo 1 level, from one conversion; campaign raw repeated across levels moved to the appendix where it fits. | Disc: `crates/tools/h1campaign/tests/assets.rs` passes on the layered twins; the import runs 10 level conversions, not 20; a note entry with the installed size before and after. |

## Interfaces

- `h2 import halo1` writes the pack, the appendix and one layer per
  map, under the stamp of [campaign.md](campaign.md) §10.
- `content::Manifest`: a variant is a file, or a base file with patches.
- `vfs`: the shared view, and the base-or-layer choice per map.
- `engine::points::CACHE_TYPE_LIMIT` and the slot table: the shared
  type, beside the campaign type.
- `map::RawPointer`: one function says whether a shared offset is in
  the appendix; every tool uses it.

## Rules and limits

- A map's Halo 2 view is never rewritten. A retail map with no layer
  applied is the disc's bytes.
- The disc's `shared.map` is never written; the appendix is a file of
  ours beside it in the data root.
- Appendix offsets are part of every layer. A converter change that
  moves them rebuilds every layer, under the import stamp.
- Served size of `shared.map` is at most 520 MiB: 340 MiB of appendix
  above the base.
- A map whose Halo 1 view does not fit in tag memory has no Halo 1
  view, and the options row says so; nothing is trimmed quietly.
- No game data in the repo: the pack, the appendix and the layers are
  built from the user's discs into the data root.

## Evidence

- **V** No retail tag lacks local data; shared maps are reached only
  through raw pointers: [map-builder.md](../formats/map-builder.md).
- **V** Four selectors, four slots: `crates/runtime/kernel/tests/cache_slots.rs`.
- **Code** Per-type file limits and the tag memory bound:
  `crates/formats/map/src/writer.rs`. Slot growth for converted
  campaign levels: `crates/extensions/game/src/campaign_cache.rs`.
- **Code** The overlay that grows a file and repairs its checksum:
  `crates/formats/content` ([maps.md](../game/maps.md), "Applying layers at load").
- **V** Sizes, repeats and room: [notes/shared-map.md](notes/shared-map.md).
  `shared.map` is 174.9 MiB of a 180 MiB limit; the Halo 1 multiplayer
  sandbox's raw is 28.9 MiB once against 215.9 MiB across 13 maps.
- **S** That the route function is the only reader of shared raw, that
  the copy takes the declared length, and that the shared row grows as
  the campaign row does: slice 0.
- **S** That a pinned map with appended tags and in-place scenario and
  globals edits loads as the from-scratch Halo 1 build does: slice 2's
  disc tests, then a game run.

## Not verified

- A grown `shared.map` copied to slot 0 and read from the appendix in a
  running game.
- A multiplayer map's tag memory past the retail bound with 16 players.
- A Halo 1 view of a retail Halo 2 map in play: spawns, starting
  weapons, vehicles, the HUD and health on a map not made for them.
- Two peers building the same layer bytes from the same discs on
  different machines.
- A guest joining a host's Halo 1 view.

## Related

- [multiplayer.md](multiplayer.md): the converted maps, the sandbox table, the Halo 1 sandbox, the options row.
- [campaign.md](campaign.md): the asset setting and importing.
- [objects.md](objects.md), [hud.md](hud.md), [health.md](health.md), [sounds.md](sounds.md).
- [map-format.md](../formats/map-format.md), [map-builder.md](../formats/map-builder.md), [tag-repo.md](../formats/tag-repo.md).
- [maps.md](../game/maps.md): packages, layers, `MapId`. [cache-slots.md](../runtime/cache-slots.md): the slots.
- [notes/shared-map.md](notes/shared-map.md): the measurements.
