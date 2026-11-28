# The content stage

The sources of the content stage, Studio's test map
([docs/editor/content-stage.md](../../docs/editor/content-stage.md)).
`crates/tools/content-kit/src/stage` builds the map from them on the user's machine.

- `layout.yaml`: the layout. It sets the bays, the floor, the spawn, the
  subject pad, the numbered pads, the scale and movement references and
  the cameras, and the `lights` (the day bay's sun and sky fill, the
  flat and dark levels). `content_kit::stage::layout` reads it and refuses
  overlaps and impossible lights.

- `kit.yaml`: the manifest. The stage's version, biped, floor material and
  salt, the sky's shape and colours, the lightmap's packing and the
  textures' colours. `content_kit::stage::data` reads it and refuses an
  unknown key.

The stage's map item is `content/stage/map.toml`: the game builds it at
boot from the disc, and it is the tag workspace's project for the tree
build (`h2 tags project build stage`).

Only our own sources live here. The geometry is made from the YAML and
the textures and the sky's picture are drawn in code
(`content_kit::stage::texture`, `content_kit::stage::sky`) and the lightmap is computed
from the lights (`content_kit::stage::light`). The player's size,
the globals and the shader templates are read from the user's disc when
the map is built. No disc bytes and no built map are committed.
