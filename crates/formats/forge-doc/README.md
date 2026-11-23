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
  `ForgeDoc::canonical_json` and hash with `ForgeDoc::id`. Floats
  are stored as eight lowercase hexadecimal digits of their
  IEEE 754 bits. It uses externally tagged snake-case ops and values.
  A draft can omit `expect`. The identity hashes the canonical form with
  the entire `expect` block omitted.
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
  its canonical digest in `was`. Mesh and Bitmap variants are reserved
  for their later slices.
- Forge-family edits hold links written as retail data. A teleporter link
  names a sender and a receiver netgame flag and a channel; a device link
  names two machines. An endpoint is a base entry by its base index or the
  entry an Add op made, so it survives later appends and swap-removes.
  `ForgeDoc::links` resolves the active links to final indices from the
  base counts, and `replay` hands them to `Apply::links` after every other
  edit. An unlink names the link op it ends; the entries then keep the
  bytes they had without it. The validator refuses a link to a missing or
  removed entry, the removal of a linked entry, a flag in two roles or on
  two channels, a repeated link and a device link whose lower endpoint is
  not first.
- A label is a string of 1 to 32 bytes from lowercase ASCII, digits, `_`,
  `-` and `.`, plus an optional `i32` number. `Forge::Label` sets or
  clears one on a placement named by an endpoint, as links do, so it
  follows the entry through appends and swap-removes. A placement has at
  most one label and a later set replaces it; labels need not be unique,
  and a number tells placements with one text apart. The validator
  refuses bad text, a missing or removed entry, a clear of an unlabelled
  entry and the removal of a labelled one. A label has no scenario bytes.
  `ForgeDoc::labels` resolves the active labels to final indices, ordered
  by block then index, and `replay` hands them to `Apply::labels` after
  the links; a document with no label op never calls it.
  `builtin_label(flag_type)` is the label a retail map's netgame flag
  gets with no document: `ctf_flag_spawn` (0), `ctf_flag_return` (1),
  `assault_bomb_spawn` (2), `assault_bomb_return` (3), `oddball_spawn`
  (4), `teleporter_sender` (7), `teleporter_receiver` (8), `territory`
  (10) and `hill_corner` with the hill number (11 to 18).
- Entry ops (`Edit::Entry`, `EntryOp`) edit the map's own entries as
  well as added ones: `Set` writes one typed, editable field, `Remove`
  removes the entry and `Reset` drops the effect of every earlier entry
  op on a base entry, so it is in the map again as the disc has it.
  Links and labels are their own ops and a reset leaves them. A target
  is an endpoint, as for links and labels; a base target carries its
  **print**, the first eight bytes (sixteen hex digits) of the SHA-256
  of the canonical JSON `{"block": <name>, "fields": [...]}` of the
  entry's typed fields as `Value`s in the layout's order, a tag
  reference or palette index as its tag key. `print(block, fields)`
  computes it, and an adapter calls `check_print` before its first
  write to a base entry: a differing print is `Refusal::Print {block,
  index}` and nothing is written. A link or label endpoint may carry a
  print and need not. The validator refuses a base target without a
  print, an index past the block's limit or the base count, an add ID
  of 0 or of no add op, a field the block lacks or does not mark
  editable, a value of another type or out of range or outside the
  BSP bounds, a target an earlier op removed, the removal of a linked
  or labelled entry, a reset of an added entry or of an entry with no
  entry op since its last reset, and one entry named with two prints.
  `replay` refuses entry ops as unsupported until they lower to
  placement edits by running index (docs/game/forge-map-variants.md,
  slice M2); `ForgeDoc::links` and `labels` already follow them, and
  leave out a removal a later reset covers.
- The `format` is the highest any op needs: 3 when any op is an entry
  op or any endpoint has a print, else 2 when any op is a label op,
  else 1. `validate` refuses any other number, so a document has one
  spelling: every document without the newer ops keeps its bytes and
  identity, and a fold that retracts the last of them returns to the
  lower format. This build reads all three; an older build refuses a
  newer document whole.
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

`cargo xtask test -p forge-doc` covers document and entry hash
vectors, chain integrity and inverses, validator refusals, a seeded
10,000-document fuzz test, link identity and refusals, label rules, format
1 and 2 vectors and the built-in table, and placement
parity against `scenario_edit::Session`. `src/tests/entry.rs` covers
print and format 3 golden vectors, each entry op rule's refusal, final
indices after an entry removal and a reset, a fold and undo of entry
ops, and a seeded fuzz of entry documents. All inputs are invented.

## Docs

- [Forge design](../../../docs/game/forge-design.md)
- [Forge features](../../../docs/game/forge-features.md)
- [Forge map variants](../../../docs/game/forge-map-variants.md)
