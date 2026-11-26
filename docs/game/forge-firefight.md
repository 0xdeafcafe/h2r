# Forge: Firefight maps

Status: Design. Built under it: the ground labels and their palette page, the `ai_ground` recipe, **Build AI ground** with the reload onto the built map, the materialiser's ground build ([forge-map-variants.md](forge-map-variants.md), "AI ground"), the director, the host runtime and the Firefight HUD ([firefight.md](firefight.md)). FA2 is built: the wave set in the document and the session, the zone remap, the 12-drop cap and `forge.waves`; the notices that show a set's first problem come with the screens that show them (FA5). FA1, FA3 and FA4 are built too: the lobby takes a variant with ground, **Test waves** plays the session's set in the session, and Map Waves plays its own set. FA5 to FA8 below are not.
Code: `crates/formats/forge-doc/src/ground.rs` (labels, recipe), `crates/formats/forge-doc/src/waves.rs` (the wave set), `crates/extensions/forge/src/session/waves.rs` (the set in a session), `crates/extensions/agent/src/forge/waves.rs` (`forge.waves`), `crates/extensions/agent/src/forge/test.rs` (`forge.waves.test`), `crates/tools/forge-build/src/ground.rs` (the build), `crates/tools/firefight-kit/src/ground.rs` (`ground::build`), `crates/extensions/forge/src/session/ground.rs`, `crates/extensions/game/src/forge_glue/ground.rs`, `crates/extensions/game/src/firefight/` (the runtime), `crates/formats/firefight` (`Def`, the director). Tests: named per slice below.
Switch: none of its own. `forge` and `firefight`, listed in [switches.md](../runtime/switches.md).

A player builds a Firefight map in a Forge session and plays it from the
lobby. The map is a map variant: the author's drop zones, bases and cover
as labelled markers, the `ai_ground` recipe, and one wave set. Nothing new
runs the match. The ground is the Firefight kit's, the waves are a
`firefight::Def`, and the director, the squad entry, the scoring and the
HUD are Firefight's own. The rule that must hold: **Forge writes only what
the director already reads.** A Forge Firefight map is a kit map with the
author's picks in place of the generator's, and a wave set in the format
the built-in sets use.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).

## The path

What a player does, and what carries each step.

| Step | The player | Carried by | State |
|---|---|---|---|
| 1 | starts a Forge lobby on a multiplayer map | the FORGE lobby mode, the kit start ([forge-map-variants.md](forge-map-variants.md), "The lobby") | built |
| 2 | places drop zones, two bases and cover from the object menu's **AI and objectives** group | `GroundLabel` markers: a netgame flag with a label op | built |
| 3 | chooses **Build AI ground** | `firefight_kit::ground::build` over the play form, then the reload | built |
| 4 | edits the waves, or keeps Holdout's | the wave set in the document (FA2), the Waves screens (FA5) | the set built, `forge.waves` edits it; the screens design |
| 5 | chooses **Test waves** and fights them in the session | the director over the session's map (FA3) | built |
| 6 | saves the map variant | the variant store; the save builds stale ground first and writes the wave set | built |
| 7 | in Custom Games picks Firefight, the map and the map variant, and starts | the lobby's kit check takes a variant with ground (FA1); **Map Waves** plays the map's own set (FA4) | built (FA1, FA4) |

FA1 alone closes the loop with the built-in wave sets: steps 1 to 3, 6
and 7. FA2 to FA5 make the waves the author's. FA6 adds the enemy classes
past the Elite. FA7 is the AI tool, and FA8 the same map with players on
other machines.

Forge slices F7 (volumes, the graph, emitters) and F10's monitor bodies
are not on this path ([forge-design.md](forge-design.md), "Slices"). A
wave is placed by the director's `PlaceSquad` command, not by a graph
node, and the director's state is the rules VM's (`RulesExt`), not
`ForgeExt`.

## How it works

### What the map variant holds

| Part | In the document as | Read by |
|---|---|---|
| Drop zones, bases, cover | `ff_drop`, `ff_base`, `ff_cover` markers (`forge_doc::GroundLabel`) | `forge_build::ground::labels`, then `firefight_kit::ground::build` |
| The ground's settings | the `ai_ground` recipe (`forge_doc::GroundRecipe`): kit version, home team, and the enemy classes (FA6) | the same build |
| The waves | `waves`, one `firefight::Def` (FA2) | the lobby and the runtime, through `firefight::validate` and `ScriptDirector::new` |

The sectors, zones, firing positions, squad slots, hunt orders and
characters are built on each machine from its own disc and are never in
the document ([game-types-without-carriers.md](game-types-without-carriers.md),
"Where the ground is kept").

**Zones.** Zone `NN` is the `NN`th `ff_drop` in the kit's canonical order
(`ground::Labels::canonical`: by position, then facing), and the
definition names zones 0 to 11 (`Drop::PreferZone`). A session holds 12
drop markers at most and refuses the thirteenth with "A map has 12 drop
zones at most." (`Session::check_drops`, on every edit and every shared
proposal). `forge_doc::waves::zone_order` gives the same order from the
session's markers (`Session::drops`: the netgame flags it adds that carry
`ff_drop`), and `the_documents_zone_order_is_the_kits` in `forge-build`
holds the two sorts together. A placed drop the kit cannot honour (off
the walkable area, no room for its firing positions) gets no zone, so the
kit numbers the drops after it one lower than the session does; the
build reports it as a ground problem.

### The wave set

`ForgeDoc::waves` is `Option<firefight::Def>`: the format the built-in
sets are written in ([firefight.md](firefight.md), "The definition"),
with no Forge wrapper.

```rust
// crates/formats/forge-doc/src/waves.rs
pub use firefight::{Def, Drop};
pub const FORMAT_WAVES: u32 = content::forge::DOCUMENT_FORMAT_WAVES;   // 5, the number after FORMAT_GROUPS
pub const MAX_WAVES_BYTES: usize = firefight::MAX_DOC_BYTES;           // 64 KiB
pub const MAX_DROPS: usize = 12;
pub const CLASSES: [&str; 6];               // elite, then firefight_kit::retail::CLASSES in keep order
pub const GROUND_CLASSES: [&str; 1];        // what the stage-one ground builds: elite (FA6 reads the report's)
pub fn limits() -> firefight::Limits;       // CLASSES
pub fn check(def: &Def) -> Result<(), Refusal>;   // the size first, then firefight::validate against limits()
pub fn default_set() -> Def;                // Holdout
pub fn zone_order<T: Clone>(drops: &[(T, [f32; 3], f32)]) -> Vec<T>;   // by position, then facing
pub fn remap<T: PartialEq>(def: &Def, old: &[T], new: &[T]) -> Def;
pub struct Problem { pub round: u8, pub wave: u8, pub squad: u8, pub what: ProblemKind }   // one-based
pub enum ProblemKind { ClassNotBuilt, ZoneNotPlaced }
pub fn problems(def: &Def, classes: &[String], drops: usize) -> Vec<Problem>;
impl ForgeDoc { pub fn with_waves(&self, waves: Option<Def>) -> ForgeDoc; }   // and the format it needs

// crates/extensions/forge/src/session/waves.rs
impl Session {
    pub fn waves(&self) -> Option<&Def>;
    pub fn set_waves(&mut self, def: Option<Def>) -> Result<(), Refusal>;
    pub fn reset_waves(&mut self);              // Reset waves: the set as loaded
    pub fn waves_or_default(&self) -> Def;      // the session's set, else Holdout
    pub fn waves_are_default(&self) -> bool;
    pub fn wave_problems(&self) -> Vec<Problem>;
    pub fn drops(&self) -> Vec<OpId>;           // the ff_drop markers in zone order
}
```

- **In the document.** `waves` is a member of the document beside
  `recipes`, in the canonical form and the identity. A document with it
  has format `FORMAT_WAVES` (`ForgeDoc::required_format`); one without keeps the format it has, so its
  identity does not change. `validate` runs `waves::check`, which
  measures the set against 64 KiB before the validator reads it. The class
  list it admits is fixed (`waves::limits`), so the document validates
  the same on a machine with no disc and on Live. `ForgeDoc::tier` allows
  `waves` in a map variant.
- **In a session.** The session keeps the set beside its document, as it
  keeps the ground recipe (`Session::ground_form`): a save writes it into
  the document and a load takes it out. It is not an op, so it is not in
  the log, the undo stack or the film's `FRGE` records. The Waves page's
  **Reset waves** returns to the set as loaded. In a shared session only
  the host's seat edits it.
- **Each edit** is one `Session::set_waves` with the whole definition. A
  definition `waves::check` refuses changes nothing and the HUD notice
  says the validator's sentence (a wave over the enemy limit at some
  player count, a round without five waves).
- **Problems** are what the set asks for that the built ground lacks: a
  class the ground did not build (`ClassNotBuilt`: never built, or cut
  for headroom) and a preferred zone with no marker (`ZoneNotPlaced`).
  The classes the ground builds are `waves::GROUND_CLASSES` until FA6
  puts the build's own in its report. The Waves page lists them, **Test
  waves** and a save show the first as a notice, and neither is refused: the director takes any unseen zone
  when the preferred one is absent, and the lobby's start names a class
  the map lacks (`Refusal::UnknownClass`).
- **Zones follow their markers.** When an op adds, moves or removes an
  `ff_drop`, the session reads the zone order before and after and calls
  `waves::remap`: a squad that prefers a zone keeps its marker under the
  new number, and one whose marker is gone returns to `Drop::Edge`. Undo
  of a move remaps back; undo of a removal brings the marker back but
  leaves its squads at the edge. A squad that prefers a zone no marker
  had is left as it is. A shared session remaps as each entry of the
  host's stream lands.
- **With no set** the session's waves are Holdout
  (`Session::waves_or_default`), and the saved variant has no `waves`.

### Playing it

A Firefight game needs the kit on the loaded map, which the runtime reads
from the scenario (`firefight::guest::read_kit`): the `ff_slot_NN` squads,
the `ff_drop_NN` zones, the `ff_class_<name>` record. A map variant
materialised with the `ai_ground` recipe has all three, so the runtime
needs no change. The lobby is what stands between.

- **The kit check** (FA1). `lobby_start::KitMaps::for_pick` counts the lobby's map
  as a kit map when it is a kit package (`is_kit_map`) or when the held
  map variant's document has a ground recipe
  (`map_variant::Picked::ground`, from `ForgeDoc::ground_recipe`, read
  with `map_variant::ground_on`) and the wave set takes its base map
  (`lobby_start::supports_ground_wave`). `lobby_mode::refusal` reads it as
  before. The sentences: "Pick a map variant with AI ground, or a
  Firefight map." on a base map with no such pick, and the existing
  line when nothing installed can play.
- **Change Map** with a Firefight variant selected lists the kit packages
  and, after them, each installed base map that has at least one saved
  map variant with a ground recipe (`forge::files::variants::list`, the
  entry's `ground`; `Catalogue::with_ground_maps`). Choosing a base map
  sets the lobby's map; the Map Variant row then picks its variant.
- **Map Variant** in a Firefight lobby
  (`lobby_mode::map_variant_pick_for`) lists the variants with ground;
  Original and a variant without ground are listed with "No AI ground."
  and cannot be chosen. A variant with its own set ends its line "Has
  its own waves."
- **Start Game** builds the pick as any map variant
  (`map_variant::pick_installed`, `forge_build::build`, which builds the
  ground the recipe names), and the build's refusal is the status line: a
  map with no room for the ground says the kit's cost sentence.
- **Map Waves** is a fifth Firefight entry under Change Rules, a record
  whose hash prefix is `firefight::builtin::MAP_WAVES`: the first eight
  bytes of the SHA-256 of `firefight.map-waves`. It is listed after the
  built-in sets (`community_variants::listed`) and names no definition
  (`builtin::variant` and `by_prefix` have none for it). The picker's line
  for a variant whose document holds a set ends "Has its own waves."
  (`lobby_mode::OWN_WAVES`, from `forge::files::variants::Listed::waves`).
  When a game starts (`Runtime::activate`), `map_variant::begin_run` keeps
  the lobby's built pick as `map_variant::running`, until the next game
  starts, and `record_options` resolves the entry to its set
  (`Options::waves`, which `Options::definition`, `player_teams` and
  `wave_set_label` read before the Wave set row's value). Game Setup's
  lives and skulls still apply to it. The lobby's panel names the set
  (`lobby_mode::map_waves_in_lobby`). A kit package holds no set, so
  `lobby_start::supports_wave` is false for the entry; a pick with ground
  takes it (`supports_ground_wave`), and
  `lobby_start::map_waves_refusal` refuses Start Game, with
  `Reason::NoOwnWaves`, "This map has no waves of its own.", when the pick
  carries none (`map_variant::waves_on`). The runtime refuses the same
  sentence if the running pick has none. Every other Firefight entry plays
  its built-in set on the Forge map as on a kit package. The session data
  names the set the match plays (`runtime::session_firefight`: its name
  and `Def::id`), whichever entry chose it.
- **Films and replicas** resolve Map Waves the same way: a film names the
  Forge document in `HEAD` and playback materialises it first
  ([forge-design.md](forge-design.md), "Films and determinism"), so the
  replica's genesis is made from that document's set.

### Test waves

**Test waves** runs the session's set on the session's map, with the
editor still open (FA3).

1. When the running map does not hold the current ground, the row builds
   it (`start_ground` with `if_stale`) and starts the test once the
   reload has opened the session again (`Editor::reopen`). A build that
   leaves the ground unloaded answers "The running map does not hold the
   AI ground, so the waves cannot be tested." (`forge_glue::ground::NOT_LOADED`).
2. The set is `forge_glue::ground::waves_or_default(session)`, the
   session's own set, else Holdout (`Session::waves_or_default`). The editor asks the
   runtime through `firefight::feature::WaveTests`;
   `Runtime::ask_test(def, settings)` queues it and the next `pre_tick`
   starts it (`Runtime::start_test`). `Host::start_test` makes a `Host` in
   the `Host` role over the same `GuestGame` a match uses, with seed 0,
   Normal and no skulls (`runtime::test_settings`). The start is refused
   outside a multiplayer game, off the host's seat, on a map with no AI
   ground, while a match or a test runs, and in a film's playback. A
   lobby Firefight match stays off in a Forge session (`record_options`);
   the test is the one way the director runs there.
3. The director runs as in a match: the AI switch, `PlaceSquad`, `Hunt`,
   `EraseSquad`, scores. Four commands differ in a test, in
   `Host::apply`'s test arm: `Lives`, `TeamLives` and `Respawn` are not
   applied (the session's variant respawns players as it does), and
   `Finish` ends the test, not the game. A test's log is not recorded to
   the film, it reports no score or result, and its stage end shows no
   VICTORY or GAME OVER.
4. The row reads **Stop waves** while a test runs. Stopping
   (`Runtime::stop_test`) erases every slot's squad (`SquadOp::Erase`),
   clears the mode blob and leaves the mode idle. A ground build, a
   Rebuild map, a load, a new map and leaving the session stop the test
   first. The notice says "Waves cleared." after a won test and "Waves
   stopped." otherwise.

A monitor is not a target the director counts on: the parked biped is
invulnerable (`forge_controls::render::park`), so it reads as alive and
the test never ends with everyone dead while a seat is in the monitor.
D-pad Up puts the player in the fight.

`firefight.state` reports `test: true`, and `forge.waves.test {stop?}`
starts and stops one ([agent-api.md](../runtime/agent-api.md#local-forge-session)).

### Enemy classes

The stage-one ground has one class, the Elite the kit authors
([firefight.md](firefight.md), "The kit"). The others come the way the
kit's stage two brings them (FA6): `firefight_kit::Classes`, a transplant
of each class's `char` tag and its closure from the player's own campaign
maps.

- The recipe carries `classes`: names from `firefight_kit::retail::CLASSES`
  (`grunt`, `jackal`, `drone`, `brute`, `hunter`) in keep order, and for
  each the donor map's name and SHA-256 as the author's machine chose it
  (`retail::choose`). The document holds names and hashes, never a tag.
- `forge_build` hands the build a `Classes` with `cut_to_fit` on, the
  donors read from the disc and checked against the recipe's hashes. A
  machine whose disc lacks a donor, or has another, is refused by name
  before the map loads. `ForgeDoc::tier` allows these donors in a map
  variant and no other source map.
- `GroundReport` gains `classes` (kept, in palette order, the Elite
  first), `cut` and each class's cost, from `Package::classes` and
  `Package::cut`. The same inputs cut alike on every machine, so tag
  indices agree.
- The stale stamp covers the class list.

### The budget

A set's live actors are bounded by its own `Caps`, which admission holds
inside the host table ([ai-networking.md](ai-networking.md#cpu-on-the-host)).
The Waves page's **Enemy limit** row offers the values at or under that
table's for one to four players. The ground's cost against the map's tag
headroom is the kit's report (`Costs`), shown on the **Map limits** page
after a build.

## Interfaces

**Agent API** (game), added in the slices named; each is listed in
[agent-api.md](../runtime/agent-api.md) when built.

| Method | Access | Does | Slice |
|---|---|---|---|
| `forge.waves` | read | the session's set and its `Def::id`, whether it is the default, the problems, the drops in zone order | FA2 |
| `forge.waves {set}` | act | replace the set with a definition, or clear it with `null`; the validator's refusal by name | FA2 |
| `forge.waves.test {stop?}` | act | start a test, building the ground first when it is not loaded, or stop one | FA3 |
| `forge.ground.build {classes?}` | act | the built method, with the class list for the recipe | FA6 |
| `forge.ground` | read | the built method; the report gains `classes`, `cut` and `costs` | FA6 |
| `firefight.state` | read | the built method; gains `test` | FA3 |
| `forge.status` | read | the built method; `variant` gains `waves`, the set's name or null | FA2 |

The screens are game screens, so `ui.screen` and `ui.press` read and
drive them, and `hud.kit` reads the HUDs
([forge-firefight-ui.md](forge-firefight-ui.md), "The AI tool").

**Engine entries.** None new. A test and a match place, hunt and erase
through `actor_net::squads::SquadEntry` and the entries of
[firefight.md](firefight.md), "Guest boundary".

## Rules and limits

- A document holds the wave set and the class names. It holds no sector,
  zone, squad or character.
- One wave set a map variant, at most 64 KiB, in `firefight::Def`'s
  format and under its validator.
- Twelve `ff_drop` markers at most. A base marker for each team or none.
- A set may name only the classes of `waves::limits`. A class the built
  ground lacks is a problem in the session and a named refusal at the
  lobby's start, never a fall back to the Elite.
- A test runs on the host's seat of a local or shared session, never in
  a match and never in Theater. A snapshot is refused while it runs, as
  in a match.
- A film of a session does not replay a wave test: the test's entries are
  not recorded, so playback shows the session without its actors.
- A map with no room for the ground is refused at the build with the
  kit's cost and biggest part. Classes are cut from the end of the list
  before the build is refused.
- No Forge Firefight map in a hopper until its id is pinned
  ([maps.md](maps.md) section 5), and Map Waves is not a hopper entry:
  Live's hopper checks refuse a prefix that names no built-in set.

## Slices

Sizes as [forge-design.md](forge-design.md), "Features". Lanes write code
and tests on invented data and run nothing; the live check is the
coordinator's, through `h2 agent` on a running game. FA1, FA2, FA3 and
FA6 start at once and run side by side. FA4 and FA5 follow FA2 (FA4 also
FA1). FA7 follows FA5, FA8 follows FA4.

| # | Slice | Size | Needs | Files | Tests (invented data) | Live check |
|---|---|---|---|---|---|---|
| **FA1** | **Firefight on a map variant with ground.** `KitMaps` and `supports_wave` take a held pick whose document has a ground recipe; Change Map's Firefight list adds the base maps that have such a variant; the Map Variant picker in a Firefight lobby marks variants without ground; the refusal sentence; `map_variant::Picked` gains `ground` | M | nothing | `crates/extensions/game/src/{lobby_start,lobby_mode,map_select,map_variant}.rs`, `crates/extensions/forge/src/files.rs`, `crates/extensions/game/src/forge_screens/variants.rs`, [firefight.md](firefight.md) "The lobby" | `a_variant_with_ground_is_a_kit_map_for_the_lobby`, `a_base_map_without_a_ground_pick_is_refused_by_name`, `change_map_lists_kit_packages_then_maps_with_ground_variants`, `the_picker_refuses_original_and_groundless_variants_in_firefight`, `a_map_specific_set_still_needs_its_own_base`, `clearing_the_pick_refuses_the_start_again`, `the_variant_list_reports_each_entrys_ground`; `Page` tables for Game Setup and the picker | on a session with ground built and saved: `ui.lobby.tabs`, then `ui.press` through Change Rules, Change Map and Map Variant; `ui.screen` shows the variant chosen and no status line; after Start `firefight.state` is running as `Host`, `firefight.kit`'s zones equal the saved variant's drops, and `world.ai` lists actors within the first wave's gap |
| **FA2** | **The wave set in the document and the session.** `ForgeDoc::waves`, `FORMAT_WAVES`, `waves::{limits, check, remap, problems, zone_order}`, the tier; `Session::{waves, set_waves, waves_or_default, wave_problems}`, the save and load forms, the 12-drop cap, the remap on a drop's add, move and removal; `forge.waves`; `waves` in `forge.status`' `variant` | M | nothing | `crates/formats/forge-doc/src/{waves.rs (new),document,validate,variant,wire,ground,lib}.rs`, `crates/formats/forge-doc/Cargo.toml` (`firefight`), `crates/formats/content/src/forge.rs` (the format number), `crates/extensions/forge/src/session/waves.rs` (new), `crates/extensions/forge/src/session.rs`, `crates/extensions/agent/src/forge/waves.rs` (new), `crates/extensions/agent-proto/`, `crates/tools/forge-build/tests/` (the order parity), [agent-api.md](../runtime/agent-api.md) | `a_document_with_waves_round_trips_at_the_waves_format`, `a_document_without_waves_keeps_its_identity`, `a_set_naming_an_unknown_class_is_refused`, `an_oversize_set_is_refused_before_parsing`, `a_variant_with_waves_is_a_map_variant`, `the_documents_zone_order_is_the_kits`, `a_moved_drop_keeps_its_squads`, `a_removed_drop_sends_its_squads_to_the_edge`, `undo_of_a_drop_move_remaps_back`, `a_thirteenth_drop_is_refused`, `a_refused_set_changes_nothing`, `a_save_writes_the_set_and_a_load_takes_it_out`, `problems_name_a_class_the_ground_lacks`, `waves_answers_the_set_its_id_and_the_drop_order`; the seeded document fuzz covers `waves` | `forge.waves {set}` with Attrition's JSON, `forge.waves` answers its id; save through `forge.input`, load, and `forge.waves` answers the same id (the saved file holds `waves`; `forge.doc` is the session's document, which does not); move a drop with `forge.propose` and `forge.waves` shows the squad's zone renumbered |
| **FA3** | **Test waves runs the director.** `Runtime::{start_test, stop_test}`, `Host`'s test arm (`Lives`, `Respawn`, `Finish`), the stop on ground build, Rebuild map, load and leave; the **Stop waves** row; the HUD while testing ([forge-firefight-ui.md](forge-firefight-ui.md), "The Firefight HUD"): `forge.enemies.*` in the monitor, the Firefight HUD's pills by scope; `forge.waves.test`; `firefight.state`'s `test`. Plays `Session::waves_or_default` | M | nothing; FA2 for the session's own set | `crates/extensions/game/src/firefight/{runtime,host,hud,feature}.rs`, `crates/extensions/game/src/forge_glue/{ground,editing}.rs`, `crates/extensions/game/src/forge_screens/sheet.rs`, `crates/extensions/game/src/forge_hud.rs`, `crates/rewrites/hud-kit-core/src/views/{forge,firefight}.rs`, `content/forge/huds/forge.hud.yaml`, `crates/extensions/agent/src/forge/test.rs` (new), `crates/extensions/agent-proto/src/firefight.rs` | `a_test_starts_only_on_loaded_ground`, `a_test_makes_a_matchs_squad_calls_in_order`, `finish_ends_the_test_and_not_the_game`, `lives_and_respawn_are_not_applied_in_a_test`, `stopping_erases_every_slot`, `a_ground_build_stops_the_test_first`, `a_lobby_match_stays_off_in_forge_mode`, `the_row_reads_stop_waves_while_a_test_runs`, `a_monitor_sees_the_banner_and_the_enemies_pill`, `a_test_shows_no_lives_pill`, `the_enemies_pill_takes_the_grenade_rows_third_place`; the kit's checks on `forge` and `firefight` in every state and mode | `forge.waves.test`; `firefight.state` has `test` and a phase past warmup; `world.ai` never over the cap; `hud.kit` lists `forge.enemies.count` drawn for the monitor's user and no `ff.lives.*`; `forge.waves.test {stop: true}`, then `world.ai` is empty; `hud.render` of the `forge` HUD's testing state for the report |
| **FA4** | **Map Waves.** `builtin::MAP_WAVES` and its listed record; `record_options` and `Options::definition` resolve it from `map_variant::running`; `supports_wave` for it; the start refusal; the picker's "Has its own waves." line; the variant panel names the set | S | FA1, FA2 | `crates/formats/firefight/src/builtin.rs`, `crates/extensions/game/src/firefight/{runtime,options}.rs`, `crates/extensions/game/src/{community_variants,lobby_start,lobby_mode,map_variant}.rs`, [firefight.md](firefight.md) "Map-specific sets" | `map_waves_is_listed_after_the_built_ins`, `map_waves_resolves_to_the_running_variants_set`, `map_waves_without_a_set_is_refused_by_name`, `a_built_in_set_plays_on_a_variant_with_its_own_waves`, `the_session_names_the_set_the_match_plays`, `a_hopper_refuses_map_waves` | in the lobby pick Map Waves and the variant; after Start `firefight.state`'s set name and id equal `forge.waves`' from the session that saved it |
| **FA5** | **The Waves screens.** WAVES, ROUND, WAVE and SQUAD pages and their pickers ([forge-firefight-ui.md](forge-firefight-ui.md), "The Waves screens"); **Waves...** on Forge options; **Enemy types...** (the recipe's classes, after FA6); each press one `Session::set_waves` | M | FA2 | `crates/extensions/game/src/forge_screens/{waves.rs (new),waves_tests.rs (new),sheet,pages}.rs`, `crates/extensions/game/src/forge_glue/editing.rs` | `Page` tables for the four pages; `each_row_is_one_set_waves`, `a_wave_over_the_enemy_limit_is_refused_with_its_sentence`, `the_class_picker_offers_the_built_classes_only`, `the_drop_picker_lists_any_drop_then_each_placed_drop`, `start_from_asks_before_replacing`, `reset_returns_to_the_loaded_set`, `the_wave_line_counts_enemies_at_one_and_four_players`; content checks | `ui.press` to Forge options, Waves, a round, a wave, a squad; change Per player; `ui.screen` shows the value and `forge.waves` the definition; Studio's `ui.render` of the four pages and `check.run` for the report |
| **FA6** | **Enemy classes in the ground recipe.** `GroundRecipe::classes` with donors, the tier's donor rule, `firefight_kit::ground::build` with `Classes`, `forge_build`'s donor check, `GroundReport::{classes, cut, costs}`, the stamp, `forge.ground.build {classes?}` | M | nothing | `crates/formats/forge-doc/src/{ground,variant}.rs`, `crates/tools/firefight-kit/src/{ground,lib}.rs`, `crates/tools/forge-build/src/ground.rs`, `crates/apps/boot/src/ground_builder.rs`, `crates/extensions/forge/src/session/ground.rs`, `crates/extensions/agent/src/forge/ground.rs` | `a_recipe_with_classes_round_trips`, `ground_with_classes_equals_the_kits_stage_two_build`, `a_donor_of_another_hash_is_refused_by_name`, `classes_cut_from_the_end_and_the_report_names_them`, `two_builds_of_one_recipe_are_byte_equal`, `a_class_change_marks_the_ground_stale`, `a_variant_may_name_class_donors_and_no_other_source` | `forge.ground.build {classes: ["grunt", "jackal"]}`; `forge.ground` lists the classes kept and cut with costs; `firefight.kit` lists them; `firefight.command` places a grunt squad and `world.ai` shows its character |
| **FA7** | **The AI tool.** The tool in the Tools picker; aim at markers only; the readout's zone number and squad count; the DROP page with **Add squad here...**; the built ground's sectors and problems in the overlay (`GroundReport` carries the sectors) | M | FA5 | `crates/extensions/forge/src/tool.rs`, `crates/extensions/game/src/forge_screens/{sheet,waves}.rs`, `crates/extensions/game/src/forge_glue/{markers_world,nav_world}.rs`, `crates/tools/forge-build/src/ground.rs`, `content/forge/huds/forge-readout.hud.yaml` | `the_ai_tool_aims_at_markers_only`, `the_readout_names_the_zone_and_its_squads`, `add_squad_here_presets_the_drop`, `the_overlay_draws_the_built_sectors_and_each_problem`; `Page` table for DROP | `forge.input` picks the tool; `forge.status` gives the aimed drop's zone; `ui.press` adds a squad from the DROP page and `forge.waves` shows `prefer_zone` |
| **FA8** | **Other machines.** A member builds the variant's ground through `MapGate`; both Live homes take a session's `firefight` whose id is no built-in's; a replica and a film make the genesis from the running document's set | S | FA4 | `crates/live/live-hle/src/maps.rs`, `crates/live/live-server/`, `crates/live/live-worker/`, `crates/extensions/game/src/firefight/{runtime,film}.rs` | `a_member_builds_the_ground_the_variant_names`, `a_session_takes_a_custom_sets_id_in_both_homes`, `a_film_of_map_waves_replays_from_its_document`; the parity suite | `h2 lab` with a second machine: `firefight.state` is `Client` there, `actors.net` shows puppets, both HUDs show the same wave |

## Evidence

- **Code** The labels, the recipe, the build and the reload: the files in
  the status line. `record_options` turns a lobby match off in Forge mode
  (`crates/extensions/game/src/firefight/runtime.rs`). The lobby's kit
  check is by package name (`lobby_start::is_kit_map`) or by the held
  pick's ground (`map_variant::ground_on`).
- **Code** `read_kit` finds the kit by squad and zone names, not by
  package (`crates/extensions/game/src/firefight/guest.rs`).
- **Test** A document with markers and a recipe materialises to the
  kit's build of the same labels (GM3,
  [game-types-without-carriers.md](game-types-without-carriers.md), "Slices").
- **Code** The kit makes a zone of every placed drop it can honour, with
  no cap of its own, and none of one it cannot
  (`crates/tools/firefight-kit/src/nav.rs`, the placed-drop path); the
  generated drops stop at twelve (`MAX_ZONES`). The session's cap keeps a
  map at twelve.
- **Test** The wave set in the document, its format and identity, the
  zone remap and the drop cap (FA2's tests in
  `crates/formats/forge-doc/src/tests/waves.rs`,
  `crates/extensions/forge/src/waves_tests.rs`,
  `crates/tools/forge-build/tests/ground.rs` and
  `crates/extensions/agent/src/forge/tests.rs`).
- **Test** Map Waves: its listed record, its resolution from the running
  pick, its refusals and the session's set
  (`crates/extensions/game/src/firefight/runtime/tests.rs`,
  `crates/extensions/game/src/community_variants/tests.rs`,
  `crates/extensions/game/src/map_variant/tests.rs`,
  `crates/extensions/game/src/lobby_mode/tests.rs`,
  `crates/live/live-server/src/admin.rs`).
- **Design** The test arm, the classes in the recipe.

## Not verified

- Everything [firefight.md](firefight.md) lists: that the engine takes
  generated sectors and added characters on a multiplayer map, and runs
  R1 to R4. A Forge Firefight map depends on each.
- That a map reloaded onto built ground in a session places actors the
  first time the AI switch is set, with no second load.
- What actors do about a parked biped: whether they fire on it, gather
  at it, or ignore it.
- That the session's variant respawns a player killed by an actor as it
  respawns any other.
- The time a ground build with five classes takes in a session, and its
  cost on each map against the headroom of
  [notes/2026-11-23-firefight-kit-headroom.md](notes/2026-11-23-firefight-kit-headroom.md).
- The Waves pages and the monitor's enemies pill at wide aspects, through
  `ui.render` and `hud.render`.
- That Change Map's list takes base maps beside kit packages under one
  Firefight variant without a second category.

## Related

- [firefight.md](firefight.md): the definition, the director, the kit, the runtime, the HUD.
- [forge-design.md](forge-design.md): the document, materialising, slices F7, F10 and F11.
- [forge-map-variants.md](forge-map-variants.md): "AI ground", the map variant file, the lobby's pick.
- [forge-firefight-ui.md](forge-firefight-ui.md): the AI tool, the Waves screens, the HUD while editing.
- [game-types-without-carriers.md](game-types-without-carriers.md): "Maps for scripted games", the build step.
- [ai-networking.md](ai-networking.md): host actors, the actor cap, the squad entry.
- [forge-nav.md](forge-nav.md): the bots' mesh over the same placed objects.
