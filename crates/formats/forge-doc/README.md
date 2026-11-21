# forge-doc

A Forge map is a versioned document of ordered edits and recipes over a
base map named by SHA-256. The host gives accepted edits a sequence,
apply tick and predecessor hash. Recipes and edits use typed author
input, tag keys, references to existing placements and import hashes.

## Where it sits

The `formats` domain. It uses `content` for canonical JSON and SHA-256,
and `map::scenario_layout` for the placement block names, types and
limits that `scenario-edit` uses. The editor is a test dependency for
replay parity on its invented cache.

## Document and log

- Read untrusted documents through `ForgeDoc::from_bytes`. Write with
  `ForgeDoc::canonical_json` and hash with `ForgeDoc::id`. Format 1
  stores floats as eight lowercase hexadecimal digits of their
  IEEE 754 bits. It uses externally tagged snake-case ops and values.
  A draft can omit `expect`.
- `validate` checks the document and its ops without a map. Pass exact
  base counts, a placement-add limit, BSP bounds and builder versions
  through `Limits` when they are available. The map-backed `Apply`
  adapter resolves tags and checks writable tag fields, map permissions
  and resource budgets. `replay` validates the document before it calls
  the adapter.
- Placement edits use the editor's set, append and swap-remove actions.
  Add starts with the editor's defaults or a local copy of the indexed
  entry, then writes the authored fields. Palette changes and tag
  references carry a group and path. Tag field writes carry a dotted
  field path with optional array indices and can pin the old value by
  its canonical digest in `was`. Mesh, Bitmap and Forge-family variants
  are reserved for their later slices.
- The sequence-zero log head hashes the starting document. An entry
  hashes its canonical JSON including the preceding hash. The host
  checks the proposal's author, conditions and map state and calls
  `LogHead::stamp`. `Entry::from_bytes` and `Proposal::from_bytes` bound
  messages at ingress. Pass a trusted head to `validate_chain` to
  detect changes to the final entry and truncation.
- `fold` derives the active ops from a whole log. `fold_from` takes a
  trusted document and head as a checkpoint for a suffix. The F1
  inverse retracts the latest active op by ID so index-based commands
  keep their ordering. Redo restores the same op. The fold can check
  authors and prior uses of IDs within the supplied history. The host
  checks author ownership from the full session before authorising
  retractions. A fold that changes the active edits clears `expect`.
- Format caps are 8 MiB per document, 16,384 active ops, 64 sources,
  1,024 recipes, 256 import slots and 32 fields per list. A log entry
  or proposal is at most 64 KiB. Each validated log batch is at most
  65,536 entries and 32 MiB of canonical entry bytes.

## Tests

`cargo test --locked -p forge-doc` covers document and entry hash
vectors, chain integrity and inverses, validator refusals, a seeded
10,000-document fuzz test and placement parity against
`scenario_edit::Session`. All inputs are invented.

## Docs

- [Forge design](../../../docs/game/forge-design.md)
- [Forge features](../../../docs/game/forge-features.md)
