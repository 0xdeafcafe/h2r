# forge-build

Materialises a Forge document against the user's own Halo 2 multiplayer
map. The builder verifies every named source hash and the pinned builder
versions. It checks the print of every base entry the document names
with one against the disc's row, then applies placement, entry and typed
tag-field edits through the scenario editor's command and save path, then
the document's links, then checks the expected MapId and geometry. Output is deterministic for the same input bytes and
versions.

## Where it sits

The `tools` domain. It uses `forge-doc` for the portable document,
`scenario-edit` for the writer and placement commands, and `content` for
format 2 packages and resolving them. Tag writes use the editor's named
field registry and stay within each tag's ordinary metadata. A runtime
session is outside this crate's scope.

## Commands

```sh
h2 forge check map.h2forge --disc halo2.iso
```

This verifies the result in memory and prints its MapId, one line for
each entry print it compared (`print <block> <index> <print>: matches`)
and any kit reservations.

```sh
h2 forge build map.h2forge --disc halo2.iso
```

This writes `<data>/maps/forge-<first 16 digits of MapId>/` through the
package store. `--maps DIR` selects another maps directory. The disc
defaults to the same XISO as other `h2` tools. The package contains the
canonical document and the materialised map. Local packages are
available by explicit selection and stay off Live.

## Kit

For a session build, add this entry to the document's recipes before
computing its identity:

```json
{"generate":{"generator":"forge_kit","parameters":[]}}
```

The builder reserves up to eight trailing slots per supported placement
or auxiliary block. The active count stays unchanged. Slot counts are
limited by each known engine maximum and by the writer's tag memory and
file size bounds. Blocks are allocated in ascending scenario root offset,
with fewer slots when a block would exceed a bound. A normal play
document omits this recipe. Other generators, transplants, variants and
imports return named errors pending their own slices. The `ai_ground`
generator is built as the next section says.

## AI ground

A document whose recipes include `ai_ground` (`forge_doc::GroundRecipe`)
has the Firefight kit's ground built over the materialised map: each
ground label of `forge_doc::GroundLabel` on a netgame flag gives the
build a marker from the finished scenario's flag (position, facing, team;
the label's number is a `siege_line`'s phase), and
`firefight_kit::ground::build` makes the bytes, which are the map.
`Built::ground` reports its zones, squad slots, problems and receipt.
The document stores only the labels and the recipe, so each machine builds
its own from its own disc. A kit of another version, a team that is not
red or blue, a map with no room, or `ai_ground` beside `forge_kit` stops
the build as `Error::Ground` or `Error::Recipe`. Labels of any other
sort are accepted and kept by nothing: they are document data.

## Entry ops

`forge_doc::replay` hands the materialiser each entry op lowered to the
placement op of its target's running index (`forge_doc::resolve`), so an
entry set is a field write and an entry removal is the editor's
swap-remove, which repoints the scenario's object names. Before the
first edit, every base entry an entry op, link or label names with a
print is checked against the disc's own row (`base_fields`,
`base_print`): its typed fields in the layout's order, a palette index
as the key of the palette entry it selects and a tag reference as its
key. A print that differs stops the build as `Error::Print`
(`variant_print`) with the block and index, and nothing is built.
`Built::prints` lists the prints compared.

## Links

Links are written as retail data after every placement edit, at the
entries' final indices (`forge_doc::ForgeDoc::links`).

- A teleporter link sets the identifier (`+0x14`) of its sender and
  receiver netgame flags to the channel. The sender must already have
  type 7 and the receiver type 8, else the build is refused naming the
  flag.
- Device links join machines into groups. Each group is a new entry
  after the map's own device groups (`scnr+0xa0`, stride `0x28`), named
  `forge link <n>` with initial value 0 and no flags, and each member's
  position group (`+0x36`) names it. These bytes are written after the
  save and before the kit, which then reserves spare groups after them.

An unlinked entry keeps the bytes it would have had without the link.

## Tests

`cargo test --locked -p forge-build` uses invented maps for deterministic
output, kit byte isolation, stale-base and expectation refusals, typed
field writes, resolution through `content::Library`, and links: a linked
teleporter pair, a device group, and unlinking back to the same bytes.
`tests/entry.rs` checks the materialiser's print against the editor's
decoded fields for every fixture entry, that moving an own entry changes
its bytes and the checksum only, that a removal matches the editor's
bytes and name repointing, that a wrong print is `variant_print` with
no build, and that two builds of entry ops are byte-equal.

## Docs

- [Forge design](../../../docs/game/forge-design.md)
- [Map packages](../../../docs/game/maps.md)
