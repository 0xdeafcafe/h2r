# forge-build

Materialises a Forge document against the user's own Halo 2 multiplayer
map. The builder verifies every named source hash and the pinned builder
versions. It applies placement and typed tag-field edits through the
scenario editor's command and save path, then checks the expected MapId
and geometry. Output is deterministic for the same input bytes and
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

This verifies the result in memory and prints its MapId and any kit
reservations.

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
imports return named errors pending their own slices.

## Tests

`cargo test --locked -p forge-build` uses invented maps for deterministic
output, kit byte isolation, stale-base and expectation refusals, typed
field writes and resolution through `content::Library`.

## Docs

- [Forge design](../../../docs/game/forge-design.md)
- [Map packages](../../../docs/game/maps.md)
