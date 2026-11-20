# viewer

A render-model viewer: it decodes model geometry from a disc map and draws it with `gfx`. It also builds the `bsp`, `bsp_cracks` and `shadowlab` offline tools.

## Where it sits

`tools` domain. Depends on `map`, `tags`, `xiso`, `content`, `gfx` and `assets`. Nothing depends on it.

## Layout

- `src/main.rs`: the `viewer` command (window, `--png`, `--all`, `--list`).
- `src/mesh.rs`, `src/render.rs`: model decoding and drawing.
- `src/level.rs`: structure BSP geometry from a scenario, shared by the `bsp` and `shadowlab` binaries.
- `src/validate.rs`: checks over decoded models.
- `src/bin/bsp.rs`, `src/bin/bsp_cracks.rs`: structure BSP tools.
- `src/bin/shadowlab/`: the shadow-map lab.

## Tests

`cargo nextest run -p viewer`. Running it needs the disc: `H2_DISC`, or `disc/halo2.iso`.

## Docs

- [graphics/native-renderer.md](../../../docs/graphics/native-renderer.md)
- [graphics/graphics.md](../../../docs/graphics/graphics.md)
- [game/maps.md](../../../docs/game/maps.md)
