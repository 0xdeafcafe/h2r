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
- A group (`Forge::Group`) names 2 to 256 entries of any block by endpoint, with a name of 1 to 32 characters, a pivot flag and a lock flag; `Forge::Ungroup` ends it and `Forge::GroupLock` sets its lock. An entry is in one group, so the validator refuses a second group, a missing or removed member, a repeated member, and the removal of a grouped entry. `ForgeDoc::groups` resolves the active groups to final indices, `ForgeDoc::group_targets` by identity, and `replay` hands them to `Apply::groups`. A group has no scenario bytes, and a document with a group op is format 4 (`FORMAT_GROUPS`).
- A wave set (`ForgeDoc::waves`, `waves.rs`) is one `firefight::Def`, the format the built-in Firefight sets are written in, with no wrapper. A document with one is format 5 (`FORMAT_WAVES`) whatever its ops; `ForgeDoc::with_waves` sets or takes it out with the format that needs, so a document without one keeps its number and identity. `waves::check` measures it against 64 KiB first, then runs `firefight::validate` against a fixed class list (`waves::CLASSES`: `elite`, then the kit's retail classes), so a document validates the same with no disc. It builds nothing, and a map variant may hold it. `waves::zone_order` numbers the `ff_drop` markers as the kit does (by position, then facing), `waves::remap` keeps a squad on the marker it prefers when the order changes, and `waves::problems` names each squad whose class the ground lacks or whose preferred zone has no drop.
- A block-out piece (`piece.rs`, docs/game/forge-blockout.md) is a crate placement whose palette index is the placeholder key `PIECE_TAG` (`bloc`, `forge\blockout\piece`) with a definition from `Forge::Piece { target, def }`, a `forge_shapes::PieceDef` of a shape, a size, a param and materials. The validator refuses a placement of the tag with no definition, a definition on any other placement, a target that is not a crate, missing or removed, and any definition `PieceDef::validate` refuses (its size, its param, a `Map` material that is not a `shad` key) or whose `Map` key is not a well-formed tag key. A later `Forge::Piece` replaces the definition, a removal clears it, and an add `like` a piece takes its definition. `ForgeDoc::pieces` resolves the active pieces to final crate indices, `ForgeDoc::piece_targets` by identity, and `replay` hands them to `Apply::pieces`, which refuses a document with a piece by default. A piece has no scenario bytes of its own, a document with a piece op is format 6 (`FORMAT_PIECES`; 5 is the Firefight wave sets) and a Forge map, not a map variant.
- A prefab (`Prefab`, `prefab.rs`) is a file of pieces, each a placement block, a tag key, an optional label and a position and rotation from the pivot (the first piece, as if it faced yaw 0). It has its own number (`PREFAB_FORMAT`), a bounded reader (`Prefab::from_bytes`, 128 KiB, 256 pieces) and canonical JSON. `Prefab::missing` names the first tag a map lacks. `ForgeDoc::label_targets` gives the active labels by placement identity.
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
  `Property` sets or clears a property with no retail field (an item
  spawner's `team`, 0 to 7, `teams` reads it; a scenery, crate or
  machine placement's `games`, engine indices in ascending order,
  `object_games` reads it and `for_game` gives the document a build for
  one engine makes); it lowers to nothing.
  Game types are `games`: a set of engines and the item slots that name it.
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
  `resolve` lowers each entry op to the placement op of its target's
  running index, following identities through appends and
  swap-removes; a reset, and every entry op a later reset of the same
  entry covers, lower to nothing. It also lists every base entry an
  entry op, link or label names with a print (`Resolved::prints`).
  `replay` hands the prints to `Apply::prints` before the first edit,
  then the lowered ops to `Apply::placement` in op order; an adapter
  without `prints` refuses a document with a print by name.
  `ForgeDoc::links` and `labels` follow entry removals the same way,
  and leave out a removal a later reset covers. `inverse` retracts an
  entry op as it does any other.
- `ForgeDoc::tier` is `Tier::Variant` when the document holds scenario
  edits only (placement adds, entry ops, links, unlinks and labels, no
  recipe but the session kit's `forge_kit`, no import slot or source
  map), else `Tier::Map`. `play_form` drops the kit recipe and
  `session_form` appends it, each dropping `expect` when it changes the
  recipes; the play form of the session form is the document.
  `variant_name` holds a map variant's name to 1 to 16 characters, not
  blank and without control characters, and `ForgeDoc::check_variant`
  refuses a document of the map tier as `Refusal::Tier`.
- The `format` is the highest any op needs: 6 when any op is a piece
  op, 4 when any op is a group op, 3 when any op is an entry
  op or any endpoint has a print, else 2 when any op is a label op,
  else 1. `validate` refuses any other number, so a document has one
  spelling: every document without the newer ops keeps its bytes and
  identity, and a fold that retracts the last of them returns to the
  lower format. This build reads every number it names; an older build refuses a
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
ops, replay's prints and lowered ops, and a seeded fuzz of entry
documents. `src/tests/piece.rs` covers a piece document's golden canonical JSON
and identity, the format 6 rule, a refusing case for each validator rule, the
definition through a replacement, a removal, a swap-remove and a duplicate, a
fold and the inverses giving the start, `replay`'s hand-over and a seeded fuzz of
piece documents. `tests/resolve.rs` checks `resolve` against hand replays of
add, remove, set and reset orders, entry-op replay against the editor's
session on its invented cache, a wrong print refused before any write,
a reset giving the fixture's bytes back, and a fold then undo of entry
ops giving the start. `tests/variant.rs` covers a table of documents and
their tiers, the forms' round trip and `expect`, and the variant name
rule. All inputs are invented.

## Docs

- [Forge design](../../../docs/game/forge-design.md)
- [Forge features](../../../docs/game/forge-features.md)
- [Forge map variants](../../../docs/game/forge-map-variants.md)
