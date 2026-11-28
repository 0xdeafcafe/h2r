# The content stage: a test map of ours that Studio drives the game on

Status: In progress. Slices S1, S2, S3, S4 and S5 are built: the layout (`stage_kit::layout`), the faces that are both the render mesh and the collision (`stage_kit::mesh`), the textures (`stage_kit::texture`), the map over a base (`stage_kit::kit`, `stage_kit::map`, `h2 tags build --project`), its three lightmaps (`stage_kit::light`) and its sky (`stage_kit::sky`), the game's `render.view` (`agent_spawn_view`), and the game's `audio.meter` (`mixer_meter`). Not yet seen in a game: the sky draws on a self-illuminated shader and the lightmap's orientation and the sun's direction in the sky tag are unverified. The rest is design; "Build slices" is the order to build it in, and "Open questions" lists what the owner still has to choose.
Code: `crates/tools/stage-kit` (the map's own tags), `art/stage/` (its sources and its project), the build in `crates/tools/workspace` (planned), `stage.*` in `crates/apps/studio/src/api/stage.rs` (planned). Tests: `crates/tools/stage-kit/src/tests.rs`, `crates/tools/stage-kit/tests/disc.rs` (`#[ignore]`), `api::tests` in Studio.
Switch: none. The stage is built on first use. It is the first project of the [tag workspace](tag-workspace.md).

A content lane (a weapon model, a sound, a HUD element, a screen) finishes
its work by looking at it and listening to it in the running game, on a map
made for that: a flat floor marked in world units, a sky that shows which
way is up and north, scale references, three fixed lightings, fixed
cameras and numbered pads to put things on. The map is ours: Studio builds
it on the user's machine from sources in this repository, with the
engine's own globals taken from the tags extracted from the user's disc.
Studio owns the flow (`stage.*` on its [agent API](agent-api.md)): it
builds the stage with the content in it, has a paired game load it, takes
the shots, meters the sound, runs the checks and gives one report. The rule that must hold: content reaches the
stage by the same code that puts it in a retail map, so a pass on the
stage is evidence about retail maps, and what the stage cannot show (room
in a retail map's tag memory) is reported beside it and never implied.

It is the second layer of the content workbench. The first layer answers
without a game: `model.render` and `model.check` (Studio's own renderer),
`sound.measure`, `ui.check`, the `check.run` aggregator, and
`pair.preview`, which pushes content into whatever map a paired game has
loaded. The stage replaces none of them ("Where it sits").

## How it works

### The map

`stage-kit` builds one multiplayer map from `art/stage/stage.yaml` (the
layout: bays, pads, references, cameras, lights) and from textures it
draws in code (`stage_kit::texture`: the grid, its digits from a small
font of our own, the sky's gradient and compass letters). No image, mesh
or tag of the stage comes from a disc.

| Part | What it is | Built by |
|---|---|---|
| Floor | a 64 by 64 world-unit slab per bay. A checker of 1-unit squares, a heavier line every 5, and the distance from the bay's origin painted every 5 units along both axes | `stage_kit::mesh` to `h1convert::structure::{pack, Structure::write}` for `sbsp` (`stage_kit::kit::install`); `collision_build::build` and `h1convert::convert::structure_mopp` for collision |
| Sky | a dome of ours, 1000 units out: a horizon line, a vertical gradient, N/E/S/W letters (north is +x, the way the player spawns facing) and a sun disc where the day bay's sun is | `stage_kit::sky`: a Halo 1-form model through `h1_assets::models::mode::convert_model` (parts as strips, as a sky's are), its one material the picture on the self-illuminated stem (`h1convert::shad::ILLUM`, read from the base like the opaque stem), and a `sky ` tag whose one light is the sun. `stage_kit::kit::install` makes the scenario's skies block one element naming it; the base's skies are emptied from the scenario first |
| Scale references | a pillar as tall as the standing player (read from the base map's biped when the stage is built, so no figure is in the repository), a 1-unit cube, a wall ruled every 0.1 unit, a doorway of the player's size | `stage_kit::mesh`, part of the `sbsp` |
| Movement references | ramps at 15, 30 and 45 degrees, a stair of 0.1, 0.2 and 0.3-unit steps, a ledge at each of four heights, a 10-unit drop | the same |
| Lighting | three bays 80 units apart, each with the same furniture: **flat** (even grey light, no direction), **day** (the sky's sun and a sky fill), **dark** (a closed room at near-black, to show self-illumination and a thing's own lights). The lightmap is computed by `stage_kit::light` from the YAML's `lights` (the sun, a sky fill, the flat and dark levels): direct light and a constant fill, no bounce. Flat and dark bays are one shared 2 by 2 cell each; day faces have cells of their own at 4 texels to the unit, shadowed by the bay's own faces, packed into the smallest power-of-two page that holds them, and written as `ltmp` by `Structure::write`. A day face's vertices are lit from the sun's direction where they look towards it | `stage_kit::light` |
| Pads | per bay: the player's spawn at the origin facing +x, a **subject pad** 3 units ahead, and a row of eight numbered pads 2 units apart for weapons, equipment, scenery, crates and vehicles. Studio reads the subject pad (`stage_kit::subject_place`) to stand the tag being edited there ([live-editors.md](live-editors.md), "The subject") | scenario placements (`scenario_edit`'s blocks) |
| Cameras | per bay, named eyes: `ground`, `front`, `side`, `orbit-0` to `orbit-7` (every 45 degrees at 2 units, 6 for a vehicle), `ruler` (side-on to the ruled wall), `third` (behind the spawn). Each is also a player starting location, so `render.spawn_view` reaches it by index | the layout, published in the build report |
| Everything else | `matg` and what it reaches, the sound gestalt, the HUD, the player's biped and weapons, the shader templates | read from the tag tree with the base map's variants and built `fresh`, as a converted Halo 1 map takes them ([tag-workspace.md](tag-workspace.md), "Projects"; [multiplayer.md](../halo1/multiplayer.md), "fresh") |

`stage-kit` is a `ContentKit`: `install` adds the stage's own tags (the
scenario, the `sbsp` and `ltmp`, the sky, the bitmaps and shaders) to
the set and gives the layout back. The map is the `map` item
`content/stage/map.toml` (`mode = "fresh"`, `kits = ["stage", ...]`),
also the tag workspace's project,
built by `workspace::build` like any other: a pure function of the base
map's tags, the sources, `stage_kit::VERSION` and the kits, so two
builds make the same bytes. It writes a `derived` map package
([maps.md](../game/maps.md)) named `stage` under `<data>/maps/` and a
report beside its receipt: the layout (every pad, camera and reference
with its point), each kit's tags, costs and pad, and the build key.
Nothing it writes is in a Git checkout.

The stage is also a package, `content/stage/`: one `map` item whose
`map.toml` repeats the project's mode, base, name, scenario and kits (a
test holds the two equal). The game builds it at boot, with every other
`map` item, through `stage_kit::fresh::StageFresh`
([studio-redesign.md](studio-redesign.md), "Contents"), and leaves
`<data>/maps/stage/` alone while the base map, the layout and
`stage_kit::VERSION` are what built it.

Until the workspace lands, the build is `stage_kit::map::build`, for the
stage kit alone (`h2 tags build --project content/stage/map.toml`, from
the disc's base map or a tag tree). It edits the base's scenario into
the stage's: the base's placements, netgame flags, light fixtures, decorators and
BSP switch volumes cleared,
its palettes' entries nulled, its trigger volumes parked, one structure
BSP naming the stage's `sbsp` and `ltmp`, and the starting locations
replaced by each bay's spawn and then its camera eyes. The base's
scenario, `sbsp` and `ltmp` are hidden from the closure, so the tags
reached from `matg` and the kept groups come from the base and the stage
adds its own. `stage_kit::map::package` writes the package and
`report.json`.

With no tree extracted yet, the build reads the base map straight from
the disc (`tags::repo::MapSource`, the same `TagSource`), so the stage
does not wait for a full extraction.

### Content goes in the way it goes into a retail map

Content is one of three kinds, each with one route:

1. **A kit** (the Pogo Stick, any later weapon, vehicle or scenery kit):
   a `content::kit::ContentKit` ([tag-workspace.md](tag-workspace.md),
   "Kits are build steps"). The boot rewrite that gives a player's
   retail maps the kit and the stage's build call the same `install`.
   `stage.open {content: [{kit: "pogo", source: "/abs/content/pogo"}]}`
   installs from the directory, so an art or YAML change needs no
   rebuild of `h2` and none of the game image. A stage built with a
   working-tree kit holds the kit's root tag, so the game's rewrite
   leaves it alone. With no `content`, the stage's project names no
   content kit and the game's rewrite adds what ships.
2. **Tag edits** made in Studio (a weapon's fields, a `nhdt`, a shader).
   Studio opens the stage's project as its document. A save writes edit
   lines and builds; a paired game gets fields by `edit.batch`, pixels
   and vertices by the raw edits, and structure by loading the new
   build ([live-mirror.md](live-mirror.md), [realtime-edits.md](realtime-edits.md)).
3. **Host-drawn content** (HUD kit records, host sounds such as the
   Pogo's clips, screens in the shared maps). It is not in the map. The
   stage gives it a known scene: the HUD shots, the meter and the
   in-game screen shot below.

A kit's item is placed on the next free numbered pad of each bay and, for
a weapon, is given to the player for the `first` and `third` shots
(`sim`'s give, [agent-api.md](../runtime/agent-api.md)).

### The game is a separate process

Studio does not host the game. It pairs with a running game or starts
one, through `studio::play` ([editor.md](editor.md#play-and-pair)):

- **Why not in process.** The game is a fixed guest address space with
  its own fault handling, GPU device and a module that is rebuilt and
  reloaded; a guest crash would take Studio's unsaved documents with
  it, and `studio::play` already speaks to a separate game for every
  other tool.
- **Which game.** Studio's own game in editor mode
  ([game-view.md](game-view.md), "Own mode"), started by `stage.open`
  when it is not running. The stage is never loaded into a game the
  user started: an agent's `stage.run` on a headless Studio uses that
  Studio's own game and leaves the shared `h2 serve` alone.
- **The launch.** `h2 serve --editor` with a GPU view backend and no
  window, on the fast tier ([cranelift-tier.md](../lifter/cranelift-tier.md)),
  straight into the stage's package. Studio never builds an image.
- **In Studio's window.** The viewport draws the stage document with
  Studio's renderer and the live mirror's objects over it. The Game
  panel shows the own game live. The **Stage** panel shows the last
  shot of each camera and the report.

Time budgets, as targets until `notes/content-stage.md` (new) records
them (**S**):

| Step | Path | Target |
|---|---|---|
| Field edit | `edit.batch` | the next tick |
| Texture or vertex edit | raw edits | the next frame |
| New or resized tags, a kit's sources changed | `project.build`, `maps.load` | 5 s |
| Stage into a running game | `project.build` (cached), `maps.load` | 5 s |
| Cold: the own game not running | start it in editor mode on the stage | 30 s to the first shot |

The stage is small (one BSP, few bitmaps), so a build is a small
closure and a load reads little. A change to a kit's Rust code
needs `h2` rebuilt; no content change needs the game image rebuilt.

### The agent flow

Studio's `stage.*` namespace (`api/stage.rs`, a provider like the HUD
tool's):

| Method | Access | Params | Result |
|---|---|---|---|
| `stage.status` | read | none | the package (`absent`, `building`, `ready`, its key and report), the game link, whether the stage is the loaded map, the last run |
| `stage.open` | act | `{content?: [{kit, source?}], bay?, rules?}` | builds if the key changed, starts the own game if needed, loads, waits for gameplay, places the player in `bay` (`flat` by default). `{map_id, activation, bay, placed: [{item, pad, tag, point}], costs, took_ms}` |
| `stage.shoot` | act | `{shots?, bay?, out?}` | pauses at a fixed tick, takes each named shot. `{shots: [{name, path, size, tick, stats}]}`; PNGs under `<cache>/stage/runs/<run>/`, refused in a Git checkout |
| `stage.listen` | act | `{cue \| tag, at?}` | plays a host cue or a sound tag at 1, 5 and 20 units from the listener and meters each. `{levels: [{at, peak_db, rms_db, lufs}], reference}` |
| `stage.run` | act | `{content?, bays?, golden?}` | `open`, `shoot` in each bay, `listen` for each sound the content declares, the checks. `{ok, checks: [{name, ok, value, bar, shot?}], shots, levels, fits, skipped}` |
| `stage.golden` | act | `{accept \| clear, content}` | keeps or drops the last run's shots as the local references for that content |

A lane's last step is one call:

```sh
h2 studio stage.run '{"content":[{"kit":"pogo","source":"/abs/path/content/pogo"}]}'
```

`stage.run` registers with `check.run` as the source `stage`. Where the
own game cannot start (no disc, no current image, no GPU) it reports
`skipped` with the reason, and `check.run` still gives the first
layer's results.

The shots, and how each is taken. Every shot is taken twice from the
same sealed scene, in `retail` and in `e3` lighting
(`render.capture {lighting}`), and the checks run on both:

- `ground`, `front`, `side`, `orbit-N`, `ruler`, `third`: a private
  world view at the layout's eye (`render.view {eye, yaw, pitch, fov}`,
  on the same `spectator_view` camera as `render.spawn_view`), then
  `render.capture`
  ([agent-api.md](../runtime/agent-api-render-checks.md#framescene-checks-compare-capture-scene-parity-hashes)).
- `first`: the player's own view holding the item, with the HUD.
- `hud-full`, `hud-low`, `hud-empty`: `first` with the shield and ammo
  set by `sim.state.set`, for HUD kit elements and `nhdt` edits.
- `screen`: an in-game screen of ours opened over the stage (needs a
  game method that opens a screen by id; the open question).

The checks need no reference image:

| Check | Fails when | Catches |
|---|---|---|
| `spawned` | the item's object is not in the world on its pad (`world` query), or the player cannot hold a weapon item | a tag the game drops, a missing weapon class |
| `visible` | in `ground` and every `orbit-N`, fewer pixels differ from the empty stage's shot than the bar | an invisible or far-off model |
| `lit` | over those pixels in the flat bay, mean luminance is outside the bar, or most are black | a black shader, a missing base map |
| `first-person` | `first` is mostly black or mostly one colour | the black screen |
| `not-clipped` | too many of those pixels are at full white | a blown-out shader |
| `scale` | the object's bounds differ from the size the kit declares, or exceed the pillar by the kit's class bar | a model in the wrong units |
| `loud` | a level is outside the band around the reference | a clip mastered too hot |
| `fits` | the content's `Costs` exceed the tag memory left in a multiplayer map on the user's disc; one row per map | the stage's own headroom hiding a retail map that is full |
| `agrees` | `model.render` from the same named camera and the game's shot disagree in silhouette or mean colour past a loose bar | Studio's renderer drifting from the game's |
| `golden` | with accepted references, a shot is outside `render.compare`'s bar | a change nobody meant |

The empty stage's shots are taken once per stage key and machine and
kept under `<cache>/stage/empty/`. The loudness reference is the level
of the base map's own weapon and footstep sounds through the same
meter, measured on first use and kept under `<cache>/stage/`; the bar is
a distance in decibels from it, so no level from the disc is in the
repository.

### Determinism and references

- The stage has no AI, no moving scenery and no weather. The lighting
  is baked. `stage.shoot` pauses the game at map tick 90 and captures
  that tick's sealed scene (`render.capture {tick, lighting}`) once per
  lighting, so two runs draw the same two pictures.
- Cameras come from the layout, never from the player's aim. The field
  of view and the capture size (1280 by 720) are fixed.
- References are local. `stage.golden {accept}` copies a run's shots to
  `<data>/stage/goldens/<content>/<stage key>/<bay>-<shot>-<lighting>.png`. They
  are valid for one stage key and one machine and are never copied
  between machines; a new key drops them from comparison and says so. None goes in Git: the stage's own page may
  show one scripted shot under `docs/images/` in LFS, taken by the
  editor's scripted screenshot route, and nothing else.
- `golden` is advisory by default (`stage.run {golden: "strict"}` makes
  it fail the run). The absolute checks carry the verdict, so a lane on
  a machine with no references still gets one.

## Interfaces

- **Studio builds** with `workspace::build` (`project.build`) and
  **calls its own game:** `maps.load`, `session.wait`, `session` pause
  and step, `render.view`, `render.spawn_view`, `render.capture`,
  `render.compare`, `sim.state.set`, the give call, `world` queries,
  `audio.meter`, `rules.reload`, `edit.*`.
- **New game methods**, both wanted by later agents and tests, so the
  API grows rather than the stage working around it:
  `render.view {eye, yaw, pitch, fov?}` or `{reset}` (act), and
  `audio.meter {window_ms}` (read): peak, RMS and integrated loudness
  of the host mixer's output bus over the window, with a `{play: <host
  cue>}` form that starts a named host clip.
- **`maps.load`** rescans the packages when it is given an id it does
  not know, so a package Studio wrote while the game runs loads
  (`map_select::packages_changed`, as `maps.test.create` publishes).
- **`h2 tags build --project content/stage/map.toml`** builds the
  package offline and prints the report, for the coordinator and for
  the disc test.

## Where it sits

| Layer one | Stays for | The stage adds |
|---|---|---|
| `model.render`, `model.check` | a look in seconds with no game, in CI and headless | the game's renderer, lighting, first person, the HUD over it, and `agrees` between the two |
| `sound.measure` | a clip's own level and length | the level after the game's mixer, at distances |
| `ui.check` | layout rules on the YAML | a screen opened over gameplay |
| `pair.preview` | seeing content on the retail map it is for | a known scene, fixed cameras, checks with bars. `stage.open` uses `pair.preview`'s push for field and raw changes once the stage is loaded |
| `check.run` | the one verdict | a `stage` source in it |

## Rules and limits

- No disc byte reaches the repository. `art/stage/` holds our YAML; the
  textures are drawn in code; the project names a base map and tag paths.
  The package, reports, shots and references live under the data and
  cache roots, and outputs into a Git checkout are refused.
- No source built from a leaked build is used for the layout, the
  shaders or the tag layouts.
- The stage needs the user's disc: the globals, the sound gestalt, the
  player and every shader template come from it. With no disc
  `stage.open` answers `UNAVAILABLE` and `check.run` reports the stage
  skipped.
- The base is Lockout (`base` in the project). A shader a kit writes needs its template in
  the base or the shared map, as on any map.
- The stage has far more free tag memory than a retail map
  (`map::writer::max_metadata_size` against a one-BSP map). `fits` is
  therefore part of every report and a `stage.run` with a failing
  `fits` row is not `ok`.
- The game type is the default Slayer. `rules` loads a script over it
  with `rules.reload`; a kit that only acts under a script (the Pogo
  under Superbounce) names that script in its `ContentKit`.
- One package, `stage`, rebuilt in place, so the stage takes one map
  slot whatever the content.
- The stage runs on Studio's own game only
  ([game-view.md](game-view.md), "Rules and limits").
- Frame time on the stage is not a result. Perf numbers come from
  `h2 lab`.

## Risks

- **A map built wholly from our own level tags.** The fresh build is
  the converted Halo 1 maps' path, but those carry a converted BSP with
  clusters, portals and sound environments from a real level. The
  stage's hand-made `sbsp` may miss a field the engine reads
  ([multiplayer.md](../halo1/multiplayer.md), "What the engine reads";
  [map-builder.md](../formats/map-builder.md), the salt risk). S2
  exists to find this out before anything is built on it.
- **Globals that cannot be synthesised.** `matg`, the gestalt, the HUD
  globals, the biped and the shader templates stay the disc's. The
  stage never tries to write them; a user with no disc has no stage.
- **Lighting of objects.** How the game lights an object standing on a
  lightmap of ours (the lightmap's object lighting data) is taken from
  the converter. If the flat bay cannot be made even, `lit`'s bar is
  set from the empty stage's reference objects instead of a constant.
- **Two renderers.** Studio's offscreen renderer draws bind poses and
  first variants and is not the game's. `agrees` is loose on purpose
  and never replaces the game's shot.
- **The fast tier's floats.** Whether clang's module and the fast tier
  draw the same pixels at a paused tick is not known; references record
  the tier and compare only within it.
- **Headless sound.** A run with no audio device renders the mixer
  into a null sink and `audio.meter` meters it; a clock-only run (a
  light host or sim) renders no samples and the meter answers
  `UNAVAILABLE`, so the stage's own game must not be one
  ([audio.md](../audio/audio.md#metering-the-mixer)).
- **Loading into a running game.** Queueing a package map from inside
  a game is unverified ([test-maps.md](../game/test-maps.md#not-verified)).
  The fallback is `game.menu`, then the load.

## Build slices

Each is one lane, lands alone, and is tested on synthetic data unless
it says disc. A lane writes code and tests; the coordinator runs them.
The order among these and the workspace's slices is in
[tag-workspace.md](tag-workspace.md#build-slices); the kit slice (the
Pogo as a `ContentKit`, sources from a directory) is its W4.

| # | Slice | Owns | Tests | Accepted when |
|---|---|---|---|---|
| S1 | Layout and geometry: `Layout` from YAML (bays, pads, cameras, references), the meshes, the collision polygons, the textures | `crates/tools/stage-kit/{Cargo.toml, src/lib.rs, layout.rs, mesh.rs, texture.rs, tests.rs}`, `art/stage/stage.yaml`, `art/stage/README.md` | the layout parses and refuses overlaps; every camera sees its bay's subject pad (a ray test on the mesh); the collision compiles (`collision_build::verify::check`); the grid's digits read back from the texture; the build is the same bytes twice | the tests pass; `Layout` serialises as the report's `layout` |
| S2 | The map, flat-lit: `sbsp`, collision, MOPP, `scnr` with the placements, a constant lightmap, the base's sky, as tags added to a set; a `fresh` build over a `TagSource` (the disc map, or the tree) and the package; `h2 tags build --project` for this one project | `stage-kit/src/{map.rs, kit.rs}`, `stage-kit/tests/disc.rs`, `content/stage/map.toml`, `crates/apps/h2-tools/src/tag_repo_cmd.rs` | synthetic: the set has one BSP, the placements at the layout's points, and reads back through `map` and `scenario_edit`. Disc (`#[ignore]`): built on the user's base, the package resolves, the references resolve, tag memory is under the bound | Studio opens the package and `view.snapshot` shows the grid; the coordinator loads it in the game with `maps.load` and stands on the floor |
| S3 | Lighting and sky: `stage_kit::light`, the three bays' lightmaps, the sky dome and its install (built) | `stage-kit/src/{light.rs, sky.rs}` | the day bay's lightmap is brighter toward the sun's side of the references and the dark bay's is under its bar; the sky model converts and the scenario names it | the three bays' `ground` shots differ as designed in the game |
| S4 | `render.view` (built) | `crates/extensions/game/src/agent_spawn_view.rs`, `crates/extensions/agent-proto/src/render.rs`, [agent-api.md](../runtime/agent-api.md) | params checked; the pose reaches the private view; reset; interrupted by a map change | a capture from a given eye matches the pose |
| S5 | `audio.meter` (built) | the host mixer's tap in `crates/extensions/game` beside `medal_announcer`, `agent-proto`, the audio page, [agent-api.md](../runtime/agent-api.md) | a synthetic sine through the tap gives its peak, RMS and loudness; `play` starts a named cue; the null sink runs with no device | the Pogo's three cues meter on a headless game |
| S6 | `stage.status`, `stage.open`, `stage.shoot` on the own game, `maps.load`'s rescan | `crates/apps/studio/src/api/stage.rs` (planned), `studio/src/play/stage.rs`, `studio-proto`, `crates/extensions/game/src/test_maps.rs`, [agent-api.md](agent-api.md), this page | over the fake game link in `play::testing`: the calls made in order, the own game started when absent, both lightings shot, the refusals (no disc, no image, a Git checkout), every method's example; `maps.load` on an id published after start | one `stage.open` then `stage.shoot` on a headless Studio gives the PNGs of a bay in both lightings |
| S7 | The checks, `stage.listen`, `stage.run`, `stage.golden`, the `fits` table, the `check.run` source | `studio/src/api/stage_checks.rs`, `stage-kit/src/fits.rs`, `studio/src/api/stage.rs` | each check on invented images and levels, passing and failing; `fits` on invented costs and maps; the references keyed by stage key and tier; `check.run` reports the stage skipped with no game | `stage.run` on a Pogo with a blacked atlas fails `lit` and `first-person`, and on a clip raised 12 dB fails `loud` |
| S8 | The Stage panel, the skill and the rule for lanes, the measured budgets | `studio/src/ui/stage.rs`, `.claude/skills/h2-agent-api/SKILL.md`, `AGENTS.md`, `docs/editor/notes/content-stage.md`, [editor-guide.md](editor-guide.md) | the panel's state from a canned report (a headless window test) | the budget table links to the note; a lane's report quotes a `stage.run` |

S1 and S2 are in order and wait for nothing in the workspace: S2 builds
on `tags::repo` as it is. S3, S4 and S5 are independent of each other
once S2 lands. S6 needs S2 and the game view's GV3 (the own game), and
takes S4 when it is there. S7 needs S5, S6 and W4.

## Open questions

1. **Screens.** Our screens live in the main menu and shared maps
   ([ui-in-maps.md](../game/ui-in-maps.md)), so the stage holds none.
   Is a game method that opens a screen by id over gameplay worth
   adding for the `screen` shot, or is `ui.check` with `pair.preview`
   at the menu enough?

## Evidence

- `maps.load` queues a local game on an installed multiplayer package
  by its id (**Code**, `crates/extensions/game/src/test_maps.rs`).
- A map package may be a whole map of ours (`derived`), as converted
  Halo 1 maps are (**Code**, `crates/formats/content`, [maps.md](../game/maps.md)).
- `h1convert::scratch::build` fills an empty tag set from a level's own
  tags and a disc base's globals, and is the default for converted
  multiplayer maps (**Code**, `crates/tools/h1convert/src/scratch.rs`).
- The Pogo's kit is the `pogo` package, applied by `PackageRewrite` at
  the map's first read, from sources baked into the build
  (**Code**, `content/pogo`, `crates/tools/workspace/src/rewrite.rs`,
  `model.rs`; `crates/tools/content-kit/src/inject.rs` is its reference
  and keeps the cost check).
- The Pogo's sounds are host clips outside the game's sound tags and
  are dropped with no audio device (**Code**,
  `crates/extensions/game/src/pogo_sound.rs`).
- `render.spawn_view` gives a private world view from a starting
  location and `render.capture` a PNG of a sealed scene at a tick
  (**Code**, `crates/extensions/game/src/agent_spawn_view.rs`).
- Studio starts and pairs a separate `h2` and never runs the game in
  its process (**Code**, `crates/apps/studio/src/play/launch.rs`).
- Every time in "The game is a separate process" is a target (**S**:
  S8's note).

## Not verified

- That the game loads a map whose `sbsp`, lightmap and sky are all made
  by hand, and what it reads from them that the converter fills from a
  real level.
- That objects in the flat and dark bays are lit as the lightmap says.
- That a paused tick draws the same pixels twice on one machine, and
  across the two tiers.
- That `maps.load` replaces a running game's map, and finds a package
  written after the game started.
- The time of each step in the budget table.
- That the host mixer can be tapped before the device on every host,
  and runs with no device.

## Related

- [tag-workspace.md](tag-workspace.md): the tree, projects, kits and the build the stage is made with
- [game-view.md](game-view.md): Studio's own game, which the stage runs on
- [live-editors.md](live-editors.md): the stage as where an edited tag stands
- [playtest.md](playtest.md): bot matches, with the stage as the first fresh map
- [agent-api.md](agent-api.md): Studio's methods, the first layer's among them
- [editor.md](editor.md#play-and-pair): starting and pairing a game
- [live-mirror.md](live-mirror.md), [realtime-edits.md](realtime-edits.md): edits to a running game
- [test-maps.md](../game/test-maps.md): `maps.load` and the recipe rigs on disc maps
- [maps.md](../game/maps.md): packages and slots
- [multiplayer.md](../halo1/multiplayer.md): the fresh build the stage's map uses
- [pogo-stick.md](../game/pogo-stick.md): the first kit
- [hud-kit.md](../game/hud-kit.md), [ui-in-maps.md](../game/ui-in-maps.md): host-drawn content
- [cranelift-tier.md](../lifter/cranelift-tier.md): the tier the stage's game runs
