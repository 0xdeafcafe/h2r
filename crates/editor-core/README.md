# editor-core

The UI-free parts of `h2-editor` (`crates/studio`), shared by its
tools: map sources (a disc image, a loose `.map`, a folder of maps), the
snapshot undo stack, status messages, and `variant_sources` (where the
game variant and playlist tools get the variant schema from the disc's
`mainmenu` or the built-in model, save names from the XBE, and the save
storage under the data root). See
[docs/editor/editor.md](../../docs/editor/editor.md).
