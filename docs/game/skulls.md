# Campaign skulls per profile

Status: Built. Pickup tracking, per-profile storage and the campaign-start apply (`boot::skulls`, the settings store's per-profile values) are seen working. The Skulls page between the difficulty and the level (`boot::skulls_screen`, `boot::skulls_menu`, our own list class with the level select's layout, "Menu" below) is tested offline; "Not verified" lists what only a run shows.
Code: `crates/extensions/game/src/skulls.rs`, `skulls_screen.rs`, `skulls_menu.rs`, `skulls_mp.rs`, `skulls_custom.rs`, `skulls_model.rs` (and `skulls_model/gpu.rs`), `skull_glyphs.rs`, `crates/rewrites/native-render/src/showcase.rs`, `showcase_gpu.rs`. Tests: in those files and `crates/rewrites/native-render/tests/showcase_gpu.rs`.
Switch: feature ids `skulls`, `skulls-page`, `skulls-screen`, `skulls-mp`, `skulls-mp-row`, `skulls-mp-screen`, `skulls-custom`, `skulls-model` (`H2_FEATURES=<id>=off`), and `H2_SKULLS_REVEAL`; listed in [switches.md](../runtime/switches.md).

The ask: a skull a player has picked up is remembered on their profile, and
when they start a new campaign (alone or in co-op) they can pick which of
their collected skulls to turn on.

Labels: **confirmed** means read from the retail XBE's instructions or the
disc's scripts with this repo's tools (`h2 disasm`, `h2 hs`, `h2 strings`,
an absolute-reference scan of the XBE); **likely** means strong evidence but
not every path read; **community** means public knowledge, cited.

## What the game does

Community: Halo 2's skulls are hidden objects in campaign levels; picking
one up turns its effect on, and it stays on until the console is reset
([Halopedia, Skulls](https://www.halopedia.org/Skull)). Internally Halo 2
calls them "ice cream flavors" (the HaloScript builtins below).

| Address | What | Confidence |
|---|---|---|
| `0x2ac230` | evaluator of builtin 876 `ice_cream_flavor_stock (long)`: evaluates the index, calls `0x1df3e0` with it in ESI | confirmed (`h2 hs --builtins`, `h2 disasm`) |
| `0x2ac270` | evaluator of builtin 877 `ice_cream_flavor_available (long) -> boolean`: index in `0..15`, active options mode (`[0x4e6948]+8`) is 1 (campaign), then the byte at `0x4f55dc + n` | confirmed |
| `0x1df3e0` | stock skull ESI (plain `ret`): only for `0 <= n < 15` and when byte `n` is 0, sets it to 1, then shows the HUD message (string id from the table at `0x445514`) and plays the sound | confirmed |
| `0x4f55dc` | the skull array: 15 bytes, one per skull, 1 = on | confirmed |
| writers of `0x4f55dc..0x4f55ea` | only `0x1df3f4` (inside `0x1df3e0`); every other reference reads one byte (`0xc50fd`, `0xc579d`, `0xca13f`, ...) | confirmed (absolute-reference scan of the XBE; register-relative writes elsewhere are not ruled out, but none use this base) |
| skull reset | none found: the array is never cleared, so a skull stays on for the boot (matches the community account) | likely |

The pickup itself is scripted. Every campaign level that has a skull runs a
dormant script that creates the skull object, waits until
`(unit_has_weapon (unit (player0)) ...)` or the same for player1 holds the
skull weapon, then calls `(ice_cream_flavor_stock n)` (confirmed, `h2 hs` on
the 14 campaign maps). `0x1df3e0` has one caller, the builtin, so hooking it
sees every pickup.

### Where each skull is

The index is what the scripts stock. The names are the game's own HUD names
for each index (`unlocked_extra_1` ... `unlocked_extra_14` in shared.map's
English strings, `h2 strings shared`, through the table at `0x445514`);
index 14 has no entry there (the table holds 0). The level is the one whose
script runs `(ice_cream_flavor_stock n)`; "Legendary" says whether that
script, or the one that wakes it, tests `(difficulty_legendary)` (each map
defines it as `(= (game_difficulty_get) legendary)`) before it creates the
skull. All read with `h2 hs <map>` on the 14 campaign maps.

| n | Name | Level (map) | Legendary only | The test |
|---|---|---|---|---|
| 0 | Envy | Delta Halo (`05a_deltaapproach`) | yes | the waker: `(if (difficulty_legendary) (wake arbiter_envy))` |
| 1 | Grunt Birthday Party | The Arbiter (`04a_gasgiant`) | yes | the waker: `dogfight_firsttime_start` wakes `grunt_birthday_party` only on Legendary |
| 2 | Assassins | Regret (`05b_deltatowers`) | yes | the waker wakes `assassin_ice_cream` only on Legendary |
| 3 | Thunderstorm | Cairo Station (`01b_spacestation`) | yes | `ice_cream_check` sleeps forever unless Legendary |
| 4 | Famine | The Oracle (`04b_floodlab`) | yes | the waker wakes `famine_flavor` only on Legendary |
| 5 | I Would Have Been Your Daddy | Outskirts (`03a_oldmombasa`) | yes | `e5b_main` sleeps forever in co-op, off Legendary, or unless `(random_range 0 7)` is 0: solo only, one start in seven |
| 6 | Blind | Outskirts (`03a_oldmombasa`) | **no** | `e1_key` is woken by `e1_main` from `mission_start` with no difficulty test: the one skull on any difficulty |
| 7 | Ghost | Uprising (`08a_deltacliffs`) | yes | `e10_key` sleeps forever unless Legendary |
| 8 | Black Eye | The Great Journey (`08b_deltacontrol`) | yes | `e11_key` sleeps forever unless Legendary |
| 9 | Catch | Metropolis (`03b_newmombasa`) | yes | `e18_key` sleeps forever unless Legendary |
| 10 | Sputnik | Quarantine Zone (`06b_floodzone`) | yes | the waker wakes `ice_cream_superman` only on Legendary |
| 11 | Iron | High Charity (`07b_forerunnership`) | yes | `e10_key` sleeps forever unless Legendary |
| 12 | Mythic | Sacred Icon (`06a_sentinelwalls`) | yes | the waker wakes `ice_cream_mythic` only on Legendary |
| 13 | Angry | Gravemind (`07a_highcharity`) | yes | the waker wakes `ice_cream_angry` only on Legendary |
| 14 | Bandana (community name; no HUD name) | The Armory (`01a_tutorial`) | yes | `icecream` waits for `(and mark_ice_cream (difficulty_legendary))`; `mark_ice_cream` is set once Johnson has asked the player onto the lift several times |

Level names are the shipped titles of the map files; the scripts above are
the evidence (Ghost is Uprising's, Iron is High Charity's). Where community lists
put a skull on another level, trust the index: it is what the engine stores.
The menu's copy of this is `skulls::ABOUT`; its effect lines are our own
words, not the game's (the game has no description text for skulls).

### Who is playing

| Address | What | Confidence |
|---|---|---|
| `0x54e8e0` | local player slots, 4 x `0xc70` | likely ([live-gamestate.md](../live/live-gamestate.md)) |
| `slot + 0`, bit 4 | signed in with a profile | confirmed (`0x1902c1`, `0x23654b` test it) |
| `slot + 0xc1c` | the profile's name, 32 UTF-16 units | confirmed: `0x1902c1` returns it for a signed-in slot, and campaign start (`0x23654b` → `0x18fd94`) copies it (31 units and a NUL, `0x13fac0`) into each player |
| `0x23643f` | the menu's campaign start: options mode 1 from the selection globals, one player per signed-in slot, `+0x12c` = more than one player (co-op), then `0x18e790` | confirmed ([level-boot.md](../runtime/level-boot.md)) |
| `0x18e790` | queue options (EAX): called by `0x23643f` (campaign), `0x18f244` (main menu) and `0x18ea83` | confirmed |

So "new campaign" and "new co-op campaign" are the same start: co-op is
the same path with two signed-in profiles.

## What we do

Everything below is the `skulls` feature on the feature plan
([feature-engine.md](../engine/feature-engine.md)), on three points: `SKULL_STOCK`
(`0x1df3e0`), `OPTIONS_QUEUE` (`0x18e790`) and the tick. Being on the plan,
its hooks chain with any other feature's at the same address, and a clash
refuses the plan at boot instead of one hook silently replacing another.

- **Pickup** (`before` and `after` on `0x1df3e0`): before, note skull ESI
  and its byte, and log it; the game's function runs; after, every skull
  that came on since the last look is recorded as collected and on for
  every signed-in profile. In co-op both players get it: the script does
  not say who held the skull, and the game turns it on for both.
- **Who it is for.** A signed-in slot with a name (the table above). When
  no slot has one (the slot test fails, nobody is signed in, the name is
  empty), the pickup is kept under the profile `default` instead of being
  dropped, and the log line shows each slot's flags and name as read. The
  menu uses the same fallback for a player with no named profile, so a
  pickup kept that way still gets the Skulls row.
- **Tick watch** (an observer after `game_tick`): only the stock function
  and our campaign start write the array, and the start says what it
  wrote, so any skull that comes on without passing through `0x1df3e0` is
  still recorded (logged as `turned on without the pickup hook`).
- **Storage**: one setting per skull (`skulls.envy` ... `skulls.bandana`,
  bool, tier Advanced, menu screen `skulls`) in the settings store, kept
  per profile:

  ```toml
  [profiles.Chief.skulls]
  mythic = true     # collected, on at the next campaign start
  iron = false      # collected, off
  ```

  A profile without a key has not collected that skull. This is in
  `settings.toml` in the data root, next to every other setting, and never
  in the signed Xbox profile ([feature-engine.md](../engine/feature-engine.md)).
  The run's own value of a `skulls.*` key is not used.
  The store change is the narrowest extension the settings crate needed:
  `Store::profile_get`, `profile_set`, `profile_values`, `profiles`, and
  `settings::write_profile_value` (atomic, keeps every other key, never
  overwrites a file that is not TOML). A profile is its in-game name; a
  later Live sync can map a name to an account and merge the per-profile
  tables (collected is a set: merge by union).
- **Campaign start** (`before` on `0x18e790`, ordered last so it runs next
  to the queue): for options with mode 1, write the array as exactly the
  union of the signed-in profiles' "on" skulls (the `default` profile's
  with no named profile), then queue the options. A skull the game turned
  on earlier in the boot but not chosen is turned off. `H2_FEATURES`
  switching `skulls` off leaves both out. Headless runs (no
  settings store) have neither.
  For an editor `map.reload`, the hook stages the selection on the
  requesting ticket. The selection takes effect when retail's pending
  options match the request, before the transition consumes them.

### What it logs

Every line starts `skulls:`. At boot, after the feature plan installs:

- `skulls: hooks installed via feature plan at 0x1df3e0 (pickup),
  0x18e790 (campaign start), 0x137fe0 (tick watch); saving to <path>/settings.toml`
- or `skulls: NOT installed: <why>; pickups are not kept`, or
  `skulls: off: <why>` (no store, `H2_FEATURES`).

Each pickup:

- `skulls: pickup call for skull 12 (Mythic), was off`
- then `skulls: Mythic kept for <profile>, saved to <path>`, or
  `... this run only; not saved: <why>`, or
  `skulls: pickup of skull 12 (Mythic) not kept: <why>` (already on,
  left off by the game, no such index, array unreadable)
- with no named profile, first `skulls: no named profile signed in (slot 0
  flags 0x.. name "..", ...); using profile "default"`.

Each campaign start: `skulls: campaign start for <profiles>: <skulls on>`.
Each time the difficulty list is built: `skulls menu: difficulty list:
Skulls row shown; <profile> <n> of 15 collected, ...`.
The plan's own `feature: SKULL_STOCK before skulls:note the skull active`
lines say each hook ran the first time.

## Menu

**The flow** is level select (or New Campaign) → difficulty → the Skulls
page → the level. The page comes when the pressing player's
profile (the one signed in on that player's slot, else `default`) has
collected at least one skull, retail's or ours, or when the level is a
Halo 1 level (its Halo 1 options, `boot::h1_options::CAMPAIGN`); with
neither, the difficulty starts the level as retail does. There is no
Skulls row on the difficulty list: the list is retail's four rows.

### The page

The campaign page (`boot::skulls_screen`, feature `skulls-page` for the
hook and `skulls-screen` for the screen's class) and the multiplayer
screen ("Choosing them" below) are one design, on the level select's
layout, from the design system's parts (`skulls_menu::SkullsScreen`,
`ui_kit::components`, docs/game/ui-builder.md "The design system"):

- **Title**: "SKULLS" on both, in capitals.
- **Rows, in groups by order**: Start Level first, focused when the page
  opens; then the level's options (on a Halo 1 level, "Assets: Halo 1"
  and "HUD: Halo 1", [Halo 1 options](../halo1/multiplayer.md)); then
  the skulls, gold first ("Gold and silver" below), then our six ("Our
  skulls"). Retail's lists have no header or separator rows, so the
  order alone groups them. The multiplayer screen has only its skulls.
- **A collected skull** is its name and its value, "Mythic: On" or
  "Mythic: Off" (`components::row::pick` on the level select's skin,
  which has one text a row, so the value is in the label; the design
  system's `row.pick-inline`).
- **A locked skull** is its name alone, "Thunderstorm"
  (`components::row::disabled`): Locked is not a value, so it is never
  shown where On and Off go. Our list class has no per-row colour, so
  the row is drawn as its neighbours are; the empty value, its help and
  its legend say it is locked.
- **A** on Start Level starts the level on the chosen difficulty with
  the selected options and skulls. A on an option or a collected skull
  opens the value picker (`ui_kit::picker`, retail's
  `low_level_settings`, as retail's settings do for their two-value
  settings): On and Off, the held one focused; A there saves the choice
  to the profile at once, B leaves it (`SkullsScreen::pick`). A on a
  locked skull does nothing. Left and right do nothing on either page
  (the class takes no sideways steps). B goes back to the difficulty
  list. The class reserves the start action for A on Start Level
  (`ListClass::action_buttons(&[])`).
- **Legend**, in the footer, following the focus
  (`skulls_menu::RowLegend`, `SkullsScreen::focused_footer`), the same
  on both pages: `A START  B BACK` on Start Level, `A CHANGE  B BACK` on
  an option or a collected skull, `B BACK` on a locked skull.
- **Help**, under the picture: what the focused row does, then on a line
  of its own where it comes from. A collected skull: "Your HUD and
  first-person weapon are hidden." then "Gold skull, found on
  Outskirts.". A locked one: the same first line, then "Locked: find it
  on The Arbiter on Legendary." ("on Legendary" when only a Legendary
  game places it, then any note its script adds). IWHBYD's first line
  starts with its full name. An option's help gives both choices; Start
  Level's says what it does. The multiplayer screen adds "The host's
  choice is everyone's." to the first line.
- **Right column**: the picture, and under it the help. With the
  native renderer on, the picture is the focused skull's own model
  picture ("The model picture" below): Halo 2's skull model from the disc,
  lit and swaying, gold or silver bone, with that skull's glyph on its
  forehead, or a dark silhouette with no glyph while it is locked. Without
  the native renderer, with `skulls-model` off, or until the model has
  loaded, it is the difficulty screen's skull bitmap, tinted gold or
  silver for the tier and dimmed (to 60%) while locked.
- **Height**: the list lays out 13 rows (`skulls_menu::VISIBLE_ROWS`) from
  the level select's anchor down to the footer band, and scrolls for the
  rest. The Controls handler embeds only six item widgets, so the tag copy
  writes the layout's count (`ListLayout::visible`) and the class's list
  gets the rows past six as item widgets of ours, linked by the "add items" hook
  (`list_class::extra_items`, `layout_glue`), as the Controls screen's own
  added rows are.
- **Going back**: the picture and the description fade with the page
  (the model picture takes the picture widget's alpha).
  They carry the level select's own animations (as its tag stores them:
  picture 13, description 11, `skulls_menu::PICTURE_ANIMATION`,
  `DESCRIPTION_ANIMATION`; checked against the disc).

**Going on** (`skulls_screen::action`, `Continue::for_press`):

- From the level select or New Campaign (the difficulty list's mode 0),
  Start Level runs the kept difficulty press at once, from the page's own
  row press, with the page still up. The press pushes the loading screen
  onto window 5 slot 4 over the page, as it would have over the difficulty
  screen: the level loads straight away. With no close there is nothing to
  race.
- The difficulty list is not there while the page is up. At Start Level
  the kept list's address holds vtable `0x458788`, not the difficulty
  list's `0x45d688`, because the stack frees the screen under a new one and
  builds it again when it comes back. The press cannot rely on the list. A mode 0 press reads its list only for the mode byte
  (`0x2c9d79`; from `0x2c9d9b` on, `this` is never read), so with the
  list gone the press runs on a stand-in: the byte at `+0x2a0` in our
  scratch page (`skulls_screen::write_stand_in`, `Target::StandIn`). A
  mode 1 press needs the real list (`0x22eeee` finds the screen's root
  through it) and is still refused without one.
- From a lobby's Game Setup (mode 1, `0x2bb945`), the press only stores the
  lobby's difficulty and animates the difficulty screen out; with the page
  on top that would leave the page up over the lobby. So there the page
  closes as B does and the press runs once it is freed.

How the flow is made (static reading):

| Address | What | Confidence |
|---|---|---|
| `0x2bb3ed`, `0x2bb432` | the difficulty screen's two loaders (`+0x8b4` = 0 New Campaign, 1 Select Level), screen id 12, constructor `0x2bb477`, vtable `0x45c588` | confirmed |
| `0x2c9c12` | its list handler constructor (at `screen + 0x614`): vtable `0x45d688`, "difficulty list" (`0x45d6e0`) of 4 rows | confirmed |
| `0x2c9d38` | row chosen (thiscall on the list, the player chain and a pointer to the row, `ret 8`): clamps the row to 0..3 and sets the difficulty (`0x148d42`). With `list + 0x2a0` (the screen's `+0x8b4`) clear it stores it (`0x121060`, `0x510994`) and opens the loading screen (`0x2b1467`, screen 209) on window 5 slot 4 for `1 << [[chain]]+4`; set, it stores it at `0x510998` (`0x1210c0`) and plays the difficulty screen's out transition (`0x22eeee` finds the root, `0x22e957(.., 3)`) | confirmed |
| `0x2300ea` | the generic screen event handler (screen slot 10): event kind 5 (a button) with button 1 (B) or 13 (Back) closes the screen with `0x14800c(eax = [screen+0x78], ecx = [screen+0x74])`; anything else goes to the children (`0x22ec73`) | confirmed |
| `0x14800c` | close the top screen of a window's stack: the stack's slot `+0x1c`, which starts the screen's out transition (`0x22e957(.., 2)`; it is freed when that ends), then the back sound | confirmed (window 2 read; window 5's stack assumed alike) |

- A `before` hook on `0x2c9d38` (`skulls_screen::DifficultyPage`) reads
  the row, the pressing player and its profile, and asks
  `skulls_screen::PageFlow`. For rows 0 to 3 with a skull collected it
  keeps the press (the list, the row, the player, the list's mode byte)
  in the flow, logs `skulls page: kept difficulty <n> (<name>) for level
  <...>, player <p> (<profile>), list 0x...`, and opens the page with
  `0x149f1e(1 << player, loader)`, never running the original.
  Otherwise the original runs. The kept press is only replaced by the
  next difficulty press and only dropped once it has run.
- Start Level (`skulls_screen::action`): A on that row is the row's
  press, which for it is the class's action (`Chosen::Act`). Our class's
  screen vtable has its own slot 10 (`list_class::event`); the page's
  class takes no sideways steps, so every event, left and right, X and
  Start included, goes to `0x2300ea`, and nothing takes left and right.

### The press that opened it

- The events (static reading): the controller update `0x1907d6` raises,
  per local player, one button event a frame (kind 5, `+4` the player,
  `+8` the button: 0 A, 1 B, 2 X, 3 Y, 4 black, 5 white, 12 Start,
  13 Back, 6 and 7 the triggers, 14 and 15 the stick clicks) only on the
  frame the button goes down (its held count is 1), so holding A raises
  nothing more. Directions come through `0x190a3d`: the left stick past
  `0x7332` either way, else the d-pad (button word 8 up, 9 down, 10 left,
  11 right; the stick's is 16), as kind 1 up, 2 left, 3 down, 4 right,
  when pushed and then every 250 ms while held. They reach the top
  screen of the player's window through `0x147dbe` and `0x23515d` (screen
  slot 10). The Controls list's handler (`0x24c024`) moves the focus on
  1 and 3, takes button 0 or 12 as its row's press, and hands 2 and 4 on
  to its children, where nothing takes them.
- The press that opened the page (A on the difficulty list) is handled
  by that list, and the dispatch stops there; it is not seen again. As
  a guard all the same, a continue within 50 ms of the page opening
  (`skulls_screen::GUARD`) is
  dropped: `skulls page: Start Level <n> ms after the page opened: the
  press that opened it; ignored`. Two presses of a thumb are further
  apart than that, so A spammed goes straight through.
- The page freed (`skulls_screen::freed`): our class's slot 0
  (`list_class::destructor`) runs the Controls destructor, then tells the
  flow. The stack calls slot 0 when the closed screen's out transition
  has ended (`0x235233`), so the difficulty screen is the top one again.
  For a page Start Level closed (mode 1), `skulls_screen::continue_level` checks
  the kept list is still a difficulty list (vtable `0x45d688`), writes a
  player chain and the row to a page of ours, and calls `0x2c9d38` on the
  kept list; the hook lets that one call through. So the start is
  retail's own, from either loader, and never runs into a closing screen.
- Around the kept press `continue_level` logs window 5 slot 4's screens
  (`skulls page: before the press: window 5 slot 4: active .., pending ..,
  list [screen/id ..]`), and after it whether the loading screen (`0xd1`)
  is among them, so a Start Level that loads nothing says whether the
  press opened its loading screen (`boot::ui_watch::stack`).

### Layout

**The layout** is the campaign level select's
(`ui\screens\game_shell\load_level\load_level`, screen 11, mainmenu.map;
`h2 ui` prints it): its list on the left, its picture box on the right
and its description under the picture.

| Part | Level select (retail) | The page |
|---|---|---|
| list | anchor (-215, 130), skin 0 (`ui\list_skins\default\default`), 16 visible items | same anchor and skin, 13 visible items |
| row label | font 1, `[28 20 8 210]` relative to the item | first label at `[158 -195 138 -5]`, with 190 units of width for the name and state |
| picture | `[177 39 -30 259]` (220 by 207) | the difficulty screen's Legendary skull (`difficulty_options` image 3, 256 by 256, drawn at its size), top-left (36, 180): its visible part (17..209 across, 3..208 down) sits centred in that box, its top on the box's top |
| description | `[-30 39 -133 260]`, font 1, colour (99, 148, 193) | the same |
| frame | `ul_02` top left (`[177 -259 -79 -131]`, its pane 0 bitmap 1) | the same, copied from the level select at run time (`skulls_menu::FRAME`, `ListClass::frame`). None of the Controls tag's frame art is kept: its bracket (`ul_03`, `[170 -280 42 -24]`) was made for the Controls list 39 units further left on skin 2, and under this list it framed an empty box and the first row's end cap cut into it (found by the UI builder's frame check, docs/game/ui-builder.md); its bottom-right piece (`br_06`) is where the description goes |

Our screens are Controls screens (the Controls constructor builds them),
and the Controls tag puts a 508-wide two-column list across the screen.
The list reads its definition (`0x24bbf3` → the current pane
`0x22f899` → the screen's tag `0x22f871`) and lays its items out and
builds their texts and bitmaps from the skin (`0x24bc12`, `0x14837a`,
`0x2bacbc`) when the pane is set up, after our loader has adopted the
screen. So the screen-tag hook (`layout_glue`, `0x22f871`, EAX the
screen) hands a screen whose vtable is a class with a layout
(`list_class::ListLayout`) a copy of its tag (`list_class::copy_tag`,
in the class's table page, made once per retail tag). The copy has one
pane with our anchor, skin and visible count. The list block's other
fields match level select's flags, animation and item value. The copy
drops both Controls bitmaps. The pane builder's bitmap step (`0x22fa30`)
adds the picture from a copy of the difficulty screen's pane 3 bitmap.
Its text step (`0x22f9bf`) adds the description from our text block.

The list's anchor is `-215,130` on skin 0, the skull's top-left
`36,180` and the description `-30,39,-133,260` (`skulls_menu::LIST_LAYOUT`,
`PICTURE`, `DESCRIPTION_TEXT`).

The multiplayer screen uses the same layout, picture and description.
Video keeps the Controls list at (-254, 130) on skin 2, with separate
label and value columns. The Skulls page starts 39 units to its right. The
retail `ul_02` bracket extends under the left part of the list, as it
does on level select. The list's text ends at x = -5, with the picture
and description column starting at x = 39. The disc-backed layout test
measures every skull row label ("Mythic: On", "Mythic: Off", "Mythic")
in the retail row font.

- **Strings** are served by the locale hook: the titles, the legends,
  Start Level, the 15 + 6 names and collected helps, and the multiplayer
  helps from ids `0x21fff000` up (family `skulls`); each skull's "On" and
  "Off" labels and its locked help from `0x23fff000` (family `skull
  rows`, three per skull). `is_ours` reads only the middle bytes, so both
  are clear of the `0x20fff0xx` ids the other rows and the `ui` model
  take. The row names are the game's (IWHBYD shortened; its help gives
  the full name).
- `H2_FEATURES=skulls-screen=off` leaves the difficulty screen as retail
  (the page
  has no class, so the difficulty starts at once);
  `H2_FEATURES=skulls-page=off` removes the hook.
- **Co-op.** The page is for the pressing player's profile. The
  campaign start applies the union of every signed-in profile's On
  skulls. The game also has `difficulty_lobby` (screen 1,
  another list); if co-op reaches that one, it has no page (not checked
  in game; "Not verified").

## Where the sections live

| Page | Covers |
|---|---|
| [skulls-model.md](skulls-model.md) | the model picture and gold and silver skulls |

Each heading below is cited from code and moved to the page named after it.

### The model picture

Moved to [skulls-model.md](skulls-model.md).

## Gold and silver

Moved to [skulls-model.md](skulls-model.md).

## Pages

| Page | What |
|---|---|
| [Skulls: effects, multiplayer and Firefight](skulls-modes.md) | Where each effect lives, and how skulls work in multiplayer and Firefight. |
| [Skulls: our skulls](skulls-ours.md) | The skulls we add, and what is not verified. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Where each effect lives

See [Skulls: effects, multiplayer and Firefight](skulls-modes.md#where-each-effect-lives).

## In multiplayer

See [Skulls: effects, multiplayer and Firefight](skulls-modes.md#in-multiplayer).

### Why the Game Setup row broke

See [Skulls: effects, multiplayer and Firefight](skulls-modes.md#why-the-game-setup-row-broke).

## In Firefight

See [Skulls: effects, multiplayer and Firefight](skulls-modes.md#in-firefight).

## Our skulls

See [Skulls: our skulls](skulls-ours.md#our-skulls).

### Calls into the game

See [Skulls: our skulls](skulls-ours.md#calls-into-the-game).

## Not verified

See [Skulls: our skulls](skulls-ours.md#not-verified).
