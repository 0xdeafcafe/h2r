# Forge block-out pieces

`kit.yaml` is the data `crates/tools/content-kit` (`blockout`) turns a
definition into tags with: the one node and region of a piece, the crate
type, the collision material, the dev set's tag folder, the fixed body's
flags, motion and mass, and the palette's words. Every key is required and
an unknown key is refused (`content_kit::blockout::data`).

The shapes (`PieceDef`, the generators) are `crates/formats/forge-shapes`
and the dev tiles and font atlas are `crates/formats/dev-texture`, drawn in
code; the kit reads them. The design is
[forge-blockout.md](../../docs/game/forge-blockout.md).
