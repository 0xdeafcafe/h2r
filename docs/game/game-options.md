# Game options the game doesn't allow (design)

Status: decided; slice 2 built (tests only). The user's answers are in
"Decisions"; section 5 has the slices. Built so far: the record tests
(slice 1) and our added choices, Bounce Practice's magnum and Bumper Cars
(slice 2, "Built: slice 2").

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

### Fast respawn (decided: 1 s, no hooks; confirmed)

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

Decided (3b): no hooks. Retail's one-second floor and its 32-tick slot
stay, and the fastest choice we offer is **1 s** (with 2 s, beside
retail's 3 s and up). Respawn 0 is never offered, and our editors refuse
any record with respawn 0 and the cycling modifier (`record::ours`, used
by Studio's save, the hopper tool's link and checks, and portable import),
since that is the one combination that divides by zero. With 1 s, cycling
divides by one second and is safe.

### Gravity (needs the extension; confirmed address)

The variant has no gravity. The engine's is the float at `[0x51e9c4] + 0`,
set to the standard value (`[0x4455d8]`) when the physics constants are set
up (`0x1eb584..0x1eb5b9`) and by `physics_set_gravity` (`0x2a2b30`) and its
reset (`0x2a2ad0`); 31 readers in physics and AI multiply by it.
[skulls.md](skulls.md) already scales it each tick for the Low Gravity skull
("Our skulls"): we write our scale of whatever the game last wrote, so a
map script's change is kept and scaled.

A variant's gravity is a scale in the extension (10% to 400%). It is
applied the way Low Gravity is, each tick after the game's, and the two
multiply (Low Gravity on a 50% variant is 25%):
`skulls_custom::gravity_step(scale, now, ..)` writes `game × scale`, keeps
the game's own value to rescale when the scale changes, and gives it back
at scale 1. The scale comes from the active record's block
(`G+0x13c + 0x10c`, read by `boot::variant_ext::in_game`) in a
multiplayer game only (globals mode 2).

Who applies it (`variant_ext::decide`): a machine alone (no networked
session, or one machine in it) and the host of a Live game. Never on System
Link (decided 6a; the squad session's network class at `+0x18` is 1, the
field `0x59840` checks before answering a LAN search). A Live guest
applies nothing until slice 4 brings it the host's block: the wire drops
the spare bytes, so its own record has none. Gravity is simulated on every
machine, so until then a guest predicts with the game's gravity against a
host with the scale (the desync in section 3). The log says
`variant options: Gravity 50% (host)` (or why not) when the decision
changes, and `variant options: gravity A -> B` on each write.

### Bounce Practice's magnum and Bumper Cars (built)

Bounce Practice is the eleventh community built-in
(`community_variants::VARIANTS`, Slayer). It now starts with a magnum and
nothing else (`StartingWeapon(Some(Weapon::Magnum))`,
`SecondaryWeapon(None)`; decided 1a), and its description says so. Both
are retail choices, so `usable()` keeps it on every disc. Glitch Hunt
keeps its sword and rockets.

The "None" start also makes **Bumper Cars** possible (decided 2a): a
Slayer built-in with no weapons, no grenades, nothing on the map, Warthogs
and Ghosts and nothing else to drive, 25 points. Its unarmed start is one
of our choices, so `usable()` keeps it only when the option lists carry
ours (`variant_options`, on by default); with `H2_NO_VARIANT_OPTIONS=1` it
is left out with a log line, as any variant with a value the game doesn't
offer is.

### More options the game doesn't allow

| Option | Standing | How |
|---|---|---|
| Starting weapon "None" | storable (primary 1) | offer it; retail code handles it |
| Fast respawn | storable (1 s, 2 s) | offered by us (decided: no hooks, retail's 1 s floor stays); respawn 0 with cycling refused |
| Gravity | needs the extension | scale `[0x51e9c4]` at game start, as Low Gravity |
| Bounce Practice magnum, Bumper Cars | offered; Bumper Cars needs our "None" | built-ins (slice 2) |
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
| Team count | storable (setting 93, `+0xb4` 0..8) | the retail editor has no row for it; a row on our "More Options" page |
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
| `+0x10f` | 1 | flags: bit 0 reserved (was instant respawn, dropped by decision 3), bit 1 unlimited grenades |
| `+0x110` | 2 | gravity, percent (0 means 100; 10..400) |
| `+0x112` | 2 | damage dealt, percent (0 means 100; 10..400, as the next three) |
| `+0x114` | 2 | shield strength, percent |
| `+0x116` | 2 | health, percent |
| `+0x118` | 2 | movement speed, percent |
| `+0x11a` | 2 | primary weapon, our id (0 means use `+0xd6`) |
| `+0x11c` | 2 | secondary weapon, our id |
| `+0x11e` | 1 | team size (0 means any; at most 16) |
| `+0x11f..+0x12b` | 13 | reserved, zero |
| `+0x12c` | 4 | CRC-32 of `+0x10c..+0x12b` |

Our weapon ids are a table in our code naming tags by path (the weapons the
multiplayer maps carry), never a datum, which differs per map. A block with
a bad magic, version or CRC, or a value out of its range, is ignored as a
whole (`variants::ext::Block::Ignored`): the game plays retail rules and
Studio lists it as an issue. The CRC is zlib's CRC-32 over the 0x20 bytes
before it, little-endian. The default options encode as all zero, so a
variant whose extras are all put back is a retail record again. Because retail never writes these
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
- **Older builds** (decided 5): they may join. Clients send their build's
  capabilities with the session join; when the host's session carries a
  block and a member lacks `variant-ext/1`, the host's lobby shows a
  warning naming the player ("plays this variant with retail rules"), and
  the older client is told the same if its build can show it (a new build
  joining an older host cannot happen the other way round: an older host
  never publishes a block). There is no refusal.

  **The desync risk.** Halo 2's host simulates and its guests predict. A
  guest playing retail rules against a host with extras predicts wrong
  wherever the rules differ: with gravity it predicts its own jumps and
  every falling object at the wrong height, and the host corrects it
  (rubber-banding, grenades landing short or long on its screen); with
  speed, its own movement snaps back; with damage, shield or health scales,
  it shows hits and kills the host does not agree with until the host's
  update arrives. Unlimited grenades and other weapons are host-side and
  replicate, so they look right. Nothing crashes and the host's result
  stands, but the older player's game feels broken, which is what the
  warning says.
- **System Link** (decided 6a): extras are off. A host on System Link
  never applies its variant's block (every machine plays retail rules, so
  nobody disagrees) and its lobby says the variant's extras are off on
  System Link.

### Showing them

- **In the game's editor.** Storable values join the retail rows: at load
  we copy each `sily` options block into our page, append our options
  (value and a label id in our private string range, served by the
  `community_variants` string hook) and repoint it, the way
  `community_variants` extends the `goof` block. Respawn gets 1 s and 2 s,
  primary weapon "None", score, time, lives and penalties more steps. The
  game's own editor then shows and stores them; nothing new to draw.
  Built in slice 2 (`boot::variant_options`).
- **Extension options** (gravity, multipliers, grenades, other weapons,
  team size) have no setting number the setter can store (its descriptor
  table at `0x470828` has 0x70 entries). They go on a kit page of ours,
  "More Options", opened from a row added to the variant editor's page list
  with the list hooks the pregame dialog's "Halo 1 Options" row uses
  ([h1-multiplayer.md](h1-multiplayer.md) "Halo 1 options",
  [ui-screens.md](ui-screens.md)). Its rows are `Row::value`s whose get and
  set read and write the block in the record being edited. The list's
  hooks are traced ("The variant editor's page list" below); the page is
  not built.
- **Studio** (`crates/studio/src/ui/variants.rs`): a "More options
  (h2recomp)" section under the pages edits the block through
  `VariantEditor::set_ext` (undoable; a drag on one option is one step).
  It shows only what the game applies (gravity in slice 3); a block the
  game would ignore shows a warning and an `Issue::Ext`. `issues()` also
  has our own rule (respawn 0 with cycling).
- **The stats site** today records only the engine name (`GameStarted`'s
  `variant`). Adding the variant's name and a summary of the extension
  ("Gravity 50%") to `GameStarted` is a later Live slice.

### The variant editor's page list (traced, static)

The list already has a row of ours: Skulls, row word 6
(`boot::skulls_mp_row`, docs/game/skulls.md "Choosing them"). "More
Options" is row word 7, added the same way. Confirmed in the code:

- **Screen.** `variant_editing_options`, screen 178: loader `0x23764f`
  (stdcall, the request, `ret 4`; 0x9b4 bytes, pushed by `0x147f6d`),
  constructor `0x23768d` (vtable `0x459ae8`), list handler at screen
  `+0x614`. Reached from Settings, Game Variants (`0x2b1b01`), then
  `0x2387d8` or `0x2ba9d0`, on window 5.
- **List constructor `0x2b74c3`** (handler vtable `0x45bdb8`): six item
  widgets, a datum array named by `0x45be10` with a **fixed** count of 6
  (`push 6`), each row's word (`+2`) set to its index by a loop over the
  array's count. The rows don't come from the `goof` pages, so
  `skulls_mp::variant_count` grows it by name; for two rows it must add 2.
- **Row text `0x2b7581`** (thiscall, `[esp+4]` the item, `ret 8`): a
  table of six (word, string id) pairs through `0x24c75c`; an unknown word
  leaves the text alone, so a `before` hook draws ours.
- **Row chosen `0x2b75e8`** (thiscall, `[esp+8]` the row's handle,
  `ret 8`): words above 5 return at `0x2b7711`. Words 0..5 open page
  `E - 1`, 9, `0x1e + E`, `0xc + E`, 0xb, 0xc (E the engine of the record
  being edited) through `0x2c83e6(page, window 5, slot 4, 1 << player)`;
  the page screen (`0x2c7e5b`, rows filled by `0x2c7f7f` from the `goof`
  page via `0x2369b3`) is retail's and has no room for our options.
- **Panes:** the tag has six; `skulls_mp::skip_pane` covers row 6 only and
  must cover 7 too (`0x230427` doesn't check the index).
- **Opening ours:** `skulls_mp::open_request(mask, [screen+0x74],
  [screen+0x78], loader)`, as the Skulls row does, onto the editor's own
  stack.
- **The record the page edits:** docs/game/skulls.md places it at
  `0x54e4a4`, and the row chosen reads the engine at `0x54e4e4`. With the
  engine at record `+0x44` that puts the record at `0x54e4a0`; which is
  right must be settled before the page writes `+0x10c`.

The page is a list class of ours (as `h1_options`) whose rows are
`Row::value`s over the block of that record, written with
`variants::ext::Ext::encode`.

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

### The plan (decided)

1. **One drawable set.** `font::coverage(&Font)` returns the units with a
   glyph of their own. The name set is the intersection over every font that
   can draw a name, after our patches, minus controls, surrogates, `|`
   (decided 9a) and the scanner's alignment units (U+E405..U+E407).
   Computed at boot from the user's fonts, never shipped. **No new glyphs**
   (decided 10/11): the set is what the game's fonts already draw (about
   394 units, above) plus the icon glyphs; nothing is rasterised or drawn
   for names.
2. **The Done check.** A wrapper on `0x238114` replaces the
   letters-digits-spaces rule (flag 2) with "every unit is in the drawable
   set" for profile, variant and clan names; the rest of the check
   (non-empty, uniqueness, the gamertag rule for contexts 12 and 13) runs
   as retail.
3. **Lengths** (decided 7b): gametype names take up to 31 characters, the
   record's whole field. The keyboard's cut to 17 units on open
   (`0x2381de`) becomes 32 for the variant contexts 5..7, and context 7's
   two-unit cut goes. Every other name keeps retail's length.
4. **The accents and symbols pages** are shown in every context (the
   gating at `0x237a46`).
5. **Characters the keyboard has no key for** (`` ` ``, `~`, the Latin-1
   letters missing from the accents page, the symbols, the icons): a kit
   page "More Characters", a grid of the drawable set by block, opened from
   the keyboard (a free button), inserting through INSERT as
   `keyboard_glue` does. Host typing already reaches INSERT; its filter
   becomes the drawable set.
6. **Icons in names** (decided 8b): allowed, in the game and on the site.
   The U+25A1 mapping (`0x140420`, and its copy in `keyboard_glue`) is
   lifted for names. The site draws them from the user's own disc: the
   glyph images are exported at asset-extract time (the
   `h2 live assets extract` game-art path) and served like the other game
   art, never committed; without the art the site shows a neutral
   fallback. Any other glyph the site needs comes from the same export.
7. **The bot gear** (U+2699, bots.md section 8.2) stays as that design
   has it: a glyph patch of its own, untouched here.
8. **Byte for byte.** The game's text is UTF-16 and BMP-only. Names stay
   as typed through the save (signed by the game; directories are hashed,
   so no host path holds a name), the record, the wire and telemetry
   (`String::from_utf16_lossy`, which loses only lone surrogates, which
   the drawable set excludes). Fix the places that change names: drop the
   kit's and sign-in's `.trim()` (refuse leading or trailing spaces
   instead), sanitise only Studio's export file name, escape `##` in
   Studio labels, and count bytes in Studio's hopper clip. On the site,
   stop `text-transform` on names (it turns ß into SS on screen).
9. **Map names:** display names follow the same set, checked by the map
   builder with a warning; ids stay ASCII (they are file names).

### Gamertags and display names (decided 12a)

Gamertags stay ASCII: a gamertag is a login, a case-insensitive key, a
stream name, a URL path and an 8-bit field in the XOnline structures the
game reads. Each account gets a **display name** beside it, which can hold
any character (BMP, the drawable set when typed in the game; the server
stores what it is given, byte for byte). It is a Live server change and its
own slice (8 below):

- **Both homes.** The native server and the Worker store and serve it the
  same way, behind the existing account store traits, with a parity test
  (docs/live, "two homes").
- **API.** Account create and update take it; every response that carries a
  player for people to read (rosters, games, leaderboards, friends, the
  stats endpoints) carries it next to the gamertag, which stays the key.
- **Game.** The HLE writes the display name into the player's UTF-16 name
  (player `+0x44`, which the lobby, scoreboard and carnage report draw)
  where it writes the gamertag today; the XOnline 8-bit fields keep the
  gamertag.
- **Site.** Shows the display name wherever a name is shown, the gamertag in
  URLs and as a secondary line; icon glyphs drawn from the exported art.
- **Rules.** 1..16 UTF-16 units, no controls, no surrogates, no `|`; no
  uniqueness (the gamertag is unique). Impersonation is the risk a
  display name brings: the site shows the gamertag beside it on profile
  pages.

## 5. Slices

1. **Done:** this document and the two record tests.
2. **Done** ("Built: slice 2"): our added choices in the game's editor and
   Studio, `record::ours` (respawn 0 with cycling refused), Bounce
   Practice's magnum, Bumper Cars.
3. **Done** ("Built: slice 3") except the in-game page:
   **The extension block and gravity:** `variants::ext` (encode, decode,
   CRC, zero means none), Studio's section, the hopper tool carrying it,
   gravity applied at game start (the Low Gravity code), and the in-game
   "More Options" kit page (tracing the variant editor's list hooks first).
   Tests: round trips, a bad CRC ignored, the block surviving
   `record::check`, portable files and a hopper slot; gravity on synthetic
   physics memory.
4. **Live transport:** `variant_ext` and the key on the session, the
   guest's apply at activation, capabilities on join and the warnings (the
   host's lobby, the older player where it can), System Link off with its
   warning. Tests in live-proto, live-server (both homes) and live-hle: a
   mismatched key ignored, an older member warned about, System Link never
   applying.
5. **The other extension options**, each traced when reached: damage,
   shield and health scales, movement speed, unlimited grenades, any
   weapon (slots 15, 16 and 20 named from the disc, other tags through
   `0x19eda0`), team size and the team count row.
6. **Names in the game:** `font::coverage` and the drawable set, the
   Done-check wrapper, the page gating, 31-character gametype names, "More
   Characters", icons allowed, the byte-for-byte fixes. Tests: coverage on
   synthetic fonts, the check as a table over contexts and character
   classes, the keyboard filter and lengths, Gamertag validation unchanged.
7. **Names on the site:** the icon glyph export at asset-extract time and
   the site drawing it (with the fallback), the variant's name on
   `GameStarted`, no `text-transform` on names.
8. **Display names** (Live): the account field in both homes, the API, the
   HLE writing it into the game's player name, the site.

## 6. To check in the game (the user's run)

After slice 2:

1. Settings, Game Variants: the list ends with the community variants,
   Bumper Cars last. Bounce Practice's description mentions the magnum; a
   custom game on it spawns everyone with a magnum and nothing else.
2. Bumper Cars: everyone spawns unarmed, no grenades; only Warthogs and
   Ghosts on the map. Note what melee, the HUD and picking a weapon up do
   for an unarmed player (not traced).
3. Edit a Slayer copy: starting weapon offers "None" after "Map Default";
   respawn time offers 1 and 2 seconds before retail's 3; score, time,
   lives and the penalties offer the new steps. Each saves, and the saved
   variant loads again after a restart. The boot log says `variant
   options: N choices added to M option lists`.
4. Respawn 1 s in a game: back in after about one to two seconds (retail's
   floor and its 32-tick slot).
5. With `H2_NO_VARIANT_OPTIONS=1` the lists are retail and the log says
   Bumper Cars was left out.
6. Studio: the new choices show with "(h2recomp)"; respawn 0 with cycling
   (typed in by hand) can't be saved.

After slice 3 (one machine; Studio writes the variant):

3a. Studio: open a saved Slayer variant, "More options (h2recomp)",
    gravity 50, save. Undo and redo move the slider back and forth. In
    the game the variant still loads. Edit and save it in the game's own
    editor, then reopen it in Studio: note whether gravity is still 50
    (whether the game's editor copies the spare bytes is not traced).
3b. A local custom game (or split-screen) on it: jumps go about twice as
    high and grenades fly flatter; the log says
    `variant options: Gravity 50% (local game)` and
    `variant options: gravity A -> B`. With the Low Gravity skull too:
    higher again (a quarter). Back in the menus and into a retail variant:
    normal gravity, and the log says `variant options: none in this game`.
3c. Host it on Live with a second machine: the host's log says `(host)`;
    the guest's says the options were not applied (slice 4), and the
    guest's jumps rubber-band. Over System Link: both say not applied,
    normal gravity.

After slices 3 and 4:

7. Gravity 50% in a custom game on Live with a second machine: both jump
   the same height; the log says the block was applied on the guest.
8. An older build joins that game: the host's lobby warns that it plays
   retail rules.
9. Low Gravity skull with gravity 50%: a quarter of normal.
10. The same variant over System Link: normal gravity on both, and the
    lobby says the extras are off.

After slices 6 to 8:

11. Name a profile "Bob!" and a variant "Wheeeeee! with 31 characters": both
    are accepted, show in the lobby, the scoreboard and the carnage report
    as typed, and a Live game's player and stats pages show them unchanged.
12. "More Characters" from the keyboard types `~`, an accented letter and a
    button icon; the site draws the icon (or the fallback without the
    extracted art).
13. A name with a character no font draws can't be entered.
14. A display name with an icon shows in a Live lobby and on the site,
    with the gamertag in the URL.

## Decisions (answered 2026-11-18)

1. Bounce Practice: a magnum and nothing else (secondary None).
2. Bumper Cars: added as a built-in.
3. Respawn: retail's 1 s floor stays; 1 s is the fastest choice; no floor
   or tick hooks; our editors refuse respawn 0 with cycling.
4. Extras live in the record's spare bytes.
5. Older builds may join a game with extras, with a warning to the host
   (and to the older player where its build can show it); no refusal. The
   desync risk is in section 3.
6. System Link: extras off, with a warning.
7. Gametype names up to 31 characters.
8. Icon glyphs in names, in the game and on the site; the site's images
   come from the user's disc at asset-extract time, never committed, with
   a neutral fallback.
9. `|` stays out of names.
10, 11. No new glyphs: names use what the game's fonts already draw. The bot
    gear's patch stays as designed. Any glyph the site needs comes from the
    same export.
12. Gamertags stay ASCII; a separate display name holds any character, in
    both homes, shown by the game, the API and the site.
13. All the extension options: gravity, damage, shield, health and speed
    scales, unlimited grenades, any weapon, team size; each traced when
    reached.

## Open questions

- Which font slot draws names in each place (fonts.md); the audit above
  assumes `conduit` and `handel_gothic`.
- What the game's editor shows for a storable value it doesn't list.
- The address of the record the game's editor edits (`0x54e4a0` or
  `0x54e4a4`), for the "More Options" page.
- What the game does for an unarmed player (Bumper Cars, starting weapon
  None): melee, the HUD, picking a weapon up.
- The unoffered vehicle values and setting 80.
- The damage, vitality and movement multipliers' code and constants.
- The unit's grenade counts.
- Readers of the spare bytes through the session copy (`S+0x4db0`) and the
  pending options (`0x4ed3a8`); a full scan before the block ships.
- Host migration: a new host holds only the wire fields, so it would need
  the block from the session as a guest does.
- Whether the game's own editor copies the spare bytes when it edits and
  saves a variant (checklist 3a).
- `coop_probe::Session::class` reads `S+0x4980`, the byte the lobby
  setters compare with 1 (`0x63f00`), and `skulls_mp::Role` calls it 1
  System Link, 2 Live. The network class `0x59840` checks is `S+0x18`;
  `variant_ext` reads that one. Which the `+0x4980` byte is was not
  traced.

## Built: slice 2

- `variants::extra::OPTIONS`: our added choices (setting, value, label),
  and `merge`, which puts them among a definition's retail values (before
  the first larger value, never a value the disc already has). Starting
  weapon "None"; respawn 1, 2, 45, 60 and 120 s; more score steps per game
  type; time limits to 9 h; lives 4, 5, 10, 25, 50; longer suicide and
  betrayal penalties. Every one passes `record::refused`.
- `variants::schema::with_extras`: the disc's choices with ours merged
  (`Choice::ours`); Studio marks ours "(h2recomp)".
- `variants::record::ours` and `refused`: respawn 0 with cycling. Studio's
  save, the hopper tool's link and its checks, and portable import use
  `refused`; `Schema::validate` reports it as `Issue::Ours`.
- `boot::variant_options`: inside the community variants' wrapper on
  `0x149f88`, before their check, each `sily` definition with choices of
  ours gets a merged copy of its options block in our page at
  `0xa0fd4000` (retail options byte for byte), labels in the "variant
  options" string family (`0x00ffe100..`, answered by the community
  string hook). A definition already pointing into the page is skipped; one
  that doesn't fit stays retail. `H2_NO_VARIANT_OPTIONS=1` turns it off.
- `community_variants`: `StartingWeapon` takes `Option<Weapon>`; Bounce
  Practice has the magnum; Bumper Cars is the thirteenth variant
  (description number 92).

## Built: slice 3

- `variants::ext`: `Ext` (gravity, damage, shield, health and speed
  percents, unlimited grenades, two weapon ids, team size), `encode`
  (all zero for the default) and `decode` (`Block::None`, `Ext` or
  `Ignored(Why)`: magic, version, CRC, range), `in_effect`, `summary`,
  `crc32`. `GameVariant::ext_block`, `ext_state`, `ext` and `set_ext`
  (refuses out-of-range values). `Schema::validate` reports a block the
  game would ignore as `Issue::Ext`; it is a warning, not refused.
- The block is in every copy of the record, so saved variants (signed
  over the whole record), `.h2variant` files, Studio's undo and hopper
  slots keep it with no further code. The hopper tool's editable fields
  inside the variant are the raw string (`+0x00`), flags (`+0x48`) and
  second dword (`+0x58`), all before `+0x10c`.
- Studio: `Action::SetVariantExt`, `VariantEditor::set_ext`, the "More
  options (h2recomp)" section (gravity).
- `boot::variant_ext`: `in_game` and `decide` (above); `skulls_custom`'s
  tick applies Low Gravity times the variant's gravity through the
  generalised `gravity_step`.
- Not built: the in-game "More Options" page (section 3, "The variant
  editor's page list", has what the trace found).

## Tests

`cargo test -p variants`, `cargo test -p hopper-edit`,
`cargo test -p studio`, `cargo test -p boot --lib` (community_variants,
variant_options, variant_ext, skulls_custom, string_ids). The slice 3
tests:

- `ext::crc_matches_zlib`, `the_default_is_all_zero_and_reads_as_none`,
  `a_block_round_trips`, `a_damaged_block_is_ignored` (every byte flipped
  in turn), `out_of_range_values_are_ignored_even_with_a_good_crc`,
  `a_damaged_block_shows_as_an_issue`;
- `record::the_extension_block_survives_the_cleanup_and_every_setter`;
- `save::a_saved_file_keeps_the_extension_block`,
  `portable::the_extension_block_travels_in_a_portable_file`,
  `panel::extension_options_edit_and_undo_per_option`;
- `hopper-edit` `model::a_hopper_slot_carries_the_extension_block`;
- `studio` `app::tests` (the variant actions test sets and undoes it);
- `boot` `variant_ext::alone_or_hosting_applies_system_link_and_guests_dont`,
  `reads_the_active_records_block_in_multiplayer_only` (globals and a
  squad session in synthetic memory), `skulls_custom`
  `a_variants_gravity_composes_with_low_gravity_on_physics_memory`.

The slice 2 tests:

- `extra::every_added_choice_is_one_the_game_keeps`,
  `added_choices_go_in_value_order_and_never_twice`,
  `the_schema_shows_ours_beside_the_discs`;
- `record::respawn_zero_with_cycling_is_refused`;
- `variant_options::our_choices_join_the_retail_lists_in_value_order`
  (retail blocks unwritten, a repeated definition extended once, a second
  load a no-op), `a_list_the_page_cannot_hold_stays_retail`,
  `every_added_choice_has_a_label_id_of_ours`;
- `community_variants::bumper_cars_needs_our_none_and_bounce_practice_has_a_magnum`.

Slice 1, `cargo test -p variants --lib record`:

- `the_cleanup_keeps_values_the_editor_never_offers`: weapon 1, 15 and 20,
  respawn 0, score, time, lives and penalties at 0x7fff and team count 8
  pass `record::check` and round-trip; one past each limit is refused.
- `the_cleanup_ignores_the_spare_bytes`: for every game type, bytes written
  to the spare ranges pass `record::check` and round-trip, and no setting
  the writer knows lives there.
