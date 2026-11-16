# editor-core

The UI-free parts of `h2-editor` (`crates/studio`), shared by its
tools:

- `source`: map sources (a disc image, a loose `.map`, a folder of maps).
- `undo`: the snapshot undo stack.
- `Status`: a status-line message.
- `variant_sources`: where the game variant and playlist tools get the
  variant schema from the disc's `mainmenu` or the built-in model, save
  names from the XBE, and the save storage under the data root.
- `packages`: the map packages under a data root, and which of them are
  converted Halo 1 maps.
- `halo1`, `halo1_art`: Halo 1 maps opened read-only (a disc or a folder),
  their triangles and placements, and their base maps and lightmap pages.

See [docs/editor/editor.md](../../docs/editor/editor.md).
