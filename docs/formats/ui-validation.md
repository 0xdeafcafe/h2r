# User-interface tags: validation

What the UI validator checks, split from [ui.md](ui.md).

| Section |
|---|
| [Validation](#validation) |

Part of [ui.md](ui.md).

## Validation

```sh
cargo test -p tags -p cli
cargo run -p tags --example verify_ui -- disc/halo2.iso
target/release/h2 ui --all all --disc disc/halo2.iso
```

Both disc commands run `ui::check::check_map` on every map. It decodes every
`wgit`, `wgtz`, `wigl` and `skin` tag in strict mode and resolves every skin
tree. It also checks what the engine indexes without bounds:

- the map holds exactly one `wigl`, and every `wgtz` names it;
- widget animation indices fall inside its animation collections;
- shader-bitmap animation values are at most their count;
- list skin indices fall inside its skin list;
- every skin a list uses has at least 3 item animations;
- every table cell the engine places has a 16-bit origin.

A map without UI tags fails. The check also counts aliased and foreign
blocks, skipped table cells, out-of-range shape groups (which the engine
ignores), duplicate screen listings and overlapping spans.

On the retail disc: 30 maps, all with UI tags; 1,220 screens, 32 UI globals,
30 shared globals and 904 skins; 36,610 followed blocks, 16,631 of them
nonempty, and 20,254 tag references. There are 87 duplicate screen listings,
71 ignored shape-group indices and no overlapping spans. Skin nesting reaches
depth 2.

The last whole-disc run of the current checks found two kinds of problem, and
both are fixed:

- **Table cells.** The first cell-origin check rejected tables that store
  zero rows or columns. The engine clamps those to 1, and `cell_grid` now
  does the same.
- **Foreign aliases.** The keyframe blocks that alias into a non-UI tag were
  rejected. They are now counted instead.

The whole-disc rerun of the final code (`h2 ui --all all` and `verify_ui`)
reports **0 failures**: no skipped table cells, 215 nonzero shader-bitmap
animation values (all in range), and a largest decode of 13,536 bytes against
a 1,368,656-byte budget. `h2 ui` prints every one of mainmenu's 84 screens
without an error.
