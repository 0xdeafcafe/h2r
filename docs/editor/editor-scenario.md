# The editor for developers: scenario and viewport

Status: Built.
Switch: none.

The Scenario part of the editor: the Maps list, packages, Halo 1 maps, the Map Pack and what the viewport shows. The index is [editor.md](editor.md).

## Scenario

Opening a map's scenario in the **Browser** (or picking a map on the grid
shown while no map is open) opens it, unless the open map has unsaved edits
(the guard asks). **File > Export map...** asks for the path of a whole
`.map` export. The base map stays untouched. **Save** writes the edit
target's package and **Open package...** opens a map package in
`<data>/maps` (scenario-editor.md §5, [maps.md](../game/maps.md) §8).

### Packages and Halo 1 in the Maps list

The Browser has no list of maps: a map is its scenario tag, and a tag
opens on the map that holds it ([studio-redesign.md](studio-redesign.md),
"The browser"). The map grid shown while no map is open still groups the
source's maps and shows each map's card when available. The sections
below describe what each kind of map is; the Browser's Packages root
lists `<data>/packages/`, and the `<data>/maps` packages and the Map
Pack have no list of their own until the package slices land.

- **Map Pack**: the nine multiplayer maps, sorted by display name and
  pictured as in the game's Select Map ("The Map Pack", below). They
  open like the disc's maps. A package that edits one of them opens the
  map first, then applies the package over it. **Save as
  package...** writes a package with the pack's map as its base
  ([maps.md](../game/maps.md) section 4).

- **Packages**: edits of disc maps and new maps, by `[package] name`.
  An edit opens its disc map, then the package over it
  (`Session::open_package`, through `content::Library`); it needs a source
  with that map. A new map (`[base] derived = true`) opens from its
  resolved bytes (`Session::open_derived`), with the open disc's
  `shared.map` region as the place the shared caches come from. Saving
  either replaces that package.
- **Halo 1 sandbox variants**: maps converted by the Halo 1 import
  ([multiplayer.md](../halo1/multiplayer.md)), sorted by display name
  with each map's H1 sandbox followed by its H2 sandbox. The **Halo 1**
  section shows the open Halo 1 source's playable maps read-only.

**What marks a converted map** is decided in one function,
`editor_core::packages::halo1_origins`: an `origin` table with
`game = "halo1"` (and `map`, `sandbox = "h1" | "h2"`, `source_sha256`),
either at the top of the manifest or in each entry of a top-level array
of tables (one per variant, each with its own `file`). That covers what
`content::Manifest` writes for converted maps: `[[variant]]` entries
with `file`, `sha256` and an `[variant.origin]` sub-table
(`packages::tests::the_converters_serialized_manifest_lists_both_variants`
feeds it `Manifest::to_toml`'s output). The manifest is read there as
plain TOML so that packages from builds with other fields still list; a
variant's own `file` is read directly, anything else through the
resolver, and a derived package the resolver refuses is read only when
its single layer is the whole map.

A converted variant opens as a new map and saves as a **new** package
(`<map>-edit`): the import's package holds both variants and their
origins, and the editor does not rewrite it. A derived package saved by
the editor carries the unedited map as its first layer and the edit as
the second (`package::save_package` with `derived`), so the resolver
rebuilds it without the import.

**Halo 1 maps, read-only.** `editor_core::halo1` reads a map through
`h1map`: every structure BSP's rendered triangles (each lightmap
material's render surfaces over its own vertices) and the multiplayer
placements (starting locations, netgame flags, netgame equipment,
vehicles), each with its fields as text. The studio shows them as a
`scenario_edit::Reference`: the session's world and markers with no
scenario, so the viewport, picking and framing are the same and nothing
can be edited.

The BSP is drawn textured and lightmapped through the same scene and
renderer as Halo 2's (`editor_core::halo1_art`, then
`scenario_edit::scene::reference_scene`):

- **Base maps.** Each lightmap material's shader names its base map at
  a place that depends on its group, past the 0x28-byte `shdr` header:
  `senv` +0x88 (flags +0x28, bit 0 alpha-tested: cut out), `soso`
  +0xa4, `swat` and `smet` +0x4c, `sgla` +0xac (else +0x158), `spla`
  +0xd4, and the first map of a maps block at +0x54 for `schi` (0xdc
  each, map at +0x6c) and `sotr`, Xbox's generic transparent shader
  (0x64 each, map at +0x1c); `scex` is read as `schi` but no
  multiplayer map has one. Every tag of those groups on five maps
  resolves a `bitm` there. The base map is the bitmap's first image,
  decoded by `h1bitmaps` at the first mip no larger than 512. Every
  group but `senv` and `soso` is drawn see-through.
- **Lightmaps.** `h1bitmaps::lightmap_pages` gives each lightmap's page
  (R5G6B5, no palette); a material with lightmap vertices draws with
  that page at its page-relative coordinates, as a Halo 2 lightmap is
  (base map × lightmap × 2 in gamma space), the page halved first
  (`halo1_art::to_halo2_light`): a Halo 1 page is the base map's
  multiplier itself, so the ×2 alone drew Halo 1's full light doubled
  (`h1bitmaps::atlas::to_halo2_light`, the converter's pages). The rest (sky, unlit
  surfaces) keep the sun.
- **Winding.** Halo 1's surfaces turn the other way from the
  renderer's front faces; each triangle is turned to face its vertex
  normals.
- `editor_core::halo1_art::tests::disc_multiplayer_maps_are_textured_and_lightmapped`
  (ignored; needs the Halo 1 disc) checks all 13 multiplayer maps: at
  least 90 % of each map's shaders textured (all but four shaders over
  the 13: three glass shaders and a monitor glow name no base map) and
  some parts lit.

Not drawn yet: the sky and scenery models (a Halo 1 map's sky is a
`mod2` model behind its `sky ` tag; `editor_core::halo1` has no reader
for either yet, so the gradient stands in), detail and bump maps, and
`senv`'s blended types (the base map's alpha blends two detail maps,
which shows as dark patches on Sidewinder's snow, for example).

**Convert to Halo 2** (`app/convert.rs`). The inspector of a Halo 1 map
offers the sandbox (Halo 2, or Halo 1, whose `Error::Halo1Sandbox` is
shown as it comes) and a button that builds a new map, `h1_<map>`, from
scratch (`convert::from_scratch`: `prepare_halo2` or `prepare_halo1`,
`structure_mopp`, then `build_fresh`). The new map still takes Halo 2's
shaders, objects and globals from a retail multiplayer map, read
silently from the open Halo 2 source (`retail_sources`: `known`'s
default for the map, else any multiplayer map the source has); the user
never picks one, and without a Halo 2 source the button is disabled
with the reason. A thread reads those bytes, builds the map, and writes
it as `h2 h1 convert` does: `<data>/maps/h1_<map>/<sandbox>/h1_<map>.map`
and the `Converted` manifest, keeping the other sandbox's `[[variant]]`
from a manifest already there. When the Halo 1 source has its `ui` map,
the map's card (its picture, name and description, `h1convert::ui`)
goes in as `preview.png` and the package's name, description and image;
the converter's notes (a structure that fell back) are logged. Each
frame `poll_convert`
picks up the result: the Maps list is re-read and the new variant opened
(`OpenPackageEntry`, guarded as usual), or the error is logged and shown
under the button in plain words (`explain`: `TooManySurfaces(n)` is
"This level has n surfaces; Halo 2 holds at most 65,536", any other error
its own text). The converter is a function pointer
(`App::set_converter`) so the tests run the whole flow with a stub;
`disc_maps_convert_open_and_find_their_source` (ignored) runs the real
one on the owner's discs, end to end.

**The Halo 1 overlay.** Opening a converted variant looks for its
`[origin] map` in the open Halo 1 source, then `$H1_MAP_DIR`, and checks
the stored file's SHA-256 against `source_sha256`. When it matches, View >
Halo 1 overlay and **Show > Halo 1 overlay** draw the source BSP and
placement glyphs in translucent orange. Turn on **X-ray** to see the
parts behind the converted geometry at a quarter of their usual opacity. The inspector says where the map came from, and why there is no
overlay when there is none.

### The Maps list's cards

Every card of the map grid is drawn the same way (`ui::map_list::Row`):
a map with a card shows a thumbnail of its picture (18 pt tall) beside
the game's name for it, and on hover the whole picture (shrunk to 260 pt
wide when larger), the name, the description wrapped at 260 pt, and a
dim line with the file and where it comes from
(`03a_oldmombasa.map, on the Halo 2 disc`). A map with no card keeps its
file name and icon, and its tooltip says what it is: `mainmenu`,
`shared` and `single_player_shared` are not levels
(`map_cards::halo2_note`, `halo1_note`). The pictures go to the GPU once
each, in one cache keyed by the row
(`MapListUi::texture`: `map-<name>`, `h1-<name>`), and are replaced when a card's picture changes.

`editor_core::map_groups::MapGroup` classifies source maps using the
cache type and reserved file names. `MapSource::map_groups` and
`H1Source::map_groups` read the headers when a source opens. A map
without a recognised type uses its menu card's kind when available and
falls under **Other maps** otherwise. `app::map_cards::MapEntry` supplies
the display name and case-insensitive ordering to Maps, Tags, HUD and
`source.maps`. The **Shared and menu** rows use a layers icon and the
subtitle “Shared resources and menu screens”. The Playlists picker uses
the multiplayer catalogue with the same name sort.

Where each group's cards come from (`app::map_cards`; nothing of them is
in the repository, all are read from the user's files):

| Group | Name and description | Picture |
|---|---|---|
| Halo 2 (the open disc or folder) | `mainmenu.map`'s level lists in the globals tag (`tags::ui_levels`): the multiplayer list Select Map shows (`matg +0x178` element 0, block `+0x10`, `0xc64`-byte slots: name `+0xc`, description `+0x24c`) and the campaign levels Select Level shows (the start list `matg +0x170` joined with the level select's entries, block `+0x8`, `0xb50` bytes: name `+0x10`, description `+0x250`); keyed by the stem of each entry's scenario path, English | the bitmap tag each entry references (multiplayer `+0x4`, campaign `+0x8`), its first image decoded with `assets::decode_image` from `mainmenu.map` and `shared.map`, else read as a linear `ARGB8888` image (`rgba_bitmap::Template::rgba`) |
| Map Pack | the installed `patch.lvl` records (below) | the record's preview |
| Packages and converted Halo 1 maps | the package's name; `[package] description` | `[package] image`, a PNG or JPEG inside the package (`content::manifest::relative`); a conversion writes Halo 1's card there (`preview.png`) |
| Halo 1 (an open Halo 1 disc or folder) | `ui.map`'s map and level lists (`h1convert::ui::mp_card`, `campaign_card`) | the same lists' bitmaps, the picture's corner of each |

`tags::ui_levels` is the one reader of the main menu's lists: the boot's
map select (`map_select::map_of_slot`) and the Live catalogue
(`cli::live_catalog::levels`, `campaign_levels`) read through it too.

The cards are read off the UI thread (`map_cards::CardSet`, one thread
a group): the Halo 2 source's when it opens and has a `mainmenu.map`, the
Halo 1 source's when it opens and has `ui.map`, the packages' pictures
whenever the packages are listed and one names an image. Until a reader
finishes its group shows file names (a Halo 2 row's tooltip says the
lists are being read); each frame takes the finished ones
(`App::poll_cards`), and a failed reader is logged once. Screenshots
(`--screenshot`) wait for them (`App::wait_cards`).

**What each group drew** is kept for scripts and tests in
`MapListUi::shown` (`Studio::map_list`): for each group, the hint, or a
row a map with its label and whether its picture was shown.

Tests (`app::map_cards_tests`, an invented main menu map built with
`map::build`: a campaign level and a multiplayer map with pictures of
their own, a free slot; the fixture map under invented level names; a
package with a PNG):
- `the_main_menu_s_lists_name_and_picture_the_halo_2_maps`: opening the
  folder reads both cards on the side, with names, descriptions, kinds
  and the menu's exact pixels; any case of the stem finds them; the
  shared cache and the menu have none;
- `a_source_without_a_main_menu_has_no_cards`: no reader starts;
- `cards_take_each_picture_once_and_the_first_card_of_a_map`: each
  picture decoded once; a stem in both lists keeps the campaign card;
  entries with no name or path are left out; a failed or misshapen
  picture leaves the card without one;
- `a_package_s_image_is_its_picture`: the image read from inside the
  package, the manifest's description and image listed; paths out of the
  package are not followed.

`tags` `ui_levels::tests`: both lists from invented tag memory (English
only, free slots and unnamed levels left out, the join on campaign and
map id), clean failures, and the lists of a built main menu map naming
their bitmaps' datums.

Headless (`headless::tests::the_maps_list_names_and_pictures_every_group`):
the folder's level and map by the menu's names with their thumbnails,
the shared cache and the menu by file name, the package with its
picture, the Map Pack's hint; a click on a named map opens it. The Halo 1
cards have no synthetic test (no invented `ui.map`); they are
`h1convert::ui`'s cards, which the conversion already uses.

### The Map Pack

The **Map Pack** group (`app::map_pack`, drawn by `ui::map_pack` with
`ui::map_list`'s rows, under the source's maps) has two places to take
the maps from:

- the map pack disc or folder opened beside the Halo 2 disc
  (`editor_core::mappack`, above): when it is open, the group lists its
  maps;
- otherwise the copy the game installed under the data root (`h2
  import-mappack` or `h2 play`'s automatic install,
  [map-pack.md](../game/map-pack.md) §4, §5): one content package a map at
  `<data>/disk/Partition1/TDATA/4D530064/$c/<offer ID>/` (`T:\$c` to the
  game, `vfs::Config::title_data_dir` + `vfs::CONTENT_DIR`), each holding
  `<map>.map` and a version 2 `patch.lvl`. So the maps are there without
  the map pack ISO.

The group uses the installed records for display names, descriptions
and pictures (the `A8R8G8B8` preview turned to RGBA).
`boot::map_select::map_of_package` reads each record and `category_of`
keeps the Map Pack's offers (`0x0101..=0x03ff`). A disc map without an
installed record uses its file name. `App::map_pack_entries` supplies
the rows in display-name order. A record whose map file is missing
is left out and logged once. The group is
rebuilt with the packages (data root change, save, `refresh_packages`)
and when a map pack opens. With neither, it shows one line on how to
install the pack.

`Action::OpenMapPack(<map>)` (the map name, or an installed package's
offer directory) opens a pack map through `select_map`, as `SelectMap`
does: its bytes from the disc (`MapPack::location`) or the installed file
(`App::pack_location`), the shared caches from the open Halo 2 source
(`MapSource::shared`, see above). The pack ships no shared map of its
own. Re-picking the open map keeps its edits; another one asks first.
Every tool reads the session's document, so the Tags tool, the viewport
and later tools see a pack map as they see a disc map. The UI Screens
tool draws the disc's screens and has nothing per map.

Every tag a map uses has its metadata in that map
([map-builder.md](../formats/map-builder.md)); shared maps are reached
only through raw pointers, whose top two bits pick the map. The Tags
tool counts the selected tag's raw pointers by map through its layout
(`app::shared_refs::raw_homes`) and shows a line such as `raw data lives
in a shared map: shared.map (3)` when any is outside this map.

Saving never writes the installed files:

- **Save** writes the edit target's package over the pack map (its base,
  by hash), as for a disc map. Its **Packages** tooltip names the pack map.
- **Export map...** writes the patched `.map` (`save::save_as`) anywhere
  but the installed content: `App::save_refusal` refuses a path inside
  `T:\$c` on the host, or any installed pack map file (whatever the data
  root).

Tests (`app::map_pack_tests`, a synthetic install of the fixture map
under invented names and records):
- the listing and its order, pictures, missing files, the empty group;
- the map pack disc's maps named and ordered from the install, and
  opened from the disc;
- opening in the Scenario and Tags tools, the unsaved-edits guard;
- Save as: byte-exact when unedited; with one edit, only the edited
  field and the checksum differ, the same as the same edit on the map
  opened loose; refused inside `$c`;
- a package based on the pack map that reopens with the edit and
  resolves to the pack map plus the edit.

Headless (`headless::tests::the_map_pack_group_lists_and_opens_a_pack_map`):
the hint, then the row with its picture on screen, clicked open. With
the owner's install:

```sh
H2_MAPPACK_DATA=<data root> cargo test -p studio --release installed_map_pack -- --ignored
```

lists the nine maps, opens the first and last with every section loaded
from the disc's shared maps, finds tags with raw data in `shared.map`,
and saves one edit.

### New maps

**File > New map...** asks for a name and a kind and builds a map that
boots and plays at once: a derived package (`[base] derived = true`) in
`<data>/maps/<name>/`. The agent API's `map.new` runs the same build
([agent-api.md](agent-api.md), "New maps"). The build is the
`starter_map` crate. The window runs it on a thread
(`app/new_map.rs`) and opens the map when it is done.

The template is data, not code: `content/new-map/courtyard.yaml`.

| Step | What it makes | Code |
|---|---|---|
| Floor | a 32 m lit floor: render mesh, collision from the collision compiler with its MOPP, the lightmap from the sun and sky fill, the sky | `content_kit::stage::map::build_over` |
| Pieces | 4 walls, a ramp, a platform, 3 cover blocks, 2 pillars: block-out pieces, each with its `bloc`, render model, `coll` and `phmo`, placed as crates | `forge_build::blockout::install` ([forge-blockout.md](../game/forge-blockout.md)) |
| Placements | spawns, netgame flags, equipment, through a scenario-edit recipe, each checked as the editor checks it | `starter_map::build::place` |
| Check | the scenario read back starts every mode the template claims | `starter_map::coverage` |
| Package | the built map as layer 1, the placements as layer 2, `report.json` | `starter_map::write` |

| Kind | Base map | Placements |
|---|---|---|
| Multiplayer | `coagulation` | 16 respawns (8 a team); CTF flag and return, assault bomb and return for each team; an oddball; a 4-corner hill; 4 territories; a battle rifle and a Warthog. Every retail mode starts |
| Campaign | `05a_deltaapproach` | the player start; one squad of two Elites in a zone of 6 firing positions under one order. The base's AI, scripts and object names are cleared first |

The pieces are the Forge block-out set, so the map is edited later with
the same pieces. A piece that does not install, a tag the template names
that the base lacks, or a mode it does not cover stops the build and
names the gap.

**Where a tag-workspace project plugs in.** The base's tags come through
`tags::repo::TagSource`. A disc map is `starter_map::build` over a
`MapSource`. A project is a `TagSource` and a `content_kit::stage::map::Base`
passed to `starter_map::build::build_over`, and adds its own tags to the
closure through `content_kit::stage::map::Over::roots`.

Not built yet:

| Gap | Effect |
|---|---|
| One cluster, no portals | the whole yard is one visibility cell |
| No cubemap | the floor shader samples none |
| No pathfinding | the campaign squad fights from where it stands |
| One-sided floor | the floor is not seen from below |

### What the viewport shows, and what it does not yet

`scenario_edit::scene` builds the map's art when it opens, from your own
disc at run time (about 30 to 150 ms for a multiplayer map in a release
build), and keeps the last four maps' scenes so reopening one is instant.

- **The structure BSP, textured.** Clusters and instanced geometry
  (`geometry::world_parts`) carry their vertex UVs and normals; each
  part's shader (`sbsp +0xa4`) gives its base map, the first bitmap of the
  compiled postprocess whose name reads as diffuse (the Live site's rule,
  docs/live/live-art.md). Bitmaps are decoded once by
  `assets::decode_image` (`scene::resource_files` gives it the map's own
  file for local raw pointers: a map held in memory, a package or a
  converted map, is read from its bytes, never from the disc map its
  source names; a converted map's source names `shared`, whose bytes
  drew as noise, `app::tests::disc_converted_textures_decode_as_their_bases`),
  shrunk to 512 a side, given a mip chain and
  lifted when very dark (the game draws them under detail maps and
  lightmaps) where no lightmap lights them.
- **Lightmaps.** A BSP is lit by its lightmap's **first** group when that
  group carries the BSP's checksum and a bitmap, as the engine and the
  native renderer pick it (`geometry::select_group`, after
  `native_render::load::load_world`). Each cluster's and instance's
  lightmap page (the group's render info: an image of the lightmap
  bitmap and a palette) is decoded from `P8` through the group's
  palette, and its parts draw with their `1f` lightmap coordinates: base
  map × lightmap × 2 in gamma space, plus a faint fill light so no texel
  is pure black. A placed instance's coordinates are its own, from its
  run in the group's buckets (`native_render::load::{bucket_streams,
  instance_run, bucket_slice}`), not its definition's, which every
  placement shares; the definition's only when the instance has none.
  Each BSP's lighting is reported (`geometry::BspLighting`: lightmapped,
  no lightmap, an empty group, another BSP's group, or a lightmap that
  does not decode), in the status line when a BSP is not lightmapped and
  in `view.stats`.
- **Unlit BSPs.** mainmenu's one BSP was never lightmapped: its
  lightmap's one group is empty (checksum 0, no bitmap, no palettes), so
  its log line reads `0 lightmap pages (mainmenu: empty lightmap group,
  lit by its sky's sun)`. The game draws such a BSP with each shader's
  implementation for a BSP without a lightmap group
  (`native_render::look::UNLIT_BSP_IMPLEMENTATION`, lit from engine
  vectors). The viewport lights it, and every part without a page, with
  the map's sun: one sun and the sky, with dark base maps lifted.
- **The map's sun and fog.** The first of the scenario's skies that
  reads (`h1_assets::sky::h2::Sky`, `SceneBuilder::atmosphere`) gives
  them. Its first light with parameters is the sun: its direction from
  the light's yaw and pitch (`scene::sun_direction`, turned above the
  horizon) and its hue (`scene::sun_tint`, the colour scaled so its
  brightest channel is 1; the viewport keeps its own brightness). Its
  first `fog_a` element is the fog, checked as the engine checks it
  (`scene::fog_of`, after `0x001301c0`: no density is no fog; colour
  clamped to 0..1, density to 1, the opaque distance at least a unit past
  the start), and drawn over the level and its objects, not the sky:
  none before its start, rising evenly to its density at its opaque
  distance (`scene::fog_amount`; the shader that consumes the engine's
  fog state is not traced, so the ramp is the viewport's). A map whose
  sky has neither keeps the editor's sun and no fog.
- **Objects.** A placed object is lit by the lightmap of the floor it
  stands on: the highest lightmapped solid triangle under its origin (at
  most half a unit above it, 64 below; `scene::LightFloor`, a grid of
  the world's lightmapped triangles by column), sampled there as the
  floor itself is (`Scene::light_at`, bilinear, in linear light) and
  applied as the placement's tint (`render::object_light`). An object
  with no lightmapped floor under it keeps the sun alone.
- **Sky.** The scenario's sky (its skies block, `scnr +0x8`: the first
  `sky ` whose render model builds, `scene::scenario_skies` and
  `Scene::sky`) is drawn at infinity in its own pass before the level:
  the view's translation is dropped (`Camera::sky_view_proj`, an
  infinite far plane), so it turns with the camera and never comes
  nearer, and the level's pass clears depth over it. Its parts are
  textured as object models are. Solid parts are drawn first; then
  the see-through ones, in the order of their parts, each
  alpha-blended or, for an `add` template ("Blends", below), added:
  Halo 1's Derelict draws an added space dome, an added galaxy whose
  picture is black around it, and two alpha-blended planets. A
  see-through layer with a coverage mask takes it (`Look::mask`, below):
  Coagulation's sky is a blue dome (`coag_stars`), then clouds that are
  a cloud tile with no alpha (`Y8`) times `coag_cloudcover_mask`, and a
  sun volume that adds a grey tile times its mask; without the masks the
  clouds were an opaque white dome over the blue. Behind
  the model is the sky's clear colour (`scene::sky_clear`: `sky +0xa0`
  when bit 5 of `+0x10` is set, else black), not the gradient, since
  an added layer adds onto it. With no sky model, the gradient from
  the zenith to a pale horizon. `tests::skies_are_read_from_the_scenario_root`,
  `tests::the_sky_turns_with_the_camera_and_never_comes_nearer`,
  `scene::tests::see_through_batches_keep_their_part_order`,
  `scene::tests::the_sky_clears_to_its_colour_only_when_flagged`,
  `tests::the_sky_model_is_drawn_behind_the_level_wherever_the_eye_is`,
  `tests::added_sky_layers_add_over_the_dome_and_black_adds_nothing`
  and `tests::a_cloud_layer_is_covered_by_its_mask` (GPU) hold it, and
  `tests::disc_coagulation_sky_layers_take_their_masks` (ignored; needs
  the disc) holds Coagulation's. Solid surfaces are
  one-sided, as in the game, so the camera sees into a level through the
  backs of the cliffs and walls around it.
- **Blends.** Templates in a `transparent` or `water` folder, plain
  `overlay` decals and part kind 4 are drawn see-through after everything
  else and are neither framed nor picked; `alpha_test` templates are cut
  out. A see-through shader composites as its material's first blended
  pass says (`scene::material_blend`: the pass's render states through
  the native renderer's decoder, `native_render::pass_state::pass_state`,
  read by `scene::blend_of_state`), else as its template's name says
  (`scene::template_blend`: the word `add` or `additive` adds, `mult` multiplies,
  `alpha` blends by the picture's alpha): **additive** (lights, light
  shafts, glows; without a picture it adds nothing), **alpha** (clouds,
  glass), **multiply**, **double** (twice the picture times what is
  behind it, as the game's gamma-space modulation: mid-grey changes
  nothing), or **translucent** at a fixed opacity (water, and templates
  neither says). A see-through shader whose bitmaps have no diffuse name
  draws its first own bitmap. mainmenu's light shafts
  (`one_add_illum_detail`, about a third of its BSP's materials) add
  light; drawn at a fixed opacity they would lay grey sheets over its
  city. The fog fades an added surface to nothing, a multiplied one to
  white. The sky pass adds an additive layer with its own pipeline
  (blend one, one), and a mesh's see-through batches keep the order of
  their parts (`Blend::draw_order`). An alpha-blended or added layer
  whose pass samples a bitmap named `mask` along with its base map
  (`scene::mask_plan`, `scene::is_coverage_mask`; else the first such
  bitmap of the shader at scale 1) multiplies it in, colour and alpha, at
  that stage's scale, as the cloud passes do (`two_alpha_clouds`: mask
  times tile in RGB and alpha); `view.stats` names it as the material's
  `mask`. `scene::tests::a_see_through_layer_takes_its_coverage_mask`
  holds the choice.
- **Self-illumination.** The `opaque\illum` template's picture is its
  own light (mainmenu's lit windows): drawn as it is, unshaded
  (`scene::template_emits`, `Look::emissive`).
- **Object models.** Every palette entry (and netgame equipment's item
  collection, by its first entry) resolves object → `hlmt` → `mode`
  through `tags::object`; each region's first permutation at its best LOD
  is drawn at the placement's position, rotation and scale, instanced per
  model. Clicking a model picks it (ray against its triangles); the
  selection is tinted and keeps its marker.
- **Layers.** Open **Show** above the viewport, the one place that toggles,
  to switch the structure, sky, object models, fog, markers,
  individual placement blocks, collision, navigation, Halo 1 reference
  and live objects. Each row shows its colour and count. Art, Gameplay,
  Collision, Navigation and Everything choose a set of layers in one
  click. Collision shows the level structure for context, the kill
  volumes and the collision layers. Navigation shows the structure, the
  invisible walls (Collision only) and the three navigation layers. The
  viewport and `view.snapshot` share these settings.
- **Lighting.** The level is drawn by the native renderer
  (`scenario_edit::native_view`), one world per BSP; the toolbar's
  **Lighting** menu picks Game (base map times lightmap), Flat (no
  lightmap), Lightmap only (the lightmap on white) or Collision (Flat at a
  third, for the collision layers). The level's base look is a
  translated pass (`native_render::base_look`), so its depth is the one a
  material's pass computes for the same vertex. When it cannot be (a
  program that does not encode or compile, or a part that does not
  bind) the BSP draws with the renderer's `world.wgsl`, its world layers
  are off, and `view.stats` names the cause (`base_look`,
  `NativeLoad::base_fallback`). The placed objects draw through the
  native renderer too (`native_view::NativeObjects`, `GpuModels`), each
  in its rest pose with its base maps in the same mode, and a selected
  one mixed towards the selection orange as the scene's models are
  (`native_view::SELECTED_HIGHLIGHT`). Each tag's model is read once:
  every palette entry's and placement's when the map opens, and a tag
  an item reference is first set to when the edit lands
  (`NativeModels::add`, after the placement's collision); an object whose
  model the native loader refuses (`native_render::model_load`) draws
  the scene's model, and `view.stats` names it. Live objects, markers and
  overlays draw over the level with its depth (view depth over the far
  plane, floored to D24's step as the game's W-buffer stores it;
  `native_view::DEPTH_RULE`), so a mark on a surface passes the depth
  test against it. A map whose BSPs the native loader does not read (a Halo 1
  reference) draws its textured scene instead.
- **Layer 3.** In the Game mode each BSP draws its materials' own
  translated layer-3 passes in place of the base look
  (`layers_load::load_layer3`, compiled as the game's world glue
  compiles them, with the camera's view constants; drawn in the base
  look's set, `native_view::base_parts`). A material whose layer 3 is
  refused, and a part its pass does not take, draw the base look, and
  `view.stats` counts each by cause (`by_layer`, layer 3: `refused`,
  `parts_refused`). A part only layer 3 draws (no base map) counts in
  the BSP's `draws` and is picked in the Game mode only: what is
  picked and counted is what the mode shows
  (`NativeLoad::triangles`, `NativeLoad::stats`). A pass that leaves `CULLMODE` unset
  culls as the D3D device's default (CCW;
  `layers_load::device_default_cull`). Flat, Lightmap only and Collision
  draw the base look alone.
- **World layers.** In the Game mode each BSP also draws the world
  layers 1, 5, 7, 8 and 20 with the materials' own translated passes
  (`native_view::NativeLayers`), in the game's order: layer 1 before the
  base look, then 5, 8, 20 and 7 over it. Layer 1 is refused whole
  (`native_view::LAYER_1_UNCHECKED`): the game draws it in a stencil mode
  the editor's pass does not hold, and that has not been checked against
  the game's picture. They load as the game loads
  them (`native_render::layers_load`) and each material is planned with
  no game (`layers_load::plan_without_game`: render time, no fog, no
  dynamic light). A material draws only when nothing it reads is left
  out: a pass that reads a view constant the camera does not give
  (`program::CAMERA_GIVEN`), an input the game-free run state does not
  give, a palettized stage bitmap (no stage palettes), a missing
  animation input or its own fog is refused, and `view.stats` counts it by
  cause. The BSP and its layer 3 (`layers_load::plan_layer3`) sample
  animated pixel and vertex constants at time zero. The Shader tab takes
  its sample at the preview clock. The other modes draw the base look alone.
  Clicking skips hidden placement categories and hidden structure.
- **Gameplay layers.** Spawn points have a team-coloured pawn and an
  arrow pointing along the spawn facing. Netgame flags have a team tint
  and a pennant for their type. The pick spheres cover the arrow and
  pennant tips. The label gives the type and identifier, using the raw
  type number for an unknown value. Hill and territory
  markers of the same type and identifier are joined in placement order.
  Two markers form a segment and three or more form a closed outline.
  Netgame equipment shows its model when available and a respawn label.
  The Gameplay switches and their corresponding placement blocks both
  control visibility. The **Placement markers** switch controls glyphs and annotations.
- **Volumes.** Trigger volumes are translucent boxes with wire edges and
  a caption with the name and what names the volume (`· kill`, `· BSP 0
  to 1`), in three layers: Trigger volumes (all of them), Kill volumes
  and BSP switches (the volumes a kill trigger or a BSP switch names), in
  the colour of the most specific one shown (`scenario_edit::volumes`,
  `theme::volume_color`). A click on a box selects it when no placement is
  under the cursor; the Scene tab's Volumes group lists them and brings
  a picked one into view, and the inspector shows the box, its parent and
  what names it. A volume with a parent object is listed and not drawn.
  A selected world-space volume takes the gizmo: move, turn, and scale
  along each axis, each drag one undo step; the inspector edits its
  centre, size and (upright boxes) facing. The Volumes group's menu adds
  a trigger volume or makes a kill volume (a volume and a kill trigger
  naming it, one step) at the camera focus; Delete and the inspector
  remove one, refused while a kill trigger or BSP switch names it, and
  a kill trigger's Remove drops just the kill trigger
  ([scenario-editor.md](scenario-editor.md), "Trigger volumes, kill
  triggers and BSP switches").
- **Collision.** Level collision draws every surface of each structure
  BSP's collision BSP as translucent faces with wire edges; Collision
  only draws, in red, the surfaces no drawn triangle covers: the
  invisible walls (`scenario_edit::collision_view::classify`). Instance
  collision draws instanced geometry's collision, placed, the same way,
  and Collision only covers it too. Object collision draws each placed
  object's `coll` BSPs and `phmo` shapes as wire, each posed by its
  node's rest transform and then the placement's
  (`scenario_edit::object_collision`). Each object tag is read once:
  those placed when the map opens, and any other the moment an edit
  first places it (another palette entry, an item reference). All four
  start hidden. A click on a shown surface selects it when nothing else
  is under the cursor, and the inspector shows its class, plane, raw
  flags, material and ring, read only.
- **Navigation.** Three layers, all hidden until shown
  (`scenario_edit::nav_view`, drawn by `studio::viewport::nav`):
  - **AI pathfinding** draws each structure BSP's own pathfinding data
    (`sbsp+0xc4`, [pathfinding.md](../formats/pathfinding.md)), what
    Halo 2's AI walks on: each sector as a translucent polygon from its
    link ring, tinted by its flags (the same flags, the same shade;
    walls and ceilings dimmer), and each link as a line, a sector's
    boundary brighter than a link two sectors share. Orphan links and
    sectors whose ring does not close are left out and counted
    (`nav.pathfinding`). The 2D BSP nodes are counted, not drawn. Read
    with the map (`LevelPathfinding::read`). Retail multiplayer maps
    carry none: the layer is empty and `nav.pathfinding` gives a note.
    Once **Build AI ground** has built ground for the map, the layer
    draws the built ground's sectors in its place
    (`Session::set_ground_pathfinding`, [game-types-without-carriers.md](../game/game-types-without-carriers.md), "In Studio").
  - **Bot nav mesh** draws the bots' own mesh
    ([bots.md](../game/bots.md), §3.2): each polygon as translucent
    floor, blue in the map and a dim grey-blue out of it
    (`theme::BOT_OUT_OF_MAP`: only a glitch such as a superbounce reaches
    it, bots.md §3.2, "In the map"), and each link as a line coloured by how it is crossed (walk,
    drop, jump, crouch-jump, teleporter, lift, elevator;
    `theme::bot_link_color`), with an arrowhead on a link no link leads
    back along (drops, mostly). A traversal is drawn from its takeoff to
    its landing; any other link from the crossing to the far polygon.
  - **Bot superbounce spots** draws the spots the bots use
    ([bots.md](../game/bots.md), "Superbounce routes"): bounce slots,
    single-crease strips and crouch spots, each a post at its stand with
    its strip or lip, coloured by what the data directory's files say:
    proven (a verified run or a hand-measured spot), failed (verified),
    dead (`.learned`), learned (a `learned` or `partial` line), training
    (an `open` or `blocked` line) or likely (the scan alone). A diamond
    round the stand marks a spot `.ghosts` keeps a run of.

  The bots' data loads on its own thread the first time a bot layer is
  shown for a map, or when `nav.bots` asks (`studio::app::bot_nav`). It
  reads each structure BSP's `<data>/bots/<map>.<bsp>.bots` when its
  stamp is the current nav generator's and this map file's. Otherwise it
  builds the mesh in memory with the bots' own builder
  (`bots::autonav::build_map`, what `h2 bots nav` runs), does not write
  it, and says why in `nav.bots`'s notes. The spots come from
  `<map>.spots` when current (otherwise a scan), the hand-measured ones,
  and `<map>.verified`, `<map>.learned` and `<map>.ghosts` beside them.
  `<data>` is the data root (`--data`, else the one `h2 play` uses).

  A click picks a shown spot or bot link first, then the nearer bot
  polygon or pathfinding sector, ahead of collision surfaces. The
  inspector shows a sector's index, flags, hint and its links (with the
  sector across each), a polygon's links and whether it is in the map, a link's kind, ends and
  whether it is one way, or a spot's kind, source, state, launch, fall,
  learned tries and verified trials, read only.
- **Glyphs and labels.** Other placements without a model use a diamond
  in their block colour. Past 40 world units glyphs and their pick
  spheres grow with distance so the whole map stays readable. Gameplay
  labels are drawn within 40 units of the eye.
- **Framing.** Frame (Home) frames the placements, padded, when there are
  a few, dropping the 4 % at each end of each axis (Lockout keeps a few
  far outside the level): distant scenery and sky geometry no longer
  shrink the level. `tests::disc_lockout_frames_its_level_not_its_outliers`
  (ignored) holds it.

Ascension used to draw only "its ground plane and rocks": its BSP has a
700-unit transparent cloud deck (part kind 4,
`transparent\sky_two_alpha_clouds`) at z = -14.5 under the structure,
drawn opaque and framed, which hid the lower half of the structure and
made the rest a speck. `tests::disc_ascension_frames_its_structure_not_its_cloud_deck`
(ignored; needs the disc) holds the fix.

Coagulation's bases and Midship's hull used to come out black. Their
lightmaps were never black (the pages, palettes and coordinates match
what the engine binds); the window was. The viewport renders into an
sRGB texture, and Dear ImGui's shader treats every texel it samples as
sRGB-encoded, raising it to the power 2.2 before its sRGB framebuffer
encodes it again. Given the texture's sRGB view it sampled linear
values and showed them as encoded ones, so a dim wall at linear 0.05
(sRGB 63) showed as 13. `Viewport::ui_view` hands the UI an `Rgba8Unorm`
view of the same pixels instead. `headless::tests::the_viewport_shows_what_it_rendered`
compares the window's pixels with the viewport's own, and
`headless::tests::disc_coagulation_bases_and_midship_hull_are_lit`
(ignored; needs the disc) checks that under 2 % of either map's
viewport is black (a quarter of Coagulation's was).

Colour: the viewport's colour texture is `Rgba8UnormSrgb`, written
through two views (`render::Shading`). The editor's own shaders (the
sky, the scene's models, glyphs and overlays) upload base maps and
lightmap pages as sRGB textures, shade in linear light and write the
sRGB view, so a picture's colours come through as stored. The native
renderer's passes (the level and the placed models) are the game's
gamma-space combiners on the stored bytes, as the Xbox draws them and
as the game presents them through a plain surface (`gfx::presenter`), so
they write the plain `Rgba8Unorm` view (`UI_FORMAT`), and their clear
colour is the background encoded (`render::encode_srgb`). Through the
sRGB view they were encoded a second time and every colour came out
pale. Glyph, overlay and grey-world colours are display values, as the
Show legend draws them (`studio::theme::layer_color`): the shader
decodes them before the sRGB view encodes them. Lightmapped surfaces are
not lifted; only surfaces the sun lights have dark base maps brightened.
`tests::each_shading_space_writes_its_own_view_of_the_target` holds
the formats and
`native_view::tests::the_viewport_keeps_the_native_worlds_colours_as_stored`
(GPU) the native floor's bytes.

`view.stats` (docs/editor/agent-api.md) reports all of the above for the
open map: lightmap pages and each BSP's lighting, the world's triangles
by blend and how many solid ones a lightmap lights, every material with
its template, blend (and whether its passes said it) and picture, and the
sky's sun and fog. `view.snapshot` renders the same view to a PNG.
`scene::tests` hold the pure parts (blends from names and states, the
sun, the fog check, colour conversion and bilinear samples, the floor
under a point), `tests::the_first_lightmap_group_lights_a_bsp_with_its_checksum`
the group choice, and `api::view::tests` the stats.

Not yet: the shaders' own layer-3 passes (the native renderer translates
each material's layer-3 pass and its unlit-BSP implementation; the
viewport draws a base map, its detail maps and a lightmap there, so the
bump, environment and specular parts of layer 3, and the engine lighting
vectors of an unlit BSP, are missing; layers 1, 5, 7, 8 and 20 draw
their own passes where the plan takes them, "World layers" above); the bucket streams (types `2f` and `30`: per-vertex colour and
light direction, not interpreted); the lightmap's light probes and the
sky's fallback object-lighting colour (reported, not drawn); fog B,
patchy fog and the BSP's fog planes; the sky's node transforms, its tag's
rotation and scale, and skies after the first; animated shaders, model
variants and permutations other than the first, and node transforms of
skinned models (models are drawn in their bind pose).
