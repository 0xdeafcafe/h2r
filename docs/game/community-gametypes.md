# Community game variants

Status: Built. 17 variants, each built from a script in `rules::builtin::LISTED`, are added after the retail built-ins; the unit tests prove the entries land in the `goof` block and their records get their scripts' blocks, and what the game then does with them is in "Not done and risks".
Switch: none.

Halo 2 ships with 39 built-in game variants (Slayer, Rockets, Ninjanaut,
Crazy King and so on). This adds thirteen classics the community set up by
hand in custom games, a practice mode (Superbounce) and three scripted
modes (Siege, Gun Game and Race), as extra built-ins.
Each is a script compiled into the game
([rules-variants.md](rules-variants.md#built-in-list-entries)): its list
entry, its description and its row come from the script, and its record
names the script. Two rows of ours at the bottom of the game type list
([Tweakers](#tweakers)) hold some of them: **Research and Devolution**
has Bounce Practice, Glitch Hunt and Superbounce, and **Party Games** has
Zombies, Jousting, Cops and Robbers, Bumper Cars, Siege, Gun Game and Race. The rest are listed
under their game types. In the
variant list they carry a
"two players" icon where the retail ones carry a padlock. Like the retail
built-ins they cannot be edited in place: choosing Edit asks for a name and
makes a copy, which is an ordinary saved variant.

Code: `crates/extensions/game/src/community_variants.rs`, installed by the boot runner; the scripts in `crates/formats/rules/src/builtin/` (`LISTED`).

No game data is in the repository. Addresses are guest virtual addresses of
the retail XBE (SHA-256 `03215919…935d`), named only as hook points. The
layouts below were read from the XBE with `h2 disasm` and checked against
`mainmenu.map` with a scratch reader in `$TMPDIR`. The settings are named
in each variant's script (`rules::settings` maps the names to the
editor's setting and value numbers). They are the values a player picks,
written out per variant; no retail table was copied.

## How Halo 2 keeps its variants

### Built-in variants come from a tag

`mainmenu.map` has one `goof` tag
(`multiplayer\game_variant_settings\multiplayer_game_settings`). The UI
globals name it at `wgtz +0x10` ([ui.md](../formats/ui.md)), and `0x236a7f` returns
it.

| Offset | Content | Read by |
|---|---|---|
| `+0x18` | setting pages, `0x18` bytes each: `+8` count and `+0xc` address of tag references to `sily` setting definitions | `0x2369b3` (find a setting's definition), `0x236a3a` |
| `+0x20` | `unic`: the built-in names and help text | `0x149f88` |
| `+0x28` | built-in variants, `0x14` bytes each (below) | `0x149f88` |
| `+0x38` + `0x14 * k` | per-game-type defaults for a new variant: a block of `(setting, value)` pairs (nine slots, two empty) | `0x1489b5` |

A built-in entry:

| Offset | Content |
|---|---|
| `+0` | name string id, resolved in the `goof` tag's `unic` |
| `+4` | game type: 0 Slayer, 1 Oddball, 2 Juggernaut, 3 King of the Hill, 4 CTF, 5 Assault, 6 Territories (`0x149f88` rejects 7 and up) |
| `+8` | block of `(u32 setting, i32 value)` pairs |
| `+0x10` | description number, a byte clamped to 0..=100 |

A `sily` tag defines one editor setting: `+0` the setting number, a `unic`
reference, title and help string ids, and at `+0x1c` a block of `0xc`-byte
options (`+0` default flag, `+4` value, `+8` label string id). Every
setting in the editor is one of these, so the tags list every value the
game will accept, with its label.

### Loading the built-ins

`0x149f88` runs once per load of the main menu's tags (from the
initialisation at `0x147645`) with the `goof` tag on the stack. For each
entry it:

1. maps the game type through `0x216ab0` and fills a `0x130`-byte variant
   with that type's defaults (`0x1489b5`, which applies the defaults block
   through the same setter);
2. resolves the name with `0x1a0180` and copies up to `0x1f` UTF-16 units to
   the variant's name at `+4`;
3. stores the description number at variant `+3`;
4. applies each pair with `0x2373be` (EAX setting, EBX value, ESI variant):
   the setter is the same code the editor uses when a player changes a row;
5. validates the result (`0x19d650`) and files it with `0x217080`, which
   appends it to the content list at `[0x51ea14]` (count, then `0x1e4`-byte
   records; at most 101 built-ins).

### Padlock, edit and copy

The padlock is not drawn by the list. It is part of each built-in name in
the `goof` tag's strings: the name, a space and U+E008, a private-use glyph
every game font carries ([button-icons.md](../input/button-icons.md) on the
`button-icons` branch lists the range). Built-in content is told apart from
saved content by bit 21 of its content handle (tested at `0x148a2c` and
`0x217300`), not by its name. A built-in can be played and copied but not
edited or deleted: the game refuses deletion, and Edit first asks for a
name for a copy. A copy is written as a normal saved variant.

Under a focused built-in, the variant pickers (`mp_variant_select` and
`mp_variant_select_lobby`) show the string `0x100001dc`, retail's
"Built-in Variant (cannot edit)". The string hook answers it with the
focused entry's origin, read from the description number the list last
asked `0x19fd00` for (from `0x2ca1fd`): "Bungie Original" for retail's
0 to 38, "Community Classic" for ours (from 40, one a listed script) and "Downloaded Content"
for the store's (94 to 100). `community_variants::origin_text` holds the
mapping.

So a variant added to the tag block before `0x149f88` reads it is a
built-in in every respect, including the lock and copy-on-edit. The icon is
whatever its name string ends with.

### Saved variants on disk

Saved variants go through XAPI's save-game calls (`XCreateSaveGame` and the
find calls; call sites are listed in [xapi.md](../runtime/xapi.md)), under
`UDATA\4D530064\<save>\` with a `SaveMeta.xbx` ([storage.md](../runtime/storage.md)),
signed by the game's own `XCalculateSignature*`. This work never touches
that path: our variants exist only in memory, and a copy goes through the
retail save code unchanged.

### The icon

U+E000 to U+E008 are small symbols every font maps: three faces, a box, a
fourth face, a camera, two people (U+E006), a download arrow (U+E007) and the
padlock (U+E008). Only one retail string uses U+E006 (a guest gamertag
format). Two people means "made by players", so the community variants use
it. The game-variant store's variants (rules-engine.md, "Live game
variants in the game") carry the download arrow, U+E007, which no retail
name uses: made elsewhere and brought in. No glyph is added or changed. If a purpose-made picture is wanted later,
the `button-icons` branch shows how to add one at an unused code point
(nothing uses U+E200 and up) without shipping art.

## The variants

Halo 2's editor has no gravity, player speed or infinite ammo for everyone
(only Juggernaut movement, ball speed and the carrier toggles change speed;
see the source list). Every classic below is built from settings the editor
offers, except the unarmed start of Bumper Cars, Siege and Gun Game, one of our added choices
([game-options.md](game-options.md)). Rules a variant cannot enforce (who may use which weapon, switching
teams on death) were honour rules then and are in the description now.

Each row is one script in `crates/formats/rules/src/builtin/` (a
`settings` block, `scoring carrier` but for Superbounce and the scripted modes, and its
description as `about`). Settings a script does not name have the stock
value of its game type (`GameVariant::stock`); the list entry carries
every setting the editor offers, so the disc's per-type defaults never
show through.

| Name | Type | Settings |
|---|---|---|
| Zombies | Slayer | team play; shotgun and energy sword; no grenades at start or on the map; shotguns on the map; no motion sensor; no overshield or camo pickups; no vehicles; no score limit |
| Hide and Seek | Juggernaut | energy sword, no secondary; nothing on the map; active camo and no motion sensor for everyone, the Juggernaut without camo but with a motion sensor and fast movement; one life; 5 minutes |
| Jousting | Slayer | Warthog as the light vehicle, no other vehicles; shotgun, no secondary; no grenades; nothing on the map |
| SWAT | Slayer | team play; no shields; no motion sensor; battle rifle and magnum; nothing on the map; no grenades; no pickups; 50 points |
| Duel | Slayer | two players; magnum and a random secondary; nothing on the map; 15 kills |
| Clint Eastwood | Juggernaut | no shields; shotgun and magnum; nothing on the map; Juggernaut damage resistance and fast movement |
| Sun King | King | team play; no shields; 3 s respawn; no suicide penalty; rocket launcher, no secondary; rockets on the map; 3 minutes on the hill; 12-minute limit |
| Tower of Power | Slayer | team play; no shields; 3 s respawn; no light vehicle; machine gun turret; shotgun, no secondary; shotguns on the map; no grenades; no pickups; 100 points |
| Wheeeeee! | Oddball | team play; no shields; extra damage; fast and tough with the ball; rocket launcher; rockets on the map, respawning twice as often; 30 seconds |
| Cops and Robbers | Slayer | team play; 3 lives; no shields; no pickups; shotgun and plasma pistol; nothing on the map |
| Bounce Practice | Slayer | no score or time limit; 3 s respawn; no suicide penalty; overshields; damage resistance; a magnum and no secondary weapon; nothing on the map; no grenades; its script requires `hud.superbounce`, so the superbounce HUD, spot lines and practising bots are on ([superbounce.md](superbounce.md#the-superbounce-game-type)), and Slayer's scoring stays |
| Glitch Hunt | Slayer | no score or time limit; 3 s respawn; no suicide penalty; overshields; damage resistance; energy sword and rocket launcher; no friendly fire |
| Bumper Cars | Slayer | no starting weapon (one of our added choices) and no secondary; no grenades at start or on the map; nothing on the map; no pickups; Warthog and Ghost, no other vehicles or turrets; 25 points |
| Superbounce | Slayer | Bounce Practice's rules, scored by bounces, with the superbounce HUD's options ([superbounce.md](superbounce.md#the-superbounce-game-type)); listed under Research and Devolution ([Tweakers](#tweakers)), not Slayer; its built-in record names the Superbounce rules script, whose header lists it there (`list research`) and turns the mode on (as Bounce Practice's `requires hud.superbounce` does) |
| Siege | Slayer | team play; no score or time limit; no starting weapon (our added choice) and no secondary, since its script gives each side a loadout at spawn; the attackers take the map's territories, then the defenders' CTF flag stand, each phase on its own clock ([rules-engine.md](rules-engine.md#phases-attack-and-defend)); scored by its script and listed under Party Games (`list party`) |
| Gun Game | Slayer | no starting weapon (our added choice) and no secondary; nothing on the map; no score limit; its script gives each player the weapon of their rung at spawn and the next rung's on each kill, from the SMG to the energy sword, and the kill past the sword wins; a suicide demotes a rung (an option); listed under Party Games |
| Race | Slayer | no score limit; the map's King of the Hill hills are the checkpoints, run in number order from the lowest and back for a lap, and the first to finish the laps (3 by default) wins; listed under Party Games |

Why these, and the sources:

- **Zombies.** The best-known Halo 2 custom game. The usual recipe is
  shotguns against swords, one team human and one zombie, with killed
  humans switching to the zombie team by hand.
  <https://www.halopedia.org/Infection>,
  <https://forum.quartertothree.com/t/need-best-halo-2-custom-game-types-how-they-are-played/20159>,
  <https://www.bungie.net/en/Forums/Post/57987399>.
- **Hide and Seek.** Built on Juggernaut: the seeker is the Juggernaut and
  has a motion sensor; everyone else hides with camo.
  <https://www.bungie.net/en/Forums/Post/57987399>. The fast seeker is our
  choice, so a round ends.
- **Jousting.** Warthogs on Coagulation, driver and shotgun passenger.
  <https://www.neoseeker.com/forums/3951/t742763-favorite-custome-games/>,
  <https://www.neoseeker.com/forums/3951/t923589-halo-2-custom-games/>.
- **SWAT.** No shields and precision weapons, so one headshot kills. Halo 2
  had no SWAT built-in (Halo 3 added one); players set it up as no shields,
  no radar, battle rifle starts, as in the Bob da Ugh and MLG-style threads
  above. A reconstruction, not one sourced recipe.
- **Duel.** One on one, winner stays on, Lockout or Ascension.
  <https://www.bungie.net/en/Forums/Post/57987399>.
- **Clint Eastwood.** Juggernaut with no shields, shotguns and pistols, a
  tough fast Juggernaut, on Ascension.
  <https://pdfcoffee.com/popular-classic-halo-custom-games-list-pdf-free.html>
  (a short description only; the exact split is our reading of it).
- **Sun King**, **Tower of Power** and **Wheeeeee!**: full variant sheets in
  HBO's gametype archive, <https://halo.bungie.org/misc/hbo_gametypes/sun_king.html>,
  <https://halo.bungie.org/misc/hbo_gametypes/tower_of_power.html>,
  <https://halo.bungie.org/misc/hbo_gametypes/wheee.html>. The sheets list
  a few more rows (rounds, team scoring); the rows here are the ones that
  make the game.
- **Cops and Robbers.** Cops with shotguns, robbers with plasma pistols, a
  jail, a few lives.
  <https://www.neoseeker.com/forums/3951/t923589-halo-2-custom-games/>.
- **Bounce Practice.** Super bouncing (Xbox only; Lockout, Zanzibar,
  Ascension, Burial Mounds and others) needs repetition, not combat:
  <https://www.halopedia.org/Super_bouncing>. No source gives a standard
  practice variant; the settings are ours: nothing ends the game, players
  are hard to kill and respawn fast, and nothing on the map gets in the way.
  Vehicles stay at the map default because the Burial Mounds bounce needs a
  Warthog. Bounces depend on the 30 Hz tick, which this project keeps exact
  ([high-framerate.md](high-framerate.md)). Everyone has a magnum and
  nothing else (the user's choice): enough to shoot a friend off a ledge,
  not enough to get in the way.
- **Glitch Hunt.** Exploring for out-of-map spots used sword flying, rocket
  and grenade jumps; a Neoseeker thread mentions a "glitches" variant with
  overshields, swords and rockets. Our settings follow that.
- **Bumper Cars.** Vehicles with no guns, splatter only. A long-standing
  custom game; no source gives a Halo 2 recipe, so the settings are ours.
  Halo 2's editor has no unarmed start; our added choice "None" for the
  starting weapon gives one (game-options.md). Without our choices
  the variant would be left out, as any variant with
  a value the game doesn't offer is.

Considered and left out: Michael Jackson (the name), Fiesta and Dodgeball (retail names), Duck Hunt
(no sourced Halo 2 setup), Grifball (Halo 3). Gun Game has no sourced
Halo 2 setup either; its weapon progression is a script, and its
ladder is ours.

Names avoid the retail ones: Slayer, Team Slayer, Rockets, Swords, Snipers,
Phantoms, Team Phantoms, Elimination, Phantom Elim, King, Team King,
Phantom King, Crazy King, Team Crazy King, Oddball, Rocketball, Swordball,
Team Ball, Low Ball, Fiesta, 2 on 1, 3 on 1, Ninjanaut, Phantom Fodder,
Dreadnaut, the CTF, Assault and Territories set
(<https://www.halopedia.org/Custom_game>).

## The merged block

The wrapper on `0x149f88` builds one block for every built-in of ours:
the retail entries, then the community variants, then the game-variant
store's (`community_variants::extend_with`, with two groups), at most 101
in all, the content list's limit. A store variant past it is left out
with a line. Each group writes its pairs into its own page: the
community variants' at `0xa0fd0000`, the store's at `0xa0fd8000`
(`game_variants::PAIRS_BASE`, 16 KiB); the merged block stays at
`0xa0fd2000`.

A store variant's entry (`game_variants::inject::entries`) is its list
type, its record's value of every setting of its type that the `sily`
pages offer, and a description number. A value the editor does not
offer leaves the variant out, by name. Its ids are a private range of
their own, `0x00ffe200..=0x00ffe2ff` (`ui_kit::ids` family "game
variants"): names at `+i`, descriptions at `+0x80 + i`, a shared line at
`+0xff`. Description numbers 94 to 99 name the first six variants'
descriptions and 100 the shared line ("A game variant from this
machine's game-variant store."), since the game takes 0 to 100. The
string and description wrappers answer these ids and numbers beside the
community ones.

A community variant's entry (`community_variants::entries`) is built
the same way from its script (`rules::builtin::LISTED`, in that order;
[rules-variants.md](rules-variants.md#built-in-list-entries)):

| Part of the entry | From |
|---|---|
| name | the script's `rules` name, a space and U+E006, at name id `+i` |
| game type | the script's `carrier` as a list type |
| pairs | the record of the script (`variants::portable::record_for`, default options): its value of every setting of its type the `sily` pages offer (`game_variants::inject::pairs`, the store's rule) |
| description | the script's `about`, at description number `40 + i` (`community_variants::DESCRIPTION_BASE`) |
| row | the script's `list` line ([Tweakers](#tweakers)) |

A script with a value the editor does not offer is left out by name,
with a line ("community variants: SWAT left out: setting 89 is 3, which
the editor does not offer"); so is one that does not compile or makes
no record. Description numbers: retail 0 to 38, the listed scripts 40
to 93 (54 at most; 14 used), the store 94 to 100.

The pairs cannot carry the record's `hx` block. When the original
`0x149f88` returns, the wrapper writes each listed script's block (the
record's: mode 2, the script's hash prefix and default options, and any
extras its settings name) into its built-in record in the content list
(`[0x51ea14]`, records of `0x1e4`; `community_variants::write_blocks`),
found by its list name and engine. From then the built-in names its
script, so a game on it runs the script (every listed script is compiled
in, `rules::runtime::compiled_in`), and a copy Edit makes keeps the
block. A store variant's block is written at the game's start instead,
as below.

A store variant's settings reach the game through the pairs; its
record's `hx` block (extras and the script reference) is written at the
game's start, as rules-engine.md "Live game variants in the game" says.

## What the code does

All three hooks are wrappers at function entries
(`register_override`/`resolve_original`, as in `d3d_glue.rs`).

1. **The list.** `0x149f88` is wrapped. Before the original runs, the
   wrapper reads the `sily` definitions the `goof` pages name and builds
   each listed script's entry whose every `(setting, value)` is one the
   editor offers; the rest are left out with a log line each. It copies
   the retail entries unchanged into a page we map at `0xa0fd0000`,
   appends ours, writes our pairs next to them, and points `goof +0x28` at
   the merged block (address first, then the count). If the tag already
   points there (a second load of the same tag memory) it changes nothing.
   The retail entries and the original block are never written. It then
   calls the original through the dispatch (a flag sends that call
   straight to it) with a continuation (`call_guest`), and on its return
   writes the listed records' blocks and returns to the caller, the stack
   as the original left it.
2. **Names and descriptions.** Our string ids have the index range
   `0xffe000..=0xffe0ff` (the low 24 bits; retail ids are small indexes). The
   wrapper on `0x1a0180` answers those ids with our UTF-16 text in the
   caller's `0x100`-unit buffer and returns; every other id goes to the
   original. Names are "<name> U+E006". The match is on the whole id (top
   byte 0): it was on the low 24 bits once, and the co-op screens' texts,
   first at `0x20ffe000`, showed these names. The ids reach the wrapper as
   written (the `goof` entry's name, the description id we return), so
   nothing needs the looser match. `crates/extensions/game/src/string_ids.rs` checks
   every family of our ids against every other's claim. The store's
variants answer from their own range ("The merged block").
3. **Description numbers.** The variant list turns the entry's description
   number into the string id of its decimal text with `0x19fd00`, from
   `0x2ca1fd`, and the list's `unic` holds the text under those ids
   (retail uses 0 to 38). Ours are 40 to 53. The wrapper on `0x19fd00`
   returns our description id (the number is in ECX) only for a number of ours and only when the
   return address is `0x2ca1fd`; every other caller (numbers shown as
   text) gets the original.

The description number lives in the variant (`+3`), so a copy should keep
it: a copied community variant would show its description while this runs
and none in a build without it (not checked in a run). Its settings are
ordinary values and play anywhere.

Tests (`cargo xtask test -p game`, `community_variants::tests`) run on
synthetic memory: the entries are built from the listed scripts (name,
type, pairs, description number), every listed record gets its block at
load (and a retail record, or a listed name on another engine, does
not), a copy lists under its script's row, a script the disc does not
offer is left out by name, Bumper Cars needs our added "None", the
private id range and the description numbers, the UTF-16 buffer, the
merged block (retail entries byte for byte, ours decode back to their
entries, a second call is a no-op), and a failed read leaving the tag
alone. The script tests (names fit the 16-character keyboard limit and
are unique) are `crates/formats/rules/tests/scripts.rs`. Against the
disc (`#[ignore]`, `$H2_DISC`):
`every_listed_script_is_offered_on_the_retail_disc`.

## Tweakers

Two game type rows of ours come last in the game type list, after
Territories, in this order (`community_variants::Row`):

| Row | Pane title | Description | Built-ins |
|---|---|---|---|
| Research and Devolution | RESEARCH AND DEVOLUTION | "Practice the tricks and find the glitches: superbounces, crouch jumps and the maps' secrets." | Bounce Practice, Glitch Hunt, Superbounce |
| Party Games | PARTY GAMES | "Zombies, jousting, cops and robbers: the party games the community made up for custom games." | Zombies, Jousting, Cops and Robbers, Bumper Cars, Siege, Gun Game, Race, Hide and Seek |

Choosing a row opens a variant list of its variants, built-in and saved;
Slayer's list no longer shows them. Each row is an ordinary list row,
chosen with A like the game types above it.

Code: `crates/extensions/game/src/tweakers.rs` (feature `tweakers`, on by
default, [switches](../runtime/switches.md)). A variant is under a row
when its block names a rules script (mode 2) whose header names the row
(`list research` or `list party`,
[rules-engine.md](rules-engine.md#the-language)), or, with no script in
its block, when its name is a listed built-in's as the list holds it
(the name and U+E006; `community_variants::row_of`): a built-in record
before its block is written. The script is found by the record's prefix
among the scripts compiled into the game (the ports, the examples and
the listed scripts) and in the game-variant store
(`rules::header::block_row`); one this machine lacks lists under its
carrier's game type. Our rows hold
Slayer and Juggernaut variants (`tweakers::record_row` checks the
record's engine), so a row's list is Slayer's and Juggernaut's variant
lists filtered: its defaults, copies and saves are Slayer's. Every
built-in's record names its script ("The merged block"), and saving or
editing it keeps the block, so the copy stays under its row. Hide and
Seek's script says `list party` and it is a Juggernaut variant, so it
and its copies are under Party Games, on the lobby's list and the pause
menu's, and off Juggernaut's own list. A row's list asks Slayer's content
category and then Juggernaut's (`tweakers::JUGGERNAUT_CATEGORY`, 5) and
filters both; Create New stays dropped on it. Clint Eastwood's script has
no `list` line, so it stays under Juggernaut.

Hooks, all on retail's own screens (static reading of the XBE, 2026-11-21):

| Address | What it is | Ours |
|---|---|---|
| `0x2b1ab3` | the game type screen's constructor (screens 207, Change Rules, and 60, Settings, Game Variants); its list at `+0x614` holds one datum per row, the row's word at `+2` | `after`: add a datum with word 9 (Research and Devolution) and one with word 10 (Party Games) last, except in the create mode (`+0xb39`) |
| `0x2c5823` | a row's text, by word | `before`: words 9 and 10 draw their row's label |
| `0x2c5897` | a row chosen | `before`: words 9 and 10 run retail's press with Slayer's word, with the row set to mark the variant list it opens |
| `0x2c9ddb` | the variant list's constructor | `after`: write the mark at `+0x4fde` (a spare byte no code reads): the row's number (`Row::mark`, 1 Research and Devolution, 2 Party Games) while its press runs, else 0 |
| `0x215900` | a content category's variants, called by the list's fill at `0x2ca113` | `before`, from that caller only, for Slayer's and Juggernaut's categories: run the original (in a marked list, Slayer's category's call is followed by Juggernaut's), then keep the row's variants and drop Create New in a marked list, or drop every row's in an unmarked one. Each handle is read with `0x212bc0`, as the fill does. |
| `0x2cad45` | the variant list's per-frame update | `before` on a marked list: the row's title and description in the texts retail sets for Slayer's |
| `0x2cac1a` | the loader a screen stack reopens the variant list with | `before` on a marked list: an entry of ours wrapping Slayer's loader for the same mode and row, which opens it marked |
| pane set (`layout_glue`) | the row's pane | each row shows Slayer's pane with its title and description over its texts |

Live browser filters have no choice for our rows: a session advertises its
base game type, so a Superbounce or Zombies game is a Slayer game there,
and the Variant filter finds it by name ([live-custom-games.md](../live/live-custom-games.md#filter-games)).

Tests (`cargo test -p game --lib tweakers`) run on synthetic memory: the
rows are last, after Territories, in order, added once, on both screens
and not in the create mode; each row keeps its own variants and Slayer's
list the rest; every row's variant is a Slayer or Juggernaut variant, and Hide and
Seek lists under Party Games (a row's fill takes both categories'
handles) and off Juggernaut's own list, where Clint Eastwood stays; a
record of another type is on no row whatever its block; a renamed copy with the block stays under
Research and Devolution, a block with only other changes or a bad check
does not; a whole fill through the hook for Slayer's list and each row's list,
and another category's left alone; the pane and its texts; the reopen
loader per mode; the hooks on the plan.

Not verified in a game run:

- both rows and their labels on both screens, and their panes with their
  texts. "Research and Devolution" is the longest label on the list and
  may not fit the row;
- the variant lists: Slayer's without the rows' variants, each row's with
  its own and no Create New, and the row's title and description;
- a game started from Research and Devolution having the mode on through
  its script (the ext block surviving from the built-in record into the
  active variant);
- Edit on Superbounce saving a copy that lists under Research and
  Devolution;
- Back from a game or a nested screen reopening the row's list;
- that the lifter exposes the seven new points as hookable entries.

Change Rules opens with Slayer focused when the lobby's variant is under
one of our rows: retail's `0x2c5782` focuses the row by game type, and
ours are not mapped.

## Not done and risks

- **Not verified in a game run.** The unit tests and the disassembly above
  prove the work. A run would check: Settings, Game
  Variants should list the fourteen after the retail ones, each ending in the
  two-players icon, with its description; Edit on one should ask for a name
  and save a copy; a custom game on one should play with its settings. The
  boot log says "community variants: 14 added to the built-in list", or names
  any variant left out, and then "community variants: 14 built-in records
  given their script's block".
- **The blocks at the loader's return.** That every built-in is filed in
  the content list when `0x149f88` returns, and that the wrapper's call
  of the original through `call_guest` returns to the game's caller as
  the original would, is read from the disassembly, not checked in a run.
  The by-name filing in `row_of` keeps a built-in under its row until its
  block is there.
- **Copy and lock are the game's.** No hook decides them: an entry in the
  `goof` block becomes a built-in through `0x217080`, as the retail ones do.
  The tests prove our entries land in that block beside the retail ones, not
  what the game then does with them.
- **Other screens.** The lobby (`mp_variant_select_lobby`) and the in-game
  summary may show the description through another `0x19fd00` caller; they
  would show the number's text ("80") or nothing. Not traced.
- **Other languages.** Names and descriptions are English in every language.
- **Hot paths.** `0x1a0180` runs for every localized string; the wrapper
  tests the id with one mask first.
- **Another lane hooking `0x1a0180` or `0x19fd00`** would replace these
  wrappers (`register_override` keeps one handler per address). The
  `input-layouts` branch hooks `0x1a02d0`, which `0x1a0180` calls, so the
  two coexist.
