# Test maps: rigs built from recipes and loaded through the agent API

Status: Built. Not run against the game: the load path is tested over a fake guest only ([Not verified](#not-verified)).
Code: `crates/tools/scenario-edit/src/recipe.rs` (recipes, `build`, `build_package`), `crates/tools/scenario-edit/recipes/*.toml` (the built-in recipes), `crates/extensions/game/src/test_maps.rs` (`maps.test.*`, and the `Recipes` trait that builds), `crates/apps/boot/src/test_maps_recipes.rs` (`Recipes` over the scenario editor and the Firefight kit), `crates/extensions/agent/src/sim/calls.rs` (`Call::QueueGame`), `crates/apps/cli/src/agent_maps.rs` (`h2 agent maps`). Tests: `crates/tools/scenario-edit/src/recipe/tests.rs`, `crates/extensions/game/src/test_maps/tests.rs`, `crates/apps/boot/src/test_maps_recipes.rs`, `crates/extensions/agent/src/sim/calls/tests.rs`, `crates/extensions/agent-proto/src/test_maps.rs`, `crates/apps/cli/src/agent_maps.rs`.
Switch: the `agent-calls` feature (`H2_FEATURES=-agent-calls` leaves `maps.test.load` unavailable), listed in [switches.md](../runtime/switches.md). Built with the `test-maps` cargo feature of `boot` (on in `h2`'s desktop build).

An agent asks the running game for a purpose-built test map and loads
it: a spawn grid, a weapon range, a superbounce patch and so on. A test
map is a recipe applied to a disc map. The scenario editor builds placement
rigs and the Firefight kit builds the AI probe. Each result is saved as a map package, given its own
multiplayer slot and queued as the next game. The rule that must hold:
nothing from the disc reaches the repository. Recipes name maps, tag
paths and offsets; the built maps and the build reports stay in the
user's data and cache directories.

## How it works

1. **The recipe.** A recipe (`scenario_edit::recipe::Recipe`, TOML on
   disk, JSON over the wire) names a `base` disc map, an `anchor`, the
   blocks to `clear`, `[[set]]` patches to the base's own entries,
   `[[place]]` entries, the `variant` and its `fidelity`. `Recipe::validate`
   checks everything that needs no map: names, blocks, fields, grid
   sizes, the type filters, at most 256 placements.
2. **Create.** `maps.test.create` checks the recipe, answers at once with
   `state: building`, and builds on a thread of its own
   (`test_maps::build`): the base map from the user's disc
   (`IsoDisc::map_file`, an image region or an extracted file), then
   `Recipes::build`. In `boot::test_maps_recipes`, the default builder
   applies placement edits through `recipe::build_package_with_replace`.
   A recipe with `builder = "firefight-probe"` calls
   `firefight_kit::build_package` in probe mode on the same base bytes.
   Both save package `test-<name>` under `<data>/maps/` and write the
   build report to `<cache>/test-maps/<MapId>.json`.
3. **Publish.** The offers are installed again
   (`map_select::packages_changed`, else `new_maps::install_at_boot`) and
   the VFS rescans, as for a Live download ([maps.md](maps.md#3-realtime)).
   The package gets a multiplayer slot of its own with the alias
   `m-<id>-<MapId prefix>`; `maps.test.list` finds it there
   (`test_maps::listed_in`).
4. **Load.** `maps.test.load` takes the slot's map id and path, builds the
   level boot's multiplayer request for it (a local game, one player, the
   game's default Slayer variant; [level-boot.md](../runtime/level-boot.md#multiplayer)),
   and turns the level boot's steps into guest-call steps
   (`test_maps::game_steps`: `OPTIONS_NEW`, the `fill_options` writes,
   `VARIANT_DEFAULTS`, `OPTIONS_PLAYERS`, `OPTIONS_CHECK`, `OPTIONS_QUEUE`).
   The agent's `agent-calls` feature makes them at the start of the next
   tick over an options block in its scratch (`Call::QueueGame`,
   `SCRATCH_OPTIONS`). The game loads the queued options at its next
   transition, as it loads the main menu's own start. The answer comes
   after that tick, with the options check's verdict.
5. **Assert.** A placement rig's report lists every entry placed (block, index,
   point, facing, tag path, team, whether it reached a floor) and an
   `expect` list for `sim.expect`: the spawn's point when the rig has one
   starting location, then the recipe's own `expect` lines. The Firefight
   probe reports the zone and squad counts, the kit's byte costs and the
   recipe's `expect` lines.

### The build

`recipe::build` reads the anchor from the unedited base (a placement's
position and facing, by default starting location 0, or a point given in
the recipe), applies `[[set]]`, then goes block by block in the editor's
order (`script::BLOCKS`):

- a placement's `at` is forward, left and up from the anchor, turned by
  its yaw (`Frame::world`); `facing` is degrees from the anchor's facing;
- with `floor` (the default) the point drops onto the highest triangle of
  the base's structure BSP under it, up to 2 units above it
  (`floor_z`, `FLOOR_REACH_UP`), plus 0.05;
- `grid` places rows by columns centred on `at`; `each` places one entry
  per matching type, `step` apart; `teams` numbers them in turn;
- a type is a palette entry of the block (`tag`, or `each` over the
  palette), or for `netgame_equipment` any tag of the map's index of the
  groups its `item` takes (`each.group`), matched by path (exact, else
  contains, case-insensitive, sorted by path);
- a cleared block's entries are reused for its placements (the base's
  other fields kept), the rest removed from the end; more are added as
  copies of entry 0 (starting locations, flags, netgame items) or as zeroed
  entries (objects, so a copy carries no scenario object name). Adding past
  the base's size needs `allow_growth`, as in the editor.

## Interfaces

| Method | Access | Params | Result |
|---|---|---|---|
| `maps.test.list` | read | `{name?}` | `{recipes: [{name, title, base, fidelity, purpose, note?}], maps: [{name, base, state, dir, error?, map_id?, listed?: {id, path}, built?}]}`; `state` is `building`, `ready` or `failed`; `built` is the report |
| `maps.test.create` | act | `{recipe: <name> \| <recipe object>, base?, replace?}` | `{name, state: "building", dir}`; poll `maps.test.list` for the end. `base` builds it on another disc map, named `<recipe>-<base>` (refused for a point anchor). An existing map needs `replace: true`; one building is `BUSY` |
| `maps.test.load` | act | `{name}` | after the next tick: `{name, map: {id, path}, check, queued, expect}`. Unknown name: `INVALID_PARAMS`; building: `BUSY`; failed, unlisted or no `agent-calls`: `UNAVAILABLE` |
| `maps.test.remove` | act | `{name}` | `{name, removed}`; deletes the package and its report, and publishes |

Wire types: `agent_proto::test_maps`. The CLI:

```sh
h2 agent maps                                   # recipes and test maps
h2 agent maps create spawn-grid                 # builds, waits until ready
h2 agent maps create firefight-probe            # one squad of two Elites
h2 agent maps create weapon-range base=lockout  # on another disc map
h2 agent maps create ./my-rig.toml replace      # a recipe file read here
h2 agent maps load spawn-grid                   # queues it, waits for gameplay
h2 agent sim expect '/player/0/pos near {"to":[1,2,3],"within":1.5}'
h2 agent maps remove spawn-grid
```

`maps load` reads `session.status` before the call and waits until a map
that started after it is in gameplay past its first second
(`agent_maps::loaded_since`).

### A recipe

```toml
name = "weapon-range"            # a-z 0-9 -, the package is test-<name>
title = "Weapon range"
purpose = "One of every weapon the map has, in a row in front of the spawn."
base = "coagulation"             # a disc map's file name
fidelity = "full"                # or "approximate", with note = "..."
variant = "slayer"               # the only one the load builds
clear = "all"                    # or ["weapons", "scenery"]
allow_growth = false
expect = []                      # more checks for sim.expect

[anchor]                         # default: starting_locations, index 0
from = "starting_locations"
index = 0
# at = [x, y, z]                 # or a world point, with facing = degrees

[[place]]
block = "starting_locations"     # at the anchor

[[place]]
block = "netgame_equipment"
at = [4, 0, 0]                   # forward, left, up from the anchor
step = [0, 2, 0]
each = { group = "weap", contains = ['weapons\'], exclude = ['\multiplayer\'] }
# also: tag = "...", grid = { rows, cols, spacing }, facing, floor,
# team, teams, fields = { scale = 2 }

[[set]]                          # a patch to the base's own entry
block = "netgame_flags"
index = 0
fields = { team = 1 }
```

### The built-in recipes

| Recipe | Base | Fidelity | What it is |
|---|---|---|---|
| `firefight-probe` | coagulation | full | One squad of two Elites for the AI networking probes. The [Firefight kit](firefight.md#the-kit) adds sectors, zones and an Elite character. |
| `empty-room` | lockout | full | every placement removed but one spawn |
| `spawn-grid` | coagulation | full | 16 spawns on a 4 by 4 grid, 3 apart, teams in turn |
| `weapon-range` | coagulation | full | one of every weapon the map's tag index names, in a row as netgame items, objective items left out |
| `vehicle-course` | coagulation | approximate | one of every vehicle, side by side; the course is the base's terrain, with nothing added |
| `hud-check` | coagulation | full | one spawn, two rifles to pick up and a Warthog to enter |
| `superbounce` | ascension | full | one spawn on the traced patch of [superbounce.md](superbounce.md#traces-trace) (a point from our own trace), dropped onto the floor |
| `lighting-box` | lockout | approximate | one spawn indoors with a weapon and a scenery piece as casters; the lights are the base's own |
| `shadow-rig` | coagulation | approximate | open ground under the base's sun with a vehicle and weapons as casters |
| `shader-row` | zanzibar | approximate | every scenery type of the base in a row; object shaders only, not a surface per shader type |

## Rules and limits

- Placement rigs use the editor's blocks ([scenario-editor.md](../editor/scenario-editor.md)).
  A type must be in the base's palette or, for netgame items, in its tag
  index. A rig requiring geometry, lighting or tags beyond these edits is
  `approximate`, with the missing part in its `note`.
- The Firefight probe samples the base's own geometry and spawn points.
  Its builder chooses the squad locations and refuses placement edits or
  anchors in the recipe. To build the same package offline, run
  `h2 firefight kit coagulation --probe`.
- Bases are disc maps. Map Pack and converted maps are not offered.
- The variant is the game's default Slayer; `variant` accepts only
  `slayer`.
- A test map needs a free multiplayer slot ([maps.md](maps.md#map-ids-and-offer-ids));
  with the list full, `maps.test.load` says so.
- A reused object entry keeps its scenario object name, if it had one.
- Recipes and their tests carry names, tag path fragments and offsets.
  The one point in a built-in recipe, the superbounce patch, is our own
  trace's. Built packages and reports are never committed, and nothing is
  downloaded.

## Evidence

- The steps a load makes are the level boot's: `test_maps::tests::the_steps_are_the_level_boots_over_the_agents_options_block`
  compares them call by call with `level_boot::call_of` and the writes
  with `level_boot::fill_options` (**Test**).
- `Call::QueueGame` runs them over the scratch's options block:
  `calls::tests::a_queued_game_builds_its_options_in_the_scratch_then_queues_them` (**Test**).
- A built package served over its base is the rig:
  `recipe::tests::a_built_package_resolves_to_the_rig` (**Test**).
- The methods through the agent's dispatcher, a failed build included:
  `test_maps::tests::the_methods_list_build_and_remove_through_the_dispatcher` (**Test**).

## Not verified

- Queueing options from inside a game tick, during a game, has not been
  run: the main menu's own start queues from the menu, and the level boot
  from `request_mainmenu`. Whether the game tears down a running game
  for options queued this way, as it does for the menu's, is open (**S**:
  `h2 agent maps load` on `h2 serve`).
- A mod slot added while a game runs reaches the game's in-memory list on
  the next main-menu tick ([maps.md](maps.md#3-realtime)). Whether options
  naming a slot the list does not hold yet load is open; loading from the
  main menu avoids the question.
- The level boot's own open questions for a multiplayer start apply: a
  local game with no network session, and the code-only Slayer variant
  ([level-boot.md](../runtime/level-boot.md#not-verified)).
- The built-in recipes have not been built against the disc: whether each
  base's palettes and tag index have what its filters ask for
  (`hud-check`'s rifles and Warthog, `lighting-box`'s rifle), and whether
  the floor drop finds the floor at each anchor.
- `/player/0/pos` as the spawn check's pointer into `sim.view`.

## Related

- [agent-api.md](../runtime/agent-api.md#test-maps): the methods in the API's index
- [maps.md](maps.md): packages, offers and slots
- [scenario-editor.md](../editor/scenario-editor.md): the placement blocks and their checks
- [level-boot.md](../runtime/level-boot.md): the options a load builds
- [superbounce.md](superbounce.md): the traced patch the `superbounce` rig stands on
