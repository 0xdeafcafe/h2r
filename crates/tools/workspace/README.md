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

The tag workspace's side (docs/editor/tag-workspace.md): `edits` (edit
files from `assets/tag-edits/` and `<data>/workspace/edits/` applied to
loose tags), `authored` (the `tag` items a map takes, generated under
`<data>/workspace/generated/`), `project` (projects, each a package's
`map` item, and the stack a build reads: edits over the authored layer
over import layers over the tree) and `project_build` (a project built
into its map package, kits as its steps, `onto` adding new tags). `h2 tags project` and `h2 tags
edits` (crates/apps/h2-tools) and Studio's `project.*` and `edits.*` call
them.
