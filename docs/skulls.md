# Campaign skulls per profile

Status: built. Pickup tracking, per-profile storage and the
campaign-start apply (`boot::skulls`, the settings store's per-profile
values) and the "Skulls" row on the difficulty screen have been seen
working. The Skulls screen that row opens (`boot::skulls_menu`, our own
list class, "Menu" below) is built and not yet seen running.

The ask: a skull a player has picked up is remembered on their profile, and
when they start a new campaign (alone or in co-op) they can pick which of
their collected skulls to turn on.

Labels: **confirmed** means read from the retail XBE's instructions or the
disc's scripts with this repo's tools (`h2 disasm`, `h2 hs`, `h2 strings`,
an absolute-reference scan of the XBE); **likely** means strong evidence but
not every path read; **community** means public knowledge, cited. Nothing
here has been seen running.

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
| `0x54e8e0` | local player slots, 4 x `0xc70` | likely ([live-gamestate.md](live-gamestate.md)) |
| `slot + 0`, bit 4 | signed in with a profile | confirmed (`0x1902c1`, `0x23654b` test it) |
| `slot + 0xc1c` | the profile's name, 32 UTF-16 units | confirmed: `0x1902c1` returns it for a signed-in slot, and campaign start (`0x23654b` → `0x18fd94`) copies it (31 units and a NUL, `0x13fac0`) into each player |
| `0x23643f` | the menu's campaign start: options mode 1 from the selection globals, one player per signed-in slot, `+0x12c` = more than one player (co-op), then `0x18e790` | confirmed ([level-boot.md](level-boot.md)) |
| `0x18e790` | queue options (EAX): called by `0x23643f` (campaign), `0x18f244` (main menu) and `0x18ea83` | confirmed |

So "new campaign" and "new co-op campaign" are the same start: co-op is
the same path with two signed-in profiles.

## What we do

Everything below is the `skulls` feature on the feature plan
([feature-engine.md](feature-engine.md)), on three points: `SKULL_STOCK`
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
  in the signed Xbox profile ([feature-engine.md](feature-engine.md)).
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

A "Skulls" row on the difficulty list, the screen both new campaign and
co-op campaign reach before the level loads, opens the Skulls screen: all
fifteen skulls in the engine's order, each with its name and the pressing
player's profile's state (the game's own On or Off for a collected skull,
our "Locked" for one not collected), and a description text with the
focused skull's effect, its level and whether it needs Legendary. A on a
collected skull flips it; A on a locked one does nothing (and says why on
stderr). B returns to the difficulty list, the cursor still on Skulls.

The parts:

- **The model** (`boot::skulls`): `ABOUT` (name, effect in our words,
  level, Legendary, a note; the table above), `state` (Locked until the
  profile has a value for the skull, then On or Off) and `toggle` (refuses
  a skull not collected). The store is the same per-profile `skulls.*`
  values the pickup writes and the campaign start reads (`selection`), so
  what the screen leaves is what the next start turns on.
- **The screen** (`boot::skulls_menu::SkullsScreen`, its class in
  `boot::skulls_screen`): `list_class::Rows` of fifteen rows for the profile the row's press set
  (`open`). It is our own list class, feature `skulls-screen`
  (`boot::list_class`, docs/feature-engine.md "UI step b"), as the Video
  screen is: built by the Controls constructor (so the Controls tag's
  layout: label on the left, value on the right), our vtable copies, our
  row text, row chosen, pane set-up (title "Skulls") and loader. No retail
  function is patched for it.
- **Fifteen rows on six item widgets.** While the class's loader runs the
  Controls constructor, the Controls list hook (`layout_glue`, the
  `0x1a480d` hook) builds the array with `list_class::array_count`'s count
  (15) instead of six; the list scrolls, as the difficulty list does over
  our fifth row.
- **The description** is a text widget of ours: at pane set-up the class
  hands a text block of its own (font 1, the difficulty description's,
  left-justified) to the pane builder's text step `0x22f9bf`, which makes
  the widget a child of the screen (docs/ui.md "A text of our own on a
  screen"). Row text asks the list for its focused row (`0x24c447`) and,
  when that changed, sets the widget to the row's description. The
  Controls list spans the screen's width, so the text sits right of the
  the list, across its width (`[-25 -225 -140 245]`, top, left, bottom, right; Y up from the screen centre);
  `H2_SKULLS_TEXT_RECT=top,left,bottom,right` moves it without a rebuild.
- **The row** (`boot::skulls_screen::DifficultyRow`, the count in `layout_glue`'s list hook, logic in `skulls_menu`):

| Address | What | Confidence |
|---|---|---|
| `0x2bb3ed`, `0x2bb432` | the difficulty screen's two loaders (`+0x8b4` = 0 or 1), screen id 12 (`ui\screens\game_shell\difficulty\difficulty`), constructor `0x2bb477`, vtable `0x45c588` | confirmed |
| `0x2c9c12` | its list handler constructor (at `screen + 0x614`), called only by `0x2bb477`: vtable `0x45d688`, datum array named `0x45d6e0` ("difficulty list") with 4 rows, row-chosen callback `0x2c9d38` | confirmed |
| `0x2c9ce7` | row text (vtable slot 20, `ret 8`, item at `[esp+4]`, row at `item+0x70`): rows 0 to 3 to the difficulty names, anything else the null id | confirmed |
| `0x2c9d38` | row chosen (thiscall, row pointer at `[esp+8]`, `ret 8`): clamps the row to 0..3, stores the difficulty and starts the campaign | confirmed |
| `0x230427` | pane for a row (`screen + 0x5f3` base + row, no bounds check here); the difficulty screen has one pane per row (4) | confirmed |

  The list constructor hook grows "difficulty list" from 4 to 5 rows
  whenever the menu is on (every skull is listed, collected or not, so
  the row is useful with none). `0x2c9ce7` draws row 4 as "Skulls"; rows
  0 to 3 go to the original. The pane hook returns at once (`ret 4`) for
  row 4 on screen 12, so the previous difficulty's pane stays up. A press
  on row 4 never runs the original (it would clamp the row to Legendary
  and start the campaign): the player is the callback's `[[esp+4]]+4`;
  the profile is the name on that player's slot (in co-op each player
  sees and edits their own; with no named profile there, `default`'s);
  the screen is opened with `0x149f1e(1 << player, loader)`. The row's
  text and press are `before` hooks on the plan
  (`boot::skulls_screen::DifficultyRow`); the Live menu instead adds a
  fifth item widget ([live-custom-games.md](live-custom-games.md) §5),
  which this list could do the same way.
- **Strings** are served by the locale hook from ids `0x21fff000` up (15
  names, the title, "Locked", 15 descriptions): `is_ours` reads only the
  middle bytes, so they are clear of the `0x20fff0xx` ids the other rows
  and the `ui` model take. The row names are the game's (IWHBYD shortened;
  its description gives the full name).
- `H2_NO_SKULLS_MENU=1` (or `H2_NO_SKULLS=1`) leaves the difficulty screen
  as retail; `H2_FEATURES=skulls-screen=off` keeps the row but opens
  nothing (it says so).
- **Co-op.** The difficulty list's handler is built only by the screen-12
  constructor. The game also has `difficulty_lobby` (screen id 1, another
  list); if co-op reaches that one, it has no Skulls row (not checked in
  game).

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

The menu:

7. Campaign, a new campaign: the difficulty list is Easy, Normal, Heroic,
   Legendary, Skulls (scrolled by one), with or without skulls collected.
   stderr: `skulls menu: difficulty list: Skulls row shown; <profile> <n>
   of 15 collected`, and `skulls menu: difficulty row text`. Moving
   onto Skulls keeps the Legendary pane up.
8. A on Skulls: a screen titled "Skulls". stderr, in order:
   `skulls menu: difficulty row chosen`,
   `skulls menu: opening the Skulls screen for <profile> (player 0)`,
   `layouts: list 0x45d298 grows from 6 to 15 rows` (the Controls list;
   only when no Controls list grew earlier in the run, since that line is
   said once),
   `skulls-screen: opened with 15 rows (our list class)`,
   `skulls-screen: row text`. If it says fewer than 15 rows, the list
   hook did not see the build: report it. If it says `no description
   text`, the widget could not be made.
9. The rows: Envy, Grunt Birthday Party, Assassins, Thunderstorm, Famine,
   IWHBYD, Blind, Ghost, Black Eye, Catch, Sputnik, Iron, Mythic, Angry,
   Bandana; each value On, Off or Locked. Scroll to the end and back: all
   fifteen are reachable.
10. The description: under the list, the focused skull's effect and
    "Found on <level>, Legendary only" (Blind: "any difficulty"); it
    changes as the cursor moves. If it is missing, clipped or over the
    list, say where it sits; `H2_SKULLS_TEXT_RECT=top,left,bottom,right`
    (screen centre origin, Y up, 640x480) moves it to try another place.
11. A on a collected skull flips On and Off at once (stderr `skulls: Mythic
    off for <profile>`), and `settings.toml` follows. A on a locked skull
    changes nothing: `skulls menu: Iron is locked for <profile>: not
    collected yet (found on High Charity, Legendary only)`.
12. B returns to the difficulty list with the cursor on Skulls; choosing a
    difficulty starts the campaign with exactly the On skulls (step 2's
    stderr line).
13. Co-op (two named profiles): each player's A opens their own profile's
    screen (`... for <profile> (player 1)`). Note which difficulty screen
    co-op shows; if it has no Skulls row, it is the lobby one (screen 1).
14. Select Level's difficulty choice (the second loader, `0x2bb432`): the
    row appears there too.
15. `H2_NO_SKULLS_MENU=1`: the difficulty screen is retail.
