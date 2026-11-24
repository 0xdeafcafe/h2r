# forge-nav

**Generate nav** for a Forge map: the bots' nav mesh over the structure
BSP with the map's solid objects merged into its collision, the
teleporter pairs linked, the polygons no player start reaches marked, and
what the mesh cannot do named as problems (a start off the mesh, a
teleporter end off it, a solid left out). `read` takes the inputs from a
built map; `generate` makes the mesh; `Generated::overlay` gives the lines
an overlay draws.

## Where it sits

The `tools` domain. It uses `bots` for the nav mesh and its generator,
`collision-build` to compile and merge each solid, and `scenario-edit`
to read the built map's placements, flags and object collision. The app
runs it for the Forge session's **Generate nav**
(`crates/apps/boot/src/nav_builder.rs`). Design: docs/game/forge-nav.md.
