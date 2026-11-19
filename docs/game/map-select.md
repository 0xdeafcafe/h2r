# The multiplayer map select by category

Status: **built and table-tested, not run in the game.** Started
2026-11-18. Packages installed while the game runs, and a mod's name,
description and picture on its map's slot: 2026-11-19, not run in the
game either.

The pregame lobby's map list now holds maps from four places: the Halo 2
disc, the Multiplayer Map Pack ([map-pack.md](map-pack.md)), community
maps ([maps.md](maps.md) §4) and converted Halo 1 maps
([multiplayer.md](../halo1/multiplayer.md)). In one list sorted by the
game's sort order they come out mixed. So Game Setup's **Change Map** now
opens a list of categories first, in the retail Select Map dialog itself
(its list, picture and description):

| Category | What | Order of its maps |
|---|---|---|
| Halo 2 | the disc's maps | the game's sort order (slot `+0xc4c`), as `0x19c7e0` sorts the list |
| Map Pack | `h2 import-mappack`'s packages | the game's sort order (130 to 210 on the user's disc) |
| Mods | new map packages, any other content package, and a disc or Map Pack map a package in `<data>/maps` replaces | by name (any case), then map id |
| Halo: CE | converted Halo 1 maps (`h1_<map>`) | by name, then map id |

A category with no maps isn't listed. With one category or none (no
content package installed, no package replacing a disc map) nothing is
hooked and Change Map opens the retail list.

Controls are the retail dialog's own: up and down move, **A** picks the
focused row, **B** (or Back) goes back. A on a category opens
its maps. B on the maps goes back to the categories with the same row
focused. A on a map makes it the lobby's map and returns to Game Setup.
B on the categories also returns to Game Setup, without changing the map.

## A map's category

The category comes from where the game loads the map, which is the slot's
path (`+0xb4c`, map-pack.md 8.1). `map_select::category_of` reads it:

1. A path that doesn't start with `t:\` is loaded from the disc as
   `d:\maps\<name>.map` (`0x214db0`), so it's **Halo 2**.
2. `t:\$C\<offer ID>\<map>` is a content package's map: 1.0 sets the
   path to `sprintf("%s\%s", content dir, basename)` (`0x19be00`). With
   the offer's low half in the Map Pack import's range `0x0101..=0x03ff`
   (map-pack.md §5), it's the **Map Pack**.
3. A new map's offer (`0x8000_0000 | list id`,
   `content::map_list::is_new_map_offer`, maps.md "Map ids and offer IDs")
   whose map name starts with `h1_` is **Halo 1**. Converted maps are
   named `h1_<map>` (docs/halo1/multiplayer.md "The setting and the map list"), and
   `boot::new_maps` installs them like any new map.
4. Any other content, new maps included, is **Mods**.
5. A disc map that a package in `<data>/maps` replaces (not a derived
   package, and chosen for its name) is **Mods** too, under the package's
   name (`Catalogue::with_mods`, `replacing_packages`). So is a Map Pack
   map a package replaces (a mod on a Map Pack base, maps.md "Importing a
   mod"): its slot keeps its `t:\$C\<offer ID>\<map>` path, which the vfs
   serves patched over the installed map. A Map Pack slot is left alone
   when the disc has a map of the same file name (the package is the
   disc map's).

**A replaced disc map can't sit in both lists.** The vfs serves one map
per name: `maps\containment.map` is the package chosen by the choice rule
(maps.md §7, `active.toml` first, else the directory that sorts last),
or the disc's when `active.toml` says `containment = false`. The slot's
path is the same `d:\maps\containment.map` either way (`0x214db0`), and
the list has one slot for it. So while a package is chosen for a disc
map, its slot goes under Mods with the package's name, and Halo 2 leaves
it out, because the disc's own map can't be loaded until the package is
switched off. Offering both would mean installing the package as a new
map (a content package of its own, as derived packages are, maps.md
§4), which changes how the package is served. That isn't done.
The choice is read at boot and again whenever a package is installed or
chosen while the game runs (below, "Installed while the game runs").

No list of retail map names or ids is kept. The Halo 2 maps are the ones
the user's own disc lists.

## The list

`map_select::read` builds the list at boot the way 1.0 does
(`0x19c120`, map-pack.md 8.1). It takes the used slots of `mainmenu.map`'s
multiplayer list from the disc (`new_maps::IsoDisc::slots`), then each
`TDATA\4D530064\$c\<offer>\patch.lvl` record (`Entry::from_v2`), read after
`new_maps` has installed its packages. Each slot is read by
`tags::ui_levels::mp_level` (`map_of_slot`), the reader the Live catalog
and the editor's Maps list share. A record whose map id matches a
slot replaces that slot, as `0x19be00` refills it. A map's name is the
English one (slot `+0xc`, record names `[0]`). At most 50 maps, the
list's slots.

## Installed while the game runs

A mod downloaded on the Mods screen, or installed by a join
(`MapGate::on_installed`, which the boot also uses for the VFS rescan),
is listed without a restart:

1. On the download's own thread, `map_select::packages_changed` installs
   new maps as content packages (`new_maps`, as at boot), then reads the
   catalogue again (`Sources::read`: the disc's slots are read once, the
   first time a catalogue needs them, and kept) and the fills below, and
   bumps the selection's generation. With one category or none at boot
   the hooks are on all the same; Change Map opens the retail list until
   there are two (`Selection::active`).
2. The next Change Map (the Select Map list asking for its array,
   `0x19c6a0` from `0x2c9985`) sees a generation the slots weren't filled
   for. Before it answers, it refills the slots (next section). When a
   content package came or went (a new map: its offer directory under
   `$c` appeared or left), it first runs the game's own content scan
   (`0x19c120`, which refills the slots from each `patch.lvl` through
   `0x19be00`) and the multiplayer array build (`0x19c7e0([0x4ee4e4])`),
   the two steps of the game's refresh `0x19c190` that matter here. Then
   the list is ours (categories) or the retail array (one category),
   built from the new catalogue.

A map load runs `0x19c190` itself (function table `0x43ca94`, every map
load; the content scan only in the main menu). Its hook
(`MP_LEVEL_REFRESH`) calls it, forgets what it had changed (the tag
memory is fresh), and refills.

## A mod's name and picture

A package chosen for a disc map (or a Map Pack map) is served in its
place, so that map's slot is the mod. Every screen reads the slot:

| What | Where it reads |
|---|---|
| the lobby's map name | `0x22d5a7`: `0x19a84e` gives the lobby's map, `0x19c5f0(id)` its slot, `0x2afd70(slot)` the name |
| Select Map rows, System Link browser rows, the post-game | `0x2afd70` (from `0x2c9afc`, `0x253569`, `0x2b6a59`) |
| Select Map's description | inline at `0x2c9bee`: slot `+0x24c + 0x100 * language` |
| Game Setup's and Select Map's picture | the slot's bitmap datum `[slot + 8]` (`0x2bbcbb`, `0x2c9bd1`, through `0x137550` and `0x2b0a7b`) |

So `boot::mod_slots` refills the slot in the loaded main menu's list
(globals `[0x4e034c] + 0x178`, element 0, block `+0x10`; only when the
cache header `[0x4e0350] + 0x10` says main menu):

- **Name and description.** The nine names (`+0xc`, 32 UTF-16 units each)
  and descriptions (`+0x24c`, 128 units each) become the package's:
  `[list] name.<lang>`, else `[package] name`; `[list]
  description.<lang>`, else `[package] description`, else empty
  (`content::map_list::Entry::for_manifest`, as a new map's record).
- **Picture**, when the package has an image (`[package] image`, which
  the Live download fetches with the layers). A retail slot's bitmap
  isn't the 220x207 32-bit one a `patch.lvl` preview is copied into, so
  the slot is first pointed at a free slot's bitmap (a free slot, id
  `-1`, has one of its own, map-pack.md 8.1; one no used slot shows).
  Then the game's own applier `0x19be00(content dir, record)` refills
  the slot from a v2 record built from the slot itself (its id, sort
  order, flags, team counts, scenario path) with the package's texts
  and the image as the preview (`map_list::preview_from_rgba`, no frame
  colours). The applier keeps the bitmap reference, copies the preview
  into that bitmap through its texture lock (`0x1cfb0`, `0x12cb80`,
  `0x1d5e0`) and sets the path to `<dir>\<basename>`, which, with the
  slot's own directory as `dir`, is the path it had. With no free bitmap
  left the mod keeps the map's picture.
- **Memory.** The record (`0x2d3fc` bytes) and its content dir go in a
  `0x2e000`-byte block of ours mapped once at install
  (`mod_slots::map_scratch`: the first free place from `0xa2fc_0000`,
  above the campaigns' region, below `0xa300_0000`), not the features'
  shared arena (512 KiB for all of them). Without it the mods keep their
  maps' pictures.
- **Undo.** A slot changed in this load whose package is gone (or no
  longer has an image) gets its own bitmap reference, names and
  descriptions back.

The Select Map row label of a replaced map (below, `MP_LEVEL_NAME`) is the
package's name as before; with the slot refilled it is the same text.

## How it's built

Both steps are the retail Select Map dialog (screen `0xce`, loader
`0x2bb2db`, list handler at screen `+0x614`, vtable `0x45d618`). Each
instance lists an array of ours, and the row texts and presses are hooked
for those instances only (`map_select::MapSelect`, feature `map-select`).

- **Arming.** Game Setup's Change Map row (`0x2bb8ac`) asks `0x149f49`
  for loader `0x2bb2db` (window 3, slot 4), returning to `0x2bb8d7`. An
  observer there (`SCREEN_REQUEST`) arms the next Select Map for the
  categories. It only observes, so the campaign picker's `before` on the
  same point needs no order with it.
- **The list's array** (`MP_LEVEL_ARRAY`, `0x19c6a0`). The Select Map list's
  constructor (`0x2c9928`, handler in ESI) asks it for `[0x4ee4e4]` at
  `0x2c9980`, then copies every element into its own list (handler
  `+0x70`): the lock byte at `+2` (it skips a locked map unless the
  profiles' progress allows it, `0x2c99cc`) and the map id at `+4`, then
  the map's picture. Called from `0x2c9985` while armed, it returns an
  array of ours instead (`write_list`: a data array in our memory with the
  retail array's shape, 8-byte elements, filled as `0x19c7e0` fills the
  retail one) and records the handler's mode:
  - **categories**: one element per category, its first map's id, so
    the dialog shows that map's picture and description;
  - **maps**: the category's maps in its order, each with the retail
    array's lock byte for that id.
- **Row text** (`MAP_LIST_ROW_TEXT` `0x2c9ac4` notes which list is
  drawing; `MP_LEVEL_NAME` `0x2afd70` answers). The row text looks up the
  row's slot (`0x19c5f0`) and calls `0x2afd70(slot)` at `0x2c9afc` for its
  name (`slot + 0xc + 0x40 * language`, the current language's UTF-16
  name). For a list of ours, called from there, the answer is our UTF-16
  label in our memory: the category's name in category mode, a replacing
  package's name in maps mode. Any other name is the slot's own.
- **A category's press** (`MAP_LIST_ROW_CHOSEN`, `0x2c9b17`, in place of
  the retail press). It finds the row's id (handler `+0x70`, element `+4`)
  and its category, then arms a maps list for it. It pushes a second
  Select Map on the pressing screen's own window and slot: our request
  (`skulls_mp::open_request`, as the Halo 1 Options row builds its own)
  handed to loader `0x2bb2db`. B on the maps returns to the categories,
  and B on the categories returns to Game Setup, both by the retail
  dialog's own back.
- **A map's press** runs retail. `0x2c9b17` calls `0x19a6f2` (map id in
  ESI, campaign `-1`), which stores the lobby's map at `0x51098c` and sets
  `0x510819`. Then it sends message 3 to its own screen
  (`0x22e957(3)` on `0x22eeee(handler)`), and the screen leaves. Before
  it does, we note the categories' window and slot for that screen.
- **Closing the categories** (`MAP_SCREEN_FREE`, the screen's destructor
  `0x2bb384`, vtable `0x45c650` slot 0). When the maps screen whose map
  was pressed is freed, the hook calls the destructor (forgotten first,
  so the hook passes it through) and then closes the categories screen
  itself: the close message (`0x22e957(2)`) that the window's close
  (`0x2351d4`) sends its top screen, unless the screen's state byte
  `+0x42` has bit 1 (already closing) (`ui_kit::lifecycle::close_screen`).
  It can't close by window and slot (`0x14800c`) as B does. The stack's
  free (`0x2351ea`, from the UI update's free loop `0x148148`) zeroes the
  window's top word (`+8`) for the screen it frees before it calls the
  destructor. So `0x2351d4` read `0 + 0x42`, the first build's crash on
  picking a map (`MemoryFault ... addr: 42`, `0x2351d9`, the window object
  `0x54d84c` for window 3, slot 4).
- **Message 3 stays inside its own screen.** `0x22eeee` walks the
  widget's parents (`+0x10`) to its root, the Select Map screen.
  `0x22e957` builds a kind-3 message (`0x22e65e`) and hands it to that
  root and its children (`+0x14` first child, `+0x18` next sibling).
  It is how the retail screen leaves. It never reaches Game Setup or the
  lobby: the lobby's side of a map change is all in `0x19a6f2`.
- **The retail list's own build**, for reference: `0x19c7e0(array)`
  (stdcall, `ret 4`) collects every used slot's id, sorts them by
  `+0xc4c` (`0x321460` with comparator `0x19c9a0`) and fills `[0x4ee4e4]`.
  We don't hook it. The retail array is read for its lock bytes and never
  written.

## Offline

`map_select::page` models the two steps as one `ui_kit::page::StackPage`
over `CategoryRows` and `MapRows` (list classes used offline only; the
game shows the retail dialog). An `Open` effect pushes the maps, and B or
Back pops them. A state is the presses since the categories, and the spec
replays them, so a named state (`maps-halo2`, `maps-mappack`, `maps-mods`,
`maps-halo1`) shows what its presses chose. `map_select::sample()` names
maps in every category, with ids and paths shaped like the game's and no
game data.

Tests (`boot` `map_select::tests`):

- categories from paths: the disc, the Map Pack's offer range, `h1_` in a
  new map's offer only, any other content as a mod; a disc map a package
  replaces under Mods by the package's name, the Map Pack map of the
  same file name left alone; a Map Pack map a package replaces under Mods
  by the package's name, on its own slot;
- categories in order with empty ones hidden; Halo 2 and the Map Pack in
  the game's order; Mods and Halo 1 by name then id; a package that names
  a slot's id takes that slot;
- reading a slot, a package's `patch.lvl` (its path as 1.0 makes it), and
  a directory with no record; one category or none leaves the retail
  list;
- the arrays: category mode lists each category's first map unlocked,
  maps mode the category's maps with the retail lock bytes; a written
  array reads back as the game reads the retail one (`FakeGuest`); row
  labels only for categories and replacing packages, in 31 UTF-16 units
  and a NUL;
- the flow: armed by Change Map only, categories then the picked maps,
  B on the maps forgetting them, a map's press closing the categories'
  own screen once, after its screen is freed, and nothing when no
  categories are under it; `ui_kit::lifecycle`: the window's top word
  is 0 mid-free, so the close goes to the screen itself;
- a table driving the two steps with presses (A opens a category's maps,
  B returns with the focus kept, B on the categories closes them, A on a
  map is the action);
- a package installed while the game runs: listed under Mods by its
  name after `refresh`, the generation bumped, the disc's slots read
  once, a new content package asking for the game's content scan once
  (`a_package_installed_while_the_game_runs_is_listed_without_a_restart`);
- `the_boot_s_features_plan_together_with_the_campaigns`: the campaigns,
  their picker, the map select's seven hooks and the Halo 1 Options row
  plan with no refusal, each hook where the table above says.

Tests (`boot` `mod_slots::tests`): a mod without a picture renames its
map's slot in every language and keeps the picture, once; one with a
picture takes the first free bitmap and a record that is the slot itself
with the package's texts and image (passes 1.0's check, the applier's
path is the slot's), keeps its own spare for the next package; a package
gone puts the slot's own bytes back, a picture dropped the map's own
bitmap; no free bitmap keeps the map's picture; which slots a fill takes
(a disc map; a Map Pack map unless the disc has its file name; never a
new map's content); texts cut to the fields; the slots read from the main
menu's globals only (`FakeGuest`-style memory); a replacing package's
name, description and image read from its directory.

## Needs a game run

1. With a Map Pack, a converted Halo 1 map or a package replacing a disc
   map installed, the console says `map select: by category (Halo 2 12,
   Map Pack 9, ...)` at boot.
2. In a custom game's Game Setup, Change Map logs `map select: Select Map
   lists Categories (N rows)`. The retail SELECT MAP dialog shows one row
   per category with the category's name, and the picture and description
   of its first map.
3. A on a category logs `map select: <category> picked; its maps on
   window 3, slot 4`, then `Select Map lists Maps(...)`. A second Select
   Map dialog lists that category's maps with their own pictures. A
   replaced disc map shows the package's name. B goes back to the
   categories.
4. A on a map logs `map select: map 0x... pressed; the categories close
   after`, then `map select: the maps closed; closing the categories`.
   Game Setup shows the new map, with no crash (the first build died
   here, at `0x2351d9` reading `0x42`). Do it for a Halo 2 map and a
   Halo: CE one, then start a game on it.
5. A mod downloaded on the Mods screen, with the game running, no
   restart (a Live server with a mod on Lockout that has an image, and
   one without): stderr `map select: packages changed; by category
   (Halo 2 12, ..., Mods 1) (1 mods on their maps)`. Back in a custom
   game's Game Setup, Change Map logs `map select: mods on their maps
   (packages changed): 1 named, 1 pictures, 0 put back, 0 failed` (or
   `1 named, 0 pictures` without an image). Mods is listed with the
   mod's name; its row shows the mod's picture (or Lockout's, without an
   image) and description. Pick it: Game Setup and the lobby show the
   mod's name and picture, and the game loads the mod.
   - At the next main menu load (after a game) stderr says `mods on
     their maps (the lists were built)` and the same shows.
   - Unchecked: that `0x19be00` called from our hooks (outside its
     own map-load path) draws the new picture at once; that the free
     slot's bitmap really is the 220x207 32-bit kind for every free
     slot; that the lobby screen already up refreshes its name text
     without leaving it.
   - A mod that is a new map (`[base] derived = true`), downloaded
     with the game running: stderr `the game read its content packages
     again` at the next Change Map, and it is listed under Mods.
6. Unchecked: whether the second Select Map takes our request on the
   categories' window and slot as the first took Game Setup's. Whether
   the close message sent to the categories from the maps screen's
   destructor closes them.
   Whether a category row standing on a locked map is skipped (category
   rows are written unlocked, but the constructor's check reads the
   element). Whether the dialog's description also comes from
   `0x2afd70`'s neighbours: it shows the first map's description in
   category mode.
