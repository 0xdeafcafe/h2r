# Community game variants

Halo 2 ships with 39 built-in game variants (Slayer, Rockets, Ninjanaut,
Crazy King and so on). This adds twelve classics the community set up by
hand in custom games, as extra built-ins. In the variant list they carry a
"two players" icon where the retail ones carry a padlock. Like the retail
built-ins they cannot be edited in place: choosing Edit asks for a name and
makes a copy, which is an ordinary saved variant.

Code: `crates/boot/src/community_variants.rs`, installed by the boot runner.
Set `H2_NO_COMMUNITY_VARIANTS=1` to leave the list as retail.

No game data is in the repository. Addresses are guest virtual addresses of
the retail XBE (SHA-256 `03215919…935d`), named only as hook points. The
layouts below were read from the XBE with `h2 disasm` and checked against
`mainmenu.map` with a scratch reader in `$TMPDIR`. The setting and value
numbers in the code are the encoding the editor uses. They are the values a
player picks, written out per variant; no retail table was copied.

## How Halo 2 keeps its variants

### Built-in variants come from a tag

`mainmenu.map` has one `goof` tag
(`multiplayer\game_variant_settings\multiplayer_game_settings`). The UI
globals name it at `wgtz +0x10` ([ui.md](ui.md)), and `0x236a7f` returns
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
every game font carries ([button-icons.md](button-icons.md) on the
`button-icons` branch lists the range). Built-in content is told apart from
saved content by bit 21 of its content handle (tested at `0x148a2c` and
`0x217300`), not by its name. A built-in can be played and copied but not
edited or deleted: the game says "Built-in Variant (cannot edit)", refuses
deletion, and Edit first asks for a name for a copy. A copy is written as a
normal saved variant.

So a variant added to the tag block before `0x149f88` reads it is a
built-in in every respect, including the lock and copy-on-edit. The icon is
whatever its name string ends with.

### Saved variants on disk

Saved variants go through XAPI's save-game calls (`XCreateSaveGame` and the
find calls; call sites are listed in [xapi.md](xapi.md)), under
`UDATA\4D530064\<save>\` with a `SaveMeta.xbx` ([storage.md](storage.md)),
signed by the game's own `XCalculateSignature*`. This work never touches
that path: our variants exist only in memory, and a copy goes through the
retail save code unchanged.

### The icon

U+E000 to U+E008 are small symbols every font maps: three faces, a box, a
fourth face, a camera, two people (U+E006), a download arrow (U+E007) and the
padlock (U+E008). Only one retail string uses U+E006 (a guest gamertag
format). Two people means "made by players", so the community variants use
it. No glyph is added or changed. If a purpose-made picture is wanted later,
the `button-icons` branch shows how to add one at an unused code point
(nothing uses U+E200 and up) without shipping art.

## The variants

Halo 2's editor has no gravity, player speed or infinite ammo for everyone
(only Juggernaut movement, ball speed and the carrier toggles change speed;
see the source list). Every classic below is built from settings the editor
offers. Rules a variant cannot enforce (who may use which weapon, switching
teams on death) were honour rules then and are in the description now.

Settings not listed stay at the game type's default.

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
| Bounce Practice | Slayer | no score or time limit; 3 s respawn; no suicide penalty; overshields; damage resistance; nothing on the map; no grenades |
| Glitch Hunt | Slayer | no score or time limit; 3 s respawn; no suicide penalty; overshields; damage resistance; energy sword and rocket launcher; no friendly fire |

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
  ([high-framerate.md](high-framerate.md)).
- **Glitch Hunt.** Exploring for out-of-map spots used sword flying, rocket
  and grenade jumps; a Neoseeker thread mentions a "glitches" variant with
  overshields, swords and rockets. Our settings follow that.

Considered and left out: Michael Jackson (the name), Bumper Cars (there is
no "no weapon" start), Fiesta and Dodgeball (retail names), Duck Hunt and
Gun Game (no sourced Halo 2 setup, and Gun Game needs weapon progression
the editor cannot express), Grifball (Halo 3).

Names avoid the retail ones: Slayer, Team Slayer, Rockets, Swords, Snipers,
Phantoms, Team Phantoms, Elimination, Phantom Elim, King, Team King,
Phantom King, Crazy King, Team Crazy King, Oddball, Rocketball, Swordball,
Team Ball, Low Ball, Fiesta, 2 on 1, 3 on 1, Ninjanaut, Phantom Fodder,
Dreadnaut, the CTF, Assault and Territories set
(<https://www.halopedia.org/Custom_game>).

## What the code does

All three hooks are wrappers at function entries
(`register_override`/`resolve_original`, as in `wrappers.rs`).

1. **The list.** `0x149f88` is wrapped. Before the original runs, the
   wrapper reads the `sily` definitions the `goof` pages name and keeps each
   variant whose every `(setting, value)` is one the editor offers; the rest
   are left out with a log line. It copies the retail entries unchanged into
   a page we map at `0xa0fd0000`, appends ours, writes our pairs next to
   them, and points `goof +0x28` at the merged block (address first, then
   the count). If the tag already points there (a second load of the same
   tag memory) it changes nothing. The retail entries and the original
   block are never written.
2. **Names and descriptions.** Our string ids have the index range
   `0xffe000..=0xffe0ff` (the low 24 bits; retail ids are small indexes). The
   wrapper on `0x1a0180` answers those ids with our UTF-16 text in the
   caller's `0x100`-unit buffer and returns; every other id goes to the
   original. Names are "<name> U+E006".
3. **Description numbers.** The variant list turns the entry's description
   number into the string id of its decimal text with `0x19fd00`, from
   `0x2ca1fd`, and the list's `unic` holds the text under those ids
   (retail uses 0 to 38). Ours are 80 to 91. The wrapper on `0x19fd00`
   returns our description id (the number is in ECX) only for a number of ours and only when the
   return address is `0x2ca1fd`; every other caller (numbers shown as
   text) gets the original.

The description number lives in the variant (`+3`), so a copy should keep
it: a copied community variant would show its description while this runs
and none in a build without it (not checked in a run). Its settings are
ordinary values and play anywhere.

Tests (`cargo test -p boot --lib community_variants`) run on synthetic
memory: the encoding of each setting, that every variant is well formed
(names fit the 16-character keyboard limit, no setting twice, never the
padlock), the private id range, the UTF-16 buffer, the merged block (retail
entries byte for byte, ours decode back to their symbolic settings, a
second call is a no-op), a variant with a value the game does not offer
being left out, and a failed read leaving the tag alone.

## Not done and risks

- **The game run.** See the report on the branch for what the one gate run
  showed. Choosing a community variant in a lobby and the copy flow were
  not driven by the scripted pad.
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
