# Click to edit: one editor for placements, tags and Halo 1 tags

Status: C1 and C2 are implemented. The remaining slices are design.
Code: C1 lives in studio's `app.rs`, `app/tag_api/chain.rs`, `ui/inspector.rs` and `api/tag_fields.rs`. C2 lives in `app/navigation.rs`, `fields.rs` and `api/nav.rs`. The full design changes `crates/apps/studio/src` (`app.rs`, `app/tag_doc.rs`, `fields.rs`, `ui/inspector.rs`, `ui/tags.rs`, `api`), `crates/tools/editor-core`, `crates/tools/scenario-edit`, `crates/tools/h1map`, `crates/tools/h1objects` and `crates/tools/h1convert`; each slice names its files and tests.
Switch: none.

The Scenario and Tags tools become one Map tool. Clicking anything in the
viewport, a placement or a live object from a paired game
([live-mirror.md](live-mirror.md)), shows its placement fields and the
whole chain of tags behind it in one inspector, every field editable in
place, with navigation through tag references. The same works for
converted Halo 1 maps, with the Halo 1 source tag beside the converted
one, and for Halo 1 maps in their own format. The rule that must hold: an
edit says what else it changes before it is made, and a Halo 1 field with
no Halo 2 counterpart is shown as that and never mapped quietly.

## What exists

| Part | State | Where |
|---|---|---|
| The Scenario tool: outliner, viewport picking, the placement inspector | Built | `ui/outliner.rs`, `viewport.rs`, `ui/inspector.rs`, `scenario_edit::Session` |
| The Tags tool: every field of every tag, blocks, string IDs, its own undo stack, saved with the map | Built | `app/tag_doc.rs` (`TagDoc`), `ui/tags.rs`, [editor.md](editor.md), "Tags" |
| A field panel any tool can draw | Built | `fields::panel` over `tags::registry` |
| Object to `hlmt` to `mode`, `coll`, `phmo`, `jmad` | Built | `tags::object` (`Context::resolve_object`, `resolve_model`) |
| A tag's references, and following one | Built | `tag.refs`, `tag.follow` ([agent-api.md](agent-api.md)) |
| Other maps and shared caches browsed read-only | Built | `app/tag_browse.rs` |
| Tag bundles: export, and import over tags the map has | Built; import does not add a tag | `editor_core::tag_bundle`, `app/tag_bundle.rs` |
| Adding a tag to a map | Built in the map builder, not in the editor's save: `TagSet::from_map` then `add_tag`; the Halo 1 converter uses it | `crates/formats/map/src/build.rs`, [map-builder.md](../formats/map-builder.md), "Adding tags to a pinned map" |
| Converted Halo 1 maps opened and edited as Halo 2 maps | Built: a derived package with an `origin` table (`game`, `map`, `sandbox`, `source_sha256`) | `editor_core::packages::halo1_origins`, `Session::open_derived` |
| Halo 1 disc maps opened read-only: structure, lightmaps, multiplayer placements | Built | `editor_core::halo1` (`H1View`), `editor_core::halo1_art` |
| Reading Halo 1 tags | Built for the header, index, scenario placements, BSPs, bitmaps and the object groups the converter carries; no writer | `crates/tools/h1map`, `h1objects::h1` |
| Which built fields came from a Halo 2 role | Built, for the role audit | `h1objects::provenance` |
| Convert to Halo 2 from the editor | Built | `app/convert.rs` |

New here: the unified selection and inspector, the tag chain, navigation,
"where is this used", one undo timeline, unique copies, the layout
migration, provenance for converted tags, Halo 1 overrides, a Halo 1
field registry and a Halo 1 writer.

## One selection, one inspector

The selection is one of three things (`app::Selection`): a placement
(block and index), a live object (its handle), or a tag. A live object
made from a placement selects both, and the outliner row and the viewport
object light together. The inspector (`ui/inspector.rs`) shows, top to
bottom:

1. **Live** (when paired): the object's state from the mirror's store:
   position, velocity, vitality, team, what it holds, its animation.
   Read-only, with the object actions of live edit (move, respawn,
   delete).
2. **Placement**: the typed fields the Scenario tool shows today.
3. **Tags**: the chain as a tree. Each node is a tag; opening one draws
   its fields with `fields::panel`, the same widgets, refusals and
   tooltips as the Tags tool, editing the same `TagDoc`.

The chain is computed by `tag_api::chain(doc, root)`, with no UI:

| From | To |
|---|---|
| a placement | its palette entry's object tag (and netgame equipment's item collection, each entry) |
| an object tag | its `hlmt`; for a weapon, its barrels' projectiles and its effects; for a projectile, its detonation and impact effects and damage effects; for a vehicle or biped, its seats and weapons; its attachments and widgets |
| `hlmt` | `mode`, `coll`, `phmo`, `jmad`, with the variant the object uses |
| `mode` | its materials' shaders |
| a shader | its template and bitmaps |
| an effect | its parts' tags: particles, sounds, damage effects, lights, decals |

The named rows are shortcuts with labels. Under them, every tag has
**All references** (`tag.refs`), so nothing is unreachable because the
table lacks a row. A reference to a tag with no data in this map (it
lives in a shared cache) is a node marked "in shared", opened read-only
through the browse path, as the Tags tree does.

A live object with no placement (a projectile, a dropped weapon, a body)
selects by its definition tag, which the `objects` layer carries, so
clicking a projectile in flight opens its `proj` tag.

### Navigation

- **Jump**: a tag reference field has a button that makes the referenced
  tag the inspector's subject. The selection in the viewport does not
  change; a line above the fields says whose tag this is.
- **Breadcrumbs**: the path from the selection to the shown tag
  (`placement > weap > proj > effe`), each crumb a jump.
- **Back and forward** (`app::NavHistory`, mouse buttons and Alt+arrows):
  a stack of (selection, tag, field path) per editor window, as a browser
  keeps. Selecting in the viewport pushes; undo does not move it.

### Where is this used

`tag_api::uses(doc, tag)` answers from a reverse index of the map's
references, built once per document shape (`TagDoc::shape_rev`):

- the tags that refer to it, and up through them to object tags;
- the placements whose palette entry reaches it;
- when paired, the live objects whose definition reaches it.

The panel lists them and the viewport outlines them. Each tag node's
header shows the count ("shared by 14 placements, 3 tags"), because that
is what an edit to it changes.

### Editing a shared tag

A tag belongs to every object that uses it. An edit in the inspector
changes all of them, so the first edit to a tag in a session says so
beside the field ("changes 14 placements: show") and offers **Make a
unique copy**.

A unique copy (`tag_api::unique(doc, placement, tag)`) gives this one
placement its own tags: a copy of every tag on the chain from the
placement's object tag down to the edited tag, each under a new name
beside the original, each parent's reference pointed at its child's
copy, a new palette entry for the copied object tag, and the placement
pointed at that entry. Tags off that path stay shared. It is one undo
step.

`MapEditor`, which the editor saves with, patches and appends metadata
and cannot add a tag (editor.md, "Tag bundles"). A map with unique copies
is therefore saved through the map builder: `TagSet::from_map` of the
source, `add_tag` for each copy, the document's patches applied to the
set, then `map::build`, written as a derived package
(`[base] derived = true`) or a whole `.map`. The palette entry is a block
growth and follows the rule for growing full blocks
([scenario-editor.md](scenario-editor.md) section 6). **Make a unique
copy** is disabled, with the reason, for a map at the tag-memory bound
and for a browsed map. The same path lets a tag bundle import add tags a
map lacks, which import refuses today.

### Undo, save and live apply

Edits from the inspector are the Tags tool's `TagAction`s and the
Scenario tool's commands, so they save as they do today. Two things
change:

- **One undo timeline for the map.** The map document keeps a journal of
  which stack each step went to (`scenario_edit::Session`'s or
  `TagDoc`'s). Undo and redo step the journal, so Cmd+Z undoes the last
  thing done wherever it was. `doc.undo {"doc": "map"}` steps the
  journal; `"doc": "tags"` stays as a name for the same document, so
  existing scripts keep working.
- **Live apply.** With a paired game in Edit, each edit goes to the game
  by its apply class (live-mirror.md, "How an edit applies"): the same
  badges, the same pending list, the same session rules. A unique copy is
  class Reload: it adds tags.

The scenario tag keeps one owner. Its placement blocks are the Scenario
commands'; its other fields show in the inspector read-only until a
definition names them, as the Tags tool has it.

## The layout

The Tools list holds **Map**, Game variants, Playlists, UI Screens and
HUD. Map is the Scenario tool with the Tags tool's windows docked into
it:

| Today | In the Map tool |
|---|---|
| Scenario: outliner, viewport, inspector, Game panel | the same windows |
| Tags: the map picker and tag tree | a **Tags** tab beside the outliner. It is how tags with no object in the scene are reached: globals, HUD and UI tags, sounds, anything in a shared cache. Selecting a tag there makes it the inspector's subject |
| Tags: the fields | the inspector's Tags section |
| Tags: Export and Import | the tag node's menu in the inspector and the tree |
| Tags: Data and bytes | a tab beside the Log |

`ToolKind::Tags` stays as a window group, not a tool, so each window
keeps its dock slot (`WindowKey::label`) and a saved layout still loads.
The `tag.*`, `scenario.*` and `doc.*` methods are unchanged. `--do`
scripts that pick the Tags tool pick the Map tool and focus the Tags tab.
The HUD tool stays its own tool: it draws a HUD, not a scene.

## Halo 1 tags

Halo 1 content reaches the editor in two forms, and both get the
inspector above.

### Converted maps

A converted map is a Halo 2 map, so clicking and editing work with no
change. What is added is where each tag came from and where an edit is
kept.

**Provenance.** The converter writes a sidecar into the package,
`provenance.json`, listing for each built tag its Halo 1 source tags
(group and name), and for each field a builder notes: the Halo 1 field it
was computed from, or "role" (a Halo 2 value, as `h1objects::provenance`
records today), or "default". Builders gain `provenance::note_source`
beside `note`; a field no builder notes has no entry.

The inspector shows the Halo 1 source tag beside the converted one, read
from the Halo 1 disc when it is open, and each field's origin:

| Origin | Shown as |
|---|---|
| computed from a Halo 1 field | the Halo 1 field and its value beside it, with a jump |
| a Halo 2 role value | "from Halo 2 (`<base map>`)" |
| a default | "default" |
| not noted | "origin not recorded" |
| a Halo 1 field nothing was computed from | in the Halo 1 tag's panel: "not used by the conversion" |

Nothing is inferred from matching names or values. A field with no
recorded origin says so.

**Where an edit lands.** In a source override, which the converter
applies again on every rebuild. A converted map is rebuilt whenever the
converter changes, so an edit kept only in the built map would be lost
without a word. The override is a file in the data root,
`<data>/h1-overrides/<halo 1 map>.toml`, keyed by the origin's
`source_sha256`, with two kinds of entry:

- **Converted-tag overrides.** An edit made on a built tag: its group
  and name, the field path, the value, and `was` (the converter's value
  when the edit was made). The converter applies them last
  (`h1convert::overrides::apply`). On a rebuild, an entry whose path does
  not resolve fails the build naming it; an entry whose `was` differs
  from the fresh value is applied and listed in the build report as
  "the conversion changed under this edit", with both values. The editor
  shows that list and lets each entry be kept, dropped or re-pointed.
- **Halo 1 source overrides.** An edit made on the Halo 1 tag (below).
  The converter applies them to the Halo 1 map's bytes in memory before
  converting, so the change goes through the conversion as any source
  value does.

Editing a built tag in the inspector writes the document and a
converted-tag override in one undo step. Saving writes the package and
the override file. Live apply of a built-tag edit is a poke like any
other. Live apply of a Halo 1 source edit is: rebuild in the background
(`app/convert.rs`), diff the new map against the one the game holds,
send the changed fields as pokes, and put what cannot be poked under
Reload.

The Halo 1 sandbox as a layer over every map
([shared-map.md](../halo1/shared-map.md)) is a design. When it is built,
an override names the layer's build as its target the same way, and an
edit to a layer tag is kept once, not per map.

### Halo 1 maps in their own format

With a Halo 1 source open, its maps are in the Maps list, read-only
today. Clicking a Halo 1 placement shows its Halo 1 tag chain: the
object tag, its model, collision, physics and animation tags, its
weapon, projectile, effect and damage tags, its shaders and bitmaps, as
the Halo 1 references name them, with **All references** as above.

It is editable. An edit is a Halo 1 source override, saved in the same
override file, undone on a document of its own (`doc: "h1"`). The map on
the disc is never written. **Convert to Halo 2** reads the disc and the
override.

What `h1map` has: the header, the zlib stream inflated once, the tag
index and tag array, the scenario's BSP list and multiplayer placements,
BSP regions and collision, bitmaps (`H1Map`, `scenario`, `bsp`,
`bitmap`). `h1objects::h1` reads the object groups the converter carries,
with hand-written offsets. What is missing:

| Missing | Needed for |
|---|---|
| A Halo 1 field registry (`h1map::registry`): each group's root and block layout, fields tiling every byte, named where [objects.md](../halo1/objects.md) and the readers have evidence, typed spans elsewhere | the field panel; walking blocks; `tag.refs` on a Halo 1 tag |
| A patch overlay on `H1Map` (`H1Map::with_patches`): same-length field writes over the decompressed image | Halo 1 source overrides; the converter reading an edited map |
| A reference walk and reverse index for Halo 1 tags | the chain, "where is this used" |
| Readers for the model and sky tags (editor.md, "Packages and Halo 1 in the Maps list") | drawing the object clicked on; until then a Halo 1 placement is its glyph |
| A writer (`h1map::write`): the patched image deflated behind its header, with whatever the header checks | **Save as a Halo 1 map** |
| Block growth, new tags, new strings | not designed; an edit that needs them is refused with the reason |

**Save as a Halo 1 map** is the last slice and the least needed: nothing
in h2recomp runs a Halo 1 map, so the override file is the save that
matters. It exists for other tools.

## Interfaces

New methods on the editor's socket, through providers
([agent-api.md](agent-api.md), "Extension points").

| Method | Params | Result |
|---|---|---|
| `scenario.select` | gains `{object}` (a live handle) and `{tag}` | the selection: placement, object, tag |
| `tag.chain` | `{block, index}`, `{object}` or `{tag}`; `depth?` | the tree: each node's `tag`, `group`, `name`, `via` (the field path that names it), `where` (`here`, `shared`), `uses` |
| `tag.uses` | `{tag}` | `{tags, placements, objects}`; `highlight?: true` outlines them in the viewport |
| `tag.unique` | `{block, index, tag}` | the copies made (`from`, `to`), the new palette entry; `REFUSED` with the reason |
| `nav.status`, `nav.back`, `nav.forward`, `nav.go` | `nav.go {tag, path?}` | the shown tag, the breadcrumbs, whether back and forward can step |
| `tag.origin` | `{tag, path?}` | a converted tag's Halo 1 sources and each field's origin; `REFUSED` on a map with no provenance |
| `h1.list`, `h1.read`, `h1.refs`, `h1.set` | as `tag.list`, `tag.read`, `tag.refs`, `tag.set`, on the open Halo 1 map | the same shapes; `h1.set` is one undo step on `doc: "h1"` |
| `override.list`, `override.drop`, `override.retarget` | `{map?}`, `{id}`, `{id, tag, path}` | the override file's entries, each with `kind`, `tag`, `path`, `value`, `was`, `state` (`applied`, `changed`, `unresolved`) |

`tag.set` on a converted map also writes the override entry and returns
its id. `doc.save` of a map with unique copies takes the builder path.

## Slices

Each ships something the user sees. Lanes write code and tests on
invented maps (the scenario crate's fixture, `h1map::testing::build`);
the coordinator runs them.

| Slice | Ships | Acceptance |
|---|---|---|
| **C1** Click a placement, edit its tags | `app::Selection`; `tag_api::chain`; the inspector's Tags section over `fields::panel`; `tag.chain` | fixture: a placement's chain lists its object, `hlmt` and `mode`; an edit made in the inspector and the same `tag.set` make identical documents and byte-identical saves |
| **C2** Navigate | jump, breadcrumbs, `NavHistory`, `nav.*` | jump, back, forward return to the same tag and field; a viewport pick pushes one entry |
| **C3** Where is this used | the reverse index, `tag.uses`, the outline, the shared count on each node | fixture with two placements on one tag: both listed and outlined; the index rebuilds after a block edit |
| **C4** One tool | the journal undo; the Tags windows docked into Map; `ToolKind::Tags` as a group; the Tools list | undo crosses a placement move and a tag edit in order; a saved layout from before loads; every `tag.*`, `scenario.*`, `doc.*` example still passes (`every_method_in_methods_has_a_test`) |
| **C5** Click a live object | selection of a mirror object; the Live section; edits through live edit (needs live-mirror M1 and M7) | fake game: picking a live object selects its placement and tag; a projectile selects its `proj` |
| **C6** Unique copy | `tag_api::unique`; the builder save path; bundle import adding tags | fixture: the placement's chain is new tags, the other placement's is untouched; the saved map reads back with both; undo restores one chain; refused at the tag-memory bound |
| **H1** Converted tags show their source | `provenance.json` with tag sources; `provenance::note_source` on the first builders (weapons, projectiles); `tag.origin`; the source tag beside the built one | invented Halo 1 map converted: each built tag lists its sources; an unnoted field answers "origin not recorded" |
| **H2** Halo 1 tags in the inspector | `h1map::registry`; the Halo 1 reference walk; the chain on a Halo 1 map; `h1.list`, `h1.read`, `h1.refs` | the registry tiles every byte of every tag of an invented map; opt-in, of the user's Halo 1 disc |
| **H3** Edit Halo 1 tags | `H1Map::with_patches`; the override file; `h1.set`; the converter reading overrides; rebuild, diff, poke | an `h1.set` changes the converted map's field and nothing else; undo removes the entry; the disc file's hash is unchanged |
| **H4** Edits to converted tags survive a rebuild | converted-tag overrides, `h1convert::overrides::apply`, the `changed` and `unresolved` states, `override.*` | rebuild keeps the edit; a changed converter value is listed with both values; an unresolved path fails the build by name |
| **H5** Save as a Halo 1 map | `h1map::write` | an unpatched map written and read back equals its source image; a patched one differs in the patched bytes only |

C1 to C4 need no game. H1 to H4 need no game and no Halo 2 disc in
their tests.

## Rules and limits

- An edit to a shared tag says how many objects it changes before the
  first write.
- A unique copy never changes another placement's tags.
- A map on a disc is never written; saves go to a new file, a package or
  an override, none inside a Git checkout.
- An edit to a converted map is in an override the converter applies
  again. A rebuild never drops one quietly: it applies it, lists it as
  changed, or fails naming it.
- A Halo 1 field's Halo 2 counterpart is what the converter recorded. No
  recorded counterpart is shown as none.
- Override files hold values from the user's disc (`was`), so they are
  user output.
- Tags in a structure BSP's region (`sbsp`, `ltmp`) are not in the
  inspector until the Tags document shows them.

## Evidence

- **Code**: the built parts are as "What exists" cites them.
- **Code**: `MapEditor` does not add tags, and the builder does
  (editor.md, "Tag bundles"; map-builder.md, "Adding tags to a pinned
  map"). The converter's maps, which add tags this way, load and play
  ([multiplayer.md](../halo1/multiplayer.md)).
- **Code**: `h1map` has no writer (`crates/tools/h1map/src/lib.rs`).
- **Design**: the journal undo, the override file and its two kinds, the
  provenance sidecar, the layout.
- **S**: that a retail map (not a converted one) with added tags loads in
  the game; that a deflated Halo 1 image with its header is all a Halo 1
  map needs; that rebuilding a converted map is quick enough to sit
  behind an edit.

## Not verified

- A retail map saved through the builder with a unique copy, loaded by
  the game.
- The time of a rebuild after a Halo 1 source edit.
- How many built fields the builders can note an origin for.

## Related

- [live-mirror.md](live-mirror.md): the stream, live edit and its rules.
- [editor.md](editor.md), "Tags" and "The window";
  [scenario-editor.md](scenario-editor.md).
- [map-builder.md](../formats/map-builder.md): adding tags.
- [rebuild.md](../halo1/rebuild.md), [objects.md](../halo1/objects.md),
  [map-format.md](../halo1/map-format.md): the Halo 1 side.
