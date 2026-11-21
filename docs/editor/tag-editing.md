# Tag editing: the field panel in plain words

Status: Design. T1 (effect words, `play::tag_sync::Effect`), T2 (the row model, `fields::rows`, and the extended `tag.fields`), T3 (the Unnamed fields group, **Show unnamed fields**, **Show as** and `tag.view`), T4 (`label=`, `unit=`, `title=`, `axes=` and section titles in the definitions), T6 (the pick dialog, `fields::picker`, the switches summary and unnamed bits, and `tag.choices`), T8 (Data and bytes following the selected row, **Reset to file value**, **Show bytes**, `tag.bytes` and `tag.reset`), T9 (the header's kind title, path and unsaved count, the link lines, the changed rows' `Sent`, `Waiting`, `Not sent` and `Sent · Not known`, the developer facts folded, and section titles) and T5 (element rows with titles, one open at a time, the filter, **Open alone** and breadcrumbs, `element` in `nav.*`, `tag.elements`) are built; the rest is not. A switches field's hex word is in Field details, under Developer details. `tag.view` takes `tag`, `path`, `field`, `unnamed` and `show_as` and refuses `search` and `changed` until T7. "The built panel" describes the panel before T1, the code the slices replace.
Code: `crates/apps/studio/src/fields.rs` (`panel_with_marks`), `crates/apps/studio/src/fields/row.rs` (`rows`), `crates/apps/studio/src/ui/tags.rs` (`fields`, `hex`), `crates/apps/studio/src/play/tag_sync.rs` (`effect`), `crates/apps/studio/src/api/tag_fields.rs`. Tests: `crates/apps/studio/src/app/tag_tests.rs`, `headless::tests::the_tags_tool_edits_a_field_through_its_widgets`.
Switch: none.

The field panel shows a tag as rows a person can read: a name in words, a
value with its unit, and, only while a game is connected, when the change
takes effect. Fields nobody has named are folded away per element and
stay reachable. The panel, the inspector and the agent API read one row
model, so what a person sees and what `tag.fields` returns are the same
rows. The one rule that must hold: the panel never hides that something is
unknown, read-only or unsent; it says so once, in plain words, in a place
that does not repeat on every row.

The editor is Dear ImGui (`dear_imgui_rs`) on wgpu, native Rust
([editor.md](editor.md)). Nothing here needs a web view.

## The built panel

Each finding names the code that produces it. Labels are **Code** unless
marked.

| # | What a user meets | Where it comes from |
|---|---|---|
| 1 | A pill reading `Poke, unknown` under almost every field name. The four pill texts (`Poke, unknown`, `Poke, each frame`, `Poke, next use`, `Poke, next creation`) name the transport, not the outcome. | `play/tag_sync.rs` `class`, drawn by `fields.rs` `Cx::label` through `theme::link_badge` |
| 2 | The pill shows with no game connected. `Cx::label` asks only whether the tag is editable, never whether a game is paired. | `fields.rs` `Cx::label` (`if self.editable`) |
| 3 | Every editable row is two lines: the name, then the pill with the widget beside it. `ui.text` is followed by the pill's `ui.dummy` with no `same_line`. | `fields.rs` `Cx::label` |
| 4 | Almost every field reads "unknown" because 3 of about 4,850 definition field lines carry `applies=`, all in `shad.txt`. | `rg -c 'applies=' crates/formats/tags/src/defs/*.txt` |
| 5 | Unnamed fields sit between named ones, labelled `+0x14`, `tagref +0x3c`, `f32 +0x1c`, `block +0x98`. About 1,570 definition lines are `unknown` spans and about 750 more have a typed-only name; the 79 schema groups without a definition have no names at all. | `registry.rs` `fill`, `unnamed_label`; `defs.rs` `FieldDef::typed_only` |
| 6 | A field with a written description but a typed-only name (`scalar_1c`) shows as `f32 +0x1c`; the description is only in the tooltip. | `registry.rs` `merge` (`named = !f.typed_only() && ...`) |
| 7 | Names are identifiers: `lod_offset`, `base_map`. Section rules read `unit.obje`. Type words in tooltips are `? 4`, `enum16`, `block 0x50`. | `fields.rs` `Cx::element`, `registry.rs` `Kind::label` |
| 8 | No units. A float is `%.4f` with nothing after it; only angles get `deg`, chosen from the field's name. | `fields.rs` `Cx::float`, `registry.rs` `hint` |
| 9 | A block shows one element at a time behind an `element` number box. Elements have no titles, so finding "the second barrel" means stepping and reading. The header reads `parts  · Reload  [2]`. | `fields.rs` `Cx::block` |
| 10 | An enum shows `2: weap` in a combo. Unnamed values are left out of the list, so a value outside the names cannot be picked. | `fields.rs` `Cx::field`, `Kind::Enum` arm |
| 11 | A flag set is a raw hex word, then one checkbox row per named bit. An unnamed bit shows only when set, so it cannot be turned on from the panel. | `fields.rs` `Cx::field`, `Kind::Flags` arm |
| 12 | The tag picker is a combo listing `group: name` rows, cut at 400 with `... type to narrow the list`. Groups are four-character codes. | `fields.rs` `Cx::picker`, `PICKS` |
| 13 | There is no search inside a tag. The only filter (`TagsUi::filter`) narrows the tag tree. | `ui/tags.rs` `all_tags` |
| 14 | The bytes pane follows the mouse: `PanelState::hovered` is cleared each frame, so moving toward the pane changes what it shows. | `fields.rs` `panel_with_marks`, `ui/tags.rs` `hex` |
| 15 | "Show as float / integer / hex" for an unnamed span is a right-click menu with no sign that it exists; the choice is not kept and the API cannot reach it. | `fields.rs` `Cx::unknown`, `PanelState::views` |
| 16 | The header leads with the datum, the address and the byte size, then `layout weap: N named fields, M by offset` and a `padding` checkbox. | `ui/tags.rs` `fields` |
| 17 | Nothing on a row says whether the value differs from the file, and nothing puts one field back. Whether a change is saved, sent or waiting is in the tree's `*` and `[Pending]` marks and the Game panel. | `ui/tags.rs` `RowView::marked`, `ui/play.rs` |
| 18 | `tag.fields` rows carry `name, offset, kind, size, value, description`: not whether a field is named, editable, changed, or when it applies. | `api/tag_fields.rs` `fields` |

Before, for an editable tag with no game running (invented tag and
values, the fixture of `app/tag_tests.rs`):

```text
weap  objects\test\blaster
0xe1230045  at 0x00001200, 64 bytes  *edited
[Export...] [ ] references [ ] resources [Import over this tag...]
layout weap: 4 named fields, 9 by offset  [ ] padding
----------------------------------------------------------------
kind
(Poke, unknown)           [          1          ]
flags
(Poke, unknown)           0x0003
                          [x] loud
                          [x] quiet
+0x8
(Poke, unknown)           [       0.4500        ]
+0xc
(Poke, unknown)           [     0x8123ab00      ]
tagref +0x10
(Poke, unknown)           [ none              v] [o]
+0x18
(Poke, unknown)           [          0          ]
v parts  · Reload  [2]
    element [ 0 ] [-] [+]  [+ Add] [Duplicate] [Remove]
    +0x0
    (Poke, unknown)       [          0          ]
    datum +0x4
    (Poke, unknown)       [ none              v] [o]
    shape
    (Poke, unknown)       [ 0: box            v]
    +0xa
    (Poke, unknown)       [          0          ]
```

## How it works

### One row model

`fields::rows` (new, `crates/apps/studio/src/fields/row.rs`) turns one
element into `Vec<Row>` with no UI calls. `panel_with_marks` draws rows;
`api/tag_fields.rs` serialises the same rows. A `Row` has:

| Part | Content | Source |
|---|---|---|
| `label` | The name in words: `Bounding radius`, `LOD offset`. | `Field::label` (below) |
| `name` | The path token, unchanged: `bounding_radius`, `+0x14`. | `Field::name` |
| `named` | Whether a definition names the field. | `Field::named` |
| `type_words` | One of the words in the type table. | `Kind`, `Hint` |
| `unit` | A unit word or none. | `Field::unit` (below) |
| `value`, `file_value` | The value, and the value in the loaded file when they differ. | `tag_api::value`, `TagDoc::read_source` |
| `read_only` | None, or the reason as a sentence. | `TagDoc::refusal`, `Kind::is_structural`, the scenario owner rule |
| `effect` | When a change takes effect (below). | `tag_sync::effect` |
| `live` | With a game connected and the field changed: sent, waiting, or refused, with why. | `play::issues::IssueIndex` |
| `description`, `evidence` | The definition's `doc` and its meaning text. | `Field::doc`, `Field::meaning` |

A row is one line: an edited dot, the label, the widget, the unit, and at
the right edge the effect or live state when there is one. The
description, type, offset, address and evidence are in the tooltip and in
the Field details pane, not on the row.

Type words, shown in tooltips, details and `tag.fields`:

| `Kind` | Words |
|---|---|
| `Int` | `Whole number` (with `0 to 65535` style range in details) |
| `Float`, plain | `Decimal number` |
| `Float`, `Hint::Angle` | `Angle` |
| `Float`, `Hint::Colour`; packed colour | `Colour` |
| `Float`, `Hint::Bounds` | `Range` |
| `Enum` | `Choice` |
| `Flags` | `Switches` |
| `StringId` | `Name` |
| `TagRef`, `Datum` | `Tag reference` |
| `Block` | `List` |
| `Data` | `Bytes` |
| `Raw`, `RawSize`, `Bsp`, `Word` | `Internal value` (always read-only or raw) |
| `Unknown` | `Unnamed value` |
| `Pad` | `Padding` |

Arrays show a letter per component: `x y z` for three floats, `i j k w`
for four, `r g b` for a colour, `min max` for a range. The definition
may say otherwise with `axes=` (below).

### When it takes effect

`tag_sync::class` becomes `tag_sync::effect`, returning an `Effect` and
the reason. The Poke strings are deleted. Each effect has an id for the
API, a short chip for the row, and a sentence for tooltips, details and
the Pending tab:

| Id | Chip | Sentence | From |
|---|---|---|---|
| `now` | `Now` | `Applies now.` | `applies=frame` |
| `next_use` | `Next use` | `Applies the next time the game uses it.` | `applies=use` |
| `next_spawn` | `Next spawn` | `Applies when the object next spawns.` | `applies=creation` |
| `reload` | `After reload` | `Applies after reload.` then the reason | the reload rules in `tag_sync::class` |
| `not_known` | `Not known` | `Not known; reload to be sure.` | no `applies` |

What a row shows depends on the game link (`app.pairing.published`):

| Link | Row | Header line |
|---|---|---|
| No game, or a game on another map | No chip. | `Changes are saved with the map.` |
| Connected, View | Chip on fields whose effect is known or `reload`. A changed field shows `Waiting`. | `Game connected, viewing. Changes wait in Pending until you turn Edit on.` |
| Connected, Edit | The same chips. A changed field shows `Sent`, `Waiting for reload` or `Not sent`, with the reason in the tooltip. | `Game connected. Changes are sent as you make them.` |

A `not_known` field has no chip until it is changed (**Design**): with
`applies` on so few fields, a chip on every row is the built panel's
noise in other words. The unknown is still named in three places: the
header adds `Fields with no mark are sent at once; when the game reads
them is not known. Reload the map to be sure.`, the changed row reads
`Sent · Not known`, and the details pane and `tag.fields` give the
`not_known` effect for every such field.

The share of `not_known` falls through data, not UI: `applies` is
inherited. An `applies=` on a `group`, `struct` or `block` line gives
that effect to every field below it that has none of its own, with the
evidence in that line's meaning text
([tag-definitions.md](../formats/tag-definitions.md)). A block whose
engine reader runs on each use is marked once.

Block and data rows have the `reload` effect when a game is connected and
no chip otherwise. The word `Reload` leaves the block header.

### Names, labels and units

`registry::Field` gains `label` and `unit`; `defs::FieldDef` gains the
attributes that feed them. The parser refuses an unknown word in each, as
it does for `applies`.

| Attribute | On | Meaning |
|---|---|---|
| `label="..."` | a field | The label when the derived one is wrong. |
| `unit=<word>` | a numeric field | One of `seconds`, `ticks`, `world_units`, `world_units_per_second`, `degrees`, `degrees_per_second`, `fraction`, `percent`, `pixels`, `count`, `index`. |
| `axes=<a\|b\|...>` | an array | Component letters. |
| `title="..."` | a `group` or `struct` line | The kind in words: `Weapon`, `Object`. |
| `title=<field>` | a `block` line | The element field whose value titles each element. |
| `applies=` | field, `block`, `struct`, `group` | As above, inherited. |

The derived label is the name with underscores as spaces, the first
letter upper case, and the words of a short list in upper case (`LOD`,
`HUD`, `AI`, `BSP`, `ID`, `FOV`, `UV`). `degrees` means stored in radians
and shown in degrees; it replaces the name match in `registry::hint` for
fields that carry it. A typed-only name with a `doc` (finding 6) is a
naming gap, closed by giving the field a name, not by a UI rule.

Section rules use the struct's `title` (`Object`, `Unit`, `Weapon`), and
the tag header uses the group's (`Weapon`). A group or struct without a
title shows its code.

### Unnamed fields

Every element (the root and each block element) ends with one folded
row:

```text
> Unnamed fields (9)    1 tag reference, 8 values
```

It holds every field with `named == false` in offset order. The summary
counts what is inside by type words, so a reference or a list in there is
not a surprise. Inside, a row is labelled by type and place: `Value at
+0x14`, `Tag reference at +0x10`, `List at +0x98`. An unnamed value has a
`Show as` button that opens a picker (Decimal, Whole number, Hex); the
choice is kept per group and offset in the editor's settings and is in
`tag.view`.

The group opens by itself when it holds the focused field
(`PanelState::focus_field`), a changed field, a field with a Pending
mark, or a search hit. **Show unnamed fields** in the panel's filter row
opens every group at once and is saved with the layout. Padding is in the
same group, last, as one line per span (`Padding, 8 bytes at +0x18`); the
`padding` checkbox goes.

A group with no definition shows only this group, open, under a line:
`No field of this kind of tag has a name yet.`

Raw hex is on demand, never the default view: the Field details pane
shows the selected field's bytes, and **Show bytes** on an element's
header opens the element's bytes there with the selected field lit.

### Lists (blocks)

A list row reads `Parts    2 items`. Expanded, it shows its elements as
rows, each `index  title`:

- The title is the value of the block's `title=` field. Without one it is
  the first named `Name` or `Tag reference` field's value. Without either
  the row shows the index alone.
- A list of one element shows that element's fields directly.
- With more, one element is open at a time, below its row. A long list is
  clipped (`ListClipper`) and has a filter box over titles once it has
  more than 12 elements.
- **Open alone** on an element shows that element as the whole panel and
  adds it to the breadcrumbs. This is `fields::panel`'s existing `path`
  argument, which the Tags tool passes as empty.
- **Add**, **Duplicate** and **Remove** stay, on the list's header row and
  on each element's row. A disabled one says why in its tooltip
  (`TagDoc::block_refusal`).

Empty lists fold to one line, `Parts    empty`, and do not expand.

### Choices, switches and references

A choice opens a picker dialog: a modal with a search box, the list, and
Cancel. There are no arrow rows and no combos in the panel.

- **Choice (enum).** The row shows the value's name. The picker lists
  every named value. A value with no name shows as `Value 7 (not named)`
  when it is the current one, and **Enter a number...** sets any value
  the field's width holds.
- **Switches (flags).** The row is a summary, `2 of 9 on: Loud, Quiet`.
  Expanded, named bits are checkboxes in two columns, then `Unnamed bits`
  folded, with every remaining bit as `Bit 7`, set or not. The hex word
  is in details.
- **Tag reference.** The row shows the kind and the tag's last path
  segment (`Model  blaster`), the whole path in the tooltip, and two
  buttons: **Open** (the built `reference_action`, with Back) and
  **Change...**. The picker lists the map's tags of the allowed groups
  with no cap (clipped), searched by words as the tag tree is
  (`tag_tree::search`), with **No tag** first. When several kinds are
  allowed, a `Kind` row at the top opens a second picker.
- **Name (string ID).** Text, as built. A name the map lacks shows `New
  name, added on save` beside the field before Enter is pressed.

### Finding a field

The filter row sits above the rows:

```text
[ Search fields            ] [ ] Changed only  [ ] Show unnamed fields
```

Search matches words against the label, the name, the description, the
type words, choice and switch names, and the names of referenced tags. It
walks every element of every list from the element shown, to depth 12 and
5,000 elements. Hits replace the rows as a flat list of breadcrumb and
value; choosing one opens its lists and scrolls to it
(`tag_api::field_focus`). Reaching the budget prints `Search stopped
after 5,000 list items. Open a list and search there.` **Changed only**
lists the fields that differ from the file, the same way.

Breadcrumbs sit under the header: `Weapon blaster > Parts > 1 barrel`.
Each crumb goes there. Back and Forward (`nav.back`, `nav.forward`) step
through tags and elements.

### Details and bytes

The **Data and bytes** window follows the selected row, not the mouse. A
click on a row's label selects it. The pane shows the label, the
description, type words and unit, the value in the file when it differs
with **Reset to file value**, the effect sentence, the read-only reason,
then, under **Developer details**, the path for `tag.set`, the offset,
the address, the bytes and the evidence text. With no row selected it
shows the tag's resources as built.

### Save and live

Two facts, kept apart everywhere:

- **Saved** is about the map file. An edit changes the document and is an
  undo step ([editor.md](editor.md), "The document"). The file changes on
  Save, Save as... or Save as package.... A changed field has a dot until
  the map is saved; the header counts them (`3 unsaved changes`).
- **Sent** is about a running game. It needs a game connected on the same
  map with Edit on ([live-mirror.md](live-mirror.md), "How an edit
  applies"). Sending does not save, and saving does not send. A change
  sent to a game is gone when that game exits unless the map was saved.

Undo and conflicts follow [live-mirror.md](live-mirror.md), "Undo and
sync". The Pending tab's headings are the effect sentences, and each row
reads `tag · breadcrumb · reason`, without addresses.

### The designed panel

The same tag, a game connected with Edit on, one flag changed (invented
values; the labels assume the fixture's definition has titles and a
unit):

```text
Weapon  blaster                          [Export...] [Import...]
objects\test\blaster · 1 unsaved change
Game connected. Changes are sent as you make them.
Fields with no mark are sent at once; when the game reads them is
not known. Reload the map to be sure.
Weapon blaster
[ Search fields          ] [ ] Changed only  [ ] Show unnamed fields
----------------------------------------------------------------
  Kind             [ Rifle                ]
* Flags            2 of 2 on: Loud, Quiet  [v]      Sent · Next use
  Range            [   0.4500 ] world units
  Model            Model  blaster    [Open] [Change...]
v Parts            2 items   [+ Add]                  After reload
    0  barrel      [Open alone] [Duplicate] [Remove]
    v 1  scope     [Open alone] [Duplicate] [Remove]
        Shape      [ Box                  ]
        Owner      No tag            [Open] [Change...]
      > Unnamed fields (2)    2 values
> Unnamed fields (3)    3 values
```

With no game the two `Game` lines are one, `Changes are saved with the
map.`, and no row has a chip.

## Panels on short windows

This rule covers every studio panel, not only the Tags tool (**Design**):

1. **A panel scrolls as a whole.** A panel has one scroll region, its
   body. Headings, lists, trees and tables draw straight into it, and
   long lists clip with `ListClipper` against that one scroll.
2. **A list never gets the leftover height.** No `child_window` is sized
   by what is left under other content, and none by a fraction of it. Two
   lists in one panel are two folding headers in the one body, their
   open state saved with the layout.
3. **Filters and commands sit at the top.** A filter covers every list
   under it. The buttons that act on a list come before it. At most two
   fixed rows of them may stay pinned above the body.
4. **A list with its own scroll has a height from its rows.** Only a
   dialog or a popup has one: `min(rows, cap)` rows, the cap clamped to
   the viewport.

`theme::panel_body` and `theme::bounded_list` (new) are the only callers
of `child_window` in `ui/`. A panel whose whole content is one surface
(the log, the map grid, a preview picture) is a body with nothing above
it and passes rule 2.

The built panels against the rule (**Code**; each is named by its child
window id or function, and `rg -n 'child_window|content_region_avail' crates/apps/studio/src/ui crates/apps/studio/src/ui.rs`
lists them):

| Panel | Built | Fails | Fix |
|---|---|---|---|
| UI Screens outliner, `ui_screens::tree` | `##ui-retail` takes what is left under the Our screens tree; its filter sits between the two lists | 2, 3 | one filter at the top, both lists under folding headers, no child |
| HUD outliner, `hud::tree` | `##hud-list` is a third of what is left, clamped to 48 to 220 points; `##hud-widgets` takes the rest | 2 | HUDs and Widgets as folding headers in the body; a filter over both |
| Scene outliner, `Studio::outliner_window` | `scene-layers` (`layers::section`) is half of what is left; `##outliner-tree` takes the rest; the filter sits under Layers | 2, 3 | filter and the Duplicate and Delete row pinned; Layers a folding header in the body, drawn at its content height; no tree child |
| Tags fields, `tags::fields` | fixed: no child; the header scrolls with the rows in the window | | when the filter row lands (T7), it is the only pinned row, over a `theme::panel_body` |
| Tags tree, `tags::tree` | no child, but the tag filter is inside All tags, under the scenario tree, and Not reachable from the scenario is last | 3 | the filter at the top over all three sections; each section a folding header |
| Data and bytes, `tags::hex` | fixed: Field details, then Bytes, then Data and resources, each a folding header | | |
| Pending, `PlayUi::pending_window` | Send, Reload map and Respawn now come after every pending row | 3 | the buttons first, then the rows |
| Variant pages, `variants::settings` | `##variant-pages` takes what is left under a header of four to six lines | 2 | the header scrolls with the pages |
| Variants list, Playlists list, Halo 1 outliner | `##variant-list`, `##playlist-list`, `##h1-outliner-tree` take what is left under one to four fixed rows | 2 (mild) | `theme::panel_body`; the fixed rows are the pinned rows |
| Command palette, `palette.rs` | `##palette-results` is always seven rows tall, whatever the window | 4 | `theme::bounded_list` |

`##log-lines`, `##map-grid` and `##bsp-bytes` are whole-surface bodies
and pass. The field panel's pickers and the search hit list of this
design use `theme::bounded_list`.

## Interfaces

### Where names come from

Names, descriptions, units, titles and effects live in
`crates/formats/tags/src/defs/*.txt`, each with its evidence in the
line's meaning text ([tag-definitions.md](../formats/tag-definitions.md),
"Where the names come from"). The sources, in order of weight:

1. **E**: the engine's reader, by guest address.
2. **V**: a check on the owner's disc.
3. **A Halo2Xbox `<file>`**: the retail Halo 2 Xbox plugin of Assembly,
   read as a reference under the permission and at the revision recorded
   in [notes/tag-field-names.md](notes/tag-field-names.md). A name is our
   own `snake_case` word and a description is our own sentence. No plugin
   file, name list, option list or comment text is committed.
4. **D**: a choice made here.

Projects built from leaked builds are not a source for anything.
`tag_names::assembly` refuses a plugin folder of any build but retail.

A lane adds names to one group like this:

1. `tag.names {"group": "weap"}` lists the group's unnamed fields by
   offset, largest lists first.
2. With a local checkout of the reference outside the repository,
   `H2_FIELD_NAMES=<dir> cargo test -p tag-names -- --ignored
   every_plugin_merges_against_our_definitions` prints, per group, the
   fields the reference describes and ours does not, and every
   disagreement in width, stride or type.
3. For each field whose offset and width agree, write the line in
   `defs/<group>.txt`: our name, `doc="..."`, `unit=`, and `-- A
   Halo2Xbox <file>` after any E or V evidence. Fields tile the span
   they replace exactly. A disagreement keeps our layout and gets a row
   in the note's table.
4. `cargo test -p tags defs registry` checks tiling, agreement with the
   schema and each attribute's words.
5. Add the group's counts to the note.

Evidence **Code**: the note records that the retail reference has no
plugin for weapons, equipment, scenery, machines and crates, and none for
14 schema groups. Their names need E or V evidence, or a source the owner
permits (see the report that accompanies this design).

### The editor's socket API

Every behaviour above is reachable through `api/tag_fields.rs`, listed
by `session.methods` and documented in [agent-api.md](agent-api.md).

| Method | Params | Result |
|---|---|---|
| `tag.fields` (extended) | `{tag, path?, unnamed?}` (`unnamed` default true) | each row gains `label`, `named`, `type`, `unit`, `editable`, `read_only`, `edited`, `file_value`, `effect: {id, text, why}`, `live`, `path`, `choices` or `bits` or `groups`; a `List` row gains `count`, `capacity` |
| `tag.elements` | `{tag, path, filter?, offset?, limit?}` | `{count, capacity, elements: [{index, title, path}], truncated}` |
| `tag.find` | `{tag, query?, path?, changed?, unnamed?, limit?}` | `{hits: [{path, breadcrumbs, label, type, value, matched}], truncated, why}` |
| `tag.choices` | `{tag, path, query?, kind?, limit?}` | what the picker lists: `{values: [{value, name}]}`, `{bits: [...]}` or `{kinds, tags: [{id, group, name}], truncated}` |
| `tag.bytes` | `{tag, path?}` (an element or a field) | `{address, size, hex, file_hex, spans: [{offset, size, name, named}]}` |
| `tag.reset` | `{tag, path}` | `{value}`; one undo step that writes the file's bytes back |
| `tag.view` | `{tag?, path?, field?, search?, changed?, unnamed?, show_as?}` (all optional; none reads) | `{tag, path, breadcrumbs, field, search, changed, unnamed, rows, unnamed_hidden, header: [string]}` |
| `tag.names` | `{group?}` | per group: `{title, named, unnamed, described, with_unit, with_effect, unnamed_fields: [{path, type, size}]}` |
| `mirror.pending` (changed) | | a row's `class` is replaced by `effect` (an id of the table above, or `object`) and `text` |

`tag.view` drives the panel itself: it sets the element shown, the
selected field, the search and the two toggles, and returns what the
panel then shows, `header` being the header's lines as drawn. A test
asserts the UI through it without reading pixels. `nav.go` and
`nav.status` carry the element path with the tag.

`tag.set`, `tag.read` and `tag.block.*` keep their paths: a label is
never a path token.

## Rules and limits

- No row shows a transport word. `Poke` appears nowhere in the UI, the
  API or the Pending tab.
- No effect is shown with no game connected.
- Unnamed is never silent: the folded row gives the count and what is
  inside; search finds its fields; a change, a Pending mark or a focus
  opens it.
- Unknown is never guessed: a field with no `applies` is `not_known` in
  the API, in details, in the header and on a changed row. The UI does
  not infer an effect from a field's type or group.
- Read-only always has a sentence: the tag-level reason in the header
  (`TagDoc::refusal`), a lock with the reason on an internal value
  (`tag_doc::STRUCTURAL`, reworded: `Set by the editor. Add or remove
  list items to change it.`), and `Edited in Scene` with **Open in
  Scene** on the scenario's placement lists.
- A search or a picker that stops at a budget says so and says what to
  do.
- Every definition attribute has a closed word list and its evidence. A
  unit or an effect written without evidence fails review, as a name
  does.
- Every panel follows "Panels on short windows": one scrolling body, no
  list sized by the leftover height, filters and commands at the top.
- Copy is sentence case, one space between words, no double spaces
  around marks, `·` with one space each side, `...` only on a button
  that opens a dialog. `headless` tests compare the strings exactly.
- Fixtures are invented (`app/tag_tests.rs`, `map::build::TagSet`). No
  test or doc uses a retail tag name or value.

Known gaps this design leaves: data references edit as hex in place, as
built; the scenario tag's fields outside placements stay read-only; list
indices held by other tags are not updated when elements move
([editor.md](editor.md), "The document").

## Slices

A lane takes them in order. Each slice updates the docs it changes
([editor.md](editor.md) "The windows and the panel",
[agent-api.md](agent-api.md), [live-mirror.md](live-mirror.md) "How an
edit applies", [tag-definitions.md](../formats/tag-definitions.md)).
Paths are under `crates/apps/studio/src` unless they start with `crates`
or `docs`.

| # | Slice | Size | Depends | Files | Tests | Accepted when |
|---|---|---|---|---|---|---|
| T1 | **Effect words.** `tag_sync::Effect` replaces the class strings; pending rows carry `effect` and `text`; the Pending tab uses the sentences | S | none | `play/tag_sync.rs`, `play/edits.rs`, `play/issues.rs`, `play/playtest.rs`, `play/objects/drive.rs`, `ui/play.rs`, `ui/inspector.rs`, `api/mirror.rs`, `api/pair.rs` | `tag_sync` unit test: each `applies` word and each reload rule gives its id, chip and sentence; `api::tests`: `mirror.pending` rows have `effect`, none has `class` | no string containing `Poke` is left in `crates/apps/studio` |
| T2 | **Row model.** `fields::rows`; one-line rows; labels derived; type words; edited dot; chip only when connected; `tag.fields` extended | M | T1 | `fields.rs`, `fields/row.rs` (new), `api/tag_fields.rs`, `app/tag_api.rs` | `fields::row` tests on an invented layout: one row per field, labels, type words, read-only reasons; `api::tests`: `tag.fields` rows equal the panel's; headless: no chip unpaired, chip with the fake game connected | the panel draws nothing `tag.fields` does not return |
| T3 | **Unnamed fields.** The folded group, its summary, auto-open rules, padding inside it, the toggle, `Show as` picker, `tag.view` | S | T2 | `fields.rs`, `fields/row.rs`, `ui/tags.rs`, `api/tag_fields.rs`, `crates/apps/studio-proto/src/lib.rs` | row tests: grouping and summary counts; headless: the group is folded, opens on a changed unnamed field and on focus; `tag.view {"unnamed": true}` raises `rows` by `unnamed_hidden` | a tag with no definition shows the named line and its fields |
| T4 | **Definition attributes.** `label`, `unit`, `axes`, `title`, inherited `applies`; `Field::label`, `Field::unit` | M | none | `crates/formats/tags/src/defs.rs`, `crates/formats/tags/src/registry.rs`, `crates/formats/tags/src/defs/tests.rs`, `docs/formats/tag-definitions.md` | parse and round-trip of each attribute; a refused word for each; inheritance with a field's own value winning; an invented group through the registry | `defs` text round-trips byte for byte; no retail definition changes meaning |
| T5 | **Lists.** Element rows with titles, one open at a time, the filter, Open alone, breadcrumbs, element paths in `nav.*`; `tag.elements` | M | T2, T4 | `fields.rs`, `ui/tags.rs`, `app/navigation.rs`, `api/nav.rs`, `api/tag_fields.rs` | `tag.elements` titles from `title=`, from the first name field, and bare; headless: open element 1, Open alone, the breadcrumb returns; Back restores the element | the `element` number box is gone |
| T6 | **Pickers.** The modal picker for choices, references and `Show as`; switches summary and unnamed bits; `tag.choices` | M | T2 | `fields.rs`, `fields/picker.rs` (new), `api/tag_fields.rs` | headless: pick an enum value and a tag through the dialog, set an unnamed bit, enter a number outside the names; `tag.choices` equals the dialog's list; a 5,000-tag invented map lists without a cap | no `begin_combo` is left in `fields.rs` |
| T7 | **Search and Changed only.** The filter row, the hit list, the budget message; `tag.find` | M | T2, T5 | `fields.rs`, `fields/find.rs` (new), `ui/tags.rs`, `api/tag_fields.rs` | `tag.find` by label, description, choice name and referenced tag; `changed` after an edit and after its undo; the budget on an invented 6,000-element list gives `truncated` and `why` | choosing a hit focuses the field inside nested lists |
| T8 | **Details and bytes.** The pane follows the selected row; Reset to file value; Show bytes; `tag.bytes`, `tag.reset` | S | T2 | `ui/tags.rs`, `fields.rs`, `app/tag_api.rs`, `api/tag_fields.rs` | `tag.reset` is one undo step and clears `edited`; `tag.bytes` spans tile the element; headless: the pane keeps its field while the mouse moves | `PanelState::hovered` no longer drives the pane |
| T9 | **Header and status.** Kind title, name, unsaved count, the link lines, `Sent`, `Waiting`, `Not sent` on rows; developer facts move to details | S | T1, T2 | `ui/tags.rs`, `fields.rs`, `play/issues.rs` | headless with the fake game (`play/testing/fake`): the three link states give the three header lines; a changed `not_known` field reads `Sent · Not known`; `tag.view` `header` equals the drawn lines | no address or datum is on screen until Developer details is opened |
| T10 | **Names for a group** (repeatable, one group per run). `tag.names` lands with the first run | S each | T4 | `crates/formats/tags/src/defs/<group>.txt`, `docs/editor/notes/tag-field-names.md`, `api/tag_fields.rs` (first run) | the `defs` and `registry` suites; `tag.names` counts on an invented group | the group's unnamed count falls and the note has its row |
| T11 | **Effects and units for a group** (repeatable) | S each | T4 | `crates/formats/tags/src/defs/<group>.txt`, `docs/graphics/hot-iteration.md` | the `defs` suite; each `applies` line cites its reader | the group's `with_effect` count rises |
| T13 | **Short windows.** `theme::panel_body`, `theme::bounded_list`; each row of the table in "Panels on short windows" fixed (the UI Screens row is fixed apart from this slice) | M | none; after T2 for `tags::fields` | `theme.rs`, `ui.rs`, `ui/hud.rs`, `ui/layers.rs`, `ui/tags.rs`, `ui/play.rs`, `ui/variants.rs`, `ui/playlists.rs`, `ui/palette.rs`, `probe.rs`, `headless.rs` | `probe` records each child window's id and size; headless `every_panel_scrolls_as_a_whole_on_a_short_window`: each tool drawn at 1000 by 420 has no child but a body or a bounded list, and every filter is above the first list row; a source test fails on `child_window(` in `ui/` outside `theme.rs` | at 1000 by 420 every list of every tool is reachable by scrolling its panel |
| T12 | **Guide.** Section 9 of the guide rewritten to the panel; scripted screenshots regenerated | S | T3, T5 to T9 | `docs/editor/editor-guide.md`, `docs/images` (Git LFS) | `scripts/docs-check.py docs/editor` | the guide names no control that is gone |

T4 touches only the formats crate and can run beside T1 to T3. T6, T7
and T8 are independent of each other once T2 (and for T7, T5) is in. T13
is independent of the tag slices except for `tags::fields`.

## Evidence

- **Code**: the counts in "The built panel" are greps over
  `crates/formats/tags/src/defs/*.txt`: field lines `^\s+0x[0-9a-f]+ `,
  `unknown` spans, `applies=`. They count definition lines, not registry
  fields; `coverage_counts_every_byte_once` in
  `crates/formats/tags/src/registry/tests.rs` prints the registry's own
  totals.
- **Code**: the two-line row is read from the call order in
  `fields.rs` `Cx::label`.
- **Design**: the chip rule for `not_known`, the type words, the unit
  list, the label derivation, the search budget and all copy.
- **S**: that titles from the first name or reference field read well on
  retail lists. A look at five groups on the owner's disc through
  `tag.elements` verifies it.

## Not verified

- Whether the folded group leaves any retail group with nothing useful on
  screen. `tag.names` over the owner's disc shows it per group.
- Whether a picker over every tag of the largest map opens without a
  visible pause.
- Which fields the engine caches at map load although their type suggests
  otherwise. Each `applies` line needs its reader; until then the field
  is `not_known`.
- Whether one line per row fits the longest labels at the inspector's
  narrowest docked width.

## Related

- [editor.md](editor.md): the Tags tool as built, the registry and the
  document.
- [live-mirror.md](live-mirror.md): how edits reach a game, Pending, undo
  and conflicts.
- [click-to-edit.md](click-to-edit.md): the inspector that shares this
  panel.
- [polish-plan.md](polish-plan.md): P12, P13 and P18, the slices this
  design builds on.
- [agent-api.md](agent-api.md): the socket API.
- [tag-definitions.md](../formats/tag-definitions.md): the definition
  file format and its evidence rules.
- [notes/tag-field-names.md](notes/tag-field-names.md): the name
  reference, its permission and the mapped groups.
