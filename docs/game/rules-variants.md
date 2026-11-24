# Game variants defined by their rules script

Status: Built. Every slice, 1 to 12, is in.
Code: `crates/formats/rules/src/settings.rs` (new, the settings a script may name), `crates/formats/rules/src/ir.rs` (the IR's settings and quick marks), `crates/formats/variants/src/portable.rs` (`record_for`, a script's record), `crates/extensions/game/src/community_variants.rs` (the built-in entries), `crates/extensions/game/src/variant_quick.rs` (new, the lobby's Quick Options), `crates/extensions/game/src/variant_quick/page.rs` (new, the Quick Options dialog), `assets/ui/screens/variant-quick.yaml` (new, its doc). Tests: named per slice under "Slices".
Switch: `variant-quick` (on by default, with `lobby-options`), listed in [switches.md](../runtime/switches.md). The built-in entries stay under the existing features (`tweakers`, `rules`, `lobby-options`).

A game variant is one `.rules` file: its rules, its own options, and
every record setting it changes (starting weapon, shields, score limit,
what is on the map). The script also says which of those the lobby's
Quick Options shows. Each built-in community variant is a script
compiled into the game, and its list entry is built from the script, so
no Rust table describes a variant. The rule that must hold: the record
is where a variant's values live once it exists, and the script is where
they come from; the same script gives the same record on every machine,
with or without a disc.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## How it works

### The settings block

A script names record settings in one `settings` block, after the header
and before its options:

```
rules "SWAT" 1
type slayer
uses slayer
about "No shields, no motion sensor, battle rifles and magnums. One headshot kills. 50 points to win."
quick score_limit, starting_weapon, secondary_weapon

settings {
    team_play = true
    score_limit = 50
    shields = none
    motion_sensor = false
    starting_weapon = battle_rifle
    secondary_weapon = magnum
    weapons_on_map = none
    grenades_on_map = false
    starting_grenades = false
    overshield_pickups = false
    camo_pickups = false
}
```

**Names.** `rules::settings::TABLE` (new) is the one list of settings a
script may name. A row has a name, a label of ours ("Starting Weapon"),
a kind, where it is stored and the engines it applies to. The names are
ours, in snake case, from `variants::record::symbolic_name` (**Code**);
a setting a script already reads as `variant.<name>`
(`caps::VARIANT_FIELDS`, **Code**) keeps that name, so reading and
setting use one word (`score_limit`, `time_limit`, `lives`,
`respawn_time`, `team_play`, `friendly_fire`, `rounds`). Rows are
appended, never reordered: a blob names a setting by its index, as it
names a variant field.

| Kind | Written as | Example |
|---|---|---|
| bool | `true`, `false` | `motion_sensor = false` |
| int | a number in the row's range | `score_limit = 50`, `lives = 3` |
| seconds | a time literal in whole seconds | `time_limit = 300s`, `respawn_time = 3s` |
| choice | a value's name from the row's list, or a number in the record's range | `starting_weapon = shotgun`, `weapons_on_map = rockets`, `light_vehicle = none` |
| percent | a number | `gravity = 50` |

A setting whose zero means none also takes `none` (`score_limit = none`,
`time_limit = none`). The choice lists are the symbolic enums the table
uses (`Weapon`, `WeaponSet`, `Shields`, `LightVehicle`, `Turret`,
`Speed` in `community_variants.rs`, **Code**), moved into the table with
a label each. A choice the table has no name for is written as its
number; the table grows by appending names.

**Where a setting is stored.** A row is either a record setting (a
`variants::record::SettingId` per engine) or a field of the `hx` block
(`variants::ext::Ext`): `gravity`, `damage`, `shield`, `health` and
`unlimited_grenades`, the extras the game applies
([game-options.md](game-options.md#showing-them)). `team_count` is the
record's setting 93. The block's `superbounce` flag has no row: a script
asks for the mode with `requires hud.superbounce`.

**Per type.** Three cases, all decided by the table:

- A setting every game type has applies on every type.
- `score_limit` is one name for the type's own score setting
  (`GameType::score_setting`, settings 1 to 9, **Code**).
- A game-type setting carries its type in its name, as the variant
  fields do (`juggernaut_movement`, `oddball_ball_speed`,
  `king_moving_hill`, `ctf_flag_type`), and applies only on that type's
  record (the CTF names on `ctf` and `assault`, where the two engines
  pair their settings). Naming one on another type is the error
  `setting.type`: "juggernaut_movement is a Juggernaut setting; this
  script's record is Slayer's".

**Settings left out.** A setting the block does not name has the stock
value: what the game's defaults writer gives a new record of the type
before the menu applies anything (`0x19d220`, **E**; modelled without a
disc by `live_proto::hopper::variant::VariantRecord::stock`, **Code**).
The disc's per-type defaults in the `goof` tag play no part, so a script
gives the same record offline, on Live and in the game. A script that
wants a value the menu's Create New would have given states it.
`h2 rules settings <file.rules>` (new) prints every setting of the
type with its value and whether the script or the stock record set
it.

**Checks, in three places.** No check falls back to another value.

1. *The compiler* (`lang/check.rs`), with no disc: the name is in the
   table (`setting.unknown`, with the nearest name as its fix), applies
   to the type (`setting.type`), is named once (`setting.twice`),
   and the value is of the row's kind and in its range
   (`setting.value`). The ranges are the record clean-up's
   (`variants::record::check`, **Code**).
2. *The record* (`variants::portable::record_for`, new): the built
   record must be one the game keeps and our rules allow
   (`record::refused`, **Code**: Juggernaut is never team play, a
   respawn time of 0 with cycling). `h2 rules check`, `Bundle::check`
   and Live's publish all build it and refuse the script by that rule's
   words.
3. *The game*, when the variant list loads: every `(setting, value)` of
   the entry must be one the editor offers on this disc, with our added
   choices (`community_variants::offered`, `variants::extra`, **Code**).
   An entry that fails is left out and named in the log, as a table
   variant is today.

### A script that keeps its carrier's rules

Thirteen of the fourteen community variants are settings and nothing else.
Such a script says so in its header:

- `uses <type>`: the game plays that retail type's built-in script
  (`slayer`, `ctf`, `assault`, `oddball`, `koth`, `juggernaut` or
  `territories`). The script has no handlers, no variables and no
  options (`uses.handlers`). It may have `type`, `points`, `teams`,
  `requires`, `settings`, `quick` and `about`.
- No `uses` line: the script is the game's one scorer, as every script
  is ([rules-engine.md](rules-engine.md#which-engine-index-a-script-rides)).
  A script with neither handlers nor `uses` is the error `scoring.none`:
  "this script scores nothing. Write `uses slayer` to play Slayer's
  rules."

The `type` line is separate: it files the variant in the game type
list's row and picks the start line, and a `uses` script need not name
the type it uses (Hide and Seek is `type party` and `uses juggernaut`).
The design is [game-types-without-carriers.md](game-types-without-carriers.md).
The record still names the script (mode 2 and its prefix), so the
variant keeps its row, its description and its quick marks wherever the
record goes. On the host, the start observer loads a `uses`
script, checks its header (prefix, engine, policy) and then runs the
used type's built-in script in its place (`runtime::used_builtin`,
`rules::builtin::of_type`) under the variant's own record: the VM reads
the record's settings (team play, score limit, time limit), and the
built-in plays its own default options, since the script has
none. From there it is a running script like any other: the built-in is
the one scorer, `rules.state` reports the built-in (`scoring: carrier`,
the wire's word for a script with a `uses` line), and the Live session
names the built-in's hash, which every build has
([rules-engine.md](rules-engine.md#the-host-runtime), "A `uses`
script").

A built-in this host cannot run yet is refused by name, as any script
is: "Sun King uses the built-in King of the Hill
script, which this host cannot run yet: ...". Of the thirteen settings
scripts, the nine on Slayer (Zombies, Jousting, SWAT, Duel, Tower of
Power, Cops and Robbers, Bounce Practice, Glitch Hunt, Bumper Cars) run
`slayer.rules`. The other four are refused: Clint Eastwood and Hide and
Seek because `juggernaut.rules` answers on `round_start` and `quit`,
Sun King because `koth.rules` answers on `round_start` and reads
`variant.king_moving_hill`, and Wheeeeee! because `oddball.rules` calls
`spawn_objective` and answers on `pickup` and `drop`, none of which the
host does yet ([rules-engine.md](rules-engine.md#the-host-runtime)).

### Quick Options marks

One header line names what the lobby's Quick Options shows, in row
order:

```
quick time_limit, bounce_points, suicide_costs
```

- A name is a record setting of the table or one of the script's
  `option`s. The two share one namespace: an option named like a setting
  is the error `option.setting`.
- A setting need not be in the `settings` block to be marked; its value
  is then the stock one until the host changes it.
- At most five names (`ir::MAX_QUICK`). A sixth is the error
  `quick.many`: "Quick Options has five rows". An unknown name is
  `quick.unknown`, a repeated one `quick.twice`. Five is the pregame
  panel's row count (`lobby_options::PANEL_ROWS`, **Code**), and the
  dialog keeps the same five so the panel always summarises what the
  dialog changes.
- A script with no `quick` line has no Quick Options rows. Nothing is
  picked for it.

**The full editor** shows everything, marked or not. The record settings
are on the game's own pages (Change Rules, then Edit, which copies a
built-in first). The `hx` extras the table has rows for (gravity, damage,
shield, health, unlimited grenades) are the table's rows, so More
Options edits them as a script's `settings` block names them, and the
extras no row stores are not offered. The extras and the script's options
are on More Options (`variant_page.rs`): after its fixed rows, one row per `option`
in declaration order, with the IR's label and description, stepping
through the option's range or its enum's names and writing the option
bits of the edited record's block (`variant_page::EDITED_BLOCK`). This
is the in-game half of roadmap phase 10 of
[rules-engine.md](rules-engine.md#roadmap).

### The IR

`ir::Header` carries these fields and `ir::Ir` two tables:

| Field | Encoding | Content |
|---|---|---|
| `header.uses` | one byte, after the type byte | the retail type the script uses, `0xff` for none; any other value fails to decode |
| `header.points` | a string, after `teams` | the score label, 1 to 32 bytes |
| `header.about` | a string | the description, at most 150 UTF-16 units (`variants::portable::MAX_DESCRIPTION_UNITS`); empty when the script has none |
| `settings` | a count, then `(u16 table index, i32 value)` | the settings the script names, in ascending index order |
| `quick` | a count of at most 5, then `(u8 kind, u16 index)` | kind 0 a table index, 1 an option index; in the script's order |

The encoding is canonical: `decode` refuses settings out of order, an
index twice and a quick count over five. `verify::load` refuses an index
past the table, a setting that does not apply to the header's type, a
value outside the row's range, a quick mark that names nothing, and a
`uses` blob with handlers, slots or options. So a blob from
anywhere holds to the rules the compiler applied.

The IR carries only what the script names. The stock record is a fixed
fact about the retail game, not part of a script.

**What the hash covers.** `Ir::id` is the SHA-256 of the whole encoding,
so it covers the settings, the marks, the description and the `uses`
line with the rules. Two variants that differ only in a setting are two
scripts with two hashes, which is what makes each community variant a
file of its own. The consequence: a saved copy names the exact script it
was made from, and editing a compiled-in script orphans the copies made
from the old text. An orphaned copy keeps its settings, is listed under
its type's game type row and is refused by name at the start
(`runtime`'s "its full hash is not known"), as any record naming a
script the machine lacks. `IR_VERSION` stays 1; blobs kept anywhere
(Live's `/v1/rules`, a store's `scripts/*.ir`) are rebuilt from source.

### The record of a script

`variants::portable::record_for(ir, options)` (new) is the one builder:

1. `GameVariant::stock(engine)` (new; the stock record, in the
   `variants` crate);
2. each IR setting through `GameVariant::set`, or into the `Ext` for a
   block row;
3. the name (`header.name`);
4. the script reference: mode 2, the hash's first 8 bytes and the option
   bits (`Ir::pack_options`, the defaults when none are given).

Every place that makes a variant from a script calls it: the built-in
entries, `Bundle` construction, Live's `bundle_for_script` and Studio.

### Built-in list entries

`rules::builtin::LISTED` (new) names the scripts that are built-in
variants, in list order. `community_variants.rs` keeps its hooks and its
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
`crates/formats/rules/src/builtin/`. All fourteen port with what exists:

| Variant | `type` | `uses` | Notes |
|---|---|---|---|
| SWAT, Duel, Tower of Power | `slayer` | `slayer` | |
| Jousting, Cops and Robbers | `party` | `slayer` | |
| Bumper Cars | `party` | `slayer` | `starting_weapon = none` is one of our added choices; without `variant_options` the entry is left out, as today |
| Glitch Hunt | `research` | `slayer` | |
| Bounce Practice | `research` | `slayer` | `bounce_practice.rules`: settings only; the bounce-scoring script is `bounce_hunt.rules` |
| Zombies | `party` | `slayer` | the honour rules stay in `about`. `infection.rules` enforces them but waits on `set_team` (untraced, `verify::unhostable`) |
| Sun King | `koth` | `koth` | |
| Wheeeeee! | `oddball` | `oddball` | |
| Clint Eastwood | `juggernaut` | `juggernaut` | |
| Hide and Seek | `party` | `juggernaut` | a row's list is Slayer's and Juggernaut's variant lists filtered (slice 12) |
| Superbounce | `research` | none | `superbounce.rules` gains its settings; the table's are deleted |
| Gun Game | `party` | none | `gun_game.rules`: a kill gives the next rung's weapon at once (`set_loadout`); players spawn unarmed (`starting_weapon = none`) and the map has no weapons |
| Race | `party` | none | `race.rules`: the map's hills are its checkpoints, run in number order, so every map with hills races |
| VIP | `party` | none | `vip.rules`: a VIP per team, worth `vip_points` to kill, the role passing to a teammate; the script ends the round at the score limit |
| Siege | `siege` | none | `siege.rules`: the map's territory and CTF flags are its zones, and neither is required, so every map plays it — with no territory the assault goes straight at the flag stand, with no stand the sites are the whole assault, with neither the round draws at its start ([rules-engine.md](rules-engine.md#phases-attack-and-defend)); players spawn unarmed (`starting_weapon = none`, our added choice) and `set_loadout` arms each side; one life a round (`lives = 1`, our added choice) holds each death for its side's respawn wave |

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

### Quick Options in the lobby

`variant_quick.rs` (new, feature `variant-quick`) holds the shared part:
`rows(record, ir) -> Vec<QuickRow>`, each row a label, the current
value's text, and the values it may take. A record setting's value is
`GameVariant::get`; an option's is its bits in the block
(`Ir::option_offsets`). Labels and value texts are ours, from the table
and the IR: a named choice's label, a number, or a duration in words.
A record setting's values are the ones the editor offers for it on this
disc, kept from the list's load.

The script is the one the lobby variant's block names, when this machine
has it (`rules::header`'s lookup by prefix). A variant with no script
keeps retail's Quick Options in both places.

**Reading: the pregame panel.** `lobby_options.rs` wraps
`LOBBY_VARIANT_PANEL`, which fills five text rows from the variant
(**Code**, [ui-builder.md](ui-builder.md#the-pregame-lobbys-quick-options)).
For a script variant the five rows are the marked rows, "Label: value",
in the script's order, in place of the formatter's. The lobby mode's
lines and the options' lines (Clankers, Halo 1 Options, skulls) then
take the bottom rows by the existing rule (`first_extra`). The texts are
five ids of a `variant quick` family that the string hook answers from
the rows last computed. `ui.lobby_options` reads them as it reads the
rest.

**Writing: the dialog.** Game Setup's "Quick Options..." row (action 4,
`h1_options_row::QUICK_OPTIONS`, **Code**) opens, for a script variant,
the page `variant-quick` in place of retail's screen 222. It is the
shared settings dialog ([ui-builder.md](ui-builder.md#the-settings-dialog))
from `assets/ui/screens/variant-quick.yaml`: Quick Options' own panel,
the variant's name, five row slots bound `quick.0` to `quick.4`, the
focused row's help under the rows (an option's description; for a
setting, what it sets: `page::help`) and the value picker on A. Its Rust
is its bindings (`variant_quick::page`). The kit gains what the page
needs and no other page has: a slot row (its doc text `{label}` and help
`{help}`, bound with `Bindings::slot`) whose label, value and help come
from its binding each time it is drawn, a slot that is not drawn when
its binding has no row, and a header name bound with `Bindings::name`
(the doc's `name: "{variant}"`). For a script with no marks the
"Quick Options..." row is greyed out: the row keeps retail's text and
its press is taken and does nothing (`page::Entry::Greyed`). Change
Rules edits everything. With the page off (`variant-quick`) every
variant keeps retail's screen 222.

A change writes the lobby's variant through the game's own setter: read
the lobby's record, change the one setting (`GameVariant::set`) or the
option bits (`Ext::encode`, which keeps the CRC), and call the lobby
variant setter with the new record (the named point
`LOBBY_SET_VARIANT`, `0x19a728`, stack: the record; AL nonzero when
taken; the call the dedicated host makes, `host_lobby::retail::SET_VARIANT`,
**Code**). The record is checked with `record::refused` first, and a
refusal by either is shown in the row's help and changes nothing: a
refusal by the setter shows the lobby's record again. The dialog shows
the change at once and queues it (`page::Changes`); the next UI update
writes the record to guest memory and calls the setter, as
`pause_change_map`'s driver does, and no snapshot is taken while a
change waits. An option change on one of our listed entries also
retargets the entry's block in the store bundle
(`game_variants::inject::retarget`), so the game's start gate applies
the changed bits; it lasts until the list loads again. The change lasts
as retail's Quick Options changes do: it is the lobby's variant, not a
save.

Only the machine that owns the lobby's Game Setup can open the dialog,
as in retail.

### Editors and the IDE

Everything language-side comes from `rules::ide`, so the LSP
(`h2 rules lsp`), Studio's script window and Live's `/v1/rules/ide`
change with it and need no code of their own.

| Part | Change |
|---|---|
| `ide/complete.rs` | setting names inside `settings { }` (only the type's), a setting's values after `=`, settings and options after `quick`, type words after `type` and `uses`; header snippets for `type`, `points`, `uses`, `about`, `quick` and the block; `type`, `uses` and `quick` join `SPACE_TRIGGER_WORDS` |
| `ide/info.rs` | hover on a setting: its label, kind, values, stock value and the types it applies to; hover on the four new words |
| `ide/model.rs` | a use for a setting name and for a quick mark; go to definition from a mark to its `option` or its `settings` line; renaming an option renames its mark |
| `ide/fix.rs` | fixes for `setting.unknown` (nearest name), `setting.type` (remove the line), `quick.many` (drop the sixth), `scoring.none` (add `uses <T>`) |
| `ide/format.rs`, `ide/fold.rs` | one setting a line inside the block, the block folds |
| `workbench::CheckReport` | the settings with their values and the quick rows, for Studio's and the web's check report |
| `editors/tree-sitter-rules`, `editors/vscode/syntaxes`, `editors/zed`, `web/src/stats/rulesMonaco.ts` | the four words, the block and its `name = value` lines |

`type`, `uses`, `points`, `settings`, `quick` and `about` are words only at the start
of a top-level line; a script may still use them as names inside a
handler.

**Studio's variant tool** (`crates/apps/studio/src/ui/variants.rs`)
shows, for a record that names a script it can find, a Script section:
the script's name and description, a row per option editing the block's
option bits through `VariantEditor::set_ext`, and a Quick mark beside
each marked setting and option. "New from script" makes a variant with
`record_for`. The script window gains "Save as game variant", which
writes the `.h2variant` of `from_script`.

## Interfaces

- `rules::settings`: `TABLE`, `named(name)`, `applies(index, engine)`,
  `value_label(index, value)`. No dependency on `variants`; a test in
  `variants` holds the table to `record::field`, `schema::applies` and
  the clean-up's ranges.
- `rules::ir`: `Header::{game_type, uses, points, about}`, `Ir::{settings, quick}`,
  `MAX_QUICK`.
- `rules::builtin::LISTED`, and `by_prefix` (the lookup
  `runtime::compiled_in` makes today, for Studio and the CLI).
- `variants::record::GameVariant::stock`, `variants::portable::{record_for, from_script}`.
- Guest points: `LOBBY_SET_VARIANT` (`0x19a728`),
  `LOBBY_VARIANT_PANEL`, `BUILTINS_LOAD`.
- Agent API: `variants.quick` (read: the lobby variant's script, its
  rows and values) and `variants.quick.set` (act: one row's value), in
  [agent-api.md](../runtime/agent-api.md). They are how a run checks the
  dialog's write without driving the pad.

## Rules and limits

- No compatibility: the IR's encoding changes in place, the table is
  deleted in the slice that ports it, and blobs are rebuilt.
- No fallback: an unknown setting, a value out of range, a sixth mark
  and a `type` the rows cannot hold are compile errors; an entry the
  disc does not offer is left out by name.
- A script names at most the table's settings. A setting with no name of
  ours (`record::symbolic_name` has none) cannot be set and has the
  stock value; naming it is adding a row.
- A `uses` script cannot add handlers on top of the used type's
  scoring. A mode that needs both is a full script.
- Our rows hold Slayer and Juggernaut variants.
- Names: a listed script's name is at most 16 characters, the game's
  keyboard limit (the test the table has today).
- Text is English in every language.

## Slices

Each lane writes code and tests and runs nothing; the coordinator builds
and tests the branches together. A file has one owner. A lane whose
change needs a line in another slice's doc puts the text in its report.

| # | Slice | After | Owns | Tests that prove it |
|---|---|---|---|---|
| 1 | Language and IR: the table, `settings`, `quick`, `uses`, `about`, the codec and the verifier | | `crates/formats/rules/src/settings.rs` (new), `lang/{ast,parse,check,lower}.rs`, `ir.rs`, `verify.rs`, `lib.rs`, `tests/diagnostics.rs`; `docs/game/rules-engine.md`, `docs/game/rules-language-types.md` | `settings_round_trip_and_decode_refuses_disorder`; `a_setting_on_the_wrong_carrier_is_refused` (compiler and `verify::load`); `score_limit_names_each_carriers_setting`; `a_sixth_quick_mark_is_refused`; `a_quick_mark_names_a_setting_or_an_option`; `scoring_carrier_takes_no_handlers`; `a_script_with_no_handlers_needs_scoring_carrier`; `the_hash_covers_settings_marks_and_about`; one diagnostics case per new code |
| 2 | The record of a script and the CLI | 1 | `crates/formats/variants/src/{record,portable}.rs`, `crates/formats/variants/tests/settings_table.rs` (new), `crates/apps/cli/src/{variants_cmd,rules_cmd}.rs`; `docs/editor/variants.md` | `the_table_matches_the_record` (every row has a `record::field`, its carriers match `schema::applies`, its range the clean-up's); `record_for_is_stock_plus_the_scripts_settings`; `record_for_refuses_what_the_game_would_not_keep`; `from_script_is_the_same_bundle_twice`; `h2 rules settings` and `h2 variants from-script` output tests; disc `#[ignore]`: `the_stock_record_is_offered` (each carrier's stock values are values its `sily` definitions offer) |
| 3 | IDE and editor grammars | 1 | `crates/formats/rules/src/ide/*.rs`, `crates/formats/rules/src/workbench.rs`, `editors/**`, `web/src/stats/rulesMonaco.ts`, `web/src/lib/rulesIde.ts` | `ide::tests`: completion in the block offers only the type's settings, values after `=`, marks after `quick`; hover on a setting; rename of an option renames its mark; each fix; the formatter's round trip; the tree-sitter corpus cases |
| 4 | The fourteen scripts | 1 | `crates/formats/rules/src/builtin/*.rules`, `builtin/mod.rs`, `crates/formats/rules/tests/scripts.rs` | `every_listed_script_compiles_and_verifies`; `listed_names_are_unique_and_fit_the_keyboard`; `each_listed_script_sets_what_the_table_set` (the old table's pairs, written into the test as the expected values, so the table can go) |
| 5 | Built-in entries from scripts | 2, 4 | `crates/extensions/game/src/{community_variants,tweakers,string_ids,superbounce_mode}.rs`, `rules/header.rs`, `game_variants/port.rs`; `docs/game/community-gametypes.md` | on synthetic memory: `entries_are_built_from_the_listed_scripts` (name, type, pairs, description number); `every_listed_record_gets_its_block_at_load`; `a_copy_lists_under_its_scripts_row`; `a_script_the_disc_does_not_offer_is_left_out_by_name`; the existing merged-block and tweakers tests, moved off the table. Disc `#[ignore]`: `every_listed_script_is_offered_on_the_retail_disc` |
| 6 | The host and `uses` | 1 | `crates/extensions/game/src/rules/{runtime,feature,scoring}.rs`, `rules/runtime/tests.rs`, `crates/extensions/agent-proto/src/rules.rs`, `crates/extensions/agent/src/rules.rs` | `a_carrier_scored_slayer_script_runs_the_builtin_slayer_under_its_record`; `a_carrier_whose_builtin_this_host_cannot_run_is_refused_by_name`; `every_listed_carrier_script_runs_its_builtin_or_is_refused_by_name`; `its_requires_still_turn_the_mode_on` |
| 7 | Live and the web | 2 | `crates/live/live-server/src/game_variants.rs`, `crates/live/live-server/tests/{variants,rule_scripts}.rs`, `crates/live/live-worker/**`, `web/src/stats/{Rules,Variant}.tsx`, `web/src/lib/{rules,variants}.ts`; `docs/live/live-api.md` | `a_script_variant_has_the_scripts_settings`; `the_script_view_lists_settings_and_quick`; `stock_agrees_with_the_hopper_stock_record` (`GameVariant::stock` against `VariantRecord::stock`, all seven types); the parity suite's variant cases |
| 8 | The panel | 5 | `crates/extensions/game/src/variant_quick.rs` (new), `lobby_options.rs`, `crates/extensions/agent/src/variants.rs` (new, the `variants.quick` read); `docs/game/ui-builder.md`, `docs/runtime/switches.md`, `docs/runtime/agent-api.md` | `rows_read_settings_and_options_from_the_record`; `marked_rows_replace_the_formatters_in_order`; `mode_and_option_lines_still_take_the_bottom`; `a_variant_with_no_script_keeps_retails_rows`; `a_script_with_no_marks_shows_none` |
| 9 | The dialog | 8 | `crates/extensions/game/src/variant_quick/page.rs` (new), `assets/ui/screens/variant-quick.yaml` (new), `ui_kit/settings.rs`, `h1_options_row.rs`, `host_lobby/retail.rs`, `crates/hooking/guest-layout/src/points.rs`, and `variants.quick.set` in slice 8's agent file once 8 has landed | `ui_kit::settings::tests` for a bound row and an empty slot; `a_change_writes_one_setting_and_calls_the_setter` (runner test); `an_option_change_keeps_the_blocks_crc`; `a_refused_record_changes_nothing`; `quick_options_opens_ours_only_for_a_script_variant`; `quick_options_is_greyed_for_a_script_with_no_marks`; the page's offline render in every state |
| 10 | Script options in the editor | 5 | `crates/extensions/game/src/variant_page.rs`, `assets/ui/screens/variant-more-options.yaml`; `docs/game/game-options.md` | `a_row_per_option_after_the_fixed_rows`; `a_step_writes_the_option_bits_and_keeps_the_extras`; `a_record_with_no_script_has_no_option_rows` |
| 11 | Studio | 2 | `crates/apps/studio/src/ui/{variants,game_script}.rs`, `crates/apps/studio/src/app/rules_edit.rs`, `crates/formats/variants/src/panel.rs`; `docs/editor/editor-guide.md` | `new_from_script_is_record_for`; `an_option_edit_is_one_undo_step`; `the_script_section_lists_marks` |
| 12 | Rows over Juggernaut's list | 5 | `crates/extensions/game/src/tweakers.rs`, `tweakers/tests.rs` (from slice 5 once it has landed), the `type party` line of the Hide and Seek script; the Hide and Seek row of `docs/game/community-gametypes.md` | `tweakers::filtered` takes Slayer's and Juggernaut's categories; a row's list is both categories' handles, filtered; `hide_and_seek_lists_under_party_games`; Juggernaut's own list drops it; Create New stays dropped on a row's list |

Slices 2, 3, 4 and 6 run together after 1; 7 and 11 after 2; 8, 10 and 12
after 5; 9 after 8.

## Open decisions

1. **Bounce Practice.** Done. The list's Bounce Practice is the
   community's settings-only variant (Slayer, no score), ported as
   settings only (`bounce_practice.rules`). The script that scores
   bounces (a point a bounce, three for a first find, a low-gravity
   option) is Bounce Hunt (`bounce_hunt.rules`) and stays an example; the
   Superbounce game type is the list's bounce-scoring game.
2. **Which settings each classic marks.** Slice 4 chooses, at most five
   each.

Decided: a script with no `quick` line greys the "Quick Options..." row;
a setting a script leaves out has the stock value, and `h2 rules
settings` prints the whole record; Hide and Seek moves under Party Games
(slice 12).

## Evidence

- **Code**: the facts labelled above are read from the files they name.
- **E**: `0x19d220` is the defaults writer `VariantRecord::stock`
  models; `0x19a728` is the lobby variant setter the dedicated host
  calls.
- **S**: retail's Quick Options dialog (screen 222) and the panel's
  formatter (`0x25092c`) choose their rows from the `goof` tag's summary
  pages (`21 + engine`, [variants.md](../editor/variants.md#the-schema-goof-and-sily)).
  Nothing here depends on it: a script variant's rows are ours. Tracing
  the dialog's list would verify it.
- **Design**: the syntax, the five-row limit, the stock base, the hash
  covering everything, `uses`.

## Not verified

- That each engine's stock values are values the retail editor offers
  (slice 2's disc test). A stock value it does not offer leaves every
  script variant of that engine out of the list.
- That `0x19a728` keeps the `hx` block of the record it is given, and
  that a change made through it reaches guests as retail's Quick Options
  changes do.
- That `0x19a728` pops its one argument (`ret 4`, as `LOBBY_SET_VARIANT`
  declares); the dedicated host's call reads only AL.
- How retail draws a disabled Game Setup row: the greyed row keeps
  retail's text and only its press does nothing.
- That the content list's records are all filed when `BUILTINS_LOAD`
  returns, so the block can be written there and not at the first fill.
- That retail's formatter leaves the five rows alone when ours are set
  after it, on every lobby kind (Xbox Live, System Link, Split Screen).
- The dialog and the panel in a game run: `ui.lobby_options`,
  `variants.quick` and `ui.screen` on a lobby with Superbounce chosen.
- Description numbers 40 to 79 on the variant list (retail uses none of
  them).

## Related

- [rules-engine.md](rules-engine.md): the language, the IR, carrying a
  script, the host runtime.
- [community-gametypes.md](community-gametypes.md): the built-in list,
  the merged block, our rows.
- [game-options.md](game-options.md): the `hx` block and More Options.
- [variants.md](../editor/variants.md): the record, the schema and
  `.h2variant`.
- [ui-builder.md](ui-builder.md): the pregame panel and the settings
  dialog.
- [rules-world-modes.md](rules-world-modes.md): Superbounce as a plain
  script.
