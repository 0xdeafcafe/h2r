# Game variants: list entries, lineup and copies

Status: part of [rules-variants.md](rules-variants.md); see there for status, labels and the index. This page: built-in list entries, the default Live lineup, copies, saves and files.

### Built-in list entries

The `variant` items of the packages that are on name the scripts that are
built-in variants, in list order (`rules::builtin::Registry::listed`, made
by `workspace::script_registry` and given to `community_variants::install_registry`
at boot; `rules::builtin::LISTED` is the first-party packages' list as
constants, which the tests and a game with no package set run on). A
package off takes its entries out of the list, and the list holds
`community_variants::MAX_ENTRIES`. `community_variants.rs` keeps its hooks and its
merged block and loses `VARIANTS`, `Setting`, `Variant`, `Category`,
`Tweak` and `superbounce_ext`. At `BUILTINS_LOAD`, for each listed
script (compiled once, kept):

| Part of the entry | From |
|---|---|
| name | `header.name`, a space and U+E006; the string hook serves it at `name_id(i)` |
| game type | the record's retail loader template (`GameType::menu_template`), as an index in `GameType::ALL` |
| pairs | the record of the script: its value of every setting of the type that the editor offers (`game_variants::inject::pairs`, the store's own rule, **Code**) |
| description | `header.about`, at description number `DESCRIPTION_BASE + i` |
| icon | by origin, not by script: U+E006 for a compiled-in script, U+E007 for the store's |
| row | `header.game_type` (`Row::of_type`) |

The entry carries every offered pair, not only the script's, so the
disc's per-type defaults never show through.

The pairs cannot carry the `hx` block. When the original loader returns,
the wrapper restores each community and store variant's block with
`community_variants::write_blocks`. The script index uses a Slayer
template in the loader and regains index 6 and its empty type area at
this point. The record a player selects or copies carries the script
reference. `community_variants::filed_type` resolves its declared type.

`DESCRIPTION_BASE` moves to 40, giving the listed scripts 40 to 93 (54
entries; retail uses 0 to 38 and the store 94 to 100).

**Porting the table.** Each of the fourteen becomes a file in
`content/modes/rules/`, a `variant` item of the `modes` package. All fourteen port with what exists:

| Variant | `type` | `uses` | Notes |
|---|---|---|---|
| SWAT, Duel, Tower of Power | `slayer` | `slayer` | |
| Jousting, Cops and Robbers | `party` | `slayer` | |
| Bumper Cars | `party` | `slayer` | `starting_weapon = none` is one of our added choices; without `variant_options` the entry is left out, as today |
| Glitch Hunt | `party` | `slayer` | |
| Bounce Practice | `party` | `slayer` | `bounce_practice.rules`: settings only; the bounce-scoring script is `bounce_hunt.rules` |
| Zombies | `party` | `slayer` | the honour rules stay in `about`. `infection.rules` enforces them but waits on `set_team` (untraced, `verify::unhostable`) |
| Sun King | `koth` | `koth` | |
| Wheeeeee! | `oddball` | `oddball` | |
| Clint Eastwood | `juggernaut` | `juggernaut` | |
| Hide and Seek | `party` | `juggernaut` | a row's list is Slayer's and Juggernaut's variant lists filtered (slice 12) |
| Superbounce | `party` | none | `superbounce.rules` gains its settings; the table's are deleted |
| Gun Game | `party` | none | `gun_game.rules`: a kill gives the next rung's weapon at once (`set_loadout`); players spawn unarmed (`starting_weapon = none`) and the map has no weapons |
| Race | `party` | none | `race.rules`: the map's hills are its checkpoints, run in number order, so every map with hills races |
| VIP | `party` | none | `vip.rules`: a VIP per team, worth `vip_points` to kill, the role passing to a teammate; the script ends the round at the score limit |
| Siege | `siege` | none | `siege.rules`: the map's territory and CTF flags are its zones, and neither is required of the script, but the start gate refuses a map with no site and no final stand ([rules-start-gate.md](rules-start-gate.md)) - with no territory the assault goes straight at the flag stand, with no stand the sites are the whole assault, and a run the gate does not guard with neither draws the round at its start ([rules-engine.md](rules-engine.md#phases-attack-and-defend)); players spawn unarmed (`starting_weapon = none`, our added choice) and `set_loadout` arms each side; one life a round (`lives = 1`, our added choice) holds each death for its side's respawn wave |

Siege, Gun Game, Race and VIP, like Superbounce, are scripted modes after
the settings scripts and are examples too (`builtin::examples`).

### The default Live lineup

`rules::builtin::MATCHMAKING` lists ten `uses` scripts, one for
each built-in matchmaking variant of Live's default playlists
([live-matchmaking.md](../live/live-matchmaking.md), 6.8): Rumble Slayer,
Team Slayer, Team Snipers, Oddball, Teamball, King of the Hill, Team King,
Classic CTF, Assault and Territories. A script is its type's settings
and nothing else, so the type's built-in rules score the game, as they do
for a record with no mode. Each names the three settings the game's hopper
defaults writer clears (`0x00193010`): `setting_twelve`,
`setting_thirteen` and `join_in_progress`. `live_proto::hopper::defaults`
makes every default variant with `record_for`, so each default entry is
mode 2, and a hopper takes scripted variants only.

The table has the rows these scripts need, appended after the others:

| Row | Setting | Stored | Values |
|---|---|---|---|
| `setting_twelve` | 12 | flags bit 4 | on, off |
| `setting_thirteen` | 13 | flags bit 5 | on, off |
| `join_in_progress` | 14 | `+0x58` | 0 to 2 |
| `respawn_modifier` | 25 | `+0xa8` | `inheritance` 0, `cycling` 1, `none` 2 |
| `second_heavy_vehicle` | 80 | `+0xd1` | `none` 4, or 0 to 4 |

Settings 12 and 13 have no label of the game's here, so the rows are
named by number. Classic CTF's type-area bits `0xc` are the existing rows
`ctf_flag_at_home` (setting 52, bit 2) and `ctf_touch_return` (setting 53,
bit 3), and Assault's `+0x14` is `ctf_flag_type` (setting 59, `+0x104`).
`crates/formats/variants/tests/settings_table.rs` writes each of these
through `record_for` and reads it back.

### Copies, saves, files and Live

- **A copy or a save** is the record. It has the settings as fields, the
  option values in the block's 32 bits and the script's prefix, so it
  needs nothing new. From the prefix a machine finds the script
  (compiled in, or in the game-variant store) and reads the row, the
  description and the quick marks.
- **The record wins.** Once a variant exists as a record, the script's
  `settings` block is not read again for it: an edit in the editor, a
  Quick Options change and a `.h2variant` record all stand. The block is
  only what a new variant of the script starts from.
- **`.h2variant`.** `portable::from_script(ir, source)` (new) makes the
  bundle of a script's own variant: the record of the script, the
  script's name and `about` as the bundle's, and the script part.
  `h2 variants from-script <file.rules> [--option KEY=VALUE]... [-o DIR]`
  (new) writes it. `h2 variants port <input> --script FILE` keeps the
  input's record settings and attaches the script; the block's extras are
  the script's settings, not the input's. The IR must match its hash. `Bundle::integrity` accepts the declared engine or the script index, where the record names its script explicitly.
- **Live.** `bundle_for_script` (`crates/live/live-server/src/game_variants.rs`)
  builds its record with `record_for` in place of the bare stock record,
  and takes the description from `about` when the request gives none.
  The script view of `/v1/rules` gains `scoring` (`script`, or `carrier` for a script with a `uses` line), `about`, `settings`
  (name, label, value text) and `quick`, for the web's script and
  variant pages. Both homes serve the same fields; the parity suite
  covers them.
- **The store and the list.** A store variant's entry is built from its
  bundle's record as now (`inject::entries`), so a Live variant made
  from a script with settings lists with those settings.
- **Guests.** The settings reach a guest in the game's own variant
  record and the block as `NewSession.variant_ext`. A guest without the
  script plays the same game and sees retail's panel lines.
