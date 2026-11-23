# Game variants: record, schema and saved files

Status: Built. Code: `crates/formats/variants/src`. Tests: in-source unit tests; `crates/formats/variants/tests/settings_table.rs`; `crates/formats/variants/tests/disc.rs` (ignored, needs a disc).
Switch: none.

`crates/formats/variants` is the game variant editor of the editor suite: the
0x130-byte variant record, the settings schema, saved-variant files and the
editor's state and commands (`VariantEditor`, drawn by `h2-editor`). No
game data is in the repository: the schema and the built-in variants are
read from the user's disc at run time, tests use synthetic bytes, and the
checks against the retail XBE and disc are `#[ignore]` tests that read
`$H2_XBE` (and `halo2.iso` beside it, or `$H2_ISO`).

Everything below was read statically from the retail XBE (SHA-256
`03215919…935d`) with a scratch disassembler and from the retail main menu
map with the ignored test. Nothing was run in the game. Each claim is
marked **confirmed** (seen in code or data) or **inferred**.

## The record

The record is the active variant at `G+0x13c` (docs/live/live-gamestate.md §3),
the whole payload of a saved variant, and the last 0x130 bytes of a hopper
record slot (`crates/live/live-proto/src/hopper/mod.rs`, `VARIANT_AT`). The editor's
hopper tool links a variant by copying `GameVariant::to_bytes()` into the
slot, after `record::check` finds nothing (docs/editor/editor.md).

| Offset | Type | Content | |
|---|---|---|---|
| `+0x00` | u16 | bit 0: built-in flag; nothing else may be set | inferred meaning, confirmed limit |
| `+0x02` | u8 | unknown, preserved | |
| `+0x03` | u8 | built-in description number (0..=100) | confirmed |
| `+0x04` | UTF-16 [32] | name; the last unit is always 0 | confirmed |
| `+0x44` | i32 | game-engine index: 1 CTF, 2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault | confirmed |
| `+0x48` | u32 | flags: bit 0 team play, other bits are settings (below) | confirmed |
| `+0x4c..+0xf0` | | common settings (below) | confirmed |
| `+0xf0..+0x10c` | | game-type area: its meaning depends on the engine index | confirmed |

### Where each setting lives (confirmed)

The editor and the built-in loader both store a setting with `0x2373be`
(EAX setting, EBX value, ESI record). It calls a per-setting descriptor
from a 0x70-entry pointer table at `0x470828`, which answers a storage kind,
an offset and a bit or scale; then it stores the value:

| Kind | Store |
|---|---|
| 0 | bit of a u16 |
| 1 | bit of a u32 |
| 2 | byte, `value != 0` |
| 3 | byte, `value * (1.0 / scale)` |
| 4 | i16, same |
| 5 | i32, same |
| 6 | f32, same |

Every scale in this build is 1.0, so values are stored as given. Settings
27, 31..37, 48 and 49 have no descriptor. `record::field` is our
encoding of the answers. The ignored test `descriptors_match_the_xbe`
decodes all 0x70 descriptors from `$H2_XBE` with iced-x86 and compares them
with `record::field` entry by entry (passes).

Which settings belong to which game type comes from the `goof` tag's pages
(below): the type-area settings are Slayer 28..30; Oddball 43..47, 100,
102; King 38..42, 101; CTF 50..58, 106, 107; Assault 59..67, 92, 95, 97,
109, 110; Juggernaut 68..73, 96, 108; Territories 74, 98, 99. Setting 103
is stored but on no page. Settings 1..9 are each type's score to win
(1 CTF, 2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault)
and all store at `+0x50`.

### The clean-up (confirmed, two limits inferred)

Loading a saved variant runs `0x19d650` on it. It returns 1 only if no
rule changed the record; otherwise it overwrites the whole record with a
default Slayer and the load fails. So an edited record must satisfy every
rule. `record::check` reports the broken ones:

- `+0` only bit 0; `+0x42` (last name unit) 0; engine 1..=4 or 7..=9;
  `+0x48` below bit 15; `+0xb0` 0.
- i32 `+0x4c` 0..6; `+0x50`, `+0x54`, `+0x7c`, `+0x80`, `+0x84`, `+0xac`
  0..0x7fff; `+0x58`, `+0x88`, `+0xa4`, `+0xa8` 0..2; `+0x74`, `+0x78`
  0..16; `+0xb4` 0..8.
- bytes `+0xcc` ≤3; `+0xcd`, `+0xce` ≤7 (inferred from `eax=7`);
  `+0xcf..+0xd1` ≤4; `+0xd2`, `+0xd3` ≤6; `+0xd4` ≤0x13; `+0xd5` ≤3;
  `+0xd6`, `+0xd7` ≤0x14.
- Per engine (jump table at `0x19de54`): CTF forces team play, `+0xf0`
  mask 0xff, i32 `+0xf4` 0..0x7fff, `+0xf8` 0..2, `+0xfc` 0..1, `+0x100`
  0..3, `+0x104` 0..2; Assault: i16 `+0x108`, `+0x10a` 0..15, then the CTF
  rules; Slayer `+0xf0` mask 7 (inferred); Oddball mask 7 (inferred), i16
  `+0xf4` 0..3, `+0xf6` 0..1, `+0xf8` 0..2, `+0xfa` 0..3; King mask 0x1f, i16
  `+0xf4` 0..0x7fff; Juggernaut never team play, mask 0x7f, i16 `+0xf4`
  0..2; Territories i16 `+0xf0` 1..8, `+0xf2` and `+0xf4` 1..0x7fff; Race
  and Headhunter always reset.

### The stock record (inferred from the port)

`GameVariant::stock(type)` is what the defaults writer `0x19d220` gives a
new record before the menu or the `goof` defaults apply anything, as
`live_proto::hopper::variant::VariantRecord::stock` ports it: description
number 0xff, flags `0xf9a` (motion sensor, friendly fire, the overshield,
camo and grenade pickups and starting grenades on) with team play for
CTF, Assault and Territories, rounds 0, the type's score to win (Slayer
25, Oddball and King 120, Juggernaut 15, CTF and Assault 3, Territories
300), time limit 480, setting 15 and max players 16, unlimited lives, a
respawn time and suicide penalty of 10 s for the team types and 5 s for
the others, respawn modifier 2, betrayal penalty 10, map-default weapons
and vehicles, and each type's own area (Juggernaut fast with an
overshield, a hill that moves every 60 s, one ball, a 30 s flag reset,
three territories). It is named by our label of the type. Every stock
record passes `record::refused`. The ignored test
`the_stock_record_is_offered` (`tests/settings_table.rs`) checks that the
retail editor offers each stock value.

## The schema (`goof` and `sily`)

`mainmenu.map` has one `goof` tag (docs/game/community-gametypes.md).

- `+0x18`: pages, 0x18 bytes each: `+0` page id, `+8` count and `+0xc`
  address of 8-byte `sily` references. Confirmed (`0x2369b3`, `0x236a3a`,
  retail tag). The page ids in the retail tag: `engine - 1` match options
  (rounds, score, time and three more), 9 player options, 11 vehicles, 12
  weapons, `12 + engine` the type's own options, `21 + engine` a summary page
  that repeats settings from the others, `31 + engine` team options. Page
  titles are not in the tag (the UI screens carry them); ours are symbolic.
- `+0x20`: the built-in names' `unic` reference (datum at `+0x24`).
- `+0x28`: the built-in variants (docs/game/community-gametypes.md).
- `+0x24 + 0x14 * content type`: each game type's defaults, a block of
  `(setting, value)` pairs that `0x1489b5` applies through the setter to a
  fresh record, indexed by the save-content type (below); the Race and
  Headhunter slots are empty. Confirmed. Before that, `0x19d220` sets up
  the record; that it starts zeroed is **inferred**.

A `sily` tag: `+0` setting number, `+8` its `unic` reference (datum
`+0xc`), `+0x10` title, `+0x14` the title in capitals and `+0x18` help
string ids (checked by reading their text), `+0x1c` the options block of 0xc-byte options
(`+0` default flag, `+4` value, `+8` label string id). Confirmed layout.

Without a disc, `Schema::fallback` lists every setting the record stores
with our names, any value the record accepts, and no built-ins.

## Saved variants

### The container (confirmed)

A saved variant is an XAPI save game (docs/runtime/xapi.md): a directory
`U:\<12 hex digits>\` named by the hash of the save name, with
`SaveMeta.xbx` (`FEFF`, `Name=<save name>` CR LF, UTF-16LE). The game
creates it with `0x2ad020` → XCreateSaveGame, CREATE_NEW and flags 0 (no
`NoCopy`). On our runtime `U:\` is
`<data root>/disk/Partition1/UDATA/4D530064/`. The editor's default data
root is the one `h2 play` and `h2 run` use without `--data`,
`~/Library/Application Support/h2recomp/data` (`content::store::default_data_root`), so variants
saved in the editor are the ones the game finds; `h2-editor --data <dir>`
matches a run with `--data <dir>`. (`vfs::Config::for_title` names
`~/Library/Application Support/h2recomp`, which no run uses today.)

The save name is `<content-type label>: <variant name>`, at most 0x80
units (`0x216da0`). The label is per language, compiled into the XBE (the
per-type functions behind `0x217a80`, e.g. `0x2176e0`), not in the tags. The
listing does not need the prefix: it strips a known label (`0x216e50`) and
takes the game type from the file present. But the game rebuilds the name
when it re-saves or deletes, and XCreateSaveGame/XDeleteSaveGame compare
`Name`, so an editor must write the game's exact name or the game cannot
later overwrite or delete the save (inferred).

### The file (confirmed)

One file per save directory, no extension, named by the content type
(`0x216b60`: a table of 11 ASCII names, read from the XBE at run time).
Content types: 0 profile, 1 Slayer, 2 King, 3 Race, 4 Oddball,
5 Juggernaut, 6 Headhunter, 7 CTF, 8 Assault, 9 Territories, 10 playlist.
The listing tries each type's file name in turn (`0x216537`); the first
that exists gives the type.

| Offset | Size | Content |
|---|---|---|
| `0x000` | 0x130 | the record, as in memory, after the clean-up (`0x212ec0`) |
| `0x130` | 0x14 | signature |

No header, magic or version. Writing: `0x2ae090` writes the data at 0,
signs it and writes the signature right after.

### The signature (confirmed; the key on our runtime unverified)

`XCalculateSignatureBegin(0)` (roamable; both the writer `0x217460` and the
reader `0x2163aa` pass 0), Update over the 0x130 bytes, End:

    signature = HMAC-SHA1(key = XboxSignatureKey (16 bytes), record)

`XCalculateSignatureBeginEx` (`0x2d65a7`) takes the key from kernel export
325 `XboxSignatureKey` (via `[0x411660]`) when the title id is the
certificate's own, else from `XboxAlternateSignatureKeys`. End's roamable
path (`0x2d64c0`) is a plain HMAC finish. The non-roamable path (not used
here) adds a second HMAC with the hard-disk key.

Our kernel never fills export 325 (crates/runtime/kernel/STOPS.md), so on our
runtime the key is 16 zero bytes. **Unverified**: no save written by the
game under our runtime has been compared yet. To check: save one variant
in the game, then run the ignored test `saved_variant_verifies` with
`H2_SAVED_VARIANT=<the payload file in its save directory>` (the editor also
reports a saved variant whose signature does not match when it is opened). A
real console derives the key from the certificate's signature key with a
console secret, so its saves cannot be re-signed here; the key is a
parameter of the codec.

### Loading (confirmed)

`0x2adf70` reads exactly 0x130 bytes (else error 0x26), signs them, reads
the next 0x14 bytes and compares. Only a match marks the load good. The
size is never checked, so trailing bytes are ignored (inferred). Then
`0x212bc0` runs the clean-up on a copy and fails if it changed anything.
What the UI shows for a failed load was not traced.

### Copy-on-edit (confirmed)

Built-ins carry bit 21 in their content handle and are copied from the
content list, never read from disk (`0x2162c0`). Editing one (`0x212b2c`)
clears record bit 0 of `+0`, runs the clean-up, sets the new name (at most
0x1f units), zeroes `+0x42` and saves it as an ordinary variant.

## Portable files

`.h2variant` (`portable`) is our sharing format: JSON with the format
tag `h2recomp-game-variant`, a version, and the 0x130 record as hex (the
source of truth, so unknown bytes travel too). The game never sees this
file, so it carries no signature.

- **Version 2** is a whole game variant (`portable::Bundle`): `name`,
  `description` (at most 150 characters), `author` (`{xuid, gamertag}`,
  written by Live on publish), `game_type` (for people), `engine`,
  `record_hex` and, when the record's block names a rules script (mode
  2), `script: {name, hash, ir_b64, source?}`: the IR blob (at most 64
  KiB) and its source (at most 256 KiB). The record carries the 8-byte
  hash prefix that names the script; the file carries the script itself
  ([rules-engine.md](../game/rules-engine.md#carrying-a-script)).
  `Bundle::canonical` is compact JSON in field order, and `Bundle::id` is
  its SHA-256: the same variant has the same id everywhere.
- **Reading** (`portable::read`) checks a version-2 file's integrity:
  the record decodes, `engine` and `name` match it, and a script is
  present exactly when the record names one, its IR hashes to `hash`,
  `hash` starts with the record's prefix, `rules::verify::load` takes it
  and its carrier is the record's engine. `Bundle::check`, which Live
  runs before publishing, adds the name rules (1 to 31 characters, no
  leading or trailing space, no control character, no `|`, none of
  U+E405..U+E407, nothing past U+FFFF; game-options.md section 4), a
  record the clean-up keeps and our rules allow (`record::refused`), and
  an extension block that is ours or absent.
- **Version 1** (name, game type and `record`) still reads, as a bundle
  with no description and no script; `portable::export` still writes it,
  for the editors, which have no script blob at hand. `import` reads
  either and lists what the clean-up would reject.
- **Porting.** `portable::port(record, name, description, script)` makes
  a version-2 file of any record, and every port attaches a script:
  `PortScript::Builtin` attaches the record's engine's built-in script
  (`rules::builtin::builtin`) with its default options and sets mode 2
  and the prefix (a record that already names a script is refused: its
  blob is not at hand), and `Ir` or `Source` attach a given script. The
  block's extras are the script's `settings` and nothing else:
  `Bundle::new` does not keep the input record's `hx` extras, and
  `Bundle::check` refuses an extra no settings row stores (movement
  speed, weapon ids, team size, the superbounce flag). The same inputs
  give the same id. `portable::port_input` takes a raw record, a
  saved-variant file (record and signature), a hopper record slot
  (`0x37c` bytes, the record at `+0x24c`) or a `.h2variant` of either
  version. `portable::port_with_options` also packs option values by
  name against the attached script (`rules::Ir::pack_options`), and
  `Bundle::check` refuses a script over the tick budget or the IR cap
  (`rules::fits`). `h2 variants port <input>... [--script
  builtin|FILE] [--name NAME] [--description TEXT] [--option
  KEY=VALUE]... [-o DIR]` (`crates/apps/cli/src/variants_cmd.rs`) writes
  one file per input, named by the variant (`portable::file_name`), and
  prints each id. `--name` and `--description` take one input; each
  `--option` is an int, `true` or `false`, or an enum value's name, and
  an option the script does not declare or a value out of range is
  refused. `--all-builtins [disc]` ports every retail built-in, read from the
  disc's `goof` tag (`Schema::build`), and refuses without a disc. The
  community variants are built by the game at boot and are not ported by
  the tool.
- **A script's own variant.** `portable::record_for(ir, options)` is the
  one builder of a variant from a rules script
  ([rules-variants.md](../game/rules-variants.md#the-record-of-a-script)):
  the stock record of the script's carrier (`GameVariant::stock`, below),
  each setting of the script's `settings` block written where its row of
  `rules::settings::TABLE` stores it (a record setting through
  `GameVariant::set`, the carrier's own score setting for `score_limit`,
  CTF's or Assault's setting for the paired `ctf_*` rows, an `hx` field
  for `gravity`, `damage`, `shield`, `health` and `unlimited_grenades`),
  the script's name, and the script reference (mode 2, the IR hash's
  first 8 bytes and the option bits, the defaults for options not
  given). It refuses a setting past the table, on a carrier it does not
  apply to or out of its range, a name the name rules refuse, and a
  record `record::refused` refuses (a Juggernaut script with team play,
  a CTF script without), in those rules' words; nothing falls back.
  `portable::settings_record` is the same record before the name and the
  reference: `Bundle::check` and `h2 rules check` refuse a script whose
  settings it refuses. `portable::from_script(ir, source, options)`
  makes the bundle: that record, the script's name and `about` as the
  bundle's name and description, and the script part. `h2 variants
  from-script <file.rules> [--option KEY=VALUE]... [-o DIR]` writes it
  and prints its id and path; the same script and options give the same
  id. `h2 rules settings <file.rules>` prints the record: every setting
  of the carrier a script may name, with its value and `script` or
  `stock`, then the record's settings no script names.

## The editor

`variants::VariantEditor::new(schema, store)` takes a `Schema`
(`Schema::from_map` on the main menu map, or `Schema::fallback()`) and an
optional `SaveStore` (`SaveStore::new(SaveStore::default_root()?,
XbeNames::read(&xbe, language).ok())`; without `XbeNames` saves list and
update in place, but new saves cannot be named).

- `is_dirty`, `save` (refuses records the clean-up would change),
  `undo`/`redo`/`can_undo`/`can_redo`/`revert`, `set_setting`,
  `set_name` (typing a name is one undo step), `set_row(name, value)`
  (a settings-table row, the way the `hx` extras are edited),
  `copy_builtin(name)`, `new_variant(type)`, `open_builtin`/`open_saved`,
  `delete`,
  `export_current`/`import`, `current`, `origin`, `issues`, `status`.

Built-ins are read-only; `copy_builtin(name)` (the editor's **Copy**)
makes a named copy and clears the built-in flag like the game's Edit.

Every record the editor opens or makes names a script. A built-in is
loaded with its engine's built-in script attached
(`portable::attach_builtin`), so its copy names it. `new_variant(type)`
opens `portable::record_for_named` of that built-in script, named "New
<type>". `open_saved` and `import` attach the built-in to a record that
names none; a saved variant opened that way is unsaved until it is
saved. A game type with no built-in script (a type the engine has none
for) opens as the type's plain defaults and the status says it has no
script.
The editor asks before picking another variant over unsaved changes
(editor.md, "Game variants").

## Tests

`cargo test -p variants`: synthetic records, files, stores in a temp dir,
an invented schema, and the editor's commands. `tests/settings_table.rs`
holds `rules::settings::TABLE` to the record: every row's setting has a
`record::field` and a name of ours, applies where `schema::applies` says
on exactly the row's carriers, and takes exactly the values the
clean-up keeps at that field (`the_table_matches_the_record`). With the
disc (`H2_XBE`, `halo2.iso` beside it), `cargo test -p variants --test disc
-- --ignored` checks the setter descriptors, the schema (every setting has
a field and values, type pages match `fallback_owner`, every built-in
built the game's way passes the clean-up and offers only listed values),
the XBE names in every language, and opens every built-in in the editor
(read-only, with no issues).

## Our added choices and rules

`variants::extra` lists values the record keeps and plays but the retail
editor leaves out (starting weapon "None", a 1 s respawn, more score, time
and lives steps; docs/game/game-options.md). `Schema` merges them into each
definition's choices (`Choice::ours`, shown "(h2recomp)" in Studio), and
the game's own editor shows them too (`boot::variant_options`).
`record::ours` is our rule beyond the clean-up: respawn 0 with the cycling
modifier divides by zero in the game, so `record::refused` (the clean-up's
rules plus ours) gates Studio's save, the hopper tool and portable import.

Options the record has no field for (gravity first) live in our
extension block in the spare bytes `+0x10c..+0x12f` (`variants::ext`;
layout and CRC in docs/game/game-options.md section 3). All zero means
none, so a retail record has none, and every copy of the record (saves,
`.h2variant` files, hopper slots) carries it. Studio edits the extras
the settings table has rows for (gravity, damage, shield, health,
unlimited grenades) in "More options (h2recomp)" as those rows
(`VariantEditor::set_row`, `portable::set_row`), and a script's own
options on the Script section; the extras no row stores are not shown.
A block the game would ignore is reported as `Issue::Ext`.

## Open

- The signing key on our runtime (above): one in-game save settles it.
- What byte `+2` of the record means; that a new record starts zeroed;
  what the UI shows when a load fails.
- Our page titles are symbolic: the game's titles live in the UI screens,
  not in the `goof` tag.
