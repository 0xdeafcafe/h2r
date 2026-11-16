# Campaign skulls per profile

Status: built, not yet seen running: pickup tracking, per-profile storage
and the campaign-start apply (`boot::skulls`, the settings store's
per-profile values), and the in-game "Skulls" menu on the difficulty
screen (`boot::skulls_menu`, "Menu" below).

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
for each index (`unlocked_extra_1` ... in the shared strings, through the
table at `0x445514`); index 14 has no entry there (the table holds 0).

| n | Name | Level whose script stocks it |
|---|---|---|
| 0 | Envy | Delta Halo |
| 1 | Grunt Birthday Party | The Arbiter |
| 2 | Assassins | Regret |
| 3 | Thunderstorm | Cairo Station (Legendary only) |
| 4 | Famine | The Oracle |
| 5 | I Would Have Been Your Daddy | Outskirts |
| 6 | Blind | Outskirts |
| 7 | Ghost | High Charity |
| 8 | Black Eye | The Great Journey |
| 9 | Catch | Metropolis |
| 10 | Sputnik | Quarantine Zone |
| 11 | Iron | Uprising |
| 12 | Mythic | Sacred Icon |
| 13 | Angry | Gravemind |
| 14 | Bandana (community name; no HUD name) | The Armory |

Level names follow the map file names (`01b_spacestation` is Cairo
Station and so on). Where community lists put a skull on another level,
trust the index: it is what the engine stores.

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
Each time the difficulty list is built: `skulls menu: difficulty list for
<profiles>: Skulls row shown` or `...: no Skulls row, no skull collected`.
The plan's own `feature: SKULL_STOCK before skulls:note the skull active`
lines say each hook ran the first time.

## Menu (built)

A "Skulls" row on the difficulty list, the screen both new
campaign and co-op campaign reach before the level loads, opening a screen
of the pressing player's profile's collected skulls, each On or Off.

`skulls::screen_for(store, profile)` builds that screen as a `ui` model
screen (one `Row::setting` per collected skull, none when nothing is
collected) and `skulls::toggle` is a press. The guest side is
`boot::skulls_menu` (logic) and its hooks in `layout_glue::install`:

| Address | What | Confidence |
|---|---|---|
| `0x2bb3ed`, `0x2bb432` | the difficulty screen's two loaders (`+0x8b4` = 0 or 1), screen id 12 (`ui\screens\game_shell\difficulty\difficulty`), constructor `0x2bb477`, vtable `0x45c588` | confirmed |
| `0x2c9c12` | its list handler constructor (at `screen + 0x614`): vtable `0x45d688`, datum array named `0x45d6e0` ("difficulty list") with 4 rows, row-chosen callback `0x2c9d38` | confirmed |
| `0x2c9ce7` | row text (vtable slot 20, `ret 8`, item at `[esp+4]`, row at `item+0x70`): rows 0 to 3 to the difficulty names, anything else the null id | confirmed |
| `0x2c9d38` | row chosen (thiscall, row pointer at `[esp+8]`, `ret 8`): clamps the row to 0..3, stores the difficulty and starts the campaign | confirmed |
| `0x230427` | pane for a row (`screen + 0x5f3` base + row, no bounds check here); the difficulty screen has one pane per row (4) | confirmed |
| `0x230616` | the Campaign Options dialog loader (screen 218), list "campaign options list" (`0x45af70`), row kinds 0 to 4, row chosen `0x2b166d` | confirmed |

What the hooks do:

- **The row.** The list constructor hook (`0x1a480d`) grows "difficulty
  list" from 4 to 5 when any signed-in named profile has a collected skull.
  `0x2c9ce7` draws row 4 as "Skulls"; rows 0 to 3 go to the original. The
  pane hook (`0x230427`) returns at once (`ret 4`) for row 4 on screen 12,
  so the previous difficulty's pane stays up. The list has four item
  widgets, so it scrolls to the fifth row, as the Live menu's Custom Games
  row does (likely; unseen).
- **The press.** `0x2c9d38` for row 4 never runs the original (it would
  clamp the row to Legendary and start the campaign). The player is the
  callback's `[[esp+4]]+4`, as for the Controls and Live menu lists; the
  profile is the name on that player's slot, so in co-op each player sees
  and edits their own skulls (with no named profile there, the `default`
  profile's). With no skull collected, it logs why and stays put. Otherwise it opens a Controls
  screen with `0x149f1e(1 << player, 0x2b7714)`, the Video screen's call.
- **The screen.** While that call runs, the list hook names the Controls
  list's datum array "h2r skulls" and sizes it to the profile's skulls.
  On a list with that name, the Controls row text hook (`0x2c8548`) draws
  the skull's name and the game's own On or Off (`0x02000181`,
  `0x03000182`, the vibration row's), and the row chosen hook (`0x2c8719`)
  flips it in the store (written at once). The header hook (`0x22fda6`)
  swaps the Controls title for "Skulls". The added-items hook leaves the
  list its six retail item widgets, so more than six skulls scroll. B
  returns to the difficulty screen.
- **Strings** are served by the locale hook from ids `0x21fff000` up (the
  15 names, the title, the list name): `is_ours` reads only the middle
  bytes, so they are clear of the `0x20fff0xx` ids the other rows and the
  `ui` model take.
- `H2_NO_SKULLS_MENU=1` (or `H2_NO_SKULLS=1`) leaves the difficulty screen
  as retail.

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

7. With a profile that has a skull: Campaign, a new campaign, the
   difficulty screen lists Easy, Normal, Heroic, Legendary, Skulls (the
   list scrolls by one to show it). stderr: `skulls menu: difficulty row
   text active` (and `layouts: list 0x45d6e0 grows from 4 to 5 rows`
   when it is the first list the run grows). Moving
   onto Skulls keeps the Legendary pane up and does not crash.
8. A on Skulls: a screen titled "Skulls" with one row per collected skull,
   each On. stderr: `skulls menu: opening with N rows`, `skulls menu: list
   built with N rows`, `skulls menu: header active`, `skulls menu: row text
   active`. If the title still reads Controls, or the screen shows the
   retail Controls rows, report the stderr lines.
9. A on a row flips On/Off (stderr `skulls: Mythic off for <profile>`) and
   `settings.toml` follows. The row's text should update at once (as the
   Video screen's do); if it only updates on reopening, say so.
10. More than six skulls: the list scrolls.
11. B returns to the difficulty screen with the cursor on Skulls; choosing
    a difficulty starts the campaign with exactly the On skulls (step 2's
    stderr line).
12. Co-op (two named profiles signed in): the row shows if either has a
    skull; each player's A opens their own profile's skulls; a player with
    none gets `skulls menu: <profile> has no skulls yet; not opened`. Also
    check the co-op path reaches this same difficulty screen (if co-op uses
    another screen, note its title: it would need its own row).
13. A profile with no skulls: no Skulls row; the difficulty screen is
    retail. Same with `H2_NO_SKULLS_MENU=1`.
14. Select Level's difficulty choice (the second loader, `0x2bb432`): the
    row appears there too.
