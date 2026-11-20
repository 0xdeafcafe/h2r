# Controller button layouts

Status: Built. Unit tested; the in-game behaviour is under "Not verified".
Code: `crates/input/src/layouts.rs` (the symbolic tables), `crates/boot/src/layout_glue.rs` (the hooks, installed by `boot::run`). Tests: the same two files.
Switch: none.

The retail game offers four button layouts. This page records how the retail
game implements them and what our hooks touch to add three more.

No game data is here. Addresses are guest virtual addresses of the retail XBE
(SHA-256 `03215919…935d`), named only as hook points. Strings were read with
`h2 strings` and are described in words.

## What the retail game offers

The Settings, Edit Profile, Controls screen has a "Button Layout" row. The
screen offers four layouts: Default, Southpaw, Boxer and Green Thumb. A fifth
name, "Jumpy" (jump on the left trigger), exists in the string tables and in
the profile summary strings but no code path implements it. It is dead data.
The thumbstick layouts (Default, Southpaw, Legacy, Legacy Southpaw) are a
separate setting. Halopedia lists the same four button layouts and their
mappings: <https://www.halopedia.org/H2:Control_schemes>.

| Layout | What it changes against Default |
|---|---|
| Default | A jump, B melee, X action/reload, Y switch weapons, LT grenade, RT fire, Black switch grenades, White flashlight, left stick click crouch, right stick click zoom |
| Southpaw | LT and RT swap |
| Boxer | Melee on LT, grenade on B |
| Green Thumb | Melee on right stick click, zoom on B |

## What it lacks

Not in Halo 2 on Xbox, wanted by players, and defined by later Halo games:

- Bumper Jumper: Halo 3 has jump on LB, melee on RB, B action/reload
  (<https://www.halopedia.org/H3:Control_schemes>). Reach has A switch
  grenades, B action/reload, X armor ability, LB jump, RB melee
  (<https://www.halopedia.org/Reach:Control_schemes>).
- Recon: Reach has RB action/reload, LB armor ability, X switch grenades,
  B melee, A jump (same page). MCC Halo 2 Anniversary calls its default
  "Universal Default (Recon)" (<https://www.halopedia.org/MCC:Control_schemes>).
- Walkie Talkie: Halo 3 only. A jump, B action/reload, X swap grenades,
  RB melee, LB flashlight/team chat.

Halo 2 has no armor ability or equipment, so Flashlight takes that role. The
Xbox pad has no bumpers; the host already maps right bumper to Black and left
bumper to White (`docs/input/input.md`). The three layouts, as `Layout` in
`layouts.rs`, with preset numbers 4, 5 and 6 after the retail 0 to 3:

| Layout | Jump | Melee | Switch grenades | Action/reload | Flashlight |
|---|---|---|---|---|---|
| Bumper Jumper (4) | White (LB) | Black (RB) | A | B | X |
| Recon (5) | A | B | X | Black (RB) | White (LB) |
| Walkie Talkie (6) | A | Black (RB) | X | B | White (LB) |

Everything else (triggers, sticks, Y, Start, Back) stays as Default.

### The debug menu chord (host, not a layout)

On a DualSense or DualShock 4, the touchpad click and Options together open
our debug menu ([docs/runtime/debug-menu.md](../runtime/debug-menu.md)). No
layout uses the touchpad, which never reaches the game. Options alone is
still Start (pause): on these pads it reaches the game 80 ms late, while the
host waits to see whether the click follows. While the menu is open the game
sees a neutral pad, whatever its layout. Buttons still held when it closes
stay hidden until they are released.

## Why not remap on the host

A host remap of the physical pad into the guest pad was the first plan. It
cannot work for menus: menu confirm and cancel are A and B, so a host that
moves Jump off A also moves "accept". The layout belongs inside the game,
which keeps confirm and cancel fixed (slots 14 and 15 below). Presets could
still be a stopgap for the sticks-and-triggers-only case, but it is not
built.

## How the retail game implements a layout (read from the XBE)

### The action table (`Local`)

Each local player has a 0x1c-byte input settings record at `0x51ea18 + 0x1c *
player`. Bytes 8 to 0x17 are sixteen action slots; slot `i` holds the index of
the physical button that drives action `i`. Indices are the analog buttons
0 to 7 (A, B, X, Y, Black, White, LT, RT) then the digital bits 8 to 15
(d-pad up, down, left, right, Start, Back, left thumb, right thumb). The
record's `+0x18` is the thumbstick layout.

- The record is initialised by `0x218420` (Default table).
- `0x217c40` (the input phase, after `0x124c00` reads XInput) uses the slots:
  for each of the sixteen it copies the physical button's pressure and hold
  time into the action's slot. All gameplay reads the action slots, never the
  physical buttons.
- `0x190c34` (EAX = player) builds the record from the player's profile
  settings (at `0x54e9f4 + 0xc70 * player`: `+0` flags, `+4` button layout
  byte, `+5` thumbstick layout, `+6` look sensitivity) and copies it to
  `0x51ea18`. The layout byte is a three-way switch: 1 swaps the trigger
  slots, 2 is Boxer, 3 is Green Thumb, and any other value (0, or 4 and up)
  falls through unchanged, which is Default. The thumbstick byte is clamped
  to 3 in the same function; the layout byte is not clamped.

So a layout is data: sixteen bytes. Adding one means adding a case here.

### The screen

`ui\screens\game_shell\settings_screen\player_profile\button_settings`
(screen id 40, `h2 ui`). It has eight panes: four layouts, then the same four
again for the Legacy thumbstick diagram (pane base 4, set from the thumbstick
layout by `0x2c65bf`). Each pane has a help text, a list, a diagram bitmap and
twelve text labels placed at the diagram's button lines, which is how a layout
is "drawn". The list rows are not in the tag. Code fills them:

| Address | Role |
|---|---|
| `0x2c647d` | screen constructor. Pushes the list size 4 and the data array name for the list |
| `0x1a480d` | datum array constructor the list's item array goes through (name pointer `[esp+4]`, count `[esp+8]`) |
| `0x2c6512` | per-row text: row index (word at `[widget+0x70]`) to the string id of Default, Southpaw, Boxer, Green Thumb; anything else gives the null id |
| `0x2c6567` | row chosen: writes the layout byte to the edit buffer global `0x54e6d8` (0 to 3, anything else is 0), then refreshes |
| `0x2b463d` | initial row from `0x54e6d8` (0 to 3, else 0) |
| `0x2c65bf` | pane base from the thumbstick layout |
| `0x24a978` | Controls row value text on the Edit Profile screen: layout byte at profile `+0x104` to a string id |
| `0x2c866d` | the same on the profile summary and Controls list (from `0x54e6d8`) |
| `0x253b1a` | set a text widget's string id (EDI = widget, id on the stack) |
| `0x1a02d0` | `locale_string_id_find`, which resolves every string id (`docs/engine/locale_string_id_find.md`) |

## What the in-game entry does (`layout_glue.rs`)

Every change is at a function entry, because the game is statically
recompiled: `rt.edit_dispatch(|d| d.register_override(addr, name, closure))`
replaces a function at its entry, `rt.resolve_original(addr)` runs the
generated original, and the shadow-page pattern in `d3d_glue.rs` runs the original and then continues in Rust through
`ctx.call_guest`. There is no way to patch inside a function, so a site that
is a switch inside a big function needs the whole function wrapped.

1. List size: wrap `0x1a480d`. Its arguments are the array's name pointer at
   `[esp+4]` and the element size at `[esp+8]`; the count is **EAX** (it
   reaches `0x16b570` in EAX, which multiplies it by the element size and
   creates that many datums). When the name is the button list's (the address
   `0x45ce18`) and EAX is 4, make EAX 4 plus the number of added layouts. The
   list then builds that many rows. The screen constructor pushes element
   size 4 and loads count 4, so the two look alike. The hook sets EAX, not
   `[esp+8]` (`layout_glue::grown_count`, test
   `the_list_count_is_eax_not_the_element_size`).
2. Row text: replace `0x2c6512`. For rows 4 and up call `0x22edb8` (this =
   widget, args 0, 0, 6) and `0x253b1a` with our string id, through
   `call_guest`; otherwise run the original.
3. Selection: wrap `0x2c6567` (thiscall, `ret 8`; the pointer to the chosen
   row's word is the **second** argument, `[esp+8]`, read by
   `layout_glue::chosen_row`, test `the_chosen_row_is_the_second_argument`).
   Run the original, then write the chosen number to `0x54e6d8` ourselves
   (the original writes 0 for rows 4 and up).
4. Initial row: wrap `0x2b463d` the same way so a saved layout 4 and up
   selects its own row.
5. Value text on the other screens: `0x24a978` and `0x2c866d` are inside
   larger functions, so wrap the string id consumer `0x253b1a` for those, or
   wrap the containing functions. Needs a decision after tracing the callers.
6. Strings: wrap `0x1a02d0`. Our own ids (a private range that cannot collide
   with the 24-bit index of any real id) return the address of our UTF-8 text in
   a guest page we map once. Strings are English only at first, which the game
   renders as it does any other; the other eight languages need their own text
   and the fonts' glyph coverage (`docs/formats/fonts.md`, `docs/formats/localization.md`).
7. The action table: wrap `0x190c34`. Run the original (an unknown layout is
   Default), then overwrite the sixteen slots with `Layout::action_map` when
   the layout byte is 4 or more.
8. Panes: the tag has eight panes, so rows 4 and up would show panes meant for
   the Legacy diagram. Add panes. Clone an existing layout pane in guest
   memory and reassign its label string ids to the layout's labels (the labels
   exist already: jump, melee, reload, switch grenades, flashlight), give it our
   help-text id, then point the screen's pane block at the larger array. Pane
   and block layouts are in `docs/formats/ui.md`. Patch at first use of screen 40 by
   wrapping the screen finder `0x148098`; do it once.
9. Persistence (read from the XBE, not seen in a run).
   The Controls edit buffer is the whole 0x1e0-byte settings block at
   `0x54e5d8` (0x78 dwords): `0x54e6d4` is its `+0xfc` flags word (the
   toggles' bits) and `0x54e6d8` its `+0x100` layout byte. `0x148ad8` fills
   it from a profile (player index at `0x54e5d0`, profile handle at
   `0x54e5d4`). The commit is `0x148b8c`, called from each row handler when
   the screen applies at once (`[handler+0x188]`/`+0x288`) and from the
   close path `0x148bff`. For a player 0 to 3 signed in with that profile it
   calls `0x18fd20(player, 0x54e5d8, profile)`, which, when the block
   differs, runs `0x18fb34`: a `rep movsd` of the 0x78 dwords into the
   player's block (`0x54e8e0 + player * 0xc70 + 0x18`, so the layout byte
   lands at `+0x118 = 0x54e9f8`, our `SETTINGS_BASE + 4`) and then
   `0x190c34` with EAX = player, the action table build we hook. It then
   calls `0x2153dd(profile, 0x54e5d8, player, 0)`, which copies the same
   0x78 dwords to the profile record at `+0x114` and marks it for saving
   (`0x2154cd`, `0x2154b4`). With no signed-in player for that profile, the
   commit calls `0x2153dd` directly. The byte is copied verbatim each
   step; nothing range-checks it. The profile save and load copy the
   record's settings block whole (`0x215367`; the signature at `0x212f10`
   covers the blob). So a layout byte of 4 to 6 reaches the live player's
   table at once and the saved profile on the next save.
10. Retail behaviour of 0 to 3: every hook above must pass those through to the
    original, and a test must prove it.

## What the hooks guard against

Each hook acts only when what it relies on is there, so a screen or list
that is not the one we grew keeps its retail behaviour.

- **Every hook resolves.** All the addresses above are function entries in
  our analysis. `0x2c6567`, `0x2b463d` and `0x24a886` are reached through
  vtables, not direct calls (`0x2c64d4` stores `0x2c6567` as the list's
  press handler; `0x45b88c` and `0x45b0c4` hold the other two), and calls
  to all of them go through the dispatcher, so the overrides are reached.
  A missing generated entry prints `layouts: no generated entry at <addr>;
  hook skipped` (`layout_glue::original`).
- **Seven rows fit.** The list embeds only four item widgets (the vector
  constructor `0x20670` at `0x2c64a3`), but each item holds a datum handle
  at `+0x70` and steps to the next or previous datum (`0x16bcc0`,
  `0x16bbc0`, from `0x24c63e`), so the extra rows are reached by scrolling.
- **Pane** (`0x230427`): a second screen also has id 40, but only the
  button-layout tag has the 8 panes and the 13-text default pane that
  `extend_panes` needs. For an added row on screen 40 the hook
  first calls the screen-tag function (`0x22f871`, EAX = screen; our hook on
  it extends the tag on first sight) and moves the row's pane only when
  that tag has 14 panes (`pane_row(row, base, panes)`). Otherwise the row
  keeps its retail pane, and stderr says once `layouts: pane for row N left
  retail (tag has M panes)`. A tag the screen-tag hook could not extend
  also says once how many panes it has. Test
  `panes_are_only_moved_on_a_tag_we_extended`.
- **Row text, row chosen, initial row** (`0x2c6512`, `0x2c6567`,
  `0x2b463d`): only for a row below the list's real datum count
  (`[handler+0x70]`, the count at `+0x20`, which the constructors loop
  over; `row_listed`). The row text and row chosen are thiscall methods of
  the list handler (the handler's vtable `0x45cdc0` holds `0x2c6512` in slot
  20, and its press record at `+0x28c` points at `0x2c6567`); the initial
  row is the screen's, with its list handler at `this + 0x614` (what it
  hands `0x24c591`). A list that did not grow keeps its retail behaviour.
  Test `row_hooks_act_only_on_rows_the_list_has`.
- **Action table** (`0x190c34`): only for players 0 to 3. The settings
  blocks and tables hold four, and the commit path checks the same range
  (`0x148b8c`). Test `the_action_table_is_left_alone_past_the_fourth_player`.

Each hook prints `layouts: <name> active` the first time it runs, and the
list hook prints `layouts: list 0x45ce18 grows from 4 to 7 rows` once. A
run that opens Settings, Controls, Button Layout prints the list, row text,
pane and screen tag lines; a missing line names the hook that did not run.

## Risks

- The profile is a signed save. A profile saved with layout 4 and up and then
  loaded in a build without our hooks plays as Default (the game's own
  fall-through), which is safe. Whether the game rejects it first is item 9.
- The Edit Profile and summary screens (item 5) show a blank label for an
  unknown byte until they are covered.
- Wrapping `0x253b1a` and `0x1a02d0` is on a hot path; the wrappers must test
  the cheap condition first.

## Tests

`crates/input/src/layouts.rs`: the layout tables (retail diffs against
Default, the three added layouts, one button per action, fixed menu
buttons, the 16-slot encoding, preset numbers).

`crates/boot/src/layout_glue.rs`, over a fake guest: only rows 4 to 6 are
ours; only the layout list grows, through EAX and not the element size; the
chosen row is the second argument; for every layout byte 0 to 255 the action
table is rewritten for 4 to 6 and left exactly as the original made it
otherwise (so retail 0 to 3 give the original table); pane rows leave retail
rows alone and land ours on panes 8 to 13; our string ids do not collide; the
strings are written NUL-terminated; relabelling moves each label to the slot
at its new button; the pane array grows from 8 to 14 once, leaves the eight
retail panes byte for byte, and ignores other screens and odd tags; panes
move only on a tag with 14 panes, and every pane handed out exists; the row
hooks act only on rows the list's datum array has (a four-row list leaves
4 to 6 alone); the action table is left alone for a player past 3; choosing
the speaker row and the Video screen's rows saves them all to `<data>/settings.toml`; and
`install` returns on a synthetic runtime (`resolve_original` must run before
`edit_dispatch`, which holds the dispatch lock).

## Not verified

Only a game run shows these:

1. Settings, a profile, Controls, Button Layout: seven rows, the last three
   "Bumper Jumper", "Recon", "Walkie Talkie" (the list has five visible
   slots in the tag; check it scrolls).
2. Highlighting each added row shows its diagram with the labels moved (for
   Bumper Jumper, Jump beside White, Melee beside Black, Swap Grenades beside A,
   Reload beside B, Flashlight beside X) and its help text; both thumbstick
   diagram variants (Default and Legacy sticks).
3. Choosing Bumper Jumper, then playing: left bumper (White) jumps, right
   bumper (Black) melees, A swaps grenades, B reloads, X is the flashlight.
4. The Controls list and the Edit Profile screen show the layout's name, not
   an empty value.
5. Reopening the screen with the added layout set highlights its row.
6. Save, quit, reload: the layout is kept (needs a named profile; the Default
   profile cannot save). The profile select dialog's summary has not been
   traced for a layout byte of 4 and up; check it does not show garbage or crash.
7. Retail rows 0 to 3 still behave as before.
8. The hooks install with no stall at boot.

## Known gaps

- Strings are English only.
- Which of the retail text widgets the Controls and Edit Profile screens
  fill with an empty id is inferred; the value-text hook substitutes the
  first empty text while the function runs.
- The guest addresses we map for our data (`DATA_BASE` `0xa2f00000`,
  `SHADOW_BASE` `0xa2f10000`, `EXTRA_BASE` `0xa2f20000` in `layout_glue.rs`)
  are picked to stay clear of the other boot mappings.
