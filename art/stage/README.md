# The content stage

The sources of the content stage, Studio's test map
([docs/editor/content-stage.md](../../docs/editor/content-stage.md)).
`crates/tools/stage-kit` builds the map from them on the user's machine.

- `stage.yaml`: the layout. It sets the bays, the floor, the spawn, the
  subject pad, the numbered pads, the scale and movement references and
  the cameras, and the `lights` (the day bay's sun and sky fill, the
  flat and dark levels). `stage_kit::layout` reads it and refuses
  overlaps and impossible lights.
- `project.toml`: the tag project that builds the stage as a `fresh` map
  over a base (`h2 tags build --project art/stage/project.toml`).

Only our own sources live here. The geometry is made from the YAML and
the textures and the sky's picture are drawn in code
(`stage_kit::texture`, `stage_kit::sky`) and the lightmap is computed
from the lights (`stage_kit::light`). The player's size,
the globals and the shader templates are read from the user's disc when
the map is built. No disc bytes and no built map are committed.
