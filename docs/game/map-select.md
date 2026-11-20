# Multiplayer map selection

Status: Built. The category dialog, the separate mod slots and the live refresh are in the code; the game checks under "Game verification" are open.
Code: `crates/extensions/game/src/map_select.rs` (`map_select::MapSelect`, `Catalogue`, `page`). Tests: `crates/extensions/game/src/map_select.rs`.
Switch: none.

Game Setup's **Change Map** uses the game's Select Map dialog to show
categories. Choose a category with **A**, then choose a map. **B** or
**Back** in a map list returns to the categories with the opened category
focused. From the categories, **B** or **Back** closes the picker.
**A** on a map applies the selection and dismisses the picker.

| Category | Maps | Order |
|---|---|---|
| Halo 2 | Maps from the Halo 2 disc | The game's order |
| Map Pack | Maps installed by `h2 import-mappack` | The game's order |
| Mods | Community packages, including mods based on disc and Map Pack maps | Name, then list ID |
| Halo: CE | Converted Halo 1 maps | Name, then list ID |

With a single category, Change Map opens the regular map list. A full
Mods category can show an overflow notice even when no mod fits.

## Separate slots for mods

The category follows the slot's load path. The DVD's
`D:\maps\<name>.map` has its original Halo 2 slot. Map Pack maps keep their
imported `T:\$c\<offer>\<name>.map` slots. Each mod uses a separate content
offer, a separate list ID, and a free multiplayer slot. A mod based on
Lockout is listed by its own name under **Mods** while **Lockout** remains
under **Halo 2**. The same rule applies to mods based on Map Pack maps.

The list contains 50 slots. The Halo 2 disc uses 12, leaving 38 for added
content. With the full nine-map Map Pack, 29 remain for Mods, converted
Halo 1 maps and any other content. When the list fills, the category
shows `Mods (N mods not shown, list full)`. Overflow packages remain
installed and can appear when a slot becomes available.

Installed mods appear in their own slots when space allows. See [generated offers](maps.md#package-offers) for slot
allocation, IDs and load paths.

## Names, descriptions and pictures

The generated record contains the package's names and descriptions
from its manifest. The preview is the package image when available, the
base map's image otherwise, or a generated card when neither can be
read. For a Map Pack base, the fallback comes from the installed base
map's `patch.lvl` preview.

The game applies the record to a free slot and uploads the preview to
that slot's bitmap. Select Map, Game Setup and the lobby use the same
slot, so all three show the chosen mod's picture. The retail slot
retains its own name, description and bitmap. See
[Map Pack §8.1](map-pack.md#81-where-10-reads-patchlvl) and
[map packages §4](maps.md#4-new-maps-and-the-map-pack).

## Changes during a session

A Live download or removal updates the generated offers. The next
main-menu tick releases obsolete generated slots and runs the game's
content scan and multiplayer array builder. The refresh runs after the
agent and skulls wrappers and before co-op lobby setup. A joining console
can then use the offered map without opening Change Map. An install that exceeds
the budget stays on disk and counts toward the overflow notice.

`map_select::Catalogue` reads the disc slots and the offered records.
The live dialog uses the game's widgets over that catalogue, and the
offline `map_select::page` models category selection and back navigation.

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

  Two things lose a row the array holds. The constructor skips a locked
  one (above), and a map id no slot of the loaded list holds has no name
  (the row text below names it from its slot; the constructor only skips
  its picture). Our catalogue comes from the disc and the packages on the
  host, the game's slots from its own content scan, so they can differ.
  Before each array is written, `map_select::unshown` compares them and
  the console says which rows won't show by name and why:
  `map select: Maps(MapPack): 4 of 9 rows won't show by name
  (Containment (0x514): not in the game's list, ...)`. Nothing is said
  when every row has an unlocked slot.
- **Row text** (`MAP_LIST_ROW_TEXT` `0x2c9ac4` notes which list is
  drawing; `MP_LEVEL_NAME` `0x2afd70` answers). The row text looks up the
  row's slot (`0x19c5f0`) and calls `0x2afd70(slot)` at `0x2c9afc` for its
  name (`slot + 0xc + 0x40 * language`, the current language's UTF-16
  name). For a list of ours, called from there, the answer is our UTF-16
  category label in category mode. In maps mode the game uses the
  slot's own name.
- **A category's press** (`MAP_LIST_ROW_CHOSEN`, `0x2c9b17`, in place of
  the retail press). It finds the row's id (handler `+0x70`, element `+4`)
  and its category, then arms a maps list for it. It pushes a second
  Select Map on the pressing screen's own window and slot. Window 3 has
  one active dialog and disposes of the categories when the maps take
  over. The maps screen carries the selected category's identity.
- **B or Back on a category's maps** (`SCREEN_BACK_EVENT`) queues Select
  Map in categories mode with request bit 0 set. The push hook resolves
  the opened category's representative map ID to a row handle in the
  rebuilt list. The window applies the handle after setting up the new
  screen's widgets and closes the maps with the normal Back message. If
  the category is unavailable, the list uses its default focus.
  Back on the category list closes the picker. With the lobby Back
  feature it returns to Game Setup. See [Back navigation](ui-screens.md#back-navigation).
- **A map's press** runs retail. `0x2c9b17` calls `0x19a6f2` (map id in
  ESI, campaign `-1`), which stores the lobby's map at `0x51098c` and sets
  `0x510819`. Then it sends message 3 to its own screen
  (`0x22e957(3)` on `0x22eeee(handler)`), and the screen leaves.
  Each instance's destructor (`MAP_SCREEN_FREE`) drops its list metadata.
- **Message 3 stays inside its own screen.** `0x22eeee` walks the
  widget's parents (`+0x10`) to its root, the Select Map screen.
  `0x22e957` builds a kind-3 message (`0x22e65e`) and hands it to that
  root and its children (`+0x14` first child, `+0x18` next sibling).
  It is how the retail screen leaves. It never reaches Game Setup or the
  lobby: the lobby's side of a map change is all in `0x19a6f2`.
- **The retail list's own build**, for reference: `0x19c7e0(array)`
  (stdcall, `ret 4`) collects every used slot's id, sorts them by
  `+0xc4c` (`0x321460` with comparator `0x19c9a0`) and fills `[0x4ee4e4]`.
  The main-menu refresh calls it after the content scan. The categorized
  lists read the retail array for its lock bytes.

## Game verification

Use [`ui.stack`](../runtime/agent-api.md#ui-what-the-menus-show) for
game checks. Open Change Map with at least two categories. Record the
focused row, open that category, and check that the active mode is
`maps` with a `folders` parent. Press B and check that active mode is
`folders` with the same focused row. Press B again and check that the
picker is gone. Repeat with Back and with the lobby Back feature disabled.

With an existing data directory,
confirm retail Lockout and a Map Pack base load from their original
categories. Pick a mod with an image and check Select Map, Game Setup
and the lobby. Check a second mod without an image for the base preview.
Download a mod while in the menu and verify it appears without a
restart. With two Live clients, join a host whose selected mod is absent
on the guest and start the match. Switch the host back to retail and
verify the next advertisement clears the mod's content ID.

Open items:

- All nine Map Pack maps appear under **Map Pack**. The dialog has 14 row
  widgets; if a map is missing, compare the catalogue's IDs with the
  loaded list. The map-select log names rows whose IDs have no loaded slot.
- Two mods of the same base, one with an image and one using the base
  fallback: pick both, go back to the category list with B, and switch to
  the retail sibling. The Mods browser and its detail page use the base
  filename for their previews, and a missing base bitmap uses the neutral
  lobby picture.
- The Mods browser on a cold open with a multi-row catalogue, a retry, and
  a background refresh that must not replace the map under the cursor.
- The pack screen: X with no focus, an empty pack, an unavailable pack
  and a full 40-map response with Download All.
- Footers per focus and state: Download All, loading, installed, failed
  and removal rows each advertise their own commands, and the detail
  footer changes after a download finishes or a removal.
- Failed downloads show on the row, with recovery instructions on the
  button that retries; batch progress counts completed bytes.
- Detail page text: long credits and feature lists stay inside their
  columns, and the checklist heading aligns with its cells.
- A row without a slot image shows the lobby's unknown-map picture, and a
  resolved picture replaces it while focus stays put.
- B restores focus on the parent row; bitmap replacement, transition
  timing and the lobby's presentation need a run through the agent API.
