# Forge map variants: slices, open decisions and evidence

Status: part of [forge-map-variants.md](forge-map-variants.md); see there for status, labels and the index. This page: slices, open decisions, evidence and what is not verified.

## Slices

Each is one lane of one to two hours: code and tests on invented data,
no game run. The coordinator builds and runs. The UI slices (K5, H1 to
H3, S1 to S6) are in
[forge-map-variants-ui.md](forge-map-variants-ui.md), "Slices". Paths
are under `crates/` unless they start with `docs/`, `assets/` or `web/`.

| # | Slice | Owns | Tests | Done when |
|---|---|---|---|---|
| **M1** | Prints and `EntryOp` ops in the document: `Endpoint::Base.print`, `print`, `EntryOp::{Set, Remove, Reset}`, format 3, the validator's rules, `Refusal::Print` | `formats/forge-doc/src/{entry.rs (new), document.rs, links.rs, validate.rs, lib.rs}`, `formats/forge-doc/src/tests/entry.rs` (new), `formats/content/src/forge.rs`, the crate README | golden vectors for a print and a format 3 id; a document without entry ops keeps its id; each validator rule has a refusing case; the seeded fuzz covers the new ops | `cargo xtask test -p forge-doc -p content` passes |
| **M2** | `resolve`: identities to running indices, lowering to `Placement` ops, the fold over `Reset` | `formats/forge-doc/src/{resolve.rs (new), log.rs, lib.rs}`, `tests/resolve.rs` (new) | a table of add, remove and set orders gives the indices a hand replay gives; lowered ops on a `scenario_edit::fixture` equal the same edits through `scenario_edit::Session`; fold then undo gives the start; `Reset` leaves the entry's bytes as the fixture's | the parity test passes with entry ops |
| **M3** | Materialise entry ops with the print check | `tools/forge-build/src/{entry.rs (new), placement.rs, lib.rs}`, `tools/forge-build/tests/entry.rs` (new), `apps/h2-tools/src/forge_cmd.rs` | moving own entry 2 changes its bytes and the checksum only; a removal swap-removes and repoints names as the editor does; a wrong print is `variant_print` with the block and index and no file; two builds are byte-equal | `h2 forge check` on a fixture document prints each print compared |
| **M4** | Tier, forms and the name rule: `Tier`, `ForgeDoc::tier`, `play_form`, `session_form`, the 16-character name | `formats/forge-doc/src/{variant.rs (new), lib.rs}`, `tests/variant.rs` (new) | a table of documents and their tiers; `play_form(session_form(d))` is `d`; a recipe other than the kit's makes `Tier::Map`; a 17-character name is refused for a variant | tests pass |
| **M5** | Things in the session: `Thing`, `Session::things`, `move_thing` on own and added entries, the hold | `extensions/forge/src/{things.rs (new), scene.rs, session.rs, lib.rs}`, `extensions/forge/src/base_tests.rs` (new) | over a fake `World`: a drop of an own entry is two `EntryOp::Set` in one undo step and one `base_move`; undo calls `base_move` with the first frame; a print the fake changes is refused and the document is the same bytes | existing `tests.rs`, `tool_tests.rs`, `link_tests.rs` pass unchanged |
| **M6** | Field edits, removal, reset: `Session::{set_field, remove, reset}`, `forge::props` for the fields that need nothing | `extensions/forge/src/{props.rs (new), session.rs}`, `extensions/forge/src/props_tests.rs` (new) | a table of `props` by kind; each row's write is one `EntryOp::Set`; remove calls `base_inert`, undo `base_restore`; removing a linked entry ends its links in the same step; a property not in the table is absent | tests pass |
| **M7** | The loaded map's things: `forge_map` reads every block's rows, typed fields and prints | `extensions/game/src/forge_map.rs`, its tests | on invented guest memory: each kind's things are listed with the layout's fields; a print equals `forge-build`'s for the same rows; a block that does not read is left out by name | tests pass |
| **M8** | `ForgeWorld`'s base calls: object by type and placement index, scenario field writes, the inert row, restore | `extensions/agent/src/forge/{base.rs (new), local.rs, tests.rs}` | over the fake game: `base_move` moves the object and writes the placement; an object that is gone is `forge_object_gone` with nothing written; the inert row equals the kit's spare form; restore writes the kept row | tests pass; `cargo check -p cli --no-default-features --features host` |
| **M9** | Aim at any thing: the reticle picks marks of starting points, goals and spawners with nothing out, and the map's own objects | `extensions/forge/src/{tool.rs, tool_tests.rs}`, `extensions/game/src/forge_glue/editing.rs` | a ray table: the nearest thing within its pick radius wins; a thing of another game type is aimed at only with Show at All | tests pass |
| **M10** | Agent API: `forge.things`, entry ops in `forge.propose`, the variant in `forge.status` | `extensions/agent-proto/src/forge.rs`, `extensions/agent/src/forge.rs`, `extensions/agent/src/forge/tests.rs`, `docs/runtime/agent-api.md` | each method answers over the fake game; a refusal carries its name | `session.methods` lists them |
| **D1** | Decode: the item spawner's and starting point's game-type slots, the respawn mode's two values, a census of how the disc's maps use them. Research with `h2 disasm`; names from the public layout are hints | `formats/map/src/scenario_layout.rs`, `tools/scenario-edit/src/tests.rs`, `docs/editor/scenario-editor.md` | the ignored disc test checks every value against the decoded range | each field has an **E** or is left out, and section 7 of the editor's page loses its line |
| **D2** | `forge_doc::games`: the set, `encode`, `decode`, the flag type's game type | `formats/forge-doc/src/{games.rs (new), lib.rs}`, `tests/games.rs` (new) | a table of sets and slots both ways; five single engines are refused; an unknown slot value decodes to "not editable" | tests pass |
| **D3** | Rows for D1's fields in `forge::props`: Game types, Place at start | `extensions/forge/src/{props.rs, props_tests.rs}` | the table gains the rows; a set change is one op a changed slot | tests pass |
| **D4** | `EntryOp::Property` and the `team` property | `formats/forge-doc/src/{entry.rs, validate.rs}`, `tests/entry.rs`, `extensions/forge/src/props.rs` | golden vector; a property on a kind that has none is refused; `forge_doc::labelled` reports the team | tests pass |
| **D5** | Spare clips and weight, each after its trace: the weapon reserve's field, the spawn chooser's rule | `docs/game/notes/forge-probes.md`, then `extensions/agent/src/forge/base.rs`, `extensions/forge/src/props.rs` | a differential test of the write on the fake game | the row appears; until then it is absent |
| **V1** | The variant store: per-map directories, `.h2mapvariant`, list, save, load, rename, duplicate, delete, the named errors | `extensions/forge/src/{files.rs, lib.rs}`, `extensions/forge/src/file_tests.rs` (new) | in a temporary directory: a save reads back to the same id; a name taken is refused without case; a file of the map tier is `variant_tier`; a base of another hash lists with `variant_base_hash` | tests pass |
| **V2** | The lobby's pick and the build before Start: `lobby_start::map_variant`, the checks, the package selection | `extensions/game/src/{map_variant.rs (new), lobby_start.rs, lobby_status.rs}` | over a fake library: a pick builds once and reuses the receipt; each refusal of the table gives its sentence and holds the start state; a map change clears the pick | tests pass |
| **V3** | `MapGate` for a variant: the document by id, the checks, the build callback, the member's sentence | `live/live-hle/src/maps.rs`, its tests | a joiner without the base is refused before load; a hidden document is `variant_missing`; a ranked session takes only a pinned id | tests pass |
| **V4** | Live: the projection and the two routes, the unlisted upload, the 1 MiB cap, in the local server | `live/live-proto/src/map_variants.rs` (new), `live/live-server/src/map_variants.rs` (new), `live/live-server/src/content.rs`, `docs/live/live-api.md` | a document of the map tier is not listed; an oversize one is refused; the list is by base hash, newest first | tests pass |
| **V5** | The same in the Worker, and the parity suite | `live/live-worker`, the parity suite's cases | each V4 case in both homes | the parity suite passes |
| **V6** | Playlist entries: `map_variant` on a record, `HopperVariantsSet`, the publish checks, the pin | `live/live-proto/src/hopper`, `live/live-server/src/admin.rs`, `docs/live/live-admin.md` | a variant of another map than the record's is refused by name; a reorder with `from` carries the variant; two like records with two variants are refused | tests pass in both homes |
| **V7** | A game variant's suggestions: `maps` in the bundle, the picker's order | `formats/variants/src/portable.rs`, `extensions/game/src/map_variant.rs`, `docs/editor/variants.md` | a bundle without `maps` keeps its bytes; more than eight are refused; the picker opens on a suggested variant | tests pass |
| **V8** | The Live site: a map's variants and a variant's page, Download | `web/` (the Live site's pages) | the pages' own tests against the routes' fixtures | the pages render from fixtures |
| **V9** | The editor: a map variant on a playlist entry; open a `.h2mapvariant` in the scenario tool | `apps/studio/src/ui/playlists.rs`, `apps/studio/src/api`, `docs/editor/agent-api.md` | the API's tests: an entry round-trips with its variant | tests pass |
| **G1** | Objects by game type: the `games` property, the build for an engine, `game` in `MapId` format 2 | `formats/forge-doc/src/entry.rs`, `tools/forge-build/src/entry.rs`, `formats/content/src/id.rs`, `docs/game/maps.md` | golden vectors: an id without `game` is unchanged; a build for Slayer lacks a CTF-only crate and has the rest | tests pass |

M1 to M4 depend only on each other, in order. M5 and M6 need M2; M7
and M8 need M1 and can run beside them; M9 and M10 need M5 to M8. D1
is research and can start at once; D2 to D4 follow it. V1 needs M4; V2
needs V1 and M3; V3 to V5 need V1; V6 and V7 need V4. G1 is last. P2 of
[forge-design.md](forge-design.md) (kit sessions) is what lets a
session *add* a spawner, a starting point or a goal; every slice here
that edits an own entry needs no kit.

## Open decisions

The owner's to make. Each has a recommendation, which the sections
above follow.

1. **One saved form or two.** A scenario-only document is a map variant
   (`.h2mapvariant`, listed per map); a document with geometry, recipes
   or tag writes is a Forge map (`.h2forge`, picked as a map).
   Recommended as written: a variant then builds in well under a
   Rebuild, fits every map and can go on a playlist entry. The other
   way is one file type for everything, with the picker listing heavy
   documents as variants.
2. **Does a game variant choose a map variant.** Recommended: it only
   suggests, and its script's labels are what it requires. The other
   way is that picking the game variant sets the lobby's map variant,
   which a leader can then undo.
3. **Variant names.** Recommended: 16 characters, as the game's own
   variant names. A longer cap needs every text that shows one checked
   for fit.
4. **Spare clips, place at start, a spawner's team and a starting point's weight.**
   Halo 2's entries have no field for them. Recommended: build them as
   Forge properties, each shown only once its hook is proven (D5), and
   ship the editor without them. The other way is to leave them out of
   the design.
5. **Ranked.** Recommended: a map variant plays ranked only as an
   admin's playlist entry. The other way is never.
6. **A variant not on Live in a Live lobby.** Recommended: the host
   uploads it unlisted when it picks, so members can fetch it. The
   other way is to refuse the pick until the author uploads it.
7. **`.h2forge` drafts already saved** under `<data>/forge`.
   Recommended: V1 reads none of them; nothing is published and the
   files are test saves.

## Evidence

- **E**, **V**: blocks, strides, typed fields, what refers to an entry
  by index, the spawner and its tick: forge.md 1.2, scenario-editor.md
  sections 1, 2 and 6.
- **Code**: `Endpoint`, `ForgeDoc::links` and the label census in
  `crates/formats/forge-doc/src/{links.rs, labels.rs}`; the fold in
  `log.rs`; the session's edit path in
  `crates/extensions/forge/src/session.rs`; the kit's spare rows in
  `crates/tools/forge-build/src/kit.rs`.
- **Code**: `NewSession.map_id`, hopper pins and the ranked rule in
  `crates/live/live-hle/src/maps.rs` (`MapGate::check`).
- **C**: Halo 3's map variants, their fields and the separate game
  variant: forge.md 1.1 and its sources.
- **E**: the item spawner's game-type slots, the matcher and the respawn
  modes: scenario-editor.md section 2 and
  [notes/2026-11-23-item-game-types.md](notes/2026-11-23-item-game-types.md).
- **C**: the absence of a team, a clip count and a weight: field names of
  the public Halo 2 scenario layout.
- **Design**: the print, the `EntryOp` family, the inert row in a
  session, the tier, the pick's forms, the refusals.

## Not verified

- That an inert row of each block is skipped by every reader in a
  running round (F3's kit probe, [notes/forge-probes.md](notes/forge-probes.md)).
- That a live object's placement index finds the map's own objects of
  every type, and that the spawner's state array gives the object an
  entry has out.
- That a starting point's or a goal's write in the loaded scenario is
  read in the round it is made, or only from the next.
- How the retail maps use the game-type slots and respawn modes: the
  census `disc_item_game_types_and_respawn_modes_census` has not been run.
- What the object bits that arm respawn modes 0 and 1 mean.
- That a write to a weapon's reserve on the ground survives its pickup, and that a spawn bias moves the chooser's pick (D5's probes, [notes/forge-probes.md](notes/forge-probes.md)).
- How long a variant takes to build on a real map, and that the lobby
  can wait for it without a hold a player notices.
- That the host can tell which record of a hopper matchmaking drew.
- That a session's package selection survives the game's own map load
  path in every lobby kind (split screen, System Link, Live).
