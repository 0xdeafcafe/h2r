# Campaign skulls per profile

Status: built. Pickup tracking, per-profile storage and the
campaign-start apply (`boot::skulls`, the settings store's per-profile
values) have been seen working. The Skulls page between the difficulty
and the level (`boot::skulls_screen`, `boot::skulls_menu`, our own list
class with the level select's layout, "Menu" below) is built and tested
offline, not yet seen running; it replaces the Skulls row the difficulty
list had.

The ask: a skull a player has picked up is remembered on their profile, and
when they start a new campaign (alone or in co-op) they can pick which of
their collected skulls to turn on.

Labels: **confirmed** means read from the retail XBE's instructions or the
disc's scripts with this repo's tools (`h2 disasm`, `h2 hs`, `h2 strings`,
an absolute-reference scan of the XBE); **likely** means strong evidence but
not every path read; **community** means public knowledge, cited. Nothing
here has been seen running, apart from what the status lines above say was seen.

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

### The skulls by index (confirmed unless noted)

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

Level names are the shipped titles of the map files. An earlier version of
this table had 7 and 11 the wrong way round (Ghost is Uprising's, Iron is
High Charity's); the scripts above are the evidence. Where community lists
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
  on earlier in the boot but not chosen is turned off. `H2_NO_SKULLS=1`, or
  `H2_FEATURES` switching `skulls` off, leaves both out. Headless runs (no
  settings store) have neither.

### What it logs

Every line starts `skulls:`. At boot, after the feature plan installs:

- `skulls: hooks installed via feature plan at 0x1df3e0 (pickup),
  0x18e790 (campaign start), 0x137fe0 (tick watch); saving to <path>/settings.toml`
- or `skulls: NOT installed: <why>; pickups are not kept`, or
  `skulls: off: <why>` (no store, `H2_NO_SKULLS`, `H2_FEATURES`).

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

## Menu (built)

**The flow** is level select (or New Campaign) → difficulty → the Game
Options page → the level. The page comes when the pressing player's
profile (the one signed in on that player's slot, else `default`) has
collected at least one skull, retail's or ours, or when the level is a
Halo 1 level (its Halo 1 options, `boot::h1_options::CAMPAIGN`); with
neither, the difficulty starts the level as retail does. There is no
Skulls row on the difficulty list: the list is retail's four rows.

The page (`boot::skulls_screen`, feature `skulls-page` for the hook and
`skulls-screen` for the screen's class), titled "Game Options":

- **Rows**: "Start Level" first (focused when the page opens), then a
  Halo 1 level's options ("Assets: < Halo 1 >", "HUD: < Halo 1 >",
  docs/halo1/multiplayer.md "Halo 1 options"), then every skull, gold
  first ("Gold and silver" below), then our six ("Our skulls"), each one
  label with its state as a value to step: "Mythic: < On >", "Mythic: <
  Off >", and "Mythic: Locked". The list skin has one text per row, so the
  state is in the label. (The multiplayer screen keeps "Mythic: On" and A
  as its toggle.)
- **Buttons**, as the game's own lists: **A** selects the focused row. On
  Start Level it starts the level on the difficulty chosen, with the
  options and the skulls marked On; on an option or a skull it changes it.
  **Left or right** (d-pad or left stick) changes an option or a collected
  skull too (the store is written at once; either direction flips a
  two-value row). A locked skull does nothing and says why on stderr.
  **B** goes back to the difficulty list (the generic back handler, as on
  every screen of ours); choosing a difficulty there opens the page again.
  X and Start do nothing (`ListClass::action_buttons(&[])`): X going on to
  the level was a mystery to the player. A pressed over and over still
  goes from the level list through the difficulty and the page into the
  game, since Start Level is the first row.
- **Footer**: "◀  ▶ CHANGE  Ⓐ SELECT  Ⓑ BACK" in the font's button glyphs, the two arrows two spaces apart (side by side the font draws them as one diamond, and one narrow space left them touching).
- **Right column**: the skull picture, gold or silver for the focused
  skull's tier and dimmed (to 60%) while it is locked, and under it the
  description: for a skull its tier, what it does, where it is found and
  whether only on Legendary; for an option both choices; for Start Level
  what it does.
- **Height**: the list lays out 13 rows (`skulls_menu::VISIBLE_ROWS`) from
  the level select's anchor down to the footer band, and scrolls for the
  rest. It used to lay out the Controls tag's 6 (5 seen), a third of the
  page: our tag copy kept the Controls list block's visible count, and the
  Controls handler embeds only six item widgets. The copy now writes the
  layout's count (`ListLayout::visible`), and the class's list gets the
  rows past six as item widgets of ours, linked by the "add items" hook
  (`list_class::extra_items`, `layout_glue`), as the Controls screen's own
  added rows are.
- **Going back**: the picture and the description fade with the page.
  They had no animation (the bitmap block's was cleared, the text block's
  zero), so through the page's out transition they stayed fully drawn, the
  skull over the difficulty screen's own skull coming back in. They now
  carry the level select's own animations (as its tag stores them:
  picture 13, description 11, `skulls_menu::PICTURE_ANIMATION`,
  `DESCRIPTION_ANIMATION`; checked against the disc).

**Going on** (`skulls_screen::action`, `Continue::for_press`):

- From the level select or New Campaign (the difficulty list's mode 0),
  Start Level runs the kept difficulty press at once, from the page's own
  row press, with the page still up. The press pushes the loading screen
  onto window 5 slot 4 over the page, as it would have over the difficulty
  screen: the level loads straight away. The first builds closed the page
  and ran the press once the stack had freed it: the page animated out and
  the difficulty screen came back before the level loaded, and a Start
  pressed while the page was still opening went back and never loaded (the
  press then ran from inside the stack's free of the page while the stack
  was still settling its top screen). With no close there is nothing to
  race. Closing and pressing at once (the very first build) put the
  loading screen over a page still closing, and the close took it down.
- From a lobby's Game Setup (mode 1, `0x2bb945`), the press only stores the
  lobby's difficulty and animates the difficulty screen out; with the page
  on top that would leave the page up over the lobby. So there the page
  closes as B does and the press runs once it is freed, as before.

How the flow is made (static reading, not seen running):

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
  screen vtable has its own slot 10 (`list_class::event`): kinds 2 and 4
  (left and right) step the focused row (`0x24c447` on the list, then
  `Rows::sideways`); everything else, X and Start included, goes to
  `0x2300ea`.
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
  (`skulls_screen::GUARD`, `H2_SKULLS_GUARD_MS=n` to change it) is
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
- `H2_SKULLS_CONTINUE=freed` or `=stay` forces either order, for trying
  in game only.

**The layout** is the campaign level select's
(`ui\screens\game_shell\load_level\load_level`, screen 11, mainmenu.map;
`h2 ui` prints it): its list on the left, its picture box on the right
and its description under the picture.

| Part | Level select (retail) | The page |
|---|---|---|
| list | anchor (-215, 130), skin 0 (`ui\list_skins\default\default`: one text `[28 20 8 210]`, row background 200 wide) | the same |
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
in the class's table page, made once per retail tag): the root, one
pane, the list block with our anchor and skin, and only the bitmaps
whose left edge is left of the centre. The retail Controls tag is not
written. The picture and the description are widgets of ours, as
before: the pane builder's bitmap step (`0x22fa30`) on a copy of the
difficulty screen's pane 3 bitmap, and its text step (`0x22f9bf`) on a
block of ours.

Env overrides, read at boot, each logged when set:

- `H2_SKULLS_LIST_AT=x,y`: the list's anchor (default `-215,130`);
- `H2_SKULLS_LIST_SKIN=n`: the list skin (default 0);
- `H2_SKULLS_PICTURE_AT=x,y`: the skull's top-left (default `36,180`);
- `H2_SKULLS_TEXT_RECT=top,left,bottom,right`: the description
  (default `-30,39,-133,260`).

The multiplayer screen uses the same layout, picture and description.

- **Strings** are served by the locale hook: the title, "Locked",
  15 + 6 names and descriptions from ids
  `0x21fff000` up; the row labels with their state from `0x23fff000`
  (three per skull). `is_ours` reads only the middle bytes, so both are
  clear of the `0x20fff0xx` ids the other rows and the `ui` model take.
  The row names are the game's (IWHBYD shortened; its description gives
  the full name).
- `H2_NO_SKULLS_MENU=1` (or `H2_NO_SKULLS=1`) leaves the difficulty
  screen as retail; `H2_FEATURES=skulls-screen=off` does too (the page
  has no class, so the difficulty starts at once);
  `H2_FEATURES=skulls-page=off` removes the hook.
- **Co-op.** The page is for the pressing player's profile. The
  campaign start applies the union of every signed-in profile's On
  skulls, as before. The game also has `difficulty_lobby` (screen 1,
  another list); if co-op reaches that one, it has no page (not checked
  in game).

## Gold and silver

Halo 2 itself does not split its skulls into kinds: they are fifteen bytes,
all alike, and the game says nothing about them beyond a name
([Halopedia, Skull](https://www.halopedia.org/Skull): the gold/silver split
starts with Halo 3). The split we use is the one the Master Chief
Collection gives Halo 2's skulls, read off its score multipliers
([Halopedia, MCC:Skulls](https://www.halopedia.org/MCC:Skulls)): a skull
that multiplies the campaign score above 1.0 makes the game harder and is
**gold**; one at 1.0 or below is for fun and is **silver**.

| n | Skull | MCC multiplier | Tier |
|---|---|---|---|
| 2 | Assassins | 1.2 | Gold |
| 3 | Thunderstorm | 1.2 | Gold |
| 4 | Famine | 1.1 | Gold |
| 6 | Blind | 1.1 | Gold |
| 8 | Black Eye | 1.1 | Gold |
| 9 | Catch | 1.1 | Gold |
| 11 | Iron | 1.3 | Gold |
| 12 | Mythic | 1.2 | Gold |
| 13 | Angry | 1.1 | Gold |
| 0 | Envy | 0.0 | Silver |
| 1 | Grunt Birthday Party | 1.0 | Silver |
| 5 | IWHBYD | 1.0 | Silver |
| 7 | Ghost | 1.0 | Silver |
| 10 | Sputnik | 1.0 | Silver |
| 14 | Bandana | 0.0 | Silver |

Uncertain, so said here:

- This is MCC's grouping applied back to Halo 2, not anything in the
  Xbox game. A rougher "is it a novelty?" rule disagrees on two: it would
  call Blind silver (it only hides the HUD) and Envy gold. MCC scores
  Blind 1.1 (gold) and Envy 0.0 (silver: active camo helps the player).
  We follow the multipliers.
- The multipliers are from Halopedia's MCC page as read for this work
  (November 2026); MCC patches have changed scoring before.

The tier is `skulls::About::tier` (`SkullTier`), next to the rest of
`ABOUT`. The Skulls page lists gold first, then silver, each in the
engine's order (`skulls::by_tier`; `skulls_menu::row_of` maps an index to
its row), and each description starts "Gold skull:" or "Silver skull:".
Coloured rows are not done: our list class sets each row's text only,
and the rows draw in the skin's colour, so the tier is in words and in
the picture's tint. The web badges
and the games filter group by the same tiers (`web/src/lib/skulls.ts`,
kept in step with `ABOUT` by hand).

`ABOUT` and every index are open-ended: the screen's row count is
`ABOUT.len()`, the wire takes indices up to 63 (`live_proto::skulls::LIMIT`),
and the web shows "Skull n" for an index it has no name for. Fifteen
(`COUNT`) is only the engine's array; a skull of ours past it needs its own
storage, not a byte of `0x4f55dc`.

## Where each effect lives

Every read of the array, from an absolute-reference scan of the XBE (the
only writer is `0x1df3f4`), with the function around it read in
`h2 disasm`. Confirmed unless noted. Every one of them tests the active
options mode (`[[0x4e6948]+8] == 1`, campaign) first, in the reader or in
its only caller, so retail runs no skull in multiplayer (mode 2) or the
main menu (mode 3), whatever the array holds.

| n | Skull | Read at (function) | What it gates | Scope |
|---|---|---|---|---|
| 0 | Envy | `0xc50fd` (`0xc50a0`, campaign-only as a whole), `0xc579d` | the player's flashlight becomes active camo | simulation (player unit) |
| 1 | GBP | `0xca13f` (`0xca0b0`) | an effect spawn (community: confetti on a headshot) | simulation, effect only |
| 2 | Assassins | `0xc6841` | AI units get the camo flag | AI |
| 3 | Thunderstorm | `0x203759` (`0x2036c0`) | AI characters take the next rank's variant | AI |
| 4 | Famine | `0x1e2e50`, `0x1e2ed6` (`0x1e2a90`); `0xfda54` (`0xfda00`) | AI weapon drops' rounds scaled; a weapon made from a scenario placement starts with the placement's rounds (`+0x4c`, `+0x4e`) halved (`cdq; sub; sar`) | AI drops; simulation for placed weapons |
| 5 | IWHBYD | `0x20b27b` (`0x20b140`) | which dialogue line plays (rare ones far more often) | AI dialogue |
| 6 | Blind | `0x1393e1` (`0x13939b`); `0x165f31` (`0x165f16`) | the HUD draw predicate returns 0; first-person weapon render (stdcall, 4 args, `ret 0x10`) returns 0 | local (what this machine draws) |
| 7 | Ghost | `0xc986b` (`0xc9770`) | the flinch cap that applies to players also applies to AI | AI |
| 8 | Black Eye | `0xd5e35` (`0xd5de0`); `0xd9770` (`0xd9640`) | shields do not recharge on their own (`[unit+0x104] = 10`); a melee hit adds shield, up to 3.0 | simulation |
| 9 | Catch | `0x1ae51f` to `0x1ae7b1`; `0x1e2d5b` | AI grenade decisions and drops | AI |
| 10 | Sputnik | `0xd9923` (`0xd9640`) | damage impulse ×3.0 (`[0x45dbd4]`), mid-function | simulation |
| 11 | Iron | `0x1388ae` (`0x1388a0`) | co-op respawn: a death reverts to the checkpoint | campaign co-op only |
| 12 | Mythic | `0x1e0578`, `0x1e0ead` | AI vitality ×2 | AI |
| 13 | Angry | `0x1fda15` (`0x1fc7e0`) | AI actor fire rate ×2 | AI |
| 14 | Bandana | `0x25cc4d` (`0x25cb60`) | actor code sets a unit-tag short at `+0xc2` to 2; the community effect is bottomless ammo (likely, not traced to the ammo path) | AI code path; likely player ammo |

Scope: **local** changes only what one machine draws; **simulation**
changes what the host computes and every machine predicts; **AI** needs
AI, which multiplayer maps do not run; "campaign" is campaign-only logic.

## In multiplayer

Status: built, tests only; not seen running.

### What is offered

Only the skulls whose effect can be made to work in a multiplayer game
without patching a retail function's middle:

- **Blind** (local). `before` hooks on `0x13939b` (`HUD_SHOWN`, returns
  `AL = 0`, `ret`) and `0x165f16` (`FIRST_PERSON_RENDER`, returns 0,
  `ret 0x10`): the same returns the campaign path takes with the skull on,
  without the campaign test.
- **Famine** (simulation). Around `0xfda00` (`WEAPON_FROM_PLACEMENT`,
  stdcall `(object, placement)`): `before` halves the placement's `+0x4c`
  and `+0x4e` (truncating toward zero, as the campaign path's
  `cdq; sub; sar` does), `after` puts them back, so the scenario's data is
  as it was and the next weapon from the same placement is halved from the
  original count. Only `+0x4c` is read by `0xfda00`; `+0x4e` is halved for
  symmetry with the campaign path. AI drops are not touched (no AI).

Not offered: Sputnik and Black Eye act in the middle of `0xd9640` and
`0xd5de0` (each would need a port of the function or a mid-function hook);
Envy reads inside a function that is campaign-only as a whole; the rest
need AI or are co-op campaign logic.

- **Low Gravity** (simulation), one of ours: the physics constants'
  gravity, halved on every machine ("Our skulls" below).

`live_proto::skulls::MULTIPLAYER` (`[4, 6, 16]`) is the list, and the server
refuses a multiplayer game or session that names any other.

### Choosing them

A "Skulls" row is added to the game variant editor's category list
(`variant_editing_options`, screen 178: Match, Player, Team, Game Type,
Vehicle and Equipment Options; feature `skulls-mp-row`,
`boot::skulls_mp_row::VariantRow`). A on it opens the multiplayer Skulls
screen: a screen of its own (a second `SkullsScreen`,
`SkullsScreen::multiplayer`, on its own list class `skulls-mp-screen`),
titled "Multiplayer Skulls", one row per skull offered (Famine, Blind
and Low Gravity), each On, Off or Locked, the description saying what it does in multiplayer, that the
host's choice is everyone's, and where it is found. The choice is per
profile, apart from the campaign one: `skulls.mp_famine` and
`skulls.mp_blind` under `[profiles."<name>".skulls]`.

**Collecting is the rule.** A skull is Locked on the multiplayer screen
until that profile has picked it up in the campaign, and a press on a
locked one changes nothing. There is no setting to waive it (the
`skulls.mp_require_collected` setting is gone); a value left in
`settings.toml` for a skull the profile has not collected stays Locked and
is never applied or advertised. Guests apply the host's advertised set
whatever their own profiles hold.

**Not stored with the variant.** The choice stays per host profile and
travels with the Live session, as before. The variant's record (the
0x130-byte game variant the editor edits at `0x54e4a0`, its name at
`0x54e4a4`) now has room of ours, the extension block in its spare bytes
(docs/game/game-options.md section 3), but the skulls have not moved there.

| Address | What | Confidence |
|---|---|---|
| `0x23764f` | the variant editor's loader, screen id 178 (`ui\screens\game_shell\settings_screen\variant_settings\variant_editing_options`), constructor `0x23768d`; opened by `0x2387d8` and `0x2ba9d0`, both with window 5 (the main menu's stack) | confirmed |
| `0x2b74c3` | its list handler constructor (ESI the handler at screen `+0x614`, `ret 4`): datum array "variant editing options list" (`0x45be10`) of 6 rows, each row's word (`+2`) set to its index by a loop over the count; six item widgets | confirmed |
| `0x2b7581` | row text (list slot 20, thiscall, item and word, `ret 8`): a table of six `(word, string id)` pairs through `0x24c75c`; a word not in it leaves the text as it was | confirmed |
| `0x2b75e8` | row chosen (the press record's function, thiscall, chain and row pointer, `ret 8`): words 0 to 5 open a category's screen (`0x2c83e6`, window 5); any other word returns at `0x2b7711` having done nothing | confirmed |

The shared datum array hook grows "variant editing options list" from 6 to
7 when the row is on, so the constructor's loop gives our row word 6. Our
`before` on `0x2b7581` draws "Skulls" for row 6; our `before` on `0x2b75e8`
opens the multiplayer screen for row 6 and returns (`ret 8`), and leaves
rows 0 to 5 to retail. The tag has one pane per category and none for
ours (`0x230427` does not check), so the pane hook keeps the last
category's pane, its help text included, while our row is focused. The
list has six item widgets for seven rows, so it scrolls.

### Why the Game Setup row broke

The first version put the row on the lobby's Game Setup dialog
(`squad_settings_dialog`, screen 23) and opened the screen the way the
difficulty row does, with `0x149f1e(1 << player, loader)`. In game it
"breaks, it looks like it tries to use the campaign screen". Read
statically (not seen running), the cause is the screen stack it opened
on:

- A screen request (`0x149f49`) carries a window at `+4` and a slot at
  `+8`. The base screen constructor `0x22f5ca` keeps them at screen
  `+0x74` and `+0x78` (screen vtable slots 20 and 21 return them,
  `0x23012c`, `0x230130`).
- The push `0x147f6d` picks the stack by the window: 5 is the per-player
  array at `0x54d62c` (stride 0x40), 3 the one at `0x54d76c` (stride
  0x38), 1 `0x54d884`, and 0, 2 and 4 single stacks. The generic back
  handler (`0x2300ea`, B or Back) closes the screen through `0x14800c`
  with the same window and slot.
- `0x149f1e` always says window 5, slot 4. The Game Setup dialog is opened
  on window 3 (`0x25102e`, `0x2c7eef` both push 3), and every screen it
  opens itself (Change Map `0x2bb8ac`, the variant list `0x2bb8df`, and the
  others in its jump table) says window 3 too.
- So our Skulls screen went onto the main menu's stack (window 5), under
  the lobby's dialogs, not over the Game Setup dialog: the screen shown is
  the main menu's (where Campaign is), and B closed our screen on window
  5, back to whatever the main menu's stack held.

Two fixes, both in: the row moved to the variant editor (where the
variant's settings are edited), and our press opens the screen on the
pressing screen's own stack: it reads the variant editor's window and slot
(`+0x74`, `+0x78`) and builds the request itself
(`skulls_mp::open_request`, written to a page of ours and handed to the
class's loader), so it would be right on a lobby's window 3 as well as the
main menu's 5. And the multiplayer screen is no longer the campaign
screen in another mode: it is its own instance and class.

### How the host's set reaches guests

Retail's multiplayer simulation is host-authoritative, and the game
options every peer builds at session start (`0x6ec80`) carry no skulls, so
the set travels beside the game, through our Live session data:

1. **The host advertises it.** When the game advertises a session
   (`XSessionCreate`, `XSessionModify`), the HLE runs the boot's probe
   (`Hle::set_skulls_probe`): the union of the signed-in profiles'
   multiplayer choices, from a cache (the probe runs with guest memory
   held, so it never touches the settings store). It goes on the wire as
   `NewSession.skulls` and, when it changed, `Update.skulls`
   ([live-api.md](../live/live-api.md)). A press on the Skulls screen pushes an
   update to an open advertisement at once (`Hle::set_session_skulls`).
2. **A guest reads it.** The HLE remembers the skulls of every session it
   saw in a search or match result. At game start
   (`GAME_ACTIVATE`, `0x18f000`, options mode 2) a guest finds its joined
   session (the XNKID registered with `XNetRegisterKey` that is not one of
   our own advertisements), asks the server for it again
   (`GET /v1/sessions/{id}`), waits up to 1.5 s, and uses what it has
   (`Hle::host_skulls`): the fresh answer, or the set seen when it joined.
3. **Everyone applies the host's set** for that game. The host, and a
   machine playing alone, apply their own choice. A guest applies only the
   host's, never its own. Telemetry's `NewGame.skulls` is the set applied
   ([live-telemetry.md](../live/live-telemetry.md)).

The role comes from the squad session (`coop_probe::read_session`): no
live session or one machine is local, our peer the host is host, anything
else is a guest.

Limits:

- **System Link and no server.** No advertisement reaches the guest, so it
  applies none and says so. The host still applies its own: Blind on the
  host's screen only, Famine to the weapons the host makes (the host
  decides objects; whether a guest's own prediction of a fresh weapon's
  rounds then shows the full count for a moment is not checked).
- **A change after the game starts** applies at the next game.
- **A modified client** can ignore the host's set for Blind (it only
  changes its own screen). Famine is enforced by the host.
- **Older servers** refuse a session update carrying `skulls`; the HLE
  sends the field only when the set changed, so a host with no skulls
  never sends it.

### Switching it off

- `H2_FEATURES=skulls-mp=off`: no multiplayer skull is applied and nothing
  is advertised (`skulls-mp: off: switched off (H2_FEATURES)`).
- `H2_FEATURES=skulls-mp-row=off`: no Skulls row in the variant editor.
- `H2_FEATURES=skulls-mp-screen=off`: the row stays, and says the screen
  is off when pressed.
- `H2_NO_SKULLS=1`: neither, nor the campaign skulls.
- As with every plan hook, a clash at one of the points refuses the plan
  at boot rather than replacing a hook.

## Our skulls

Status: built, tests only; not seen running. Every effect and the hidden
skulls want the in-game check at the end of this page.

Six skulls of our own, indices 15 to 20 after the game's fifteen. Retail's
array at `0x4f55dc` is fifteen bytes and `0x1df3e0` takes only `n < 15`,
so ours have no byte there and the game never runs them: each is our code
on the feature plan (`boot::skulls_custom`, feature `skulls-custom`). The
rest is shared with retail's skulls: the same per-profile `skulls.*`
values (`skulls.acrophobia`, `skulls.low_gravity`, `skulls.fall_damage`,
`skulls.unlimited_ammo`, `skulls.unlimited_grenades`,
`skulls.invincible`), Locked on the Skulls screen until collected (listed
after retail's fifteen, gold first again), turned on at a campaign start
with the profile's others (`skulls::custom_selection`, the same
`OPTIONS_QUEUE` hook), and carried in telemetry `NewGame.skulls` with the
retail ones (`live_proto::skulls::OURS`). The web's name table has them.

| n | Skull | Tier | Level | What it does |
|---|---|---|---|---|
| 15 | Acrophobia | silver | Uprising | flight: in the air, look up to climb, level out to hover, crouch to drop |
| 16 | Low Gravity | silver | Delta Halo | gravity halved for everything; offered in multiplayer |
| 17 | Fall Damage | gold | Metropolis | a landing faster than a jump's drains shield, then health |
| 18 | Unlimited Ammo | silver | Outskirts | spare ammo never runs out; reloads still happen |
| 19 | Unlimited Grenades | silver | Sacred Icon | both grenade types kept at four |
| 20 | Invincible | silver | The Great Journey | the player takes no damage |

Each is found on any difficulty (the description says so).

### How each works

All but Low Gravity act on the **local players' units** (players with a
local user, `+0x28 >= 0`, on a live unit), after every `game_tick`, in
campaign only (options mode `[[0x4e6948]+8] == 1`).

- **Acrophobia** (likely). Simplest sound way: no new input, no physics
  change, one float. While the biped is airborne (biped `+0x3dc` physics
  mode 2, likely: 1 on the ground, 2 in the air) and attached to nothing
  (`+0x14` is none) and not crouching (`+0x2c4` below 0.5), its vertical
  speed (`+0x88` velocity, z at `+0x90`) is raised to `aim_z * 4.0` world
  units a second, `aim_z` being the unit's aim vector's z (`+0x168`,
  likely). Looking up climbs, looking level hovers, looking down sinks
  slowly; crouching drops out; a jump's own speed is never lowered. Get
  airborne with a jump. The jump button held was not traced (the control
  latch at `[0x4ed284]+4` is the action test, not the held state), which
  is why it is the view and not the button. Since traced for the `fly`
  cheat (docs/runtime/debug-menu.md): the unit's control flags at
  `+0x148` hold it, bit 1 jump and bit 0 crouch, so the skull could move
  to the cheat's controls; it has not been changed. If the physics rewrites the
  velocity before it is used, the effect is weaker than meant: the in-game
  check says.
- **Low Gravity** (confirmed address, likely effect). The physics
  constants' gravity is the float at `[0x51e9c4] + 0`: the constants'
  set-up (`0x1eb584..0x1eb5b9`) writes the standard gravity
  (`[0x4455d8]`) there and publishes the pointer; builtin 127
  `physics_set_gravity` (`0x2a2b30`) writes `arg * [0x4455d8]`, and the
  reset builtin `0x2a2ad0` writes `[0x4455d8]`; 31 readers in physics and
  AI (e.g. the AI fall check at `0xe32af`) multiply by it. After each tick
  we write half of what the game last put there; when the game writes a
  new value (a map, a script) we halve that; when the skull is off we put
  the game's value back. Retail never reads `[0x4455d8]` again for gravity,
  so nothing undoes ours.
- **Fall Damage** (ours, likely offsets). Landing is the biped going from
  airborne to not between two ticks; the impact speed is the vertical speed
  of the tick before. Past 3.0 wu/s (a jump lands at about 2.5), each
  wu/s more takes 0.3 vitality: shield (`+0xf0`) first, then health
  (`+0xec`), the fractions `object_get_shield` (`0x2a1a90`) and
  `object_get_health` (`0x2a1a20`) read. A drained body is killed as
  builtin `unit_kill` (`0x2a3320`) does: `+0x10a |= 0x20`, then a call to
  `0xb7360` with EAX the unit. Retail's own falling damage was not found
  (the globals' damage block at `[0x4e034c] + 0x144` has the kill and
  fall-below-the-map damage, not landings), so this is ours, not a
  scaled retail value. Off while Invincible is on.
- **Unlimited Ammo** (likely). For each weapon the unit holds (`+0x218`,
  four datums), each magazine's reserve (`W+0x22a`, magazines at `W+0x228`,
  `0x10` each, at most two) is set to its definition's maximum (the `weap`
  magazine block, count `+0x2c0`, pointer `+0x2c4`, `0x5c` each; the
  maximum at `+0x0c`, the field `0x101a10` caps the reserve with). The
  loaded rounds (`W+0x22c`) are not touched, so the magazine empties and
  the weapon reloads. A weapon with no magazines (plasma) has its charge
  kept full (`W+0x184`, the used fraction `0x101a10` writes, set to 0).
  **Not Bandana**: retail's Bandana (14) is gated in actor code
  (`0x25cc4d`) and, by the community's account, makes the magazine itself
  bottomless (no reloads) and grenades too; ours keeps reloads, does
  nothing for grenades, and acts on the local players only.
- **Unlimited Grenades** (confirmed offsets). The unit's two grenade counts
  (`+0x23e`, signed bytes, fragmentation and plasma; `+0x23c` the selected
  type) are kept at four or more. Four is the most the campaign carries
  (community).
- **Invincible** (confirmed). The bit builtin 90 `object_cannot_take_damage`
  (`0x2a20c0`, through `0xd87e0`) sets: body `+0x10a |= 0x80`, kept set on
  the local players' units every tick (builtin 91 clears it; a script
  that does is overridden the next tick). Turning the skull off leaves the
  bit until the next unit (a death, a level): we do not know whether a
  script set it too.

### Hidden collectables

One per level, from each level's real geometry (`map.glb`, the BSPs and
the scenario, read off the disc with this repo's tools; renders kept
outside the repo). On a campaign level with a spot, once its BSP is the
active one (`[[0x4e6948]+2]`), and while any signed-in profile has not
collected that skull, we create the retail skull weapon there:
`objects\weapons\multiplayer\ball\head_sp` (`head` on High Charity), its
tag datum read from the loaded map's own file on the host (so map packages
and patches work) and checked against the guest's scenario datum.
Creation is the game's own: `placement_data_new` (`0xb7930`, ESI a
`0xc4`-byte placement, stdcall definition, owner -1, 0) then position at
placement `+0x1c` (as the netgame equipment spawn at `0x15ab9d` copies it)
and `object_new` (`0xb7b40`, stdcall placement, the datum in EAX). It has
the retail skull's colour: the placement's colour fields were not traced,
so there is no tint.

Pickup is a local player's unit holding it: the skull's datum in one of
the unit's weapon slots or the unit as its parent (`held_by`, the pure
function the tests run). Then: recorded and on for every signed-in
profile (`skulls::record_pickup`, as for retail's), on for the rest of
this level, our HUD message "Skull found: <name>" through the game's own
(`0x24cbbf`, EDI our string id, stdcall local user; what `0x1df3e0`
calls), a loud stderr line, and the object removed as `object_destroy`
(builtin, `0x2a14d0`) does: `0xbeb30` (refuses with AL non-zero), then
`object_delete` (`0xb8540`). A checkpoint revert or a BSP change that
takes the object away places it again.

Calls into the game run at one point: a `before` on `game_tick`, first in
its chain, that, when calls are queued, answers with an async body that
makes them and then runs the tick (whose chain runs again and continues),
as `coop_probe`'s lobby setup does.

Where, in words:

- **Outskirts** (Unlimited Ammo): in the shade beneath the far end of the
  pier.
- **Metropolis** (Fall Damage): at the very end of the gantry, where the
  bridge stops short.
- **Delta Halo** (Low Gravity): behind the falling water above the gorge
  bridge.
- **Sacred Icon** (Unlimited Grenades): up in the corner bay of the great
  octagonal chamber.
- **Uprising** (Acrophobia): among the supply crates on the raised platform
  past the first cliff descent.
- **The Great Journey** (Invincible): at the end of the gully under the
  overhang, just north of where you begin.

<details>
<summary>Exact coordinates (spoilers)</summary>

Game units, z up, the floor plus 0.15 (`boot::skulls_custom::SPOTS`).

| Map | Skull | BSP | x | y | z | Where |
|---|---|---|---|---|---|---|
| `03a_oldmombasa` | Unlimited Ammo | 1 | -452.00 | -468.00 | -7.12 | on the beach, under the deck at the seaward end of the long pier (y -460), among the pilings |
| `03b_newmombasa` | Fall Damage | 1 | 452.50 | 71.80 | -5.45 | the east end of the northern covered bridge-tube off the walkway at x 432, 12 above the ground |
| `05a_deltaapproach` | Low Gravity | 0 | -168.00 | -302.50 | 25.42 | a ledge behind the tall waterfall south of the Forerunner bridge, reached from the ruins path |
| `06a_sentinelwalls` | Unlimited Grenades | 0 | -390.80 | 134.20 | 4.35 | the south-west bay of the raised ring in the first octagonal room (centre -387, 145) |
| `08a_deltacliffs` | Acrophobia | 1 | 134.40 | 117.60 | 68.65 | a roofed platform between Covenant supply crates, stepped up to from the east |
| `08b_deltacontrol` | Invincible | 0 | 167.00 | 22.80 | 12.55 | the tip of the dead-end gully north-west of the start (175.6, 10.0) |

How they were picked: each floor is a flat render triangle (normal z
above 0.95) with headroom, reachable on foot in a flood fill over the
level's floors from its placements and start, clear of placed weapons, and
far from the retail skull. The render geometry is not collision: crates,
scenery, kill volumes and water are not all in it, so the in-game check
confirms each. Known soft spots: Outskirts' sand is a little below the
river's water plane (the river does not cover it); Sacred Icon's bay is on
the main path; Uprising's crates are physics objects; The Great Journey's
gully tip slopes, so the skull may roll a little.

</details>

`H2_SKULLS_REVEAL=1` logs every spot at boot and, on a level with one,
its coordinates, state and each local player's distance every 150 ticks
(five seconds), on stderr only.

### In multiplayer

Only Low Gravity is offered (`skulls_mp::MP`,
`live_proto::skulls::MULTIPLAYER` = `[4, 6, 16]`): gravity is one constant
every machine simulates with, so the host's set, applied on every machine
at game start the way Famine and Blind are, keeps them agreeing. The rest
act on the local players only; in a game the host simulates, a guest's
invincibility or ammo would be undone or disagree, so they are campaign
only.

### Switching it off

`H2_NO_SKULLS=1` (with the rest) or `H2_FEATURES=skulls-custom=off`. When
the plan cannot place its tick hooks (another feature holding an address
it needs), boot says `skulls-custom: NOT installed: <why>` and nothing of
ours is placed or applied; retail's skulls are untouched either way.

### What it logs

- boot: `skulls-custom: hooks installed via feature plan at 0x... (tick:
  calls, effects, hidden skulls); hidden: Acrophobia on Uprising, ...`,
  or `skulls-custom: NOT installed: <why>`, or `skulls-custom: off: <why>`
- campaign start: `skulls: campaign start for <profiles>: <all on, ours
  included>` and `skulls: ours on this campaign: <ours>`
- level load: `skulls: <map> hides <skull> (bsp n): not yet collected by
  everyone here; it will be placed` (or `... already collected by every
  profile here; not placed`, or `... cannot be placed: <why>`)
- `skulls: placed <skull> at (x, y, z) as object 0x...` (or `skulls:
  object_new refused <skull>; not placed this level`)
- pickup: `skulls: ==== FOUND <skull> on <map> by player 0 (object 0x...);
  on for the rest of this level ====`, the `skulls: <skull> kept for
  <profile>, saved to <path>` lines, `skulls: removed the found skull
  (object 0x...)`
- effects, once a game each: `skulls: Low Gravity: gravity <g> -> <g/2>
  (physics constants 0x...)`, `skulls: <skull> is acting on the local
  players this game`; each hard landing: `skulls: Fall Damage: player 0
  landed at 8.00 wu/s: 1.50 vitality (shield 1.00 -> 0.00, health 1.00 ->
  0.50)`
- with `H2_SKULLS_REVEAL=1`: `skulls reveal: <skull> on <map> (bsp n) at
  (x, y, z) [hint]: <placed|collected|waiting for bsp n ...>; player 0
  12.3 wu away`

## To check in game

1. `h2 play`, sign in a named profile, start Cairo Station on Legendary and
   pick up its skull. stderr at boot: `skulls: hooks installed via feature
   plan at ...; saving to <path>`; on pickup: `skulls: pickup call for
   skull 3 (Thunderstorm), was off`, then `skulls: Thunderstorm kept for
   <profile>, saved to <path>`. That `settings.toml` now has
   `[profiles."<profile>".skulls] thunderstorm = true`.
2. Quit, start `h2 play` again, start any campaign level with that profile:
   stderr `skulls: campaign start for <profile>: Thunderstorm`, and the
   skull's effect is on from the first encounter.
3. Set `thunderstorm = false` (in the menu, step 7, or the file), start
   a campaign: `... : none`, and the effect is off even after picking up
   another skull earlier in the same boot.
4. Co-op with two named profiles: a pickup is kept for both; the start
   turns on the union.
5. The default profile (no name) or `H2_LEVEL`: a pickup is kept under
   `default` (with the `no named profile signed in (...)` line showing the
   slots as read), and a campaign start applies `default`'s skulls.
6. The HUD message and sound on pickup are unchanged (the game's own
   function still runs).

The page (step numbers kept for the multiplayer steps below):

7. With a profile that has no skull collected: Campaign, New Campaign,
   pick a difficulty. The difficulty list is retail's four rows (no
   Skulls row), and the level starts at once. stderr: `skulls page:
   <profile> has no skulls collected; starting as retail`.
8. With a profile that has a skull: pick a difficulty. A page titled
   "Skulls" opens instead of the level. stderr, in order: `skulls page:
   difficulty chosen; the Skulls page comes first`, `skulls page:
   kept difficulty <n> (<name>) for level New Campaign's first, player 0
   (<profile>), list 0x...` ("chosen on Select Level" from there),
   `skulls-screen: opened with 21 rows (our list class)`, `list class: a
   screen of ours gets its own layout (tag copy at 0x...)`,
   `skulls-screen: row text`. If the layout line is missing, the list is
   still full width: report it.
9. **Look at the layout** (the point of this change; it can only be seen
   in game). It should match Select Level's screen:
   - the list on the **left**, rows about as wide as Select Level's
     (each row ends left of the screen's centre), **no Start row**: first
     row Assassins, then Thunderstorm, Famine, Blind, Black Eye, Catch, Iron,
     Mythic, Angry, Envy, Grunt Birthday Party, IWHBYD, Ghost, Sputnik,
     Bandana, Fall Damage, Acrophobia, Low Gravity, Unlimited Ammo,
     Unlimited Grenades, Invincible; each "Name: < On >",
     "Name: < Off >" or "Name: Locked". Scroll to the end and back: all 21 reachable.
   - on the **right**, a large skull in Select Level's picture box,
     **gold** on a gold skull, **silver** on a silver one, dimmer while
     locked, changing as the cursor moves.
   - **under the skull**, inside the right column, the description,
     wrapped to the picture's width: "Gold skull:" or "Silver skull:",
     the effect, "Found on <level>, Legendary only" (Blind: "any
     difficulty"), then a blank line and "◀▶ Toggle Ⓐ Start Level Ⓑ
     Back" with the controller's button pictures (the d-pad's left and
     right, A, B) (if they show as boxes
     or blanks, report it). The list's left bracket wraps its top-left
     corner as on the level select; no frame art bottom right.
   If something sits wrong, try without a rebuild and say which values
   look right: `H2_SKULLS_LIST_AT=x,y` (default `-215,130`),
   `H2_SKULLS_LIST_SKIN=n` (default 0), `H2_SKULLS_PICTURE_AT=x,y`
   (default `36,180`), `H2_SKULLS_TEXT_RECT=top,left,bottom,right`
   (default `-30,39,-133,260`); screen centre origin, Y up, 640x480.
   Also note: whether long labels ("Unlimited Grenades: Locked",
   "Grunt Birthday Party: < Off >") fit their row; the button prompts (the Controls screen's own: B may still
   say "Done", though it goes back).
10. Left or right (d-pad or left stick) on a collected skull flips On
    and Off at once, the row reading "< On >" or "< Off >" (stderr
    `skulls page: right on row <n>: skulls: Mythic off for <profile>`),
    and `settings.toml` follows. Holding the stick or d-pad flips it
    every 250 ms. On a locked skull it changes nothing: `skulls page:
    left on row <n>: skulls menu: Iron is locked for <profile>: not
    collected yet (found on High Charity, Legendary only)`.
11. B: back on the difficulty list. Pick a difficulty again: the page
    opens again.
12. A on the page: the page animates out like B, and the level starts on
    the difficulty picked. stderr, in order: `skulls page: A/X/Start:
    starting difficulty <n> (<name>) once the page is freed`, `skulls
    page: close requested for the page 0x... (window 5, slot 4)`,
    `skulls page: the page 0x... is freed; back on the difficulty list`,
    `skulls page: starting: the difficulty press (<name> for <profile>,
    list 0x...) with <skulls on>`, `skulls page: the difficulty press
    returned (0x...); the level should be loading`, then the campaign
    start's `skulls: campaign start for <profile>: ...` with exactly the
    On skulls. Then again with X, and with Start: the same. Pressing A
    twice quickly on the page: `A/X/Start again; already closing`, nothing
    more. Spam A from the level list: level, difficulty, page, game, with
    no stop on the page; holding A on the difficulty does not skip the
    page. A `skulls page: A/X/Start <n> ms after the page opened: the
    press that opened it; ignored` line means a press was dropped as the
    opening one (report it with `<n>`). If it sticks, report every `skulls page:` line. Which line is
    last says where: no "freed" line (the stack did not call our
    destructor: the page is back on the difficulty list and nothing
    starts), or "returned" with no level (the press ran but did
    nothing from there). Then try `H2_SKULLS_CONTINUE=now` and
    `H2_SKULLS_CONTINUE=stay` and say which starts the level. B after
    picking: `the page 0x... is freed (B); nothing started`.
13. Select Level: pick a level, then a difficulty: the same page (the
    kept line says "chosen on Select Level"), and A loads that level (the other loader, `0x2bb432`).
14. Co-op (two named profiles): each player's difficulty press opens the
    page for their own profile. Note which difficulty screen co-op shows;
    if it has no page, it is the lobby one (screen 1).
15. `H2_NO_SKULLS_MENU=1`: the difficulty starts the level as retail.

Multiplayer (a Live server running; two machines for 20 to 22):

16. At boot: `skulls-mp: hooks installed via feature plan at 0x18f000
    (multiplayer start), 0x13939b (Blind: HUD), 0x165f16 (Blind:
    first-person weapon), 0xfda00 (Famine: placed weapons); offered:
    Famine, Blind, Low Gravity (each once the host's profile has collected it)`.
17. Main menu, Settings, Game Variants, pick or create a variant, edit it:
    the category list is Match Options ... Equipment Options, then
    "Skulls" (the list scrolls by one). stderr `layouts: list 0x45be10
    grows from 6 to 7 rows` (only when no other list
    grew earlier in the run, as that line is said once) and `skulls menu: variant editor
    row text`. Moving onto Skulls keeps the Equipment Options pane.
18. A on it: "Multiplayer Skulls", rows Famine, Blind and Low Gravity, over
    the variant editor. stderr `skulls menu: opening the multiplayer Skulls screen for
    <profile> (player 0) on window 5`, then `skulls-mp-screen: opened with
    3 rows (our list class)`. The same layout as the campaign page
    (list left, "Famine: Locked" style labels, skull right, description
    under it; no Start row). The big skull: gold or silver by the
    focused skull's tier, dim while locked. A on Blind (collected): `skulls: Blind on
    in multiplayer for <profile>`; on one not collected: Locked, nothing
    changes, and `skulls menu: Famine is locked for <profile> in
    multiplayer: not collected yet (found on The Oracle, Legendary only)`.
    B returns to the variant editor, the cursor on Skulls. The other
    categories still open their screens.
19. Start the game alone: `skulls-mp: multiplayer start (local): Blind`;
    no HUD and no first-person weapon, and `skulls-mp: Blind is hiding
    the HUD and first-person weapon this game`. With Famine: `skulls-mp:
    Famine halved a placed weapon's ammo (the first this game)`, and a
    map's placed weapons start with half their rounds.
20. Host with a guest: the host says `skulls-mp: multiplayer start as host
    of 2 machines: <set> (advertised to guests)`; the guest says
    `skulls-mp: multiplayer start as a guest: the host's skulls, read from
    the server now: <set>` (or `as advertised when we joined ...`), and
    its HUD is gone if Blind is in the set, whatever its own profile says.
21. Over System Link: the guest says `... the host's skulls are unknown
    (...); none here ...`.
22. After the game, the stats site shows the skull badges on that game and
    in the games list, and `?skulls=6` on `/v1/games` finds it.

Our skulls (no Live server needed; `H2_SKULLS_REVEAL=1` helps find them):

23. At boot: `skulls-custom: hooks installed via feature plan at 0x...
    (tick: calls, effects, hidden skulls); hidden: Acrophobia on Uprising,
    Low Gravity on Delta Halo, Fall Damage on Metropolis, Unlimited Ammo
    on Outskirts, Unlimited Grenades on Sacred Icon, Invincible on The
    Great Journey`; with `H2_SKULLS_REVEAL=1`, six `skulls reveal: ... at
    boot; no local player` lines.
24. Start Outskirts (any difficulty). stderr at load: `skulls:
    03a_oldmombasa hides Unlimited Ammo (bsp 1): not yet collected by
    everyone here; it will be placed`. On the beach (BSP 1): `skulls:
    placed Unlimited Ammo at (-452.00, -468.00, -7.12) as object 0x...`.
    If instead `... cannot be placed: <why>` or `object_new refused`,
    report the line. With the reveal on, the distance in `skulls reveal:`
    falls as you walk there.
25. The skull sits on the floor under the pier, retail skull model. Walk
    over it (or press the pick-up button, as for a retail skull): `skulls:
    ==== FOUND Unlimited Ammo on 03a_oldmombasa by player 0 (object 0x...);
    on for the rest of this level ====`, `skulls: Unlimited Ammo kept for
    <profile>, saved to <path>`, `skulls: removed the found skull (object
    0x...)`; the HUD says "Skull found: Unlimited Ammo"; the skull is gone
    from the hands and the world. If it stays in the hands or the game
    stops, report the last `skulls:` lines.
26. Then `skulls: Unlimited Ammo is acting on the local players this
    game`: fire a battle rifle dry: the magazine empties and reloads, the
    reserve stays full. A plasma rifle never drains its charge.
27. Restart the level: `... already collected by every profile here; not
    placed`. The Skulls screen lists Unlimited Ammo as On; starting any
    campaign level says `skulls: ours on this campaign: Unlimited Ammo`.
28. Each effect, turned on in the Skulls screen (or found):
    - Invincible: no damage from anything; a grenade at your feet leaves the
      shield full.
    - Unlimited Grenades: throw both kinds: each count stays at 4.
    - Low Gravity: `skulls: Low Gravity: gravity 4.17.. -> 2.08..
      (physics constants 0x...)`; jumps go higher and fall slower, and so
      do grenades and bodies.
    - Fall Damage: drop from a ledge: `skulls: Fall Damage: player 0
      landed at <speed> wu/s: ...`; a jump on flat ground says nothing. A
      long enough drop kills (`... ; killed`). If a plain jump hurts, give
      the speed it reports.
    - Acrophobia: jump and look up: you keep climbing; look level: you
      hover; look down: you sink slowly; crouch: you fall. If you only
      fall, the physics overrides the velocity write: report it.
29. Multiplayer: Low Gravity on in the multiplayer Skulls screen (once
    collected): `skulls-mp: multiplayer start (local): Low Gravity` and the
    Low Gravity line from step 28, on every machine.
30. `H2_FEATURES=skulls-custom=off`: `skulls-custom: off: switched off
    (H2_FEATURES)`; no skull is placed and none of ours acts.
