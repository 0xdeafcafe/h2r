# Game types without carriers

Status: Design, with GT0 (the reads, [the GT0 note](notes/2026-11-24-game-type-engine-reads.md)), GT1 (the header), GT2 (eligible Slayer scripts on index 6 and script role traits), GT5 (filing and round-start lines), GE1 to GE4 (the record clean-up, the wire codec, the protocol map and the engine object for the script index), GM1 (`crates/tools/firefight-kit/src/ground.rs`) and GM4 (`siege.rules`'s labels, `rules::labels::reads`) built, and GM3's document, session and screens built (the builder hook is not wired, below); "What the carrier supplies" is the state of the code and the slices are the order to change the rest in.
Code: `crates/formats/rules/src` (`ir.rs` the header, `types.rs` the type table, `lang/check.rs`, `caps.rs`, `builtin/`), `crates/extensions/game/src/rules/` (`runtime.rs`, `scoring.rs`, `objectives.rs`, `roles.rs`), `crates/formats/variants/src/portable.rs` (`record_for`), `crates/formats/variants/src/record.rs` (`check`, `clean_up`, `protocol`), `crates/formats/variants/src/wire.rs` (the codec), `crates/extensions/game/src/rules/record.rs` (the record hooks), `crates/extensions/game/src/rules/engine_object.rs` (the engine object), `crates/hooking/guest-layout/src/points/game_types.rs` (its image entries), `crates/extensions/game/src/community_variants.rs`. Tests: `crates/formats/rules/tests/engines.rs`, `tests/modes.rs`, `crates/rewrites/engine/tests/variant_record_differential.rs`, `crates/extensions/game/src/rules/runtime/tests/script_index.rs`.
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

A script's `type` or `uses` selects its retail settings template
(`ir::Header::engine`). When the template is Slayer and the script writes
no type-area setting, `variants::portable::record_for` builds a record on
index 6 with Slayer's common defaults and a zero type area. Changes to common
settings, extension fields and script options travel in that record. Other
scripts use the retail template's engine. Existing saved records keep their
engine and settings. See [rules-variants.md](rules-variants.md#the-record-of-a-script).

| What | How the carrier gives it | Evidence | What replaces it |
|---|---|---|---|
| A record the game accepts | four retail paths refuse or reset an unknown engine index (the engine table, the record clean-up, the wire codec, the lobby protocol map), and a fifth files saved variants by it (the content category) | **E**, rules-engine.md, [the GT0 note](notes/2026-11-24-game-type-engine-reads.md), "Owning the four paths" | index 6 for eligible scripts (GT2), with the four paths answered natively and a host-owned engine object ("No retail engine object") |
| Scoring | per-engine routines: the kill callbacks, and `SCORE_ADD` from ten carrier sites; a script game answers them in place (`rules/scoring.rs`, "One scorer") | **Code**; **E** | `score(who, n)` from the script is the only source of points; the object on index 6 has no retail scorer, and games on retail indexes still need their scoring hooks |
| Objectives | the flag, bomb, ball, hill and territories are run by the carrier's per-tick, per-player and objective slots, which still run in a script game | **E**, rules-engine.md | the script owns them: objects it spawns and tracks, zones it tests ([rules-world-api.md](rules-world-api.md)) |
| Player traits by role | the VM tracks role membership and the trait query answers for members on every script | **Code**, `rules/roles.rs` | built (GT2), through `ENGINE_TRAIT_QUERY` |
| The type area of the record | each engine's own settings and editor pages (`oddball_ball_speed`, `ctf_flag_type`), applying on that carrier only (`rules::settings::TABLE`) | **Code** | script options and common settings, with a zero type area on index 6 |
| The menu category | Each save keeps the content category of its retail loader template. The lobby and pause lists file it by the script header (`community_variants::filed_type`). | **Code**, [community-gametypes.md](community-gametypes.md#tweakers) | built (GT5) |
| The announcer's line at the start | The authority waits for the seated players, then broadcasts the script type's retail event, sends its custom clip cue, or stays silent. The carrier's player-ready slot cannot post a second line. | **Code**, `rules/start_line.rs`; **E** for the event path, [the GT0 note](notes/2026-11-24-game-type-engine-reads.md) | built (GT5) |
| The HUD's objective parts and the scoreboard's labels | the replicated engine-globals entity is applied by each client's own engine slots after a type match (`0xa45e0`..`0xa4910`); events and the statborg need no engine; the HUD widget state (`0x13a050`) reads no engine; which widget shows which engine's state is not traced | **E**, partial, [the GT0 note](notes/2026-11-24-game-type-engine-reads.md) | HUD kit HUDs fed by script state ([hud-kit.md](hud-kit.md)); labels from `type`; **E first** for the widget map |
| The post-game report | the statborg carries scores for every engine; the report's per-engine part is keyed on the record's engine index copied at game start (`0x195a40`, the build at `0x232d77`), and its per-engine stat rows are written only by engine code | **E**, [firefight.md](firefight.md), "The game engine underneath"; [the GT0 note](notes/2026-11-24-game-type-engine-reads.md) | the statborg as it is; labels from `type` |

Firefight already works this way: it is a Slayer game whose rules the
host makes ([firefight.md](firefight.md)), and its director is a rules
script (`firefight::script::compile`). Siege is a script on Slayer that
uses the map's own flags (`content/modes/rules/siege.rules`). This page makes the
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
- **`uses <type>`** replaces `scoring carrier`: a script with settings
  and no handlers plays that retail type's built-in rules under its own
  record (SWAT `uses slayer`; `runtime::used_builtin`).
- `carrier`, `list` and `scoring` lines are refused (`header.retired`)
  with the line to write instead. The IR header holds the type byte, the
  `uses` byte and the `points` label (`ir::Header`).

### One scoring model

`score(who, n)` adds points to a player or a team. The host compares the
total with `variant.score_limit` and ends the round; ties, sudden death
and rounds are the common round resolution, which calls no engine slot
(**E**, rules-engine.md). Slayer is `on kill: score(killer, 1)`; CTF is
`score(team, 1)` when a flag reaches a stand; King of the Hill is
`score(holder, 1)` each second. The host-owned object on index 6 skips the retail scoring slots.
For a script on a retail index, `rules/scoring.rs` still intercepts the
retail kill callbacks and objective awards so the script scores each event
once.

### Role traits

`role_traits(player, enabled)` sets or clears that player's bit in the
VM's role mask. A role transfer clears the old holder and sets the new
holder in the same handler. The mask is part of the VM snapshot. A quit
clears the departing player's bit and round end clears all bits.

The trait query gives a holder extra damage by default, with active camo
and damage resistance off. A script can override those defaults with the
boolean options `role_active_camo`, `role_extra_damage` and
`role_damage_resistance`. A Juggernaut record retains the role flags the
player set in the variant editor. The Live actor channel sends the
authority’s role mask, trait values and script prefix in its mode-state
deltas and keys. Guests apply a matching state to their trait queries.
[Actor replication](ai-networking.md).

The runtime admits a script independently of its header's retail template.
The retail `reset_objective` implementation still needs the ball spawns in
an active Oddball record.

### Objectives belong to the script

The script reads where things go from the map's netgame flags by label
([rules-language-types.md](rules-language-types.md), "Labels"), spawns
the flag, bomb or ball as an object, and tracks who holds it; hills and
territories are zones it tests. Siege does this for territories and a
flag stand now. What the port of each objective type needs:

| Need | State |
|---|---|
| Zones from flag polygons, occupants, contest | built for Siege and the King and Territories ports (`tests/engines.rs`) on the fake game |
| A carried objective: pick up, drop, the holder, return to its stand | the weapon system's: the object is a weapon item the player holds (`0x10ca80`), and the engine's slots (`+0x5c`, `+0x60`) are only told and keep a registry (**E**, [the GT0 note](notes/2026-11-24-game-type-engine-reads.md)) |
| Holder traits (speed, no vehicle, the damage of a ball) | the host's trait answer, above |
| The waypoint over an objective | a HUD kit world mark (`hud-kit-core/src/world.rs`) |

### Retail records

GT7 covers records with no script on engine 1, 3, 4, 7, 8 or 9, including
retail built-ins, saved variants and hopper slots. Its target is an
index-6 record of the built-in script of that type, with common settings
preserved and type-area settings mapped into script options. The swap belongs at `GAME_ACTIVATE` entry, before the engine's
subsystem start copies the engine pointer from the options
(`0x1574c0`), and every machine makes the same swap (**E**, [the GT0 note](notes/2026-11-24-game-type-engine-reads.md)). The record on disk and on the wire is not
changed, so the clean-up and the codec see what they expect.

A script's options are 32 bits in the record
(rules-engine.md, "Carrying a script"). CTF's and Assault's type areas
hold more choices than Slayer's. Counted over the settings table's
type-area rows at the record's full range, plus each built-in's own
options: CTF needs 22 bits and Assault 25, so both fit
(`builtin::tests::each_retail_game_fits_its_type_area_in_the_option_bits`).
A script whose options do not fit is refused.

### The display

- **Menu.** `community_variants::filed_type` resolves a record's script
  reference and files it by its declared type, including renamed saves and
  store imports. The lobby reads all seven physical content categories.
  The pause menu applies the same classification to the built-in list.
  Index 6 uses Slayer's physical save file and loader template. Before a
  record is selected or copied, `write_blocks` restores its index and
  script reference and clears its type area. A script missing on this
  machine falls back to the template's row.
- **Start line.** The script header supplies the line even when `uses`
  supplies another type's rules. Once the round's seated players are ready,
  the host posts the type's index-zero retail event to the simulation
  (`GAME_EVENT_POST`) or queues a Firefight or Siege clip locally and sends
  a `StartClip` cue to viewers. Research and Party Games are silent.
  The Firefight director also speaks its type at the start of each director
  round. The game's announcer queue shares playback with the custom clips.
  See [the clip sources](../../art/announcer/README.md).
- **HUD and scoreboard.** Slayer's HUD shows score and time. Each type's
  objective parts (flag state, the hill, the ball's holder) are a HUD
  kit HUD in the type's package, fed by the script's replicated state.
- **Post-game.** The statborg's scores, under the `points` label.

### No retail engine object

A record on index 2 selects the retail Slayer object. A record on
`rules::SCRIPT_INDEX` selects the host-owned engine object. Native
handlers answer the retail paths that consume this index.

**The script index is 6**, the slot of the deprecated Headhunter engine
(`rules::SCRIPT_INDEX`, `variants::record::SCRIPT_INDEX`). From the reads
in [the GT0 note](notes/2026-11-24-game-type-engine-reads.md), "Owning
the four paths":

- It is inside every range the game checks: the clean-up clamps `+0x44`
  to 1..9 and the protocol map's table covers 1..9, so a path we have
  not answered sees a deprecated engine, never another type. Index 10,
  the only other free entry of the engine table, is clamped to 9 by the
  clean-up and would turn into Assault.
- Its engine table entry (`0x55e4e8`) is in `.bss` and nothing writes it,
  so the engine object of GE4 goes there without replacing a retail
  object or racing the table's initializer (`0x378500`, which writes
  index 10).
- It fits the codec's 4-bit engine field.
- Every retail path left unanswered fails safe on it: the clean-up
  resets the record to Slayer, the decode refuses it, the protocol map
  answers synchronous, as for Race and Headhunter today.

Race (5) has the same answers on every path read; 6 is taken so that 5
stays an index every path refuses.

| Path | Native body | State |
|---|---|---|
| Record clean-up `0x19d650` (`VARIANT_CLEAN_UP`) | `variants::record::clean_up`: a script record that `check` keeps returns AL 1 with the registers of the game's clean exit; one whose only fault is a non-zero type area has the area zeroed and returns AL 0; one that breaks a common rule gets Slayer's index and the game's body, which resets it to its default Slayer record. `check` takes the script index: common rules as every engine's, a type area of zero | built (GE1); every caller is in [the GT0 note](notes/2026-11-24-game-type-engine-reads.md); the decode and the content reads treat AL 0 as a failed read, the options copy keeps the cleaned record |
| Wire codec, encode `0x7cc50` (`VARIANT_ENCODE`) and decode `0x7d520` (`VARIANT_DECODE`) | `variants::wire`: the game's bit layout and bit stream, with no type area on the wire for the script index and a zero one in the decoded record, then the native clean-up on a copy | built (GE2); the game's encoder jumps through a zero entry for 5 and 6 (its type-area table has no bounds check) and its decoder refuses them ([the GT0 note](notes/2026-11-24-game-type-engine-reads.md), "GE2"); only a script record is answered natively, every other goes to the game's body |
| Lobby protocol map `0x161c90` (`VARIANT_PROTOCOL`) | `variants::record::protocol`: the game's table, with the script index distributed, and the registers the game's body leaves | built (GE3); its two callers choose the protocol only (`0x640a7`, `0x7155d`) |
| Engine table `0x55e4d0[index]` and the engine start `0x1574c0` | `rules::engine_object`: at `GAME_ACTIVATE` entry, on every machine, the entry for the script index (`0x55e4e8`) gets a host-owned object whose vtable is Slayer's 51 slots read from the image with Slayer's four rule bodies (`+0x14`, `+0x18`, `+0x24`, `+0x78`, the kill scoring) replaced by the empty bodies of the same pop the other engines use. `+0x00` answers 2, `+0x04` sets up the globals the shared entity bodies read, `+0x9c` answers 0, so every client builds Slayer's engine-globals entity. The runtime takes its events from the common paths (`PLAYER_KILLED`, `GAME_TICK`, `ENGINE_ROUND_RESET`, `ENGINE_PLAYER_LEFT`) and accepts any script on a record of the script index | built (GE4); the slots marked own or shared and the entity type in [the GT0 note](notes/2026-11-24-game-type-engine-reads.md), "GE4" |
| Spawn gather `0x23b170`, its jump at `0x23b2dc` (`SPAWN_TYPE_MASK`) | the type's bit of a starting location's mask, through a jump on `E+0xc14` with no bounds check and zero entries for 5 and 6; answered for the script index as Slayer's case (bit 0), resuming at `0x23b2ea`. Both are image entries (`points::game_types`) | built (GE4); every multiplayer spawn reaches it from `PLAYER_SPAWN_LOCATION` |
| Content category `0x216a50` (`VARIANT_CATEGORY`) | `variants::record::content_category`: Slayer's category (1) for the script index, the game's table for every other; the visible row is the script's declared type (GT5) | built for the script index; the game's body jumps on `+0x44` with no bounds check (a zero entry for 5 and 6), on save and on the built-in add |

The record answers are `before`s of the `rules` feature (`rules/record.rs`),
on every machine, so a record of the script index is kept by the host
and by each guest's decode alike.

`rules/scoring.rs` still installs its hooks for games on retail indexes.
A game using the host-owned object cannot reach the retail kill or
objective scoring slots. GT6 supplies the base score and time HUD. The
engine-globals entity uses the shared Slayer body on each client.

Retail records retain their encoding and clean-up rules. GT7 covers
their conversion at game start.

## Maps for scripted games

A Firefight or Siege map is a map with places the script reads by label
([rules-engine.md](rules-engine.md), "Forge labels") and, for AI, ground
the actors can walk. It is made in Forge or in Studio; both write the
same things and build the same way. Waves, rounds and scoring are the
game variant's (the script and its options), not the map's.

### What the author places

| Label | Is | Placed as | With none placed |
|---|---|---|---|
| `ff_drop` | where a wave arrives | a marker with a facing | the generator picks 6 to 12 as `firefight-kit/src/nav.rs` does |
| `ff_base` | a base the actors push at, and the team that spawns there | a marker with a team | the two bases `firefight-kit/src/bases.rs` finds |
| `ff_cover` | a firing position | a marker with a facing | sampled from the sectors |
| `siege_site` | a site the attackers take | a territory flag, as `siege.rules` reads now | the map's own territories |
| `siege_goal`, `siege_line` | the final stand; where each side spawns per phase | a flag stand; markers with a phase and a team | the map's CTF stands |

A script declares the labels it reads, so the tools offer exactly those
for the game type the author is building for, and a map with none of a
script's required labels is refused in the lobby by name. The declaration
is the rules language's own `label` line, with no addition to the
grammar: `siege.rules` declares `site` as `zones = "siege_site"`, `goal`
as `zone? = "siege_goal"`, and `placed_line` as
`zones at most 16 = "siege_line"` (a `siege_line`'s number is its phase,
its team the side that spawns there). A script's `siege_site` reads the
map's territory flags while none is placed, and a `siege_goal` or a
`siege_line` is read ahead of the defenders' CTF stand and the phase's own
site (`siege.rules`, `rules::labels::reads`; "Forge labels" in
[rules-engine.md](rules-engine.md)). Siege and Firefight require none, as
the table's last column says (every map plays); a script that does, such
as a Holdout that declares `label ff_drop: objects at least 2`, is
refused on a map short of them with `the map has no object labelled
"ff_drop"; Holdout needs at least 2`
(`crates/formats/rules/tests/scripts.rs`). Firefight's director reads its
drops from the kit's `ff_drop_NN` zones, which the build makes one for
each placed `ff_drop` (GM1), so a wave lands at a placed drop.

### In Forge

The monitor's Objects tool has an **AI and objectives** palette page
with the labels above (`forge_screens::sheet::objects_with_ai`); a
marker is a netgame flag the document adds with a label op
(`crates/formats/forge-doc/src/ground.rs`), placed in front of the
monitor, and moved and removed through the session
([forge-map-variants.md](forge-map-variants.md), "AI ground"). The page
offers every label; a script's own list narrows it when the Game Type
row picks one.

Ground is built when it is asked for, never while placing:

1. **Build AI ground** on the Forge options page (and **Test waves**,
   which builds first when the ground is stale or never built). The
   session marks the ground stale on any op that moves a label, or adds,
   moves or removes something with collision
   (`ForgeDoc::ground_stamp`, `Session::ground_state`).
2. The build runs off the game thread: the generator over the base's
   collision plus the session's solid objects
   ([forge-design.md](forge-design.md), slice F9), zones and firing
   positions from the labels, sixteen squad slots, the characters.
3. Pathfinding in `sbsp` is a Rebuild change, so the builder
   (`forge::GroundBuilder`, `boot::ground_builder::AppGround`)
   materialises the play form, which builds the ground over it
   (`forge_build::build`). It is meant to reload the map, keeping
   the document, the monitor's position and the tool, and that
   reload is not built ([forge-map-variants.md](forge-map-variants.md),
   "AI ground"); the report
   (`GroundReport`) lists the problems the overlay draws (a drop with no
   path to a base, a site off the ground, the headroom refusal with its
   biggest part).
4. **Save** builds first when the ground was built and is stale
   (`Session::save_building`), so a saved variant always has ground
   that matches it; a build that fails stops the save. A session that
   never built ground saves as it is.

### In Studio

Opening the scenario gives the map editor; its Scene tab has an **AI
and objectives** group with the same labels, placed with **Place new
with the mouse**. **Build AI ground** is on the toolbar; the result
draws in the Navigation layers (`view.layers`, `nav.pathfinding`), the
problems go to Problems, and **Playtest** runs waves on it headless
([playtest.md](../editor/playtest.md)). Saving writes a `map` item into
the edit target with its `ground` table.

| Method (Studio) | Does |
|---|---|
| `ground.status {doc?}` | stale or built, counts of sectors, drops and positions, the cost against headroom |
| `ground.build {doc?}` | builds; answers with a job as `playtest.run` does |
| `scenario.add {block: "label", name, ...}` | places a label |

### The build step

`firefight_kit::ground::build(map, name, labels, recipe)`
(`crates/tools/firefight-kit/src/ground.rs`) is the one build; Forge,
Studio and `h2 firefight kit` call it, and it runs only when asked.

- **Labels** (`Labels`): `ff_drop`, `ff_cover` (a position and a yaw),
  `ff_base` (a position and a team), `siege_site`, `siege_goal`,
  `siege_line` (a position, a phase and a team). They are sorted before
  use, so the order they were placed in changes nothing: zone `NN` is
  the `NN`th drop by position.
- **What a label replaces.** Any `ff_drop` replaces the generated drops:
  one `ff_drop_NN` zone per label, its first firing position the
  label's own point, cover facing its yaw. `ff_base` labels, one for
  each team or none, replace the bases `bases.rs` finds
  (`Source::Labels`). A placed `ff_cover` within reach of a zone is that
  zone's first firing position after the drop's own. With none placed,
  the generator picks as the table above says.
- **Problems** (`Ground::problems`): a drop or cover off the walkable
  area the bases share, a drop with no room for two firing positions,
  and a siege label off the ground the AI keeps. The build succeeds
  with the rest, and fails only when no drop zone is left.
- **The recipe** (`Recipe`): the kit's version, the home team and kit or
  probe. A recipe for another kit version is refused.
- **The receipt** (`Receipt`) holds the SHA-256 of the base map, the
  digest of the canonical labels and the recipe. **The stale rule**
  (`Receipt::stale`): the ground is stale when the kit version, the
  base map's bytes (its collision, with a session's solid objects), the
  recipe or the labels differ from the receipt's, and only then; it
  answers which (`Stale`).
- **Headroom.** A ground that does not fit is refused with its cost and
  its biggest part, before `Ground::install` writes anything.

### Where the ground is kept

The ground is made from the disc map's collision, so it is disc-derived
and is stored nowhere but in the built map on each machine.

- **A Forge map variant** stores the labels and an `ai_ground` recipe
  (`forge_doc::GroundRecipe`: the kit's version, the home team and the
  mode) in the document, beside the kit's recipe; `Tier::Variant` allows
  it. The
  built ground is in the materialised package,
  `<data>/maps/forge-<id>/`, whose receipt covers the recipe.
- **A Studio map** stores the same in its package: the labels as edit
  lines on the scenario and `ground` in `map.toml`. The built ground is
  in the map package under `<data>/maps/`.
- Every machine builds its own from its own disc when the map is picked
  ([forge-map-variants.md](forge-map-variants.md), "Materialising for a
  match"): the build is deterministic, so tag indices agree, which the
  host's entities need ([firefight.md](firefight.md), "The kit").
- Nothing is built at boot and no map gets ground it was not given.
  `h2 firefight kit <map>` stays as the command that makes a map with
  generated labels and builds it.

## Rules and limits

- A game on index 6 receives its scores from the script. Games that still
  use retail engine objects need their scoring hooks to prevent a second award.
- A type is display and filing. It changes no rule, and two scripts of
  one type need share nothing else.
- A script names no engine. `variant.engine` is removed; CTF and
  Assault are two scripts over one shared body (`content/core/rules/ctf.rules`
  under two headers, `ctf.head` and `assault.head`, with `const assault`).
- Films and Live name a script by hash, so a script rewritten for this
  has a new hash; nothing published depends on the old ones.
- AI ground is map content, built on demand and kept only in built
  maps ("Where the ground is kept"). It is never in Git, in a package's
  sources or on Live.
- A map that has no room for the ground is refused at build with the
  cost and its biggest part, before anything is installed.

## Slices

Sizes as [rich-editors.md](../editor/rich-editors.md): S under 800 lines
with tests, M under 2,500. Tests are headless on `rules::fake::FakeGame`.

| # | Slice | Size | Needs | Accepted when |
|---|---|---|---|---|
| GT0 (built) | **The reads** (**E**), as a note in `docs/game/notes/`: what plays the start line; where the game type list groups by engine; what each engine's HUD and scoreboard draw from; the post-game's per-engine parts; who attaches a carried flag, bomb or ball; where the engine object is chosen at game start | S | nothing | the note gives each a guest address and a verdict, or says the disc has no such path |
| GT1 (built) | **The header**: `type`, `points`, `uses`; `carrier`, `list` and `scoring carrier` refused; the IR header; `rules::types::TABLE`; every built-in rewritten; the option count per built-in | M | nothing | every file of `builtin/` compiles with a type; `carrier slayer` is an error naming `type`; an unknown type is refused by name; `tests/modes.rs` passes |
| GT2 (built) | **Eligible scripts on index 6**. `record_for` moves scripts with the Slayer template and no type-area writes. Any script can set role traits through the VM and the trait query. | M | GT1, GT5, GE1 to GE4 | an eligible script retains its common settings and options with a zero type area. A Juggernaut script transfers its role on index 6, and role membership survives a snapshot and clears on quit and round end |
| GT3 | **Zones**: King of the Hill and Territories as scripts that own their hills and territories | M | GT2 | the two engines' core rules in `tests/engines.rs` pass with no carrier slot delivered |
| GT4 | **Carried objectives**: the flag, bomb and ball as script objects; CTF, Assault and Oddball | M | GT2, GT0 | a capture scores and the flag returns, by time and by touch; a bomb armed on the plate scores; time held scores; with no carrier slot delivered |
| GT5 (built) | **Filing and the start line**: variants filed by type; the host plays the type's line; clips of ours for our types | S | GT1, GT0 | each built-in lands in its type's row on an invented `goof` block; the round start posts the type's line on the fake game |
| GT6 | **The display**: a HUD kit HUD per objective type, the `points` label on the scoreboard and post-game | M | GT3, GT4, GT0; PK11 of studio-redesign | each HUD compiles and shows its state from an invented script state; the label reaches the fake scoreboard |
| GT7 | **Retail records**: a no-script record of another engine starts as its type's built-in on index 6, type-area settings mapped | M | GT3, GT4, GT0 | each retail engine's invented record maps to a script record whose settings read back equal; the stored record's bytes are unchanged |
| GE0 | **The engine-object reads** (**E**), a note beside GT0's: the "E first" column of "No retail engine object"; the table's slots for the deprecated indexes; which number `SCRIPT_INDEX` is | S | nothing | the note lists every reader and caller with its address, the 51 slots marked own or shared, and the chosen index with its reason |
| GE1 (built) | **The record**: `variants::record::check` and the native clean-up at `0x19d650` take the script index | M | GE0 | on invented records the native body equals `check` for every retail index; a script record keeps its index and reference; a non-zero type area on one is cleaned |
| GE2 (built) | **The codec**: native encode and decode with an empty type area for the script index | M | GE1 | every invented record of a retail index encodes to the bytes the model in `crates/formats/variants` gives and decodes back; a script record round-trips; an out-of-range index fails the decode |
| GE3 (built) | **The protocol map**: the native answer at `0x161c90` | S | GE0 | the script index answers distributed; every retail index answers as the table of [live-coop.md](../live/live-coop.md) says |
| GE4 (built) | **The engine object**. Index 6 selects a host-owned object with shared engine bodies. The runtime receives kills and round events from the common paths. | L | GE0 to GE3 | a scripted kill on index 6 reaches `on kill` without invoking a retail scoring slot |
| GE5 | **Every script game on the script index**. Move the remaining templates and retail-record swap, films and sessions to index 6. Remove retail scoring hooks after the last scripted retail record is covered. The base HUD shows score and time. | M | GT2, GT6, GT7 | `tests/modes.rs` and `tests/engines.rs` pass on script-index records; no built-in's record is index 2; the base HUD compiles and shows an invented score and clock |
| GM1 (built) | **Ground as a build step**: `firefight_kit`'s generator behind `ground::build(map, labels, recipe)`, labels in place of found bases and picked drops, the stale rule, the receipt | M | nothing | on the kit's invented court: no labels gives the tags `build_package` gives; a moved `ff_drop` moves its zone; the same inputs give the same bytes; over headroom is refused with its biggest part |
| GM2 | **Studio**: the AI and objectives group, **Build AI ground**, `ground.status`, `ground.build`, the Navigation layers, the `map` item's `ground` | M | GM1, PK9 of studio-redesign | a label placed in the window and by `scenario.add` make the same document; a build on an invented map lists its sectors; an edit that moves collision marks it stale |
| GM3 (built, but for the reload) | **Forge**: the palette page, the `ground` recipe in the document and `Tier::Variant`, **Build AI ground**, the rebuild and reload (the build is built; the reload is not), build on save, the overlay | M | GM1 | a variant with labels and a recipe validates as a variant; materialising it equals GM1's build of the same inputs; a save with stale ground builds first |
| GM4 (built) | **Labels the scripts read**: Firefight's director and `siege.rules` take their places from labels; the lobby's refusal for a map without them | S | GM1, GT1 | on the fake game a wave lands at a placed drop; Siege's sites are the placed ones; a map with no required label is refused by name |

GT2 uses the GE1 to GE4 record and engine paths and GT5 filing. GT3 and
GT4 supply the script-owned objectives. GT7 maps retail records onto the
new scripts. GE5 extends index 6 to the remaining games.

## Evidence

- The four paths that need a retail index, the engine slots and the
  in-place answers are **E** and **Code** in
  [rules-engine.md](rules-engine.md).
- The start line, the list's filing, the post-game's keying, carried
  objectives, where the engine is chosen and the four paths: [the GT0 note](notes/2026-11-24-game-type-engine-reads.md).
  Which HUD widget shows which engine's state is not traced.
- No port of a retail engine has had a parity run (rules-engine.md,
  "Porting the seven engines").

## Not verified

- That a Slayer game whose script runs CTF's rules leaves every
  client's HUD and scoreboard consistent.
- That a flag or ball a script spawns is carried, dropped and thrown as
  the retail one is.
- That swapping a retail record at game start is safe on a guest and in
  a film.
- That a game with a host-owned engine object and an index of ours
  starts, replicates and ends on a guest, in a lobby and in a film; and
  that the image builder makes the spawn gather's site and its resume
  entries (`render_aspect_sites_are_dispatch_blocks_in_the_build_tiers_analysis`
  with the retail image).
- The time a ground build takes in a Forge session, and that the reload
  after it keeps the session whole.
- That two machines' ground builds of one variant give the same tag
  indices in a real session.

## Related

- [rules-engine.md](rules-engine.md), [rules-language-types.md](rules-language-types.md), [rules-variants.md](rules-variants.md): the engine, the language and variants as built
- [rules-world-api.md](rules-world-api.md): the objects and zones a script owns
- [firefight.md](firefight.md), [community-gametypes.md](community-gametypes.md): the modes that already ride Slayer
- [forge-design.md](forge-design.md), [forge-map-variants.md](forge-map-variants.md), [forge-features.md](forge-features.md): the Forge document, materialising and the nav tools
- [studio-redesign.md](../editor/studio-redesign.md): the packages game types ship in
