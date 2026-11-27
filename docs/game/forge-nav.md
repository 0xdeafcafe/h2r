# Forge nav

Status: Built. **Generate nav**, its problems and their marks, `forge.nav`, the Nav tool with the sectors and links drawn in the game, and the bots taking the generated mesh are built. Not yet checked on a retail map in a running match.
Code: `crates/tools/forge-nav/src/lib.rs` (`generate`), `crates/tools/forge-nav/src/read.rs` (`read`), `crates/extensions/forge/src/session/nav.rs` (`Session::generate_nav`), `crates/extensions/forge/src/tool.rs` (`Tool::update_nav`), `crates/extensions/game/src/forge_glue/nav.rs`, `crates/extensions/game/src/forge_glue/nav_world.rs`, `crates/extensions/game/src/bot_glue.rs` (`MapRead::staged`), `crates/live/bots/src/autonav.rs` (`offer_loaded`, `teleporter_flags`, `link_teleporters`, `Lifts`), `crates/apps/boot/src/nav_builder.rs` (`AppNav`). Tests: `crates/tools/forge-nav/src/tests.rs`, `crates/tools/forge-build/tests/links.rs`, `crates/extensions/forge/src/nav_tests.rs`, `crates/extensions/forge/src/tool_tests.rs`, `crates/extensions/game/src/forge_glue/editing/tests.rs` (`mod nav`), `crates/extensions/game/src/forge_glue/nav_world.rs`, `crates/extensions/agent/src/forge/tests.rs`.
Switch: none.

The bots walk a nav mesh made from the structure BSP's collision
(`bots::cache::build`, [bots.md](bots.md) 3.2). A Forge map adds solid
objects the BSP does not have: a block-out ramp makes a ledge reachable,
a crate wall cuts a room in two. **Generate nav** makes the bots' mesh
with the objects the document adds merged into the level's collision,
links the teleporters, and names what the mesh cannot do. The rule: the
mesh it reports is the bots' own `MapData`, made by the bots' own
generator, so what the overlay shows is what a bot would walk.

## How it works

1. **The row.** **Generate nav** on the Forge menu's Options tab
   (`menu::sheet`, `Choice::GenerateNav`) or `forge.nav
   {generate: true}` starts a file job of the editor
   (`Editor::start_nav`): a copy of the session goes to a worker, and the
   editor takes it back when the worker is done. Edits wait meanwhile, as
   for **Build AI ground**. A shared session refuses it.
2. **The build.** The worker calls the installed generator
   (`forge_glue::nav::install`; the app's is `boot::nav_builder::AppNav`)
   with the document's play form and the loaded map's block counts.
   `AppNav` materialises the play form over the disc's maps
   (`forge_build::build`), as the AI ground's build does.
3. **The inputs.** `forge_nav::read::read` takes from the built map the
   first structure BSP's collision, the starting locations, each
   teleporter sender with each receiver of its channel (netgame flag types
   7 and 8, the identifier), and the solids: each scenery, crate and
   machine placement the document adds (an index at or past the base
   map's count, `nav_builder::added`), its object's `coll` (each region's
   first permutation's BSPs) placed by the placement's frame
   (`Solid::placed`). The base map's own scenery stays out, as the bots'
   generator leaves it out.
4. **The merge.** `forge_nav::generate` compiles each solid
   (`collision_build::build`) and appends it to a copy of the level's BSP
   (`collision_build::merge`), which keeps every index the level has. A
   solid that does not compile or merge is left out and named.
5. **The mesh.** `bots::cache::build` makes the mesh and walls over the
   merged BSP: walkable polygons (the sectors), walks between neighbours,
   and step, drop and jump links. The merge keeps the level's polygons
   whole, so a floor has no side where a solid stands on it; `generate`
   gives each walk that has no way back the reverse walk. Each teleporter
   pair whose two ends stand on the mesh becomes a `Teleporter` link, and
   the scenario's lifts and elevators are linked by the bots' own reader
   (`bots::autonav::Lifts`, read by `read::read`). `mark_out_of_map` marks
   the polygons no player start reaches.
6. **The report.** The session keeps `NavReport` beside the document:
   sectors, links by kind, polygons out of the map, solids merged,
   problems and the overlay's lines. An edit after it makes it `stale`; a
   load starts with none; a failure keeps the last one.

## The overlay

The Nav problems overlay (`Overlay::Nav`, a row of the Overlays page)
marks each problem of the last generation in the problem tone, through
the game type markers' world lines (`markers_world::nav_problems`): a
start off the mesh as a spawn, a teleporter end as that end, a solid left
out as a cover mark, each named `nav:` and its subject. It draws with the
Markers overlay off.

## The tool

The Nav tool (the Tools picker's third row, `Tool::nav`) aims at nothing
and edits nothing in the document. With it selected and the Nav overlay on,
the last generation's sectors and links are drawn in the world
(`forge_glue::nav_world`, a world-line source beside the markers'): a
polygon's sides the starts reach, the sides no start reaches (dashed), and
each link but a walk from where it is taken to where it lands, each kind in
a colour of its own (`nav_world::colour`). Lines within 60 units of the eye
are drawn, the nearest 6000 at most, fading out toward that range. A
changed report is copied once; the same one is not copied again. The HUD
names the tool NAV and says A generates the nav; A runs **Generate nav**
(`Command::GenerateNav`). Link and the object menu return to the Objects
tool. Another tool draws no sectors; the problems' marks stay with the
overlay.

## The bots

The generator keeps the mesh it reports (`forge_glue::nav::stage`, called
by `AppNav`, keyed by the document's base map name). The editor puts it
under the bots at once when that map is the loaded one
(`nav::offer`, `bots::autonav::offer_loaded`), and it stays staged for a map
load: `bot_glue::load_map_data` takes it in place of the mesh built from
the BSP alone, for the pinned play copy of a Rebuild as for the retail
file. The editor drops it (`Editor::retire_nav`) as soon as the document's
log head differs from the one it was generated at, and the session's end
drops it. The mesh is the bots' own `MapData`, so a bot walks the ramp
the overlay shows.

Lines of the generated sectors are not in the document or the log, and a
teleporter link is the one `forge-build` writes
(`bots::autonav::teleporter_flags` reads the same flags;
`crates/tools/forge-build/tests/links.rs`).

## Interfaces

- `forge.nav {generate?}` (act): the state, counts, problems and overlay;
  with `generate: true`, **Generate nav** first, answered when it ends
  ([agent-api.md](../runtime/agent-api.md), "Local Forge session").
  `forge.status` carries `nav` in brief.
- `forge::NavBuilder`: the generator a game installs.

## Rules and limits

- Problems: `start_off_mesh` (no floor within 0.8 across, 0.5 up or 2.0
  down, as the bots' starts snap), `teleporter_off_mesh` (that end is not
  linked), `solid_left_out`.
- A solid's shapes are not posed by their node: a one-node object at rest
  is the identity. Vehicles move, so they are not solids. Only the first
  structure BSP is read.
- A block-out piece ([forge-blockout.md](forge-blockout.md)) is a crate
  with a `coll` and so a solid. A piece the running map has no definition
  for (pending) is one too: the generator builds the play form, which
  holds the tags of every definition the document's pieces use.
- Islands no link reaches and objectives a team cannot path to are not
  reported.
- A mesh offered to a loaded map stays until the map loads again, even
  when an edit has since made it stale; only a later load goes without it.
  The nav worker's own build of the retail map may publish after the
  offer and replace it.
- The generated mesh has no tactical places and no weapon pads seed the
  out-of-map marks, as the BSP-only build has none.

## Not verified

- That a retail object's `coll` compiles back and merges at the sizes
  real maps have, and how long a generation takes on one.
- That bots walk a Forge map's generated ramp in a running match.
- That the bots' generator finds a ramp's step onto a ledge on real
  geometry as it does on the invented room.

## Related

- [forge-design.md](forge-design.md), slice F9.
- [forge-features.md](forge-features.md), "Nav mesh".
- [forge-editing.md](forge-editing.md), "Tools" and "Overlays".
- [bots.md](bots.md): the bots' nav.
- [modeling.md](../editor/modeling.md), slice 11: merging a solid into a
  level's collision.
