# Forge map variants: game types and AI ground

Status: part of [forge-map-variants.md](forge-map-variants.md); see there for status, labels and the index. This page: game types and the AI ground.

### Game types

Which things a game type has is the **Game types** field: a set of
engines, or all of them.

| Kind | Where the set lives | Who filters in a match |
|---|---|---|
| Item spawner | the entry's game-type slots | the engine, as retail: the tick skips an entry the matcher `0x15a090` does not match (**E**) |
| Starting point | nowhere: no retail reader of a game type was found (**E**, **S**, as in "Fields by kind") | not filtered |
| Goal, teleporter | the flag's type: a flag is its game type's by what it is | the engine: each game engine walks the types it reads |
| Object | the `games` property | the materialiser, at build time |

- **The set and the slots.** An item spawner has four slots, each an
  engine's code or a group value: 12 every engine, 13 and 14 every
  engine but CTF and Assault, which match each other's code; King of
  the Hill's code is 0 (**E**, `map::scenario_layout::game_types`).
  `forge_doc::games::encode(set)` gives the slots for a set: a group value when one equals the
  set, else one slot an engine. A set that needs more than four slots
  is refused with "An item can name four game types at most."
  `decode(slots)` gives the set back, and an entry whose slots hold a
  value the table does not know is shown as it is and not edited.
- **In a session.** The session runs the lobby's game type
  ([forge-firefight-ui.md](forge-firefight-ui.md), "The lobbies"), so
  the engine spawns that game type's items and reads its goals, and
  the Goals group offers its goals. A tool setting, **Show** (`This
  game type`, `All game types`; `forge.show`, default the first),
  decides what else is drawn: with All, a thing of another game type
  has its world mark in the `Inactive` style and its game types in its
  label, and can be aimed at and edited. Its field writes are Live in
  the loaded scenario like any other; nothing spawns from it until a
  game of its type.
- **Which game type a thing is in**, for aiming (`forge::tool::in_game`,
  given the variant record's engine index, `+0x44`, which the host reads
  each tick): an item spawner when one of its slots matches the engine
  by the engine's own matcher (`game_types::slot_matches`; King of the
  Hill's index 4 is its code 0); a goal by its flag type, CTF's flag
  spawn and return (0, 1), Assault's bomb spawn and return (2, 3),
  Oddball's ball spawn (4), a territory (10) and a hill corner (11 to
  18), as forge-doc's `builtin_label` traces them (**S**: which engines
  read types 0 to 3 beyond their own); every other thing, and a flag of
  an untraced type, in every game type. With no engine index read,
  every thing is the session's. The Tool settings screen's Show row is
  not built: the tool's `Show` is `This game type` until it is.
- **A new thing's set.** A goal's is its role's. An item spawner or a
  starting point made in a session starts as all game types, as the
  map's own mostly are (**S**: the ignored test
  `disc_item_game_types_and_respawn_modes_census` prints how the retail
  maps use the slots).
- **Objects.** A scenery, crate or machine placement has no game-type
  field (**C**). Its set is the `games` property
  (`Property::Games(Option<GameSet>)`, `forge_doc::OBJECT_BLOCKS`),
  written as engine indices in ascending order (`[1, 9]`); a clear is
  every game type, and an empty set or one of every engine is refused.
  `forge_build::build_for(doc, engine, ..)` builds the map for an engine:
  `forge_doc::for_game` removes each object whose set lacks it with an
  entry removal after the last op and leaves out the ops that set its
  game types, so the built document never has more ops than the
  authored one; the package holds the document as authored. A document
  with such a property has the engine in its package's identity: the
  `[[forge]]` layer's `game` and the format 2 `MapId`'s `game` member,
  present only then, so every other document keeps its id
  (`forge_build::map_id`; [maps.md](maps.md), "Identity: `MapId`").
  `forge_build::build` names no engine and refuses such a document
  (`forge_game`). An object with game types has no link and no label,
  since a build may leave it out, and a document with one has no
  `expect`: each engine builds its own map. Each machine knows the
  engine from the session's variant, so each builds the same file. The
  lobby's pick (`map_variant::pick`, `Maps::build`) does not pass an
  engine yet, so a variant with object game types is refused there.
- **The start gate.** A map variant that lacks what the game variant
  needs blocks the start by name: a script's labels through
  [rules-start-gate.md](rules-start-gate.md), and a retail engine's
  goals through the same census (`forge_doc::labelled`, which reads
  the document's flags with the map's).

### AI ground

A Firefight or Siege map is made in Forge as well as in Studio: the
author places labelled markers and asks for the ground
([game-types-without-carriers.md](game-types-without-carriers.md), "Maps
for scripted games"). The document holds the places and the recipe and
never what the build makes, since the ground is derived from the disc's
collision and each machine builds its own.

- **A marker** is a netgame flag the document adds
  (`Placement::Add` on `netgame_flags` with `position`, `facing` and
  `team`) with a label op naming it, one of `ff_drop`, `ff_base`,
  `ff_cover`, `siege_site`, `siege_goal` and `siege_line`
  (`forge_doc::GroundLabel`). A `siege_line`'s number is its phase. A
  seventh label, `siege_barrier`, is the one that marks an object, not a
  flag: scenery, a crate, a machine or a vehicle, numbered by the phase it
  opens in (no number is open from the start). The AI and objectives page
  offers it (`Session::label_barrier` labels the object the monitor aims
  at, opening in phase 1), Properties still takes it typed, and the ground
  build and the stamp do not read it, so labelling a barrier leaves the
  ground current. A
  map's own flag labelled the same way is a marker too. A ground label
  on any other block, or a `siege_line` with no phase, is refused by name
  (`ForgeDoc::ground_marks`). A session adds a marker as a flag with no
  object, so it places nothing in the world
  (`Session::place_marker`, `crates/extensions/forge/src/scene.rs`
  `Marker`).
- **The recipe** is a `generate` recipe named `ai_ground`
  (`forge_doc::GROUND_RECIPE`) with the kit's version, the home team,
  the mode and the enemy classes past the Elite, each with its donor
  map's name and hash (`GroundRecipe`, `GroundClass`;
  [forge-firefight.md](forge-firefight.md), "Enemy classes").
  `ForgeDoc::tier` allows it, so a variant with markers and a recipe is
  a map variant (`Tier::Variant`); the validator refuses a malformed one
  and a second. A session keeps the
  recipe beside its document, not in it, so the log's chain is
  untouched: a save writes the document with the recipe
  (`Session::ground_form`) and a load takes it out again.
- **Materialising.** `forge_build::build` reads each marker's position,
  facing and team from the finished scenario and hands them, with the
  recipe, to `firefight_kit::ground::build_with` over the materialised
  map, with the recipe's classes copied from their donors. The bytes it
  makes are the map, and `Built::ground` reports the zones, squad slots,
  problems, the classes kept and cut, the costs and the receipt
  (`crates/tools/forge-build/src/ground.rs`).
  The build equals the kit's own over the same labels. `ai_ground` and
  `forge_kit` are not built together: the ground is built over the live
  form of the play form (`Session::ground_copy_form`; forge-design.md,
  "Rebuild map"), which has no spare rows for the kit.
- **When it is built.** Never while placing. **Build AI ground** on the
  Forge menu's Waves tab builds now; **Test waves** builds when the running
  map does not hold the current ground (`Session::ground_loaded`) and
  the game reloads, and otherwise when the ground is stale or was never
  built; a save builds first when it was built and is
  stale, and a build that fails stops the save with its reason
  (`Session::{build_ground, ensure_ground, save_building}`).
- **Stale** (`ForgeDoc::ground_stamp`): the stamp is a digest of the
  recipe, the markers and every op that moves one or that adds, moves or
  removes something solid (scenery, crates, machines, vehicles). The
  session compares it with the stamp of its last build
  (`Session::ground_state`: `NotBuilt`, `Current`, `Stale`). An edit
  that touches none of those, a weapon or a title, leaves it current.
- **The builder** is the app's (`forge::GroundBuilder`): the composition
  root installs `boot::ground_builder::AppGround` through
  `forge_glue::ground::install`, which runs `forge_build::build` over the
  play form with the disc's maps and hands the session `Built::ground`'s
  report (zones, squad slots, problems, receipt). The session holds no
  generator. The editor runs the build as a file job
  (`Editor::start_ground`): a worker builds on a copy of the session
  with the installed builder, edits wait, and the editor takes the copy
  back, so the game thread never waits for it. The HUD says
  `AI ground built: N zones, M squad slots, P problems.`, or the build's
  own sentence when it fails (a failed build changes nothing); with no
  builder installed both rows and `forge.ground.build` refuse by name.
- **Loading it.** The builder keeps the map it built
  (`AppGround::after`, `forge_glue::reload::keep_ground_copy`); the
  editor writes it as the play copy and the running map is reloaded onto
  it as **Rebuild map** reloads (forge-design.md, "Rebuild map"): the
  session goes on over it with its document, log, undo, the monitor's
  place and the tool. The notice adds `Loading the map with it...`, then
  says `The map has the AI ground.` when the session has gone on over
  it. A game that cannot reload (no agent socket or editor map mount)
  adds `The running map is not changed.` instead. The running map then
  holds the ground until an edit makes it stale
  (`Session::ground_loaded`, `forge.ground`'s `loaded`). It has no
  spare kit rows, so an add the kit would take waits for **Rebuild
  map**, which builds the document with the kit and without the
  ground.
- **The overlay.** While the monitor is on, the play window draws each
  placed marker as its game type marker (a drop green, a cover yellow, a
  base its team's colour, the siege labels violet) and each label the
  last build could not honour in orange in place of its marker
  (`Editor::marker_entries`, `forge_glue::markers_world`;
  [gametype-markers.md](gametype-markers.md), "The game"). The sectors
  are not drawn: the report holds their count, not their places.
- **What is left.** The sectors' places need
  `forge_build::GroundReport` to carry them. A Rebuild map after a
  ground build drops the ground from the running map; Test waves loads
  it again.
- **The palette page.** The Objects menu's last row, **AI and
  objectives**, lists the six labels with how many are placed; choosing
  one places the next marker in front of the monitor
  (`sheet::objects_with_ai`, `Command::Marker`). A base or a siege line
  takes the team the placed ones leave short, and a siege line the phase
  of its pair.
