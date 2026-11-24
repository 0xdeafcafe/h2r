# Game types without carriers

Status: Design. Nothing here is built; "What the carrier supplies" is the state of the code and the slices are the order to change it in.
Code: `crates/formats/rules/src` (`ir.rs` the header, `lang/check.rs`, `caps.rs`, `builtin/`), `crates/extensions/game/src/rules/` (`runtime.rs`, `scoring.rs`, `objectives.rs`, `roles.rs`), `crates/formats/variants/src/portable.rs` (`record_for`), `crates/extensions/game/src/community_variants.rs`. Tests: `crates/formats/rules/tests/engines.rs`, `tests/modes.rs`.
Switch: none (the `rules` feature).

A game type is a rules script and nothing else. Slayer, Capture the Flag,
Firefight and Siege are the same kind of thing: a script that awards
points and says what type of game it is. The script does not name a
retail engine to ride (`carrier`), and no rule of the game is made by a
retail engine's own code. The rule that must hold: one scorer, the
script, and one scoring model, points; what a point stands for (a kill,
a capture, a second on the hill) is the script's business.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
"**E first**" marks a read of the retail executable a slice waits on.

## What the carrier supplies

A script names a carrier, one of the seven retail engines, and its
record carries that engine's index ([rules-engine.md](rules-engine.md),
"Which engine index a script rides"; `ir::Header::carrier`). The carrier
gives a script game these things:

| What | How the carrier gives it | Evidence | What replaces it |
|---|---|---|---|
| A record the game accepts | four retail paths refuse or reset an unknown engine index (the engine table, the record clean-up, the wire codec, the lobby protocol map) | **E**, rules-engine.md | every script game's record is index 2 (Slayer): the engine with the least behaviour of its own |
| Scoring | per-engine routines: the kill callbacks, and `SCORE_ADD` from ten carrier sites; a script game answers them in place (`rules/scoring.rs`, "One scorer") | **Code**; **E** | `score(who, n)` from the script is the only source of points; with one index only Slayer's kill callback is left to answer |
| Objectives | the flag, bomb, ball, hill and territories are run by the carrier's per-tick, per-player and objective slots, which still run in a script game | **E**, rules-engine.md | the script owns them: objects it spawns and tracks, zones it tests ([rules-world-api.md](rules-world-api.md)) |
| Player traits by role | the trait query slot; a host makes `role_traits` on its own carrier only | **Code**, `rules/roles.rs` | the host answers the trait query itself for any script (Slayer's query is every engine's fallback, `ENGINE_TRAIT_QUERY`) |
| The type area of the record | each engine's own settings and editor pages (`oddball_ball_speed`, `ctf_flag_type`), applying on that carrier only (`rules::settings::TABLE`) | **Code** | script options and settings; the type area is Slayer's and unused |
| The menu category | a variant is filed under its record's game type, or a `list` row of ours on Slayer and Juggernaut (`ir::List`) | **Code**, [community-gametypes.md](community-gametypes.md) | the script's `type` |
| The announcer's line at the start | not traced | rules-engine.md, "Not verified" | the script's `type`; **E first** |
| The HUD's objective parts and the scoreboard's labels | not traced: the replicated engine-globals entity (types 0 to 5), the engine event or the client's engine vtable | the same | HUD kit HUDs fed by script state ([hud-kit.md](hud-kit.md)); labels from `type`; **E first** |
| The post-game report | the statborg carries scores for every engine; per-engine columns are not traced | **E**, [firefight.md](firefight.md), "The game engine underneath" | the statborg as it is; labels from `type`; **E first** |

Firefight already works this way: it is a Slayer game whose rules the
host makes ([firefight.md](firefight.md)), and its director is a rules
script (`firefight::script::compile`). Siege is a script on Slayer that
uses the map's own flags (`builtin/siege.rules`). This page makes the
seven retail types the same.

## How it works

### The header

```text
rules "Capture the Flag" 1
type ctf
points "Captures"
teams 2
```

- **`type <name>`** replaces `carrier` and `list`. It is a category, not
  an engine. It picks the row of the game type list the script's
  variants are filed under, the announcer's line when the game starts,
  and the defaults for `points`. The names are a closed table,
  `rules::types::TABLE` (new):

  | `type` | Menu row | Start line | Points are |
  |---|---|---|---|
  | `slayer`, `ctf`, `assault`, `oddball`, `koth`, `juggernaut`, `territories` | the retail row | the retail line, played from the disc | Kills, Captures, Detonations, Time, Time, Kills, Time |
  | `research`, `party` | our rows after Territories | none | Points |
  | `firefight`, `siege` | rows of ours | a clip of ours (`art/announcer/`) | Points |

  A new type is a row of the table with a string and a clip from a
  package ([studio-redesign.md](../editor/studio-redesign.md),
  "Packages"); a community script picks from the table.
- **`points "<label>"`** names what a point is, for the scoreboard and
  the post-game. It is display only.
- **`uses <script>`** replaces `scoring carrier`: a script with settings
  and no handlers plays the named built-in's rules under its own record
  (SWAT `uses slayer`). `runtime::carrier_builtin` becomes a lookup by
  name.
- `carrier` is refused with the line to write instead. The IR header
  loses `carrier` and `List` and gains the type byte and the label.

### One scoring model

`score(who, n)` adds points to a player or a team. The host compares the
total with `variant.score_limit` and ends the round; ties, sudden death
and rounds are the common round resolution, which calls no engine slot
(**E**, rules-engine.md). Slayer is `on kill: score(killer, 1)`; CTF is
`score(team, 1)` when a flag reaches a stand; King of the Hill is
`score(holder, 1)` each second. Nothing else scores: with every game on
index 2 the one retail scorer left is Slayer's kill callback, which the
feature answers as it does today.

### Objectives belong to the script

The script reads where things go from the map's netgame flags by label
([rules-language-types.md](rules-language-types.md), "Labels"), spawns
the flag, bomb or ball as an object, and tracks who holds it; hills and
territories are zones it tests. Siege does this for territories and a
flag stand now. What the port of each objective type needs:

| Need | State |
|---|---|
| Zones from flag polygons, occupants, contest | built for Siege and the King and Territories ports (`tests/engines.rs`) on the fake game |
| A carried objective: pick up, drop, the holder, return to its stand | **E first**: whether carrying the flag, bomb and ball is the weapon system's (the object is a weapon the player holds) with the engine's slots only told about it, or the engine's slots do the attaching |
| Holder traits (speed, no vehicle, the damage of a ball) | the host's trait answer, above |
| The waypoint over an objective | a HUD kit world mark (`hud-kit-core/src/world.rs`) |

### Retail records

A record with no script on engine 1, 3, 4, 7, 8 or 9 (a retail built-in,
a player's saved CTF variant, a hopper slot) is turned into a script
game when the game starts: the host makes the index 2 record of the
built-in script of that type (`variants::portable::record_for`), with
the record's common settings kept and its type-area settings read into
the script's options. Where that swap can happen before the engine
object is chosen is **E first** (the engine pointer is written at game
start, rules-engine.md). The record on disk and on the wire is not
changed, so the clean-up and the codec see what they expect.

A script's options are 32 bits in the record
(rules-engine.md, "Carrying a script"). CTF's and Assault's type areas
hold more choices than Slayer's; whether each built-in's options fit is
counted in GT1, and a script whose options do not fit is refused there.

### The display

- **Menu.** `community_variants` files each built-in under its `type`'s
  row. Where the list groups saved variants by engine index is **E
  first**; until it is ours, a script variant of a retail type is filed
  by a row of ours with that name.
- **Start line.** The host plays the type's line when the round starts.
  What plays the retail lines today, and whether a Slayer game can be
  made to say another type's line, is **E first**.
- **HUD and scoreboard.** Slayer's HUD shows score and time. Each type's
  objective parts (flag state, the hill, the ball's holder) are a HUD
  kit HUD in the type's package, fed by the script's replicated state.
- **Post-game.** The statborg's scores, under the `points` label.

### Later: no retail engine object

Index 2 still means a retail Slayer object exists with its empty
callbacks. Removing it is owning the four paths that need an index (the
engine table, the clean-up, the codec, the protocol map), which is part
of the sim rewrite ([sim-rewrite.md](../engine/sim-rewrite.md)) and not
of this page's slices.

## Rules and limits

- One scorer. A host that cannot answer Slayer's kill callback runs no
  script and says so, as today.
- A type is display and filing. It changes no rule, and two scripts of
  one type need share nothing else.
- A script names no engine. `variant.engine` is removed; `ctf.rules`
  becomes two scripts (CTF and Assault) over one shared body.
- Films and Live name a script by hash, so a script rewritten for this
  has a new hash; nothing published depends on the old ones.
- Firefight needs AI ground on a multiplayer map (sectors, zones, squad
  slots, characters). That is map content a script requires through a
  capability, not part of a game type
  ([firefight.md](firefight.md), "The kit").

## Slices

Sizes as [rich-editors.md](../editor/rich-editors.md): S under 800 lines
with tests, M under 2,500. Tests are headless on `rules::fake::FakeGame`.

| # | Slice | Size | Needs | Accepted when |
|---|---|---|---|---|
| GT0 | **The reads** (**E**), as a note in `docs/game/notes/`: what plays the start line; where the game type list groups by engine; what each engine's HUD and scoreboard draw from; the post-game's per-engine parts; who attaches a carried flag, bomb or ball; where the engine object is chosen at game start | S | nothing | the note gives each a guest address and a verdict, or says the disc has no such path |
| GT1 | **The header**: `type`, `points`, `uses`; `carrier`, `list` and `scoring carrier` refused; the IR header; `rules::types::TABLE`; every built-in rewritten; the option count per built-in | M | nothing | every file of `builtin/` compiles with a type; `carrier slayer` is an error naming `type`; an unknown type is refused by name; `tests/modes.rs` passes |
| GT2 | **One index**: every script record is index 2 (`record_for`); `role_traits` for any script through the trait query; the refusals by carrier deleted from `rules/runtime.rs` | M | GT1 | on the fake game a Juggernaut script passes the role and its traits on an index 2 record; no code path reads a carrier |
| GT3 | **Zones**: King of the Hill and Territories as scripts that own their hills and territories | M | GT2 | the two engines' core rules in `tests/engines.rs` pass with no carrier slot delivered |
| GT4 | **Carried objectives**: the flag, bomb and ball as script objects; CTF, Assault and Oddball | M | GT2, GT0 | a capture scores and the flag returns, by time and by touch; a bomb armed on the plate scores; time held scores; with no carrier slot delivered |
| GT5 | **Filing and the start line**: variants filed by type; the host plays the type's line; clips of ours for our types | S | GT1, GT0 | each built-in lands in its type's row on an invented `goof` block; the round start posts the type's line on the fake game |
| GT6 | **The display**: a HUD kit HUD per objective type, the `points` label on the scoreboard and post-game | M | GT3, GT4, GT0; PK11 of studio-redesign | each HUD compiles and shows its state from an invented script state; the label reaches the fake scoreboard |
| GT7 | **Retail records**: a no-script record of another engine starts as its type's built-in on index 2, type-area settings mapped | M | GT3, GT4, GT0 | each retail engine's invented record maps to a script record whose settings read back equal; the stored record's bytes are unchanged |

GT1 and GT0 start together.

## Evidence

- The four paths that need a retail index, the engine slots and the
  in-place answers are **E** and **Code** in
  [rules-engine.md](rules-engine.md).
- How a client shows objective state, labels and announcer lines is not
  traced (rules-engine.md, "Not verified"); GT0 is that read.
- No port of a retail engine has had a parity run (rules-engine.md,
  "Porting the seven engines").

## Not verified

- That a Slayer game whose script runs CTF's rules leaves every
  client's HUD and scoreboard consistent.
- That a flag or ball a script spawns is carried, dropped and thrown as
  the retail one is.
- That swapping a retail record at game start is safe on a guest and in
  a film.

## Related

- [rules-engine.md](rules-engine.md), [rules-language-types.md](rules-language-types.md), [rules-variants.md](rules-variants.md): the engine, the language and variants as built
- [rules-world-api.md](rules-world-api.md): the objects and zones a script owns
- [firefight.md](firefight.md), [community-gametypes.md](community-gametypes.md): the modes that already ride Slayer
- [studio-redesign.md](../editor/studio-redesign.md): the packages game types ship in
