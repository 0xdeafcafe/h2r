# Game options the game doesn't allow (design)

Status: design (slice 1). Nothing here is built except two tests in
`crates/variants/src/record.rs` that pin the record facts the design relies
on (see "Tests").

The asks:

- more game variant options: no weapon as the starting weapon, instant
  respawn, gravity, and anything else the retail editor won't let you set;
- Bounce Practice (a community built-in) starts with a magnum;
- any character in player names, gametype names, map names and every other
  name a player types.

The evidence is static: `h2 disasm` on the retail XBE (SHA-256
`03215919…935d`), the repository's code and the docs it links. Nothing was
run in the game. Each claim is **confirmed** (seen in code or data) or
**inferred**. No retail table, string or glyph is copied here. Where this
says what the retail editor offers, it gives the shape (lowest, highest,
missing values); the lists themselves are read from the user's disc at run
time (`variants::Schema::from_map`).

Related docs: [community-gametypes.md](community-gametypes.md) (the `goof`
tag, `sily` definitions, built-ins),
[../editor/variants.md](../editor/variants.md) (the 0x130-byte record, the
setter, the clean-up, saved files),
[../input/keyboard-charset.md](../input/keyboard-charset.md) (the
keyboard), [../live/live-gamestate.md](../live/live-gamestate.md) (the
active variant at `G+0x13c`), [skulls.md](skulls.md) (gravity, and how a
host's choice reaches guests).

## 1. How a variant keeps its settings

### The record and its limits (confirmed)

A variant is one 0x130-byte record: the active one at `G+0x13c`
(`G = [0x4e6948]`), a saved variant's whole payload, the tail of a hopper
slot. Every editor row is a `sily` setting definition: a setting number and
a list of `(value, label)` options. Choosing an option calls the setter
`0x2373be` (EAX setting, EBX value, ESI record), which looks up where the
setting lives (`record::field`) and stores the value.

The editor offers only the listed options. The record is stricter in one
place only: the clean-up `0x19d650`. Loading a saved variant runs it and
fails the load if it changed anything; starting a game runs it on
`G+0x13c` (`0x137dd0`, from `0x137e05`), resetting a bad variant to default
Slayer; and a client decoding the host's variant runs it and drops the
message if it changed anything. `record::check` is our copy of its rules.

So a value has three possible standings:

1. **Offered**: in the `sily` list. Retail does it.
2. **Storable**: inside the clean-up's limits but not offered. It loads,
   saves, signs and travels exactly like an offered value; the retail
   editor just can't pick it. Our editor and our built-ins can.
3. **Outside**: past the clean-up's limits, or no place in the record at
   all. It needs our own extension (section 3).

The settings this design touches:

| Setting | Where | Clean-up limit | Retail editor offers |
|---|---|---|---|
| 2..9 score to win | i32 `+0x50` | 0..0x7fff | 0 (unlimited) up to 250 |
| 10 time limit (s) | i32 `+0x54` | 0..0x7fff (9 h 6 min) | none, 1 min up to 1 h |
| 16 lives | i32 `+0x7c` | 0..0x7fff | unlimited, a few steps up to 99 |
| 17 respawn time (s) | i32 `+0x80` | 0..0x7fff | 3 s up to 30 s; **no 0** |
| 18 suicide penalty (s) | i32 `+0x84` | 0..0x7fff | none up to 15 s |
| 26 betrayal penalty (s) | i32 `+0xac` | 0..0x7fff | none up to 2 min |
| 25 respawn modifier | i32 `+0xa8` | 0..2 | all three (inheritance, cycling, none) |
| 19 shields | i32 `+0x88` | 0..2 | all three |
| 90 max players | i32 `+0x74` | 0..16 | 2..16 |
| 93 max teams | i32 `+0xb4` | 0..8 | **no row at all** |
| 89 starting weapon | u8 `+0xd6` | 0..0x14 | map default, random, 15 weapons; **no 1 (none)**, no 15, 16 or 20 |
| 94 secondary weapon | u8 `+0xd7` | 0..0x14 | as above plus 1 (none); no 15, 16 or 20 |
| 88 starting grenades | flag bit 11 | on/off | on/off |
| 104 extra damage, 105 damage resistance | flag bits 12, 13 | on/off | on/off |
| 75..83 vehicles, turrets, weapons on map | u8 `+0xcc..+0xd4` | per byte, 3 to 0x13 | most values; a few in range are not offered (section 2) |
| 96 Juggernaut movement, 45 speed with ball | i16, type area | 0..2 | all three |

The two tests in "Tests" pin the storable values above against
`record::check`.

### Spare bytes (confirmed for the clean-up; "nothing reads them" inferred)

The clean-up never reads or writes these, and no setting lives there:
`+0x02`, `+0x5c..+0x73`, `+0x8c..+0xa3`, `+0xb8..+0xcb`, `+0xd8..+0xef`,
`+0x10c..+0x12f` (0x24 bytes), and for Slayer `+0xf4..+0x10b`. A linear
scan of `.text` for reads shortly after a load of `[0x4e6948]` found no
reader of them (the session copy at `S+0x4db0` and the pending options at `0x4ed3a8` were not
scanned, so "nothing reads them" stays inferred). They survive the game's
own copy-on-edit (whole record), its save (0x130 bytes after the clean-up,
signed by the game) and every byte copy we make (portable files, cloud
saves, hopper slots).

### How a variant reaches other machines (confirmed)

The record is never sent whole. Messages `0x21` (parameters-update, host to
clients) and `0x22` (parameters-request) carry it through one codec,
encode `0x7cc50` and decode `0x7d520`, which packs each field in the
clean-up's bit width (`+0x48` in 15 bits, the type area per engine through
the jump table at `0x7da38`, and so on). The decoder ends by running the
clean-up and the caller treats a change as a failed decode (`0x7d9b7`,
`0xb0c1e`, `0xb1e17`).

Two consequences:

- **Storable values travel.** The codec's widths are the clean-up's limits,
  so respawn 0, weapon value 1 or a score of 0x7fff reach every client
  unchanged.
- **The spare bytes do not.** A client's copy holds whatever its receive
  buffer had there. Anything outside the wire fields needs a channel of its
  own (section 3).

Matchmaking: the host picks a hopper entry (`0x192eb0`) and installs its
record with `0x5b9d0` without the clean-up; clients get it as above. The
hopper file has no signature (a CRC only), a fixed inflated size of
`0x162cc0` and no version field to grow; it does have bytes no code reads
(docs/live/live-matchmaking.md section 6).

Retail consoles cannot join our Live (docs/live/live.md section 3, and the
HLE implements none of retail's network security), and no variant can reach
a retail save: a real console signs with a key we can't reproduce
(docs/editor/variants.md, "The signature"). So "a player without our build"
means an **older h2recomp build**: it must load our variants and must not be
in a game whose rules it can't apply.

## 2. The options

### Starting weapon "None" (storable, confirmed)

The spawn's weapon pass is `0x19ef40`: for each of the scenario's starting
profiles (`[0x4e0350] + 0x128`, `0x9c` bytes each) it reads the variant's
primary (`+0xd6`) and secondary (`+0xd7`) and maps each through
`0x19eda0` (weapon value, default tag, the other weapon's tag):

- 0, or anything above 20: the profile's own weapon (map default);
- 1: `-1`, no weapon; the caller skips a `-1` (`0x19f085`);
- 2: a random weapon from the multiplayer globals' list, values 3..18,
  never the same as the other weapon;
- 3..20: the weapon reference in slot `value - 3` of that list
  (`[[0x4e034c]+0x16c]` names the tag; `+0xc` then `+0x6c` is the block;
  jump table `0x19eee8`).

So primary 1 is already "None" in the engine: the editor offers it for the
secondary only. Primary and secondary both 1 spawns the player unarmed.
An older build does the same with it (it is the retail code).

What the game does with an unarmed player (melee with no weapon, the HUD,
picking a weapon up) is not traced: the game-run checklist covers it.

### Any weapon as the starting or secondary weapon

- **Storable:** values 15, 16 and 20 are slots of the same globals list the
  editor doesn't offer. They are real references on the disc (random picks
  15 and 16). Our editor names them by reading the slot's tag at run time,
  as the schema does; nothing is listed here.
- **Needs the extension:** a weapon that isn't in that list (one of the
  other weapon tags the map has). The byte can't hold it past 0x14. An
  extension field names it, and a hook at `0x19eda0` returns that tag for
  our value; the record keeps a fallback weapon in `+0xd6` so an older build
  gets something sensible.

### Instant respawn (storable value, needs hooks; confirmed)

Respawn time 0 is storable, but the death code doesn't honour it:

- **A one-second floor.** On death (`0x15cf80..0x15d0fd`) the countdown in
  ticks is `max(tick_rate, respawn * tick_rate + penalties)`
  (`cmovg` at `0x15d0c1`), written to the player's `+0x164`. A second floor
  of one second is at `0x162294..0x1622a4`.
- **A 32-tick slot.** When the countdown reaches 0, the player is let back
  in only on the tick where `(ticks since round start) % 32 ==
  player index % 32` (`0x15de72..0x15de93`), except in the first 3 ticks of
  a round. Up to about one more second.
- **Cycling divides by the respawn time.** With the respawn modifier
  (setting 25) on cycling (1) in a team game, the countdown is rounded up
  to a multiple of `respawn * tick_rate` (`idiv esi` at `0x15d11f`, then a
  loop adding `esi`). With 0 that is a division by zero; the lifted code
  would fault or, where the host's divide returns 0, loop forever.

So the plan:

1. Our editor and built-ins may store 0, but `record`-level validation
   (ours, not the game's) refuses respawn 0 with cycling. That also keeps
   an older build safe: it plays respawn 0 as the one-second floor, which
   is the nearest it can do, and never divides by zero.
2. A hook on our builds lifts the floor to 0 ticks and skips the 32-tick
   slot for players whose countdown was set from a 0 respawn. "Instant" is
   then the next tick, plus whatever the spawn itself needs.
3. The inheritance modifier (0) goes through `0x15d6c0`, not traced; the
   hook leaves it alone until it is.

Where respawn is decided (host only, or every machine) is not traced; the
hooks go on every machine, which is right either way.

### Gravity (needs the extension; confirmed address)

The variant has no gravity. The engine's is the float at `[0x51e9c4] + 0`,
set to the standard value (`[0x4455d8]`) when the physics constants are set
up (`0x1eb584..0x1eb5b9`) and by `physics_set_gravity` (`0x2a2b30`) and its
reset (`0x2a2ad0`); 31 readers in physics and AI multiply by it.
[skulls.md](skulls.md) already scales it each tick for the Low Gravity skull
("Our skulls"): we write our scale of whatever the game last wrote, so a
map script's change is kept and scaled.

A variant's gravity is a scale in the extension (say 25% to 200%). It is
applied at game start the way Low Gravity is, on every machine, and the two
multiply (Low Gravity on a 50% variant is 25%). Gravity is simulated on
every machine, so a machine without the scale disagrees with the host;
section 3 keeps such a machine out of the game.

### Bounce Practice gets a magnum (offered values; confirmed)

Bounce Practice is the eleventh community built-in
(`community_variants::VARIANTS`, Slayer). Today it sets no weapons, so
players get the map's. The change adds `StartingWeapon(Weapon::Magnum)` and
`SecondaryWeapon(None)` (magnum only) and says so in the description. Both
are offered values, so `usable()` keeps the variant on every disc and no
hook is needed. Glitch Hunt keeps its sword and rockets.

The same "None" start makes **Bumper Cars** possible, which
community-gametypes.md left out for want of it (optional; a decision
below).

### More options the game doesn't allow

| Option | Standing | How |
|---|---|---|
| Starting weapon "None" | storable (primary 1) | offer it; retail code handles it |
| Instant respawn | storable (0), needs hooks | floor and slot hooks; refuse 0 with cycling |
| Gravity | needs the extension | scale `[0x51e9c4]` at game start, as Low Gravity |
| Bounce Practice magnum | offered | change the built-in |
| Any weapon, from the globals list | storable (15, 16, 20) | name them from the disc; offer them |
| Any weapon, outside that list | needs the extension | extension names the tag; hook `0x19eda0`; record keeps a fallback |
| Score above 250 | storable to 0x7fff | offer more steps |
| Time above 1 h | storable to 0x7fff s (9 h 6 min) | offer more steps |
| Lives above 99 | storable to 0x7fff | offer more steps |
| Respawn above 30 s | storable to 0x7fff s | offer more steps |
| Suicide and betrayal penalties beyond retail's | storable to 0x7fff s | offer more steps |
| Damage multipliers beyond on/off | needs the extension | the record has two booleans (flag bits 12, 13), read through the trait query `0x24dda0` (trait 2 extra damage, 3 resistance) and per-engine overrides (`0x1926c0`); the scale they apply is in the damage code, not traced |
| Shield and health multipliers | needs the extension | shields are a 0..2 choice (`+0x88`); a vitality scale needs the damage path above |
| Player speed for everyone | needs the extension | only Juggernaut and ball carrier speed exist (i16 0..2); the movement scale is not traced |
| Unlimited grenades | needs the extension | only a starting on/off bit; host refills each unit's counts each tick (unit offsets not traced) |
| Mixed vehicle sets | mostly offered; some storable | each slot (light, second light, heavy, flying, two turrets) is its own byte, so mixes exist today; values in range but not offered (75: 2; 76, 77: 4; 79: 2, 3; 81, 82: 2, 3; 83: 18) and setting 80 (no row, `+0xd0` 0..4) are not traced and are not offered until they are; a vehicle outside the sets needs the extension and a placement hook |
| Team count | storable (setting 93, `+0xb4` 0..8) | the retail editor has no row for it; offer one |
| Team size | needs the extension | nothing in the record; the lobby would have to enforce it |
| More than 16 players | impossible | the engine's player limit |

## 3. The extension

### Where it lives: in the record's spare bytes

The choice was an extension block the game ignores, or a side table keyed by
the variant. **The block goes in the record's spare bytes** at
`+0x10c..+0x12f`, which every copy of the record already carries: saved
variants (signed by the game), cloud saves, `.h2variant` files, hopper
slots and the editor. A side table would need its own storage, sync and
cleanup beside each of those, and could fall out of step with the record.

Layout (0x24 bytes; all zero means "no extension", which is every retail
variant):

| Offset | Size | Content |
|---|---|---|
| `+0x10c` | 2 | magic `hx` |
| `+0x10e` | 1 | version (1) |
| `+0x10f` | 1 | flags: bit 0 instant respawn, bit 1 unlimited grenades |
| `+0x110` | 2 | gravity, percent (0 means 100) |
| `+0x112` | 2 | damage dealt, percent (0 means 100) |
| `+0x114` | 2 | shield strength, percent |
| `+0x116` | 2 | health, percent |
| `+0x118` | 2 | movement speed, percent |
| `+0x11a` | 2 | primary weapon, our id (0 means use `+0xd6`) |
| `+0x11c` | 2 | secondary weapon, our id |
| `+0x11e` | 1 | team size (0 means any) |
| `+0x11f..+0x12b` | 13 | reserved, zero |
| `+0x12c` | 4 | CRC-32 of `+0x10c..+0x12b` |

Our weapon ids are a table in our code naming tags by path (the weapons the
multiplayer maps carry), never a datum, which differs per map. A block with
a bad magic, version or CRC is ignored. Because retail never writes these
bytes, a record from a retail editor is all zero there.

An older build ignores the block and keeps it: it loads, plays and re-saves
the variant with only its retail fields in effect.

### How it reaches every player

The wire drops the spare bytes, so the host sends the block another way,
through Live, as skulls already do (skulls.md "How the host's set reaches
guests"):

- **Custom games and hoppers:** the host's HLE puts `variant_ext` (the 0x24
  bytes, base64) and a key on its session (`NewSession` / `Update`). At
  `GAME_ACTIVATE` (`0x18f000`) each guest re-fetches the session, checks the
  key and writes the block into its own `G+0x13c + 0x10c` before the first
  tick; every hook reads only there. The key is a hash of the record's
  wire projection (what encode then decode leaves: name, engine, the sent
  fields, the rest zero) plus the map, the only part host and guests hold
  identically. A guest whose key differs applies nothing and says so in
  the log.
- **Hoppers:** the extension is already in the hopper slot's record, so the
  host has it the moment it picks the entry; the rest is the custom-game
  path. The server's hopper model and the editor's hopper tool carry it
  untouched (both copy the record whole).
- **Older builds:** a session with a non-zero block also advertises
  `requires: ["variant-ext/1"]`. The server refuses a join from a client
  that doesn't list the capability, with a message that says why. Without
  this, an older build would play the game with normal gravity and damage
  against a host that doesn't.
- **System Link** has no server. A host with an extension shows a warning
  in the lobby and the guests play without it (or, later, a peer-to-peer
  message on the QoS port as co-op's ping does; docs/live/live-coop.md
  section 10.6). A decision below.

### Showing them

- **In the game's editor.** Storable values join the retail rows: at load
  we copy each `sily` options block into our page, append our options
  (value and a label id in our private string range, served by the
  `community_variants` string hook) and repoint it, the way
  `community_variants` extends the `goof` block. Respawn gets "Instant",
  primary weapon "None", score, time and lives more steps. The game's own
  editor then shows and stores them; nothing new to draw.
- **Extension options** (gravity, multipliers, grenades, other weapons,
  team size) have no setting number the setter can store (its descriptor
  table at `0x470828` has 0x70 entries). They go on a kit page of ours,
  "More Options", opened from a row added to the variant editor's page list
  with the list hooks the pregame dialog's "Halo 1 Options" row uses
  ([h1-multiplayer.md](h1-multiplayer.md) "Halo 1 options",
  [ui-screens.md](ui-screens.md)). Its rows are `Row::value`s whose get and
  set read and write the block in the record being edited. The editor
  screen's list hooks are not traced yet.
- **Studio** (`crates/studio/src/ui/variants.rs`): a "More options" section
  under the pages, editing the block through `variants::ext`, and
  `issues()` gains our own rule (respawn 0 with cycling).
- **The stats site** today records only the engine name (`GameStarted`'s
  `variant`). Adding the variant's name and a summary of the extension
  ("Gravity 50%") to `GameStarted` is a later Live slice.

## 4. Names with any character

### What limits a name today (confirmed unless marked)

The keyboard screen opens with a context (0..20) that sets its title, its
length and its check on Done (`0x238114`, flags table `0x44a9f8`):

| Context | Name | Length | Check on Done |
|---|---|---|---|
| 0..4 | profile name | 16 (cut to 17 units with the NUL when it opens) | **letters, digits and spaces only**; must not already exist |
| 5..7 | variant name (7 also cuts 2 units on open) | 16 | **letters, digits and spaces only**; unique per type |
| 8, 9 | playlist name | 16 | one letter or digit |
| 10, 11 | saved film name | 16 | one letter or digit |
| 12, 13 | gamertag, clan-mate gamertag | caller's | the gamertag rule (`0x238ce9`) |
| 14 | message | caller's | may be empty |
| 15 | squad (session) name | 16 | may be empty |
| 16 | clan name | 15 | letters, digits and spaces only |
| 17..19 | clan description, motto, (inferred) URL | 150 | may be empty |
| 20 | ours (kit text rows, invites) | caller's | none |

- "Letters, digits and spaces" is `iswalnum` / `iswspace` (`0x321e23`,
  `0x321e15`), which refuse every unit from U+0100 up and all punctuation:
  "Wheeeeee!" can't be typed as a variant name, and (inferred) our
  community built-ins with punctuation can't be copied under their own
  name.
- Contexts 0..7, 12, 13 and 16 also hide the accents and symbols pages.
- The key handler maps U+E000..U+F8FF to U+25A1 (`0x140420`); INSERT
  (`0x238932`) checks only length. `keyboard_glue` (host typing) copies
  that mapping and drops controls and anything past U+FFFF.
- The text scanner reads `|` as an escape (`||`, `|n`, `|t`, alignment;
  `0x13fbd0`): a name with `|` draws differently from its bytes. The retail
  keyboard has no `|` key.

Other named things:

- **Gamertag:** `live_proto::Gamertag`, 1..15 ASCII letters, digits and
  single spaces, first a letter, case-insensitive key. Enforced by the
  server, the worker, the HLE sign-in and the game (contexts 12 and 13).
  The XOnline structures hold it as 8-bit bytes (`live-hle` `state.rs`,
  `invites.rs`).
- **Clan (team):** the server allows any text under 16 UTF-16 units, trimmed,
  no NUL (`live-proto/src/team.rs`); the game's keyboard is the narrow part.
- **Map names:** the content package's display name is free text, cut to 31
  units in the map list (`content/src/map_list.rs`); the map id is ASCII
  (`is_map_name`, `[A-Za-z0-9_-]{1,64}`), a file and folder name. Forge has
  no named maps.
- **Hopper titles and descriptions:** UTF-8, 15 and 127 bytes.
- **Screenshots and films:** theater is not built; films are named by
  context 10 or 11.
- **Ours:** the kit's text rows and the sign-in prompt `.trim()` what comes
  back; Studio's variant name allows 31 units (the game's keyboard then cuts
  it to 16 on rename) and names its export file `{name}.h2variant`
  unsanitised; Studio's ImGui labels treat `##` in a name as an id
  separator; its hopper title clip counts characters where the format counts
  bytes.

### What the fonts can draw

A scratch audit of the disc's fonts (run locally with the font crate; the
planned `font::coverage` test makes it permanent) counted, for the UI fonts
that draw names (`conduit` and `handel_gothic`, all sizes; which slot draws
which field is still open in fonts.md), the units that have a glyph of
their own:

| Range | Drawable in every UI font |
|---|---|
| ASCII U+0020..U+007E | all 95, including the `` ` ``, `|` and `~` the keyboard lacks |
| Latin-1 U+00A0..U+00FF | all 96 |
| Latin Extended-A U+0100..U+017F | 10 of 128 |
| Latin Extended-B | 1 of 208 |
| Spacing modifiers | 9 of 80 |
| Greek | 1 of 144 |
| Cyrillic | none |
| General punctuation U+2000..U+206F | 15 of 112 |
| Currency | 1 of 48 |
| Letterlike | 2 of 80 |
| Arrows | none |
| Mathematical operators | 13 of 256 |
| Geometric shapes | 1 of 96 |
| Miscellaneous symbols | 6 of 256 |
| Dingbats | 2 of 192 |
| Private use U+E000..U+E0FF, U+E100..U+E1FF | 90, 50 (the button and HUD icons) |
| CJK, kana, Hangul | none (the Asian builds swap in fonts that have them) |

About 394 units in all, plus the icon glyphs `boot::icons` adds. The
`arial` fonts carry more (about 1,170: all of Latin Extended-A and part
of Greek and Cyrillic) but are not known to draw names. U+2699 (the bot gear,
bots.md section 8.2) is in none yet.

### The plan

1. **One drawable set.** `font::coverage(&Font)` returns the units with a
   glyph of their own. The name set is the intersection over every font that
   can draw a name, after our patches, minus controls, surrogates, `|` and
   the scanner's alignment units (U+E405..U+E407). Computed at boot from the
   user's fonts, never shipped.
2. **The Done check.** A wrapper on `0x238114` replaces the
   letters-digits-spaces rule (flag 2) with "every unit is in the drawable
   set" for profile, variant and clan names; the rest of the check
   (non-empty, uniqueness, the gamertag rule for contexts 12 and 13) runs
   as retail. Lengths stay as retail (16 units) unless decided otherwise.
3. **The accents and symbols pages** are shown in every context (the
   gating at `0x237a46`).
4. **Characters the keyboard has no key for** (`` ` ``, `|` excepted, `~`,
   the Latin-1 letters missing from the accents page, the symbols, the
   icons): a kit page "More Characters", a grid of the drawable set by
   block, opened from the keyboard (a free button), inserting through
   INSERT as `keyboard_glue` does. Host typing already reaches INSERT; its
   filter becomes the drawable set.
5. **Icons in names** (U+E0xx, U+E1xx): the game draws them, the web
   doesn't (a box). Allowing them means lifting the U+25A1 mapping for names
   and drawing them on the site from our own icon pictures. A decision
   below.
6. **Adding glyphs.** `font::patch` adds units at the end of a font (as
   `boot::icons` does). The first set worth adding: the rest of Latin
   Extended-A, Greek and Cyrillic (the European names), arrows and the
   common symbols (™ and friends where missing), and U+2699. The pixels come
   from an OFL font rasterised at run time or drawn by hand like our icons;
   which font is a licence decision (keyboard-charset.md "Adding a glyph").
7. **Byte for byte.** The game's text is UTF-16 and BMP-only. Names stay
   as typed through the save (signed by the game; directories are hashed,
   so no host path holds a name), the record, the wire and telemetry
   (`String::from_utf16_lossy`, which loses only lone surrogates, which
   the drawable set excludes). Fix the places that change names: drop the
   kit's and sign-in's `.trim()` (refuse leading or trailing spaces
   instead), clip Studio's names to the game's 16, sanitise only the export
   file name, escape `##` in Studio labels, and count bytes in Studio's
   hopper clip. On the site, stop `text-transform` on names (it turns ß into
   SS on screen).
8. **Map names:** display names follow the same set, checked by the map
   builder with a warning; ids stay ASCII (they are file names).

### Live and gamertags (a proposal; the server decides)

Gamertags stay as they are in slice 1. The proposal for later: keep the
account's gamertag ASCII (it is a login, a case-insensitive key, a stream
name, a URL path and an 8-bit field in the XOnline structures the game
reads), and add a **display name** on the account: UTF-16, up to 16 units,
from the drawable set, shown in games and on the site. Widening the gamertag
itself would need Unicode case folding and NFKC for the key, a confusables
check against impersonation (UTS #39), percent-encoded URLs, and wider
fields in the HLE's XOnline structures; the game's own gamertag contexts
would still refuse it.

## 5. Slices

1. This document and the two record tests.
2. **Variant values, no hooks:** Bounce Practice's magnum; `variants`
   gains our validation (respawn 0 with cycling) and names for weapon
   slots 15, 16 and 20 from the disc; Studio offers the storable values;
   the `sily` option extension shows them in the game's editor. Tests:
   option extension on synthetic tags (retail options byte for byte, ours
   appended), encode and decode of every new value, the validation rule.
3. **Instant respawn hooks** (floor, slot) with tests on synthetic player
   and game-time memory, and the cycling guard.
4. **The extension block:** `variants::ext` (encode, decode, CRC, zero means
   none), Studio's section, the "More Options" kit page, gravity first
   (reusing the Low Gravity code). Tests: round trips, a bad CRC ignored,
   the block surviving `record::check`, portable files and a hopper slot.
5. **Live transport:** `variant_ext` and the key on the session, the
   guest's apply at activation, the `requires` gate. Tests in live-proto,
   live-server and live-hle (an older client refused, a mismatched key
   ignored).
6. **Names:** `font::coverage`, the Done-check wrapper, the page gating,
   "More Characters", the byte-for-byte fixes. Tests: coverage on synthetic
   fonts, the check as a table over contexts and character classes, the
   keyboard filter, Gamertag validation unchanged.
7. **Glyphs** for the chosen ranges; the multipliers, grenades and weapon
   extension once their engine paths are traced.

## 6. To check in the game (the user's run)

After slice 2:

1. Settings, Game Variants: Bounce Practice shows its description with
   the magnum; a custom game on it spawns everyone with a magnum and
   nothing else.
2. Edit a Slayer copy: respawn time offers "Instant"; starting weapon
   offers "None"; score, time and lives offer the new steps; each saves,
   and the saved variant loads again after a restart.
3. Starting weapon None, secondary None: players spawn unarmed. Note what
   melee, the HUD and picking up a weapon do.
4. A variant with a storable value made by Studio (say score 500) shows in
   the game's editor; note what label the row shows.

After slice 3:

5. Respawn Instant, modifier none: dying puts you back in at once (under a
   second). With the Bounce Practice variant, bounces still behave.
6. Respawn Instant can't be saved with cycling (our editor refuses it).

After slices 4 and 5:

7. Gravity 50% in a custom game on Live with a second machine: both jump
   the same height; the log says the block was applied on the guest.
8. An older build can't join that game and is told why.
9. Low Gravity skull with gravity 50%: a quarter of normal.

After slice 6:

10. Name a profile "Bob!" and a variant "Wheeeeee! 2": both are accepted,
    show in the lobby, the scoreboard and the carnage report as typed, and
    a Live game's player and stats pages show them unchanged.
11. "More Characters" from the keyboard types `~` and an accented letter.
12. A name with a character no font draws can't be entered.

## Decisions for the user

1. **Bounce Practice:** magnum only (secondary None), or magnum plus the
   map's secondary?
2. **Bumper Cars:** add it now that "None" exists?
3. **Respawn floor:** truly instant (next tick, our hook), or keep retail's
   one-second floor and only allow 0 as "as fast as retail allows"?
4. **Extension storage:** in the record's spare bytes (recommended), or a
   side table keyed by the variant?
5. **Older builds:** refuse their join to a game with an extension
   (recommended), or let them in with retail rules?
6. **System Link:** extensions off with a warning, or build a peer-to-peer
   channel?
7. **Name lengths:** keep 16 everywhere, or allow variant names up to the
   record's 31?
8. **Icons in names** (button and HUD glyphs): allowed in game only, also on
   the site (we draw them), or not at all?
9. **`|` in names:** leave it out (recommended), or allow it and escape it
   when drawn?
10. **Glyph source** for new characters: which OFL font, or hand-drawn?
11. **Which glyph ranges first** (Latin Extended-A, Greek, Cyrillic,
    symbols)?
12. **Gamertags:** keep ASCII and add a display name (proposed), or widen
    the gamertag itself? A Live server decision.
13. **Which "more options" to build after gravity:** damage, shields,
    health, speed, unlimited grenades, any weapon, team size.

## Open questions

- Which font slot draws names in each place (fonts.md); the audit above
  assumes `conduit` and `handel_gothic`.
- What the game's editor shows for a storable value it doesn't list.
- The variant editor screen's list hooks, for the "More Options" row.
- Whether respawn runs on the host only; the inheritance modifier
  (`0x15d6c0`).
- The unoffered vehicle values and setting 80.
- The damage, vitality and movement multipliers' code and constants.
- The unit's grenade counts.
- Readers of the spare bytes through the session copy (`S+0x4db0`) and the
  pending options (`0x4ed3a8`); a full scan before the block ships.
- Host migration: a new host holds only the wire fields, so it would need
  the block from the session as a guest does.

## Tests

`cargo test -p variants --lib record`:

- `the_cleanup_keeps_values_the_editor_never_offers`: weapon 1, 15 and 20,
  respawn 0, score, time, lives and penalties at 0x7fff and team count 8
  pass `record::check` and round-trip; one past each limit is refused.
- `the_cleanup_ignores_the_spare_bytes`: for every game type, bytes written
  to the spare ranges pass `record::check` and round-trip, and no setting
  the writer knows lives there.
