# workspace

The package build (docs/editor/studio-redesign.md, "Packages"): a closed
registry of builders (`tag`, `edits`, `bitmap`), `Layered` tag sources, and
`build`, which applies an ordered set of packages to a map's `TagSet`.
`h2 package check|build` (crates/apps/h2-tools) is its command line.

The game's side: `GameSet` (the baked packages, `<data>/packages/`,
`set.toml` and the game's features) and `PackageRewrite`, the VFS rewrite
that serves each map with the set applied, cached under
`<data>/packages/cache/`. `boot::packages` makes both and registers
`packages.list`. `PackageRewrite` also runs the set's `kit` items (`kits.rs`:
`campaign-cards` on mainmenu, `glyphs` on the font files) on the inputs the
boot gives it, and `firefight_sets.rs` reads Firefight's wave sets, the
`rules` items of `modes`.
