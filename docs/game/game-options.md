# Game options the game doesn't allow

Status: Partly built. Sections 1 to 3 are built: the record's limits, our added choices, the extension block with its Live transport, the "More Options" page, movement speed and the weapons outside the globals list. Team size, slots 15, 16 and 20 in the game's own editor rows, and the lobby texts are not built; section 4 (names with any character) is a design.
Code: `crates/formats/variants` (`ext`, `extra`, `record`, `schema`), `crates/extensions/game/src/variant_options.rs`, `variant_ext.rs`, `variant_page.rs`, `variant_weapons.rs`, `community_variants.rs`. Tests: listed under "Tests".
Switch: `features.variant-more-options-row`, `features.variant-more-options`, `variant-vitality`, `variant-speed`, `variant-weapons`, `variant-ext-live` (feature ids), listed in [switches.md](../runtime/switches.md).

The goals:

- more game variant options: no weapon as the starting weapon, instant
  respawn, gravity, and anything else the retail editor won't let you set;
- Bounce Practice (a community built-in) starts with a magnum;
- any character in player names, gametype names, map names and every other
  name a player types (design, section 4).

The evidence is static: `h2 disasm` on the retail XBE (SHA-256
`03215919…935d`), the repository's code and the docs it links. Each claim is
**confirmed** (seen in code or data) or **inferred**. No retail table, string or glyph is copied here. Where this
says what the retail editor offers, it gives the shape (lowest, highest,
missing values); the lists themselves are read from the user's disc at run
time (`variants::Schema::from_map`).

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
`variants::wire` is the codec's model, field by field and bit by bit,
checked against the lifted original
(`crates/rewrites/engine/tests/variant_record_differential.rs`); the
`rules` feature answers both routines with it for a record of the script
index ([game-types-without-carriers.md](game-types-without-carriers.md#no-retail-engine-object)).

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

### Starting weapon "None"

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
picking a weapon up) is not traced ("Not verified").

### Any weapon as the starting or secondary weapon

- **Storable:** values 15, 16 and 20 are slots of the same globals list the
  editor doesn't offer (list entries 11, 15 and 17 by the jump table).
  They are real references on the disc (random picks 15 and 16). Our
  editor names them by reading the slot's tag at run time, as the schema
  does; nothing is listed here. Studio offers them
  (`variants::schema::{weapon_slots, weapon_label, add_weapon_slots}`):
  the main menu map's `mulg` tag, universal element 0, list `+0x68`; the
  label is the tag path's last part (a path ending
  `\beam_rifle\beam_rifle` reads "Beam Rifle"). The game's own editor doesn't offer
  them yet: its lists take our labels from fixed strings
  (`boot::variant_options`), and these need the disc's names.
- **Needs the extension:** a weapon that isn't in that list (one of the
  other weapon tags the map has). The byte can't hold it past 0x14. An
  extension field names it, and a hook at `0x19eda0` returns that tag for
  our value; the record keeps a fallback weapon in `+0xd6` so an older build
  gets something sensible. `variants::ext_weapons::TABLE` lists the weapons
  by id and tag path (never a datum, which differs per map); `variant_weapons`
  tells the primary from the secondary by the call's return address
  (`0x19f056`, `0x19f063` primary; `0x19f042`, `0x19f07b` secondary; the
  routine's own retry at `0x19ee18` is left alone), reads the loaded map's
  file for the `weap` tag of the path (through the VFS, once per map and
  scenario, as our skulls' tag is read) and returns it in EAX. A map
  without the tag, an id the build lacks, or a game that is not this
  machine's to start (a Live guest, System Link) keeps the record's
  weapon; a Superbounce game with the stick on keeps the stick as its
  secondary. A block holding an id the table lacks is ignored whole, as
  any out-of-range value is. A rules script's `set_loadout` and a bundle
  that carries a script never name these ids: the bundle check refuses a
  weapon id beside a script (`PortableError::Extras`).

### Fast respawn

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

There are no hooks. Retail's one-second floor and its 32-tick slot
stay, and the fastest choice we offer is **1 s** (with 2 s, beside
retail's 3 s and up). Respawn 0 is never offered, and our editors refuse
any record with respawn 0 and the cycling modifier (`record::ours`, used
by Studio's save, the hopper tool's link and checks, and portable import),
since that is the one combination that divides by zero. With 1 s, cycling
divides by one second and is safe.

### Gravity

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
Link (the squad session's network class at `+0x18` is 1, the
field `0x59840` checks before answering a LAN search). A Live guest
applies the block its host sent (section 3, "How it reaches every player"):
the wire drops the spare bytes, so its own record has none. Gravity is
simulated on every machine, so a guest without the block predicts with the
game's gravity against a host with the scale (the desync in section 3). The log says
`variant options: Gravity 50% (host)` (or why not) when the decision
changes, and `variant options: gravity A -> B` on each write.

### Bounce Practice's magnum and Bumper Cars

Bounce Practice is a community built-in
(`bounce_practice.rules`, Slayer, listed in `rules::builtin::LISTED`). It
starts with a magnum and nothing else (`starting_weapon = magnum`,
`secondary_weapon = none` in its `settings`), and its description says so. Both
are retail choices, so `usable()` keeps it on every disc. Glitch Hunt
keeps its sword and rockets.

The "None" start also makes **Bumper Cars** possible
(`bumper_cars.rules`): a Slayer built-in with no weapons, no grenades, nothing on the map, Warthogs
and Ghosts and nothing else to drive, 25 points. Its unarmed start is one
of our choices, so `usable()` keeps it only when the option lists carry
ours (`variant_options`); without them it would be left out with a log
line, as any variant with a value the game doesn't offer is.

### Our added choices

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
  that doesn't fit stays retail.
- `community_variants::entries`: builds each listed script's entry;
  Bounce Practice has the magnum; Bumper Cars is description number 52.

### The options to add

The table lists each option, whether the record can store it, and how it is offered.

| Option | Standing | How |
|---|---|---|
| Starting weapon "None" | storable (primary 1) | offer it; retail code handles it |
| Fast respawn | storable (1 s, 2 s) | offered by us (no hooks, retail's 1 s floor stays); respawn 0 with cycling refused |
| Gravity | needs the extension | scale `[0x51e9c4]` at game start, as Low Gravity |
| Bounce Practice magnum, Bumper Cars | offered; Bumper Cars needs our "None" | built-ins |
| Any weapon, from the globals list | storable (15, 16, 20) | name them from the disc; Studio offers them, the game's editor does not yet |
| Any weapon, outside that list | needs the extension (built) | the block names a weapon of ours (`variants::ext_weapons`) and a hook on `0x19eda0` answers its tag; the record keeps a fallback |
| Score above 250 | storable to 0x7fff | offer more steps |
| Time above 1 h | storable to 0x7fff s (9 h 6 min) | offer more steps |
| Lives above 99 | storable to 0x7fff | offer more steps |
| Respawn above 30 s | storable to 0x7fff s | offer more steps |
| Suicide and betrayal penalties beyond retail's | storable to 0x7fff s | offer more steps |
| Damage multipliers beyond on/off | needs the extension (built) | the record has two booleans (flag bits 12, 13), read through the trait query, engine vtable slot `+0x8c` (`ENGINE_TRAIT_QUERY` `0x24dda0`, trait 2 extra damage, 3 resistance; Slayer's slot, and where each other engine's own query, Juggernaut's `0x1926c0` among them, falls back). Ours scales the amount of each damage instance on a player instead ("Damage, shield and health" below) |
| Shield and health multipliers | needs the extension (built) | shields are a 0..2 choice (`+0x88`); ours scales the amount the shield and body steps take |
| Player speed for everyone | needs the extension (built) | only Juggernaut and ball carrier speed exist (i16 0..2); ours scales the player's movement multiplier the engine's speed step keeps ("Movement speed" below) |
| Unlimited grenades | needs the extension (built) | only a starting on/off bit; the host keeps every player's unit at four or more of each type each tick (the unit's counts at `+0x23e`, as the Unlimited Grenades skull) |
| Mixed vehicle sets | mostly offered; some storable | each slot (light, second light, heavy, flying, two turrets) is its own byte, so mixes exist today; values in range but not offered (75: 2; 76, 77: 4; 79: 2, 3; 81, 82: 2, 3; 83: 18) and setting 80 (no row, `+0xd0` 0..4) are not traced and are not offered until they are; a vehicle outside the sets needs the extension and a placement hook |
| Team count | storable (setting 93, `+0xb4` 0..8; built) | the retail editor has no row for it; a row on our "More Options" page and in Studio. Traced: the team set builder `0x157790` reads `[G+0x1f0]`, takes 0 as 8, caps it at the map's teams (`[[0x4e9ae8]+4]`, a mask) and adds the lowest unused teams to the players' until there are that many; the mask goes to `[[0x4e9ae8]+6]` |
| Team size | needs the extension (not built) | nothing in the record; each console's profile picks its own player's team (configuration `+0x7c`) and no lobby code that limits a team's size is traced ("Not built") |
| More than 16 players | impossible | the engine's player limit |

## Pages

| Page | What |
|---|---|
| [Game options: the extension](game-options-extension.md) | The option extension. |
| [Game options: names and tests](game-options-names-tests.md) | Names with any character, tests and open items. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## 3. The extension

See [Game options: the extension](game-options-extension.md#3-the-extension).

### Where the extension lives

See [Game options: the extension](game-options-extension.md#where-the-extension-lives).

### Showing them

See [Game options: the extension](game-options-extension.md#showing-them).

## 4. Names with any character

See [Game options: names and tests](game-options-names-tests.md#4-names-with-any-character).

## 5. Tests

See [Game options: names and tests](game-options-names-tests.md#5-tests).

## Not verified

See [Game options: names and tests](game-options-names-tests.md#not-verified).

## Related

See [Game options: names and tests](game-options-names-tests.md#related).
