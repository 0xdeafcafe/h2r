# tags

The tag layer for Halo 2: reviewed tag definitions, generated fixed-record
codecs, owned domain types for the groups we edit or render, and a tag
repository that holds tags apart from a map. No community XML or game data
is a build input; definitions are text in `src/defs/` and are checked
against the owner's disc.

## Where it sits

Domain `formats`; it depends on `map`. Most of the workspace reads tags
through it: the renderer crates, `assets`, the editors, the converters,
`engine`, `game`, `agent` and `studio`.

## Layout

- `src/defs.rs`, `src/defs/*.txt`, `src/schema/`, `src/registry.rs`: the
  definitions, the schema and the group registry.
- `src/decode.rs`, `src/wire.rs`, `src/edit.rs`, `src/rebuild.rs`: decode,
  wire types, in-place edit and map rebuild.
- `src/repo.rs`: the tag repository.
- One module per domain: `object`, `collision`, `anim`, `lighting`,
  `material`, `effects`, `localization`, `geometry`, `rgba_bitmap`.

## Tests

`cargo nextest run -p tags`. Tests that read maps are `#[ignore]`d and need
`H2_DISC` (your disc image); the repository tests need `H2_REPO_DIR`.
`examples/verify_disc.rs` checks every definition against a disc.

## Docs

- [docs/formats/tag-definitions.md](../../../docs/formats/tag-definitions.md)
- [docs/formats/tag-schema.md](../../../docs/formats/tag-schema.md)
- [docs/formats/tag-repo.md](../../../docs/formats/tag-repo.md)
- [docs/formats/objects.md](../../../docs/formats/objects.md)
