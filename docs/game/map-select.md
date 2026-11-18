# The multiplayer map select by category

Status: **built and table-tested, not run in the game.** Started
2026-11-18.

The pregame lobby's map list now holds maps from four places: the Halo 2
disc, the Multiplayer Map Pack ([map-pack.md](map-pack.md)), community
maps ([maps.md](maps.md) §4) and converted Halo 1 maps
([h1-multiplayer.md](h1-multiplayer.md)). In one list sorted by the
game's sort order they come out mixed. So Game Setup's **Change Map** now
opens a list of categories first:

| Category | What | Order of its maps |
|---|---|---|
| Halo 2 | the disc's maps | the game's sort order (slot `+0xc4c`), as `0x19c7e0` sorts the list |
| Map Pack | `h2 import-mappack`'s packages | the game's sort order (130 to 210 on the user's disc) |
| Mods | new map packages and any other content package | by name (any case), then map id |
| Halo: CE | converted Halo 1 maps (`h1_<map>`) | by name, then map id |

A category with no maps isn't listed. With one category or none (no
content package installed) nothing is hooked and Change Map opens the
retail list.

Controls are the same as on the other screens: up and down move, **A**
picks the focused row, **B** (or Back) goes back. A on a category opens
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
   named `h1_<map>` (h1-multiplayer.md "The setting and the map list"), and
   `boot::new_maps` installs them like any new map.
4. Any other content, new maps included, is **Mods**.

No list of retail map names or ids is kept. The Halo 2 maps are the ones
the user's own disc lists.

## The list

`map_select::read` builds the list at boot the way 1.0 does
(`0x19c120`, map-pack.md 8.1). It takes the used slots of `mainmenu.map`'s
multiplayer list from the disc (`new_maps::IsoDisc::slots`), then each
`TDATA\4D530064\$c\<offer>\patch.lvl` record (`Entry::from_v2`), read after
`new_maps` has installed its packages. A record whose map id matches a
slot replaces that slot, as `0x19be00` refills it. A map's name is the
English one (slot `+0xc`, record names `[0]`). At most 50 maps, the
list's slots.

## How it's built

- **Two list screens of ours** (`list_class`, with the kit's legend in the
  footer and the Controls list layout): `map-categories` (title "Select
  Map", rows from `CategoryRows`) and `map-list` (titled with the category,
  rows from `MapRows`). Their strings come from the `map select` id family
  (`TABLE + 0x0600_0000`, 0x40 ids: title, four category labels, two
  legends, one per map), served from the layouts' table and written once at
  boot.
- **Opening on the dialog's stack.** Game Setup's Change Map row
  (`0x2bb8ac`) asks `0x149f49` for loader `0x2bb2db` (window 3, slot 4),
  returning to `0x2bb8d7`. The `map-select` feature swaps that request's
  loader for the categories' loader, so the categories open where the
  retail list would. A category's press returns the new
  `Chosen::Push(screen)`: open a screen of ours on the pressing screen's
  own window and slot (`list_class::push`), with a request built the way
  the Halo 1 Options row builds its own. `Chosen::Open` goes through
  `0x149f1e`, which always uses window 5, and from the lobby that put the
  Skulls screen under it (skulls.md "Why the Game Setup row broke").
- **A map's press** stores the map and returns `Chosen::Act`. The list's
  action calls `0x19a6f2` the way the retail press does (`0x2c9b5d`): map
  id in ESI, campaign `-1` on the stack, stdcall `ret 4`, AL true when the
  map is accepted. It stores the lobby's map at `0x51098c` and sets
  `0x510819`. Then the action closes the map list (`0x14800c`), and once
  that screen is freed its destructor closes the categories under it, in
  the same window and slot (`lifecycle::AfterClose`). If AL is false, the
  list stays open and logs why.
- **One point, two pickers.** The campaign picker swaps the loader of
  another request at the same `0x149f49` (`campaign_menu`). The two never
  act on the same request, but the feature plan refuses two `before`
  hooks with no order between them, and the first integration build
  stopped every boot on it. `map-select`'s hook is declared
  `after_feature("campaigns")` (it sits inside theirs), and
  `the_boot_s_features_plan_together_with_the_campaigns` plans the
  campaigns, their picker, the map select's hook and screens and the
  Halo 1 Options row together.
- **The retail press's last call.** After `0x19a6f2`, the retail press
  calls `0x22e957(3)` on `0x22eeee(list handler)`. `0x22eeee` walks the
  widget's parents (`+0x10`) to its root, the Change Map screen itself.
  `0x22e957` builds a message of kind 3 (`0x22e65e`) and hands it to that
  root and its children (`+0x14` first child, `+0x18` next sibling),
  through vtable `+0x14` for type-2 widgets. So the message stays inside
  the pressing screen's own tree and never reaches Game Setup or the
  lobby. It is how the retail screen leaves; ours leaves through
  `0x14800c`. The lobby's side of a map change is all in `0x19a6f2`
  (`0x51098c` and the flag at `0x510819`). So we don't send the message:
  sending it to our screen would close it a second time. Game Setup
  coming back on top is the same in both flows.
- **The retail list's own build**, for reference: `0x19c7e0(array)`
  (stdcall, `ret 4`) collects every used slot's id, sorts them by
  `+0xc4c` (`0x321460` with comparator `0x19c9a0`, which compares the
  slots' sort orders through `0x19c5f0`) and fills the data array
  `[0x4ee4e4]` one 8-byte element per map (id at `+4`). The Change Map
  screen (`0x2c9960`) copies that array into its list (handler `+0x70`),
  and its press is `0x2c9b17`. We don't hook any of these.

## Offline

`map_select::page` is the two screens as one `ui_kit::page::StackPage`,
a new generic kit page for list screens that open one another. An
`Open` effect pushes the named screen, and B or Back pops it. A state is
the presses since the first screen, and the spec replays them, so a
named state (`maps-halo2`, `maps-mappack`, `maps-mods`, `maps-halo1`)
shows what its presses chose. `map-select` is in `ui_kit::screens::NAMES`
with `map_select::sample()`: invented names in every category, with
ids and paths shaped like the game's and no game data. It's drawn by
`h2 ui preview map-select` and the sheet, and checked by the kit's checks.

Tests (`boot` `map_select::tests`):

- categories from paths: the disc, the Map Pack's offer range, `h1_` in a
  new map's offer only, and any other content as a mod;
- categories in order with empty ones hidden; Halo 2 and the Map Pack in
  the game's order; Mods and Halo 1 by name then id; a package that names
  a slot's id takes that slot;
- strings: all in the family, one per map, no id used twice;
- reading a slot, a package's `patch.lvl` (its path as 1.0 makes it), and
  a directory with no record;
- one category or none leaves the retail list;
- a table driving the pair with presses: A opens a category's maps, B
  returns to the categories with the focus kept, B on the categories
  closes them, A on a map is the action and says which map, the maps'
  title follows the category, and X, Y, Start and sideways presses do
  nothing;
- `ui_kit::screens` and `ui_kit::page` cover it with every other screen:
  every input in every state, the fallback checks, and drawing every state
  without a disc.

## Needs a game run

1. With a Map Pack or a converted Halo 1 map installed, the console says
   `map select: by category (Halo 2 12, Map Pack 9, ...)` at boot.
2. In a custom game's Game Setup, Change Map logs `map select: opening the
   map categories in place of the map list`, and "Select Map" shows the
   categories that have maps, on top of the lobby (window 3), not under
   it.
3. A on a category logs `ui: opening map-list on window 3, slot 4` and
   shows that category's maps, titled with it. B goes back to the
   categories.
4. A on a map logs `map select: the lobby's map is now 0x...`, then
   `map select: the maps closed; closing the categories`, and Game Setup
   shows the new map. Start a game on it.
5. Unchecked: whether the second close (`0x14800c` on the same window and
   slot from the maps' destructor) closes the categories, or whether a
   screen under one that's closing needs something else. Whether Game
   Setup shows the new map's name as soon as it is back on top: the
   reading above says the retail flow does nothing more for the lobby
   than `0x19a6f2`.
   Also unchecked: the map list's preview pictures and descriptions,
   which the retail screen shows and ours doesn't, and whether the list's
   flag byte (slot `+0xc50` bit 0, collected separately by `0x19c7e0`) or
   the retail list's unlock check (`0x2c99cc`) should hide any map.
