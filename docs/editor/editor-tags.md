# The editor for developers: tags

Status: Built.
Switch: none.

The Tags tool: field names, the field registry, the document, bundles, and the windows and panel. The index is [editor.md](editor.md).

## Tags

Every field of any tag of the open map (a disc map, a package, a
converted Halo 1 map), labelled, with a widget per type. Edits save with
the map. **Save** and **Export map...** write them
through the same lossless `MapEditor` path as the scenario's. Any other map of
the source (the shared caches among them) can be browsed read-only, and
tags go out to and come back from tag bundles (below).

### Field names

The editor reads names and `doc="..."` descriptions from the checked-in tag
definitions. Hover a field to see its description, offset and evidence. The
detail row shows the same description, and `tag.fields` exposes it to
scripts. Offset labels identify fields whose purpose is still unknown. See [the field-name research note](notes/tag-field-names.md).

### The field registry

`tags::registry` (`crates/formats/tags/src/registry.rs`) gives every group's root
and block elements as fields tiling every byte, merged from:

- the **schema** (`tags::schema`, `schema/layouts.txt`, inferred from
  retail data): every group's structure (element sizes, blocks, data
  references, tag references, datums, string IDs, raw pointers), no names;
- the **definitions** (`tags::defs`, `defs/*.txt`): typed, named fields
  for groups used by the editor and converters, with their evidence.

A definition's field is used where its size matches the schema's
element; its name counts as a name unless it is only its type and offset
(`string_1c`). A schema field no definition covers keeps the schema's
type, labelled by type and offset (`tagref +0x3c`). Every byte nothing
describes becomes an unnamed span labelled by its offset (`+0x14`), in
words where the alignment allows; the panel shows it as its value
suggests (a float, an integer, hex) and its context menu switches. Float
arrays get a hint from their name and note: colours (`colour`/`color`,
3 floats), angles (`angle`, `field_of_view`, `_yaw`, `_pitch`, or
"radians" in the note; shown in degrees), bounds (`bounds`, `_range`, 2
floats).

The opt-in test in `crates/formats/tags/tests/tag_defs.rs` decodes disc
tags and reports byte coverage. `coverage_counts_every_byte_once` in
`crates/formats/tags/src/registry/tests.rs` reports named, typed,
padding and unknown bytes for the built-in layouts. The editor reads
each field label directly from those layouts.

### The document

`app/tag_doc.rs` (`TagDoc`) holds the open map's tags, ordinary metadata
and BSP regions. `App::sync_tags` loads them when the tool first opens
for a map revision. The BSP buffers use file offsets as keys because
their virtual addresses can overlap. The overlay and undo stack track
each write by region and address. Writes from a drag merge by key within
the region until `TagAction::Seal`. The session stores the changed runs
through `Session::set_tag_patches` and `Session::set_bsp_patches`. The
map's dirty mark, map files and packages include both. Undo and redo in
the Tags tool step through tag edits.

What can be written, and what not (the reason shows in the panel and as
the refusal's text):

| What | Why |
| --- | --- |
| Fields of any tag in the ordinary metadata whose group has a layout | Editable: fixed-length patches in place. |
| Counts, addresses, raw pointers and sizes, BSP descriptors | Shown only: changing them by hand corrupts the map. Blocks change through add / remove / duplicate. |
| The scenario tag | The field panel shows the source values through `scnr.txt`. The Scenario tool owns edits. Placement and palette blocks link to Scene. |
| Tags in a structure BSP region (`sbsp`, `ltmp`) | Editable in place through their layouts. Child block addresses resolve in the same region. The tree row identifies the BSP. |
| Tags with no data in the map (shared-map tags) | Nothing to show here; the tree names the shared cache that has their data (`[in shared]`), and **Show it in ...** browses it there. |
| Tags of a browsed map | Read-only: only the open map saves. |
| A group with no layout, a tag shorter than its layout | Read-only (hex only without a layout). |
| String IDs | Any string the map's string table has, or new text: a string the map lacks is added after its own (below). |
| Data references | Their bytes, in place (same length), up to 4 KiB shown and edited. |

**Blocks**: remove moves later elements down (the last slot keeps its
bytes, dead until re-added), duplicate inserts a copy after the element,
add appends a blank one (zero, null references). Within the elements it
had in the source (and while it still points where it did) a block
changes in place. BSP blocks use their source capacity. Blocks in
ordinary metadata grow past that capacity while **Edit > Allow growing
full blocks** is on (the session's switch, shared with the
Scenario tool; off by default because the engine's headroom for more tag
memory is not established, [scenario-editor.md](scenario-editor.md) §6).
A grown block's elements are copied, with the new one, to room past the
end of the metadata (provisional addresses only the document uses, with
room for twice the elements so the next adds fit there) and its pair
points there; undo points it back. On save each grown block is appended
to the metadata through `MapEditor::append_metadata` and its `(count,
address)` pair patched to the real address
(`scenario_edit::TagGrown`, `Changes::tag_grown`); the old elements
stay as dead bytes, and the elements' own child blocks keep pointing
where they did. A block inside grown elements does not grow until the
map is saved and reopened. Growth needs the metadata to end the file (as
on retail maps) and the engine's size limits to hold; the save refuses
otherwise. An element with nonempty child blocks or data is not
duplicated: the copy would share them. Elsewhere in the map, indices into
a block are not updated when its elements move.

**New string IDs**: typing text the map's string table lacks into a
string ID field (or `set_field` with it) adds the string after the map's
own, with the value its index and length encode (index in the low 24
bits, byte length in the top 8), as one undo step with the write; undo
takes the string back. Under 128 bytes (the fixed slot), no NUL. On save
`MapEditor::add_string` rewrites the three string tables (the 0x80-byte
fixed slots, the offset index, the blob) whole at the end of the file,
the source's entries first and unchanged, and points the header's
`+0x160..+0x174` at them; the old tables stay as dead bytes. Maps written
so have not been loaded by the game yet. The string tables then follow
the metadata, so a map saved with new strings cannot grow blocks until it
is rebuilt.

### Browsing other maps and where tags live

A retail map holds the metadata of every tag it uses; a tag whose data
is all in a shared cache has an index entry and no data. Its raw data
(bitmap pixels, geometry, sound) is reached by raw pointers whose top two
bits name the file: the map itself, `mainmenu`, `shared` or
`single_player_shared` (`assets::ResourceFile`).

The tool shows one document at a time: the open map's (the session's,
editable) or a **browsed** map's (`app/tag_browse.rs`). Browsing opens
another map of the source (`MapSource::open_map`) into a `TagDoc` of its
own whose tags are all read-only (`TagDoc::read_only`), so the open map's
edits and undo are untouched; picking the open map again drops it.
`shared` and `single_player_shared` are not levels: picking one in
**Maps** browses it in the Tags tool and leaves the open map as it is
(`App::select_map`). A map that fails to open as a level is browsed
instead. `App::shown_tags` is the document the tree, fields, inspector
and agent methods read.

Two marks say where a tag lives, both computed from the maps alone:

- **elsewhere**: for a tag with no data in the shown map, the first
  shared cache (`shared`, `single_player_shared`, `mainmenu`) that has
  the same group and name with data (`TagEntry::elsewhere`, set once per
  document by `App::mark_elsewhere`).
- **homes**: the tag's raw pointers counted by file
  (`shared_refs::Homes`, `TagDoc::homes`), cached until the document's
  shape changes (`TagDoc::shape_rev`: load, block changes, undo, redo).

`app/tag_tree.rs` builds the open scenario's tree from its structure
BSPs and lightmaps, skies, object palettes, globals and other references.
Palette counts and scenario-owned reference fields use the current
placements. The reference walker follows layout fields, reads BSP-region
payloads, and scans aligned group and datum pairs for unlabelled tags.
The reachability walk starts from the scenario and globals. The tree
shows an incomplete walk in place of the unreachable list. All tags keeps
the group and folder views, with a search matching each word in
`group:name`. `tag.list` uses the same search.

The inspector lists a tag's **data and resources** (`tag_api::resources`):
every data reference (path, size, address) and raw pointer (path, file,
offset, size). A bitmap previews its first image, decoded from the map
and the shared caches as stored in the source (`App::tag_picture`; field
edits not reflected); `App::tag_media` lists a bitmap's images or a
sound's pitch ranges, permutations and chunks.

### Tag bundles

A tag bundle (`.h2tag`, `editor_core::tag_bundle`) is one or more tags
written as a self-describing JSON file:

```
{ "format": "h2recomp.tag-bundle", "version": 1,
  "origin": { "map": "ascension", "build": "<map header build string>" },
  "tags":  [ { "group": "weap", "name": "objects\\...", "layout": "<fingerprint>",
               "root": { "bytes": "<base64>", "fixups": [ ... ] },
               "fields": { ... } } ],
  "blobs": [ { "file": "shared", "offset": 1234, "bytes": "<base64>" } ] }
```

- `root` is the tag's root element: its bytes and **fixups**, each at a
  byte offset of the element: `tag` (a reference, by `group:name`, or
  null), `string_id` (by its text), `block` (its elements, each an
  element in turn), `data` (the bytes it points at) and `raw` (a raw
  pointer: file, offset, size, and the index of its blob if copied).
  Addresses, indices into the map and string table indices are never
  carried.
- `layout` is a hash of the group's registry layout (sizes and structural
  fields, `tag_bundle::fingerprint`); an import into a build whose layout
  differs is refused.
- `fields` is the tag read by name (`tag_api::read_tag`), for reading
  only; import ignores it.
- `blobs` holds raw resources copied with **resources** on (up to 256
  MiB a bundle).

`app/tag_bundle.rs` writes and applies bundles over a `TagDoc`.
**Export** walks the tag's layout into fixups; with **dependencies** it
also writes every tag it references, and theirs (up to 256 tags; tags
with no data or no layout are named in `left_out`). Bundles hold bytes
from the disc, so they are user output: the editor refuses a path inside
a Git checkout, and an agent export with no path goes to
`<data>/tag-bundles/<group>-<name>.h2tag` (the name's separators as
dots: `weap-objects.weapons.rifle.h2tag`).

**Import** replaces tags of the open map, each with the bundle's tag of
the same group and name (or, for a bundle of one tag, a chosen tag of
the same group). `MapEditor` patches and appends metadata but does not
add tags, so a tag the map lacks refuses the import (or is skipped, with
`skip_missing`). Everything is checked before anything is written, and a
failure part way rolls the document back; a whole import is one undo
step (`TagDoc::squash`) and saves with the map like any edit.

| Part | On import |
| --- | --- |
| Element bytes | Written over the tag, except the fields shown only (counts, addresses, raw pointers and sizes, BSP descriptors), which follow the rules below. |
| Blocks | Resized to the bundle's count first (removed from the end, added blank), growing only while growing full blocks is allowed; then each element applied. |
| Tag references, datums | Resolved by `group:name` in the map, and checked against the field's groups; a name the map lacks refuses the import. One with no name in the bundle (none, or an ID the bundle's map could not name) is copied into the same map and set to none in another (listed in `nulled`). |
| String IDs | Resolved by text; text the map's table lacks is added as a new string ID. One with no text keeps the map's value in another map. |
| Data | Written in place when the length matches; another length is refused. |
| Raw pointers | Carried when they point into a shared cache and the build matches, or into the map itself and it is the same map; otherwise the map's own pointer and size stay (counted in `raw_kept`). Blobs are not written into maps. |
| `snd!`, `ugh!` | Imported only into the map they came from: they index the map's sound gestalt. |

### The windows and the panel

`ui/tags.rs` draws the tool: the **map picker** (the source maps in their
sections with an **(open map)** marker for the Scenario tool's map),
the **tag tree** (Scenario, All tags and Not reachable from the
scenario, with edited and shared-cache marks), the **fields** of the selected tag with Export and
Import, and **Data and bytes** (`tags::hex`), which follows the selected
row, never the mouse. A click on a row's label (or any of its widgets)
selects it (`fields::PanelState::selected`). The window has three folding
headers. **Field details** gives the label, description, type and unit,
the value in the file with **Reset to file value** when it differs, when
a change takes effect and why the field is read-only; under **Developer
details** it gives the path for `tag.set`, the offset, address, bytes and
evidence text. **Bytes**, folded until **Show bytes** on a list element
opens it, gives the selected row's element (or that element) with the
field lit and edited bytes marked. **Data and resources** gives the tag's
data and resources, with a bitmap preview. `tag.bytes` and `tag.reset`
are the window's bytes and its reset (`tag_api::{bytes_of, reset}`).

The fields' header (`tags::header_top`; `tag.view` returns its lines)
is the kind and name (`Weapon  blaster`: the group's `title=`, else its
code), the tag's path with the unsaved count (`objects\test\blaster ·
1 unsaved change`, `fields::row::unsaved`: the changed fields, as the
dots mark them), notes for a browsed map, a BSP and a shared map, how a
change reaches the game for a tag edited here (`tags::link_lines`: `Changes
are saved with the map.`; viewing, `Changes wait in Pending until you
turn Edit on.`; with Edit on, `Changes are sent as you make them.` and
what an unmarked field means), and why the tag is read-only. The datum,
address, size and the layout's coverage are under a folded **Developer
details**. Section rules read the embedded struct's `title=`
(`fields::row::section_title`).

`fields.rs` is the field panel, reusable by any tool:
`fields::panel(ui, &app.tags, tag, path, &mut PanelState, &mut actions)`
draws the element at `path` (per step, a block field's offset and the
element index; empty is the root) and pushes `Action::Tag` commands.

The panel draws the rows of `fields::rows` (`fields/row.rs`), which makes
no UI calls; `tag.fields` returns the same rows
([tag-editing.md](tag-editing.md), "One row model"). A row is one line:
a dot when the value differs from the loaded file, the label in words
(the definition's `label=`, else the name with underscores as spaces,
the first letter upper case, and `LOD`, `HUD`, `AI`, `BSP`, `ID`, `FOV`
and `UV` upper case; an unnamed field by its type and offset, `Value at
+0x14`), the widget, the unit (the definition's `unit=` in words,
`world units`; else `degrees` for an angle), and at the right edge when a change takes effect
(`play::tag_sync::Effect`). That chip shows only with a game connected
on the open map (`fields::Link`, set by `PanelState::follow`), never on
a read-only row, and for a field whose effect is not known only once it
is changed. A changed field's chip is its state in the game instead
(`fields::row::Sending`): `Waiting` while viewing, `Not sent` with a
refusal or a mismatch (the reason in the tooltip), `Waiting for reload`,
else `Sent` with when the game reads it (`Sent · Not known`). The tooltip gives the type in words, the description, the
read-only reason, the effect sentence with a game connected, and the
evidence. A read-only row always has a sentence: the tag's refusal,
`Edited in Scene.` for the scenario's placement lists, `Set by the
editor. Add or remove list items to change it.` for internal values
(`tag_doc::STRUCTURAL`). `PanelState::drawn` keeps what the last frame
drew, for tests.

An element's named rows come first. Its unnamed ones (`named == false`,
offset order, padding last as `Padding, 8 bytes at +0x18`) sit in one
folded **Unnamed fields (N)** row whose summary counts them by type
(`1 tag reference, 8 values`; `fields::row::summary`). The group opens by
itself (`fields::row::opens`) when **Show unnamed fields** is on, when
nothing in the element has a name (under the line `No field of this kind
of tag has a name yet.`), on the focused field, and once when it holds a
changed field or a field with a mark, so it can be folded again. An
unnamed value has **Show as** (Decimal, Whole number, Hex), kept per
group, element path and offset (`fields::show_as_key`, `weap:parts+0x8`)
in `App::tag_fields`; without a choice it reads as its bits suggest
(`fields::guess`). **Show unnamed fields** sits over the rows in the
Tags tool. A field named by the navigation (`nav.go`, `tag.view`) is
focused in the Tags tool as in the inspector.

Numbers drag. Choices, tag references and **Show as** open the pick
dialog (`fields::picker`): a modal with a search box, the list (never
capped; clipped as it scrolls) and Cancel, each pick one undo step. A
choice's row shows its value's name (`Value 7 (not named)` when none
names it); the dialog lists the named values and **Enter a number...**
sets any value the field's width holds. A switches row shows `1 of 2 on:
Loud` and opens to the named bits as checkboxes in two columns, then the
rest folded as **Unnamed bits**, each `Bit N` (shown directly when no
bit is named). A tag reference or datum shows the kind and the tag's last
path segment (`Model  blaster`; the whole path on hover), with **Open**
and **Change...**; the dialog lists **No tag**, then every tag of the
allowed groups (and their child groups), searched by words of
`group:name` as the tag tree is, with a `Kind` row when several kinds are
allowed. Colours pick; angles show degrees; bounds drag as a pair;
string IDs are text; data is hex. `tag.choices` returns the dialog's
lists.

A list (block) row reads `Parts    2 items` (`2 items, room for 4`) with
**Add**. An empty one is that one line and does not open. A list of one
element shows its fields directly. A longer one shows a row per element,
`1  scope` (`fields::row::element_text`), titled by the block's `title=`
field, else by its first named name or tag reference, else the index
alone (`fields::row::element_title`); one element is open at a time,
below its row, the rows are clipped, and past 12 elements a filter box
narrows them by their words (`fields::row::matching`). Each element has
**Open alone**, Duplicate, Remove and **Show bytes**; Add, Duplicate and
Remove are disabled with the reason as tooltip. **Open alone** (the Tags
tool only) navigates to the element (`NavAction::Go` with `element`,
kept in `NavLocation::element`, so Back returns to it): the panel shows
it as the whole element (`fields::panel`'s `path`) under breadcrumbs,
the tag then each list step (`Parts 1  scope`, `fields::row::breadcrumbs`),
each a button back. `tag.elements` lists a list's element rows and
`tag.view path` shows one alone.

`app/tag_api.rs` is the same editor as plain functions, for scripts and
the agent API: `read_tag(doc, tag)` (the fields as JSON by name),
`resolve(doc, tag, "parts[1].speed")`, `set_field(doc, tag, path, json)`
and `block(doc, tag, path, BlockOp)` build the `TagAction` to dispatch,
`find_tag(doc, "weap:objects\\...")`.

Tests: `tags::registry::tests` (every schema group's fields tile every
byte of every element at every depth with the schema's fields at their
offsets; the definitions merge with no conflict; named definition fields
sit at their offsets; an invented group), `tests/tag_registry.rs`
(opt-in, `H2_DISC`: every tag of every disc map walks through the
registry with its blocks inside the metadata),
`scenario_edit::tests::other_tags_patches_save_with_the_scenario_s`,
`scenario_edit::tests::another_tags_grown_block_is_appended_and_repointed`,
`scenario_edit::tests::new_string_ids_save_with_the_patches_that_use_them`,
`editor_core::tag_bundle::tests` (the JSON format, base64, refusals of
another format or version, file names), `app::tag_tree::tests` (search,
groups, the path tree),
`map::tests::added_string_ids_follow_the_maps_own`,
`app::tag_tests` (listing and refusals, a field edit saved byte-exactly
then undone, drag merging, blocks within capacity, a full block grown
and saved appended, a new string ID added, saved and undone; opt-in
`disc_a_retail_tag_field_saves_byte_exactly`; tag bundles: a round
trip through a file as one undo step, references by name, a missing
reference rolled back, missing tags refused or skipped, a stale layout
refused, growth only when allowed, exports kept out of Git; a shared
cache browsed read-only and selected from Maps; resources and lookups),
`api::tests::tags_browse_a_shared_cache_and_round_trip_through_a_bundle`,
`fields::row::tests` (labels, type words, read-only sentences, paths, a
changed row's file value, chips only with a game, the Unnamed fields
group's order, summary and open rules; choice, switches and reference
summaries), `fields::tests` (Show as keys and the settings' ini section),
`fields::picker::tests` (named values and search, every bit, typed
numbers, a 5,000-tag invented map listed with no cap),
`api::tests::tag_view_folds_unnamed_fields_and_opens_them_on_request`,
`api::tests::tag_choices_lists_the_pick_dialogs_choices`,
`headless::tests::the_pick_dialog_sets_choices_references_and_unnamed_bits`,
`headless::tests::the_unnamed_fields_group_folds_and_opens_on_focus_and_change`,
`headless::tests::the_tags_tool_edits_a_field_through_its_widgets`,
which clicks the tree and a flag through the test-engine hooks, and
`headless::tests::tag_rows_match_tag_fields_and_chips_need_a_game`
(the drawn rows equal `tag.fields`, with no chip unpaired and the
list's chip with the fake game viewing).
