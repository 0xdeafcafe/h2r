# Halo 1 multiplayer: converting a map

The conversion of one map and the Halo 1 options, split from [multiplayer.md](multiplayer.md).

| Section |
|---|
| [Converting a map (**Built**: `h1convert::convert`, `h2 h1 convert`)](#converting-a-map-built-h1convertconvert-h2-h1-convert) |
| [Halo 1 options (**Built**, tests only: `boot::h1_options`, `boot::h1_options_row`)](#halo-1-options-built-tests-only-booth1_options-booth1_options_row) |

Part of [multiplayer.md](multiplayer.md).

## Converting a map (**Built**: `h1convert::convert`, `h2 h1 convert`)

The library entry point, for the CLI and the editor:

```rust
let p = h1convert::convert::prepare(&h1, source_sha256, &mut base, Sandbox::Halo2)?;
// p: placements, structure counts, what wasn't placed (a dry run's report)
let done = h1convert::convert::convert(&h1, source_sha256, &mut base, Sandbox::Halo2)?;
// done.name ("h1_beavercreek"), done.bytes (the map), done.manifest
```

`convert(h1, source_sha256, base, sandbox)` takes the Halo 1 map's file
hash as well as the map (the `[origin]` below needs it, and `H1Map` holds
the inflated bytes, not the file). It is `prepare`, then the MOPP code
from `convert::structure_mopp`, then `Prepared::build`.
`structure_mopp` calls `mopp::build_structure(&bsp)`
([mopp.md](../formats/mopp.md)). `prepare` and `convert` with
`Sandbox::Halo1` return `Error::Halo1Sandbox`: the Halo 1 sandbox is
`convert::halo1::{prepare_halo1, convert_halo1}`, which take the objects'
converter (`h1objects::sandbox::Objects::new(&h1, &Base::read(&mut base)?)`;
below, "Halo 1 sandbox"). `h2 h1 convert` and the editor call it.

```sh
cargo h2 h1 convert disc/halo1.iso beavercreek --out ~/h2-h1maps [--sandbox h2] [--base beavercreek] [--disc disc/halo2.iso] [--force]
cargo h2 h1 convert path/to/beavercreek.map --dry-run
```

Multiplayer maps are written **fresh** by default (`--pinned` writes a
pinned map), and so does `h2 import halo1`: the same map with **nothing
pinned**: every tag is rebuilt as a tree, and the base's raw resources, sound
chunks and low-detail bitmaps are copied, so nothing depends on the base's
file layout. The scenario is built from its blocks. See
[tag-schema.md](../formats/tag-schema.md), "Fresh maps". The output path
and manifest are the same as without it:

```sh
cargo h2 h1 convert disc/halo1.iso bloodgulch --out ~/h2-h1maps [--pinned]
```

**Pinned maps drop what nothing reaches** (`h1convert::fresh::prune_pinned`,
in `Prepared::assemble`, so both the `--pinned` map and the fresh build's
intermediate). A pinned map keeps the base's metadata where it is and puts
every built tag after it, so it holds all of Coagulation's retail tags
(546 effects, 708 bitmaps) beside the conversion's; Blood Gulch's Halo 1
sandbox with Halo 1's effects and sounds would be 0x634000 of the
multiplayer 0x62c000 (`map::Error::TagMemory` names the groups). The fresh build's roots and edits are applied to the
pinned set: base entries of the scenery, sound scenery, light fixture and
crate palettes nulled in the pinned scenario's bytes, and in every object
palette (`tags::object::OBJECT_TYPES`) each base entry no placement names
(a placement's `+0` short is its palette index; `fresh::null_unplaced`):
in the Halo 1 sandbox the base's vehicles, bipeds and weapons stay in
their palettes with nothing placing them, and keep everything they name),
the scenario's tag list (`+0x3d8`) keeping a base tag only when something
else reaches it, then every pinned base tag not reached from the scenario, the globals,
`fresh::KEEP_GROUPS`, `fresh::KEEP_PREFIXES`, the precision collections
and the Halo 2 HUDs is dropped, its slot left empty and its bytes free
for built tags (`TagSet::reuse_free_space`). What a pinned map holds
and less (`fresh::pinned_reach`): a pinned tag the schema describes is
walked from its root through its blocks and names every word of the
arrays it holds (as the fresh build reads a built tree), so stale words
no array covers (an emptied block's old elements, a removed
placement's) keep nothing. Retail shares block data between tags: an
array it points to can sit in another tag's bytes (V, Coagulation: the
`c_turret_ap` model's block at `0x804f4164` lies inside the wraith
model; the compiler shares identical function data, a `jpt!`'s
`[0x6c, 0x44]`). Such an array keeps the datums in it (its count times
the element size of the block field pointing at it, and each block
below it the same way), which the walk has read, and the tag it falls
in stays whole as its holder (`fresh::Reach::Holds`,
`Pruned::holders`) without being reached: each word of the holder
naming a dropped tag is nulled, so its own references keep nothing. A
holder's blocks in a third tag make that one a holder too; one whose
root doesn't read by the schema is reached whole. The holder must stay
a tag: the schema moves a block outside its tag only when it sits
inside another tag (`tags::schema::walk`). Kept as metadata no tag owns
instead, the bytes break the fresh build (1288 references out of bounds
on every map), and keeping the whole tag and every reference in it would
keep the wraith with its model.
A size-0 tag (an alias) reads its root in another tag's bytes, so a
reached alias makes that tag its holder the same way; otherwise the
pinned map's alias points at freed bytes.
Every word of a kept pinned tag that names a dropped tag is nulled
(`Pruned::nulled_words`): stale words no walked array covers keep
nothing (the pinned Blood Gulch scenario otherwise names the base's
dropped powerups, vehicles, ambience, reverbs and decorators through
them).
An array below the root that doesn't read (outside the
metadata, implausibly long) is skipped, and the tag's own words then
name tags as datums only (`Pruned::partly_walked`). Only a field the
schema says is a block gives an address. A tag the schema doesn't
describe (or whose root doesn't read) is read word by word, every word a
datum and an address: metadata no tag owns that it points into is read
the same way, and a base tag it points into is kept. A gap nothing
points into keeps nothing. A built tag names its fixups and words as
datums only, not as addresses: the sound gestalt, rebuilt with Halo 1's
sounds, holds raw offsets that fall inside Coagulation's wraith and
warthog models and collision, and read as addresses they would keep
every retail vehicle (`Pruned::why` names the edge). The scenario's own edits (trigger
volumes parked, Halo 1's skies and ambience, the base's light fixtures
and decorators emptied) are made before this, so the base's sky,
ambience and decorators they replace go too. On the fresh map Blood Gulch drops
510 of Coagulation's tags (0x74698 bytes, most of it three `jmad`s and
retail shaders and bitmaps); `sandbox_disc::pinned_bloodgulch_fits_its_tag_memory`
prints the pinned map's tag memory, the largest base tags kept in the
metadata with what names each, and checks no kept tag names a dropped
one. The largest tag under a base name is no base tag: the structure BSP
is Halo 1's, written into Coagulation's `sbsp` slot (and so under its
path), and lives in the BSP region, not the metadata.

**The multiplayer globals' vehicle list** (`mulg` universal `+0x70`,
8-byte tag references like the weapon list at `+0x68`;
`h1convert::globals::UNIVERSAL_VEHICLES`). V, Coagulation: 11 entries,
the warthog, ghost, scorpion, wraith, banshee, spectre, none,
`h_turret_ap`, none, none, `c_turret_ap`. It is the only place that
names every retail vehicle (S: the swaps a variant's vehicle options
make take their vehicle from it, by its shape; the reader is not
traced), and the `mulg` is kept whole, so it kept
every retail vehicle and all each names (models, collision, physics,
animations, the warthog's and scorpion's turrets) in a pinned Halo 1
sandbox map: the chain each kept tag was reached by is
`fresh::Pruned::why` (`Prepared::build_pruned`), and the pinned test
prints it for the vehicles. In the Halo 1 sandbox each entry whose
vehicle the spawns place a Halo 1 vehicle of the same role for
(`convert::halo1::halo1_vehicle_for`: the warthog's entry the plain
warthog, not the rocket warthog; a role Halo 1 has no vehicle of takes
the placed one of its netgame type, so the wraith's entry names the
scorpion, the other heavy vehicle) names that vehicle
(`plan_vehicle_list`, written with `globals::set_universal_list` when the
map is built, as the weapon list is), so a variant's swap gives a
Halo 1 vehicle and the retail ones the spawns stand in for are dropped.
The others keep the base's and are reported: on Blood Gulch the banshee,
spectre and both turrets. The wraith goes too: its minigun turret
names `ai\generic_turret`, which names `ai\generic`, which names every
retail vehicle (V, Coagulation; read off the disc), so a kept wraith keeps
the warthog, ghost and scorpion the list no longer names. A pinned map
that is over its tag memory still shows what it keeps and why:
`Prepared::pinned_set` gives the pruned set before the build, with each
kept tag's first edge (`Pruned::why`) and the pinned tags read word by
word (`Pruned::word_read`).

**Ambient sound (Built, `h1convert::ambience`, `h1sounds::looping`).**
Halo 1 keeps a level's background sounds and sound environments in the
structure (`sbsp +0x1fc`, `0x74` each, and `+0x208`, `0x50` each; a name
and the `lsnd` or `snde` at `+0x20`); Halo 2 in the scenario
(`scnr +0x250`, `0x64` each, and `+0x258`, `0x48` each). Each Halo 1
looping sound converts with every `snd!` it plays
(`h1sounds::looping::read_looping`, then `install_looping`, which adds
the sounds to the `ugh!` in one pass and reuses any tag already of that
path); each sound environment converts with its four intensities from
linear to decibels (`read_environment`). The base scenario is pinned, so
the entries are written into the base's own palette storage, as many as
it holds (Coagulation 3 and 4, Beaver Creek 2 and 3; every Halo 1
multiplayer level has 1 and 1), and the count shrinks to that. Halo 1 has
no cutoff distance or interpolation speed; the values written are ones
retail uses. Blood Gulch gets `sound\sfx\ambience\multiplayer\bloodgultch`
(its one track plays `countryday2`) and `mountains`. Each cluster names
its sound (`clusters::sound_zones`). Halo 1's cluster keeps its
background sound and sound environment at `+0x04` and `+0x06` (`i16`
indices into the structure's palettes, -1 none; clusters at
`sbsp +0x134`, `0x68` each), and each structure leaf (`+0xe0`, `0x10`
each, one per collision leaf) its cluster at `+0x08`
(`h1convert/tests/h1_clusters.rs` checks both on every level). Halo 2
reads the listener's cluster (its collision leaf's) and that cluster's
`+0x72` and `+0x74`, indices into the structure's own copies of the
palettes (`sbsp +0xd4` and `+0xdc`, which equal the scenario's on all
160 retail structures; [structure-bsp.md](../formats/structure-bsp.md)).
The converter maps each Halo 1 pair through the written palettes
(`Ambience::index`; anything not written, or past the engine's 64, is
-1); each converted cluster is its Halo 1 cluster and plays its pair,
and each collision leaf names its Halo 1 cluster
(`clusters::sound_zones`). `check_sound_clusters` checks the ranges on
every converted map. A gestalt that gains sounds is built, not pinned, and a fresh
build copies its base chunks from the pinned file too
(`fresh::sound_gestalt`). `h1sounds/tests/looping_disc.rs`: all 734
looping sounds (829 tracks, 52 detail sounds, 1261 sounds) and 63 sound
environments on the disc convert.

**The base's unreachable tags are dropped (`fresh::prune`).** After the
fresh tree is built, the base's palette entries at `scnr+0xe0` (scenery),
`+0xf0` (lights) and `+0x330` (sound scenery) are nulled, and the
scenario's tag list at `+0x3d8` (4-byte datums) keeps only base tags
still reachable. Then every base tag nothing reaches from `scnr`, `matg`
or the kept roots loses its slot (the slot stays, empty, so indices
don't move). Roots kept whatever references them (`fresh::KEEP_GROUPS`):
`matg`, `scnr`, `ugh!`, `unic`, `mulg`, `stem`, `spas`, `vrtx`, `pmov`,
`sncl`, `snmx`, and every tag named under `shaders\` or `rasterizer\`
(`KEEP_PREFIXES`: a `stem` names `spas` passes by a word the tree doesn't
hold as a reference). No group the engine walks on its own is confirmed
in the XBE yet; the list is the conservative guess a game run tests.
Campaign levels use the same machinery with their own scenario
(`fresh::prune_with`, `fresh::Keep`; docs/halo1/campaign.md §9).
Blood Gulch on Coagulation drops 253 tags (85 `bitm`, 50 `shad`, 15
`snd!`, 14 `mode`, 13 `hlmt`, 12 `scen`, 11 `lsnd`, ...), 25 palette
entries and 21 tag list entries; all 21471 references in the kept tags
still resolve (`tests/fresh.rs`). The scenery palette at `+0x58` is
pruned like the others: its elements are `0x28`
bytes, a reference at `+0`, which the XBE settles
(`docs/formats/tag-schema.md`, "Engine strides"). The dropped tags' raw
data stays in the file, unused.

**Then built from scratch (`h1convert::scratch`).**
The pruned set is a step, not the output: `fresh::from_scratch` reads
every tag by `(group, path)` through `tags::repo` (`scratch::LevelSource`:
the conversion's tags from the pinned map it wrote, the rest from the
retail base as it is on the disc), from the roots `matg`, the keep list
and every tag the built scenario and structure name, and lays them out on
a dense index of their own (`to_tag_set`); the scenario and structure are
the fresh build's, their datums renamed (`scratch::Transfer`). Equal
blocks are shared across tags, as retail does, except bitmaps and the
sound gestalt, which the readers that take one tag's bytes
(`tags::DecodedTag`, `SoundGestalt::decode`) need whole
(`scratch::SELF_CONTAINED`). If it fails, the conversion fails with
the reason (`Error::Fresh`, "not built from scratch"); the pruned set is
never written. A reference to a tag no source holds is the same
error, naming the tag, field and missing tag. Blood Gulch on Coagulation: 4,385 tags
(4,333 retail, 49 converted, 3 scenario and structure), 4,385 index
slots, 0 dangling, 25.2 MB (pinned 48.6 MB); every tag but the sounds
identical to the pinned map's by name, address- and index-free, the
scenario's 20 blocks and root, every bitmap and the 9 languages' strings
the same; 21,570 references resolve (`tests/fresh.rs`). The sound gestalt
is cut to the 1,270 kept sounds (`h1convert::compact`; campaign.md §9):
5,293 chunks to 5,054, 223 raw resources (8.1 MB) dropped, each sound
naming the same entries, import names and chunk bytes as in the pinned
map.

**The map's card is Halo 1's** (`h1convert::ui`). Halo 1's `ui.map` holds
the map select's pictures (`ui\shell\bitmaps\mp_map_grafix`, 14 images,
256x128), names (`ui\shell\main_menu\mp_map_list`) and descriptions
(`...\mp_map_select\map_data`), all in one list order (`ui::MP_ORDER`;
image 9 is Blood Gulch); each picture fills the top-left 139x113 of its
image. The converter crops it, writes it as `preview.png` in the package
and sets the manifest's `name`, `description` and `image`, which the map
list already shows for added maps (`boot::new_maps`). From a disc it reads
the disc's `ui.map`; from a `.map` file, a `ui.map` beside it, else the
card stays the base's. The campaign's (`ui\shell\bitmaps\sp_levels`,
`ui::campaign_card`) read the same way; the campaign picker's pictures are
bitmaps in `mainmenu.map` (content::campaign_list), which the map card does not serve.

The Halo 1 map is a `.map` file or a disc and a map name; the base comes
from the Halo 2 disc (`--disc`, else `$H2_DISC`, else `disc/halo2.iso`).
It prints the structure (clusters, vertices, triangles, materials), the
placements and what wasn't placed, then writes the package
`DIR/h1_<map>/`: `manifest.toml` and the variant's map at
`<sandbox>/h1_<map>.map` (`h2/h1_beavercreek.map`). A second run with the
other sandbox adds its variant to the same manifest. `--dry-run` does
everything but write and prints what it would write.

**Manifest.** The standalone export uses one `[[variant]]` per sandbox
with a relative file name, hash and origin. The automatic import keeps
that metadata under `<data>/tags/halo1-mp/h1_<map>/` and stores each
sandbox's map as a separate level. The file names shown here describe
the standalone export.

```toml
[package]
format = 1
name = "Battle Creek"

[base]
map = "h1_beavercreek"
derived = true

[list]
base = "beavercreek"

[[variant]]
file = "h2/h1_beavercreek.map"
sha256 = "<the map's SHA-256>"
origin = { game = "halo1", map = "beavercreek", sandbox = "h2", source_sha256 = "<the Halo 1 map file's SHA-256>" }

[[variant]]
file = "h1/h1_beavercreek.map"
sha256 = "..."
origin = { game = "halo1", map = "beavercreek", sandbox = "h1", source_sha256 = "..." }
```

`content::Manifest` holds them (`variants`, `content::Variant`,
`content::Origin`; `sha256` is ours beyond the editor's form, which
ignores it). A variant package is derived and has no `[[patch]]`;
sandboxes are `h1` or `h2`, one variant each. `Manifest::select_variant`
gives the plain derived manifest of one variant (its file as the one
layer), so each variant has its own `MapId` from its bytes, and `origin`
is in no `MapId`. The resolver (`content::Library`) loads the default
variant: `h1` if the package has it, else the first. Retail-style and
other manifests parse and resolve unchanged (the existing content
tests).

**The setting (Built):** `multiplayer.halo1_sandbox` in
`boot::setting_defs` (`HALO1_SANDBOX`: `h1` default or `h2`, `Live`,
`H2_HALO1_SANDBOX`). `boot::new_maps::install` reads it at boot and
writes that variant of each converted map into its content directory
(`new_maps::tests::a_converted_map_installs_the_sandbox_the_setting_names`);
a change after boot reaches `boot::h1_variants` through the setting's
change callback (`h1_options::install_switches`) and counts from the
map's next load (below). It is not on the Settings or Video screens: it
is chosen on the Halo 1 options screen ("Halo 1 options" below).
`h2 settings set multiplayer.halo1_sandbox h2` and `settings.toml` set it
too.

## Halo 1 options (**Built**, tests only: `boot::h1_options`, `boot::h1_options_row`)

The per-game Halo 1 or Halo 2 choices (`h1_options::OptionRow`: a
two-choice setting, its label and what each choice means). On the
multiplayer page the rows are `setting:` rows of its doc,
`content/ui/screens/h1-options.yaml`; on the campaign's Skulls page they
are the skulls page's rows. Adding an option is a setting in
`setting_defs` (choices 1 Halo 1, 2 Halo 2), an `OptionRow`, a row of
the doc for a multiplayer option, and a switch in `h1_options::switch`.

| Row | Setting | Where | Switch |
|---|---|---|---|
| Sandbox | `multiplayer.halo1_sandbox` (`H2_HALO1_SANDBOX`) | multiplayer | `h1_variants::set_mp_sandbox` |
| Assets | `campaign.halo1_assets` (`H2_HALO1_ASSETS`) | campaign | `h1_variants::set_campaign_assets` |
| HUD | `halo1.hud` (`H2_HALO1_HUD`) | both | `hud::set_choice` |

- **Multiplayer:** the pregame Game Setup dialog (squad settings, screen
  23; list ctor `0x2bb4d2`, row text `0x2bb74f`, row chosen `0x2bb801`)
  gets a "Halo 1 Options" row under Quick Options, above the "Switch
  To:" rows (Change Map 0, Change Rules 1, Quick Options 4, Switch To 6
  to 8, from the row text table `0x2bb74f`), when the lobby's map
  (`0x51098c`, set by `0x19a6f2`) is a converted map: its content
  package's `h2recomp-map.toml` names an `h1_` map
  (`h1_options_row::converted_map_name`). Only custom lobbies have Change
  Map (action 0), so only they get the row. Choosing it opens the
  options screen (`h1-options`, the shared settings dialog,
  `h1_options::page`) on the dialog's own window and slot, the way the
  skulls row opened its page. The screen is the lobby's Quick Options
  inside and out (docs/game/ui-builder.md "The settings dialog"): its
  panel, its header ("HALO 1 OPTIONS"), its `setting_lists` list with
  each row's name left and its choice alone in the value column
  ("Sandbox  Halo 1"), and the legend in its footer. Its top is the same
  at every focus: the helmet in Quick Options' game type picture, "This
  Halo 1 map", and the Game Setup row's help ("Play this Halo 1
  map with Halo 1's weapons and HUD, or Halo 2's."). The
  focused row's help is under the rows. Left or right changes a row; A
  opens the value picker, which explains the option and both choices
  (the row's `explain`); B backs out.
- **Campaign:** a Halo 1 level's Skulls page (after the difficulty,
  docs/game/skulls.md "The page") lists Assets and HUD under Start
  Level, above the skulls, and opens even with no skull collected.
  A opens the same value picker for either option; left and right do
  nothing there.
- Each choice is saved to `settings.toml` at once and applies from the
  next game or level load; an environment variable locks it ("locked").
- **HUD:** `boot::hud` is the switch only: each game start picks
  `HudStyle::Halo1` on a Halo 1 map (an `h1_` map or a Halo 1 campaign
  level) with Halo 1's chosen, else Halo 2's, and logs
  `hud: <map>: Halo 1 HUD`. `boot::hud_swap` draws it: at game
  activation it points the converted owners' HUD references at Halo 1's
  or Halo 2's `nhdt` by the same rule (docs/halo1/hud.md).

**Serving the chosen sandbox (Built, tests only).** The choice is
`boot::h1_variants` (`set_mp_sandbox`, set from the setting at boot and
by the pregame screen's row). 1.0 opens a new map from its content
package, `T:\$c\<offer>\h1_<map>.map` (map-pack.md "Loading"), which
`boot::new_maps` registers that path and both store entries in
`vfs::ContentVariants`. `vfs::VariantFiles` serves the selected sandbox
from the store when the file opens. A missing sandbox produces an error
that names its level.

- A file open keeps its variant until its last handle closes
  (`opened`, `closed`).
- `map_changed` is true for a closed map last opened in the other
  variant. `boot::map_slots` reads content package slots too
  (`t:\$c\<offer>\<map>.map`, `map_slots::slot_file`), so the game's
  cached copy of the old variant (slot 3) is dropped once that slot isn't
  the current map's, and the next load copies the chosen one.
- `health::set_halo1_sandbox` follows the switch, so the health model and
  the precision hook (`boot::precision`) are the chosen sandbox's at the
  next game start.
- `map_id` of the path is the served variant's `MapId`. The map probe the
  Live advertisement and telemetry use asks for `d:\maps\<map>.map`, which
  a new map has none of, so they send no `MapId` for a converted map.
  A guest loads its own setting's
  variant: the host's choice is not carried to guests (not solved).
- The other variant's file is served as the package has it; its hash is
  checked at import, not again when the choice switches to it.

Tests: `vfs` `variants::tests::the_served_variant_follows_the_choice_at_the_next_open`,
`boot` `new_maps::tests::a_converted_map_is_served_in_the_sandbox_chosen_per_game`
(the install's table through a whole `Vfs`),
`map_slots::tests::a_content_package_slot_names_its_file_on_the_data_partition`,
`h1_variants::tests`.

**One list ID per map (Built).** `content::Package::list_key` is
`Manifest::variants_key` for a package with variants (from its origin's
game and map), else its `MapId`; `boot::new_maps` derives list IDs from
it, so a converted map keeps its list ID and content folder whichever
sandbox is installed (`new_maps::tests::a_converted_map_installs_the_sandbox_the_setting_names`).
