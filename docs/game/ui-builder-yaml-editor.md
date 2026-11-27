# UI builder: screens in YAML, design system and editing

Screens in YAML, the design system, the layout gallery, editing in the editor and retail-screen fidelity. Part of [ui-builder.md](ui-builder.md).

## Screens in YAML

A screen's presentation is data in `content/ui/`; its Rust is behaviour
(`crate::ui_kit::data`):

- `content/ui/design.yaml` is the design system: the palette (named
  colours), fonts by role (`title`, `legend`, `body`, `cell`,
  `row_controls`, `row_default`), spacing (row heights, the dialog body's
  top, the code cells' gaps), alphas (an unfocused row's text, a dimmed
  frame), the legend's format, and every component's preset (role, font,
  colour, justification, place, variants and specimen page).
- `content/ui/screens/<name>.yaml` is one screen of
  `ui_kit::screens::NAMES`: `screen`, `kind` (`list`, `dialog`,
  `code_entry`, `picker`), `layout` (a list's `controls`,
  `level_select` or `moved`; a dialog's `settings` or `large`), `backing`
  (`shell`, `over` an `opener` such as `main-menu`, `screen` over a screen
  of ours, or `none`), `scrolls`, its named `states` (the focus and a
  code's cursor), `widgets` keyed by view id (`component`, `text` with
  `{slot}` templates Rust fills, `at`, `size`, a legend's `prompts` such
  as `[[A, "SELECT"], [B, "BACK"]]`, cells' `digits` and `gap_after`, an
  art widget's `art`), and fixed `rows` (`row.0: {text, ...}`). A
  settings dialog has a `settings:` block and its rows' `help`,
  `setting`, `bind`, `explain` and `open` ("The settings dialog").

Both are baked into the game at compile time (`build.rs` writes
`ui_baked.rs`, `ui_kit::data::baked`) and parsed once a process, the first
time a doc is asked for (`data::baked_parses`). The game never reads them
from disk. The editor shows an edited or reloaded doc over the baked one
through the explicit `data::reload` (validated first) until
`data::forget_reloads`.

A doc that names what doesn't exist (a component, a glyph, a layout for
its kind, an opener, a slot that isn't closed, a value row's current value
past its values, an icon, a setting) or breaks a settings dialog's rules
is a `DataError` naming the file and the field
(`content/ui/screens/t.yaml widgets.title.component: ...`), never a quiet
fallback; a screen without a doc is one too (`data::require`). The tests
load every doc, so a bad one fails them by name.

`data::to_yaml` writes a doc in its canonical form: keys in order,
two-space indents, texts double-quoted, pairs and prompts in flow, no
comments. A load and a save with no edit gives the file's bytes.

**Which screens draw from their docs.** `data::FROM_DOC` lists them: the
kit screens (`KitScreen::new(feature, name, ids)` reads the title, the
legend, the kind, layout and opener, the description block, `scrolls`
and the fixed rows' labels; docs/game/ui-screens.md "Adding a screen"),
and the settings dialogs (`settings::screen`, "The settings dialog").
Every other screen's doc is written from its Rust and held to it: the
equivalence test (`data::tests::each_state_drawn_from_its_doc_is_the_view_rust_drew`)
draws each state of each screen from its Rust alone and with its doc laid
over (`data::present`), and the two views must be equal.

**The converter.** `h2 ui export-yaml [--out content/ui]`
(`data::export::export`) builds every screen in every state, reads its
presentation off the specs, and writes `screens/<name>.yaml` and
`design.yaml`, starting from the `design.yaml` already there. A style
that matches a preset is written as the preset's name; one that matches
none gets a preset named after the screen (`description.kit.example.0`),
and a colour on no palette entry a colour named after it
(`theater.playback.0`): a name with a dot is what the converter found,
not the design system's, and the consistency check reports it. A text
that is the same in every state is written as it is; one that changes,
or that `data::export::DYNAMIC` names (a film's title, the card's profile
line), is a lone slot (`{title}`) Rust fills. Rows are written for the
screens whose rows are fixed (`data::export::FIXED_ROWS`, the specimen
and layout pages). On a screen drawn from its doc the converter applies
the design system's rules too (`data::export::house_rules`: a dialog's
title in capitals); the others are fixed in their Rust. A doc with a
`settings:` block is its own source: the converter writes it back as it
loaded it. Run it after a
change to a screen's Rust, then the tests.

## The design system

`ui_kit::components` puts a screen together from the components:
`Screen::list`, `level_select`, `moved`, `large`,
`message`, `code_entry` and `picker`, with `row`, `legend`,
`legend_on_panel`, `description`, `help`, `text`, `art`, `status`,
`scrolls` and `over`; `build()` derives the inputs its rows and legend
need. `components::row` makes each kind of row (`action`, `open`,
`value`, `pick`, `info`, `text`, `disabled`) and `components::legend` the
common prompts and the legends by row kind (`setting_row`: `A CHANGE  B
BACK` on a row whose A opens the picker; `disabled_row`: `B BACK` alone).
A `disabled` row is its label alone, with nothing where a value goes, and
never pressed; its help line says why. On the level select's one-text
skin a `pick` row's value is in its label (`row.pick-inline`, the skulls
pages' rows). `legend::well_formed` is the legend's format: each
prompt's glyphs three spaces apart, one space, its words in capitals,
prompts two spaces apart. The settings dialog is the `settings`
component, built from its doc by `ui_kit::settings` ("The settings
dialog"), its specimen `layout.dialog.settings`.

A component's `pattern` names the Halo 2 pattern its widgets keep, and its
`ids` the widget ids it draws (`button.*`, a prefix and a star); the
consistency drive holds those widgets to the pattern (`button`, the only
one so far).

Each component of `design.yaml` with a `specimen` has its page,
`design.<component>` in `NAMES` (`design.title`, `design.row.action`,
`design.legend`, `design.cells` ...): its variants as the page's named
states, drawn and checked as any screen is. `h2 ui preview design.legend`
draws one; `h2 ui sheet` draws them all. The editor's agent API gives the
catalogue as `ui.components` (docs/editor/agent-api.md).

## The layout gallery

`layout.<name>` in `NAMES` is every layout a screen can take, with
placeholder copy so each reads as a template: `layout.list`,
`layout.list.description`, `layout.list.panel-edge`,
`layout.list.level-select`, `layout.list.texts`,
`layout.dialog.settings`, `layout.dialog.large`, `layout.dialog.message`,
`layout.dialog.over-screen`, `layout.code-entry` and `layout.picker`
(`ui_kit::components::specimens::PAGES`). A new screen
starts from the one it looks like.

## Consistency

`ui_kit::consistency::check` draws every screen of `NAMES` in every
named state without a disc and holds it to the design system:

| Rule | What it checks |
|---|---|
| `palette` | a text's colour is a palette colour, or one dimmed in proportion (a lobby's unselected tab); a converter-named colour is drift |
| `font` | a title and a legend in their role's font, a description, a dialog's body and status in the body font, code cells in the cell font, row texts in a row font (HUDs are their own) |
| `place` | a title and a legend where most of their kind's are (a list by its skin, a dialog by its base) |
| `legend` | the legend's format (`legend::well_formed`) |
| `case` | a dialog's, a code entry's and a picker's title in capitals (`DYNAMIC` titles aside) |
| `pitch` | a list's rows its skin's row height apart (`row_controls`, `row_default`) |
| `fit` | a title and a legend fit their rectangles |
| `items` | a list's item widgets are the rows it lays out (`list_class::item_widgets`), so no row is on a widget never placed, and no list draws more rows than it lays out ("Item count") |
| `scroll` | a scroll arrow shows only on a list with more rows than it lays out, inside a dialog's panel or, on a screen, inside the screen and the title-safe area's height ("Scroll arrows") |
| `empty` | a settings dialog's header lines and its help under the rows say something in every state, and so does any dialog's description area above its list; a page with nothing to say there has no description block, so its rows sit under the title |
| `settings` | a settings dialog drawn at every focus, not only its named states: its top (title, icon, name, about) the same at every focus; the about saying something and no row's label, help or explain; the help under the rows the focused row's `help`, two lines at most, below the last row and above the legend ("The settings dialog"). That A opens a picker saying the row's help is `settings::tests::a_rows_picker_carries_its_help`: the drive presses nothing, since a press sets the process's pick |
| `help` | a screen with value rows has no page-wide line (`ScreenSpec::help`) where the focused row's help belongs |
| `button` | a text whose component has `pattern: button` (its `ids` name the widgets: the lobby's `button.*`) is drawn on a list skin's art or a row's (`Art::Skin` or the `Under` layer under at least half its ink), as retail draws its buttons, never as bare text |
| `handled` | every prompt the legend shows (the focused row's, else the page's) is an input the page takes (HUDs aside) |
| `arrows` | no text draws a value between arrows (`< All >`, or an arrow alone): a value is chosen in the picker A opens |

`consistency::view_findings(spec, view)` runs the rules that read one view
(all but `place` and `settings`), which `ui_kit::findings::of_state` adds
to a state's findings; `consistency::settings_check(name)` runs the
`settings` rule on one screen.

`consistency::ALLOWED` is the short list of findings that stand, each
with its reason (the sign-in dialog's title carries the gamertag as
typed). The test `consistency::tests::every_screen_keeps_to_the_design_system`
fails on any other finding. A finding is fixed as a converter rule for a
screen drawn from its doc, or in the screen's Rust, then
`h2 ui export-yaml` rewrites its doc. The editor's agent API gives the
findings as `ui.consistency {screen?}`.

## In the editor

The editor's fourth tool is **UI Screens**
(docs/editor/editor.md "UI Screens", editor-guide.md §7). It shows every
native screen in every state and the disc's retail screens, drawn from the
editor's disc (synthetic without one) with zoom; widget rectangles with
the hovered widget's JSON; the checks' findings with their widgets
outlined; the title-safe and reserved areas, each togglable; a pad that
steps the `Page` live; and export of the PNG, the JSON and the sheet into
a folder outside any Git checkout. The studio takes `boot` as a library
for it: the pages step the screens' real classes, which can't leave
`boot`.

## Editing in the editor

Status: Partly built. The code uses `ui_kit::data` (`ScreenDoc`, `load`,
`parse`, `to_yaml`, `with_screens`), which arrives with the YAML screens,
and it has not been compiled or run in the editor yet.
Code: `crates/apps/studio/src/app/ui_screens/edit.rs` (`edit/ours.rs`,
`edit/retail.rs`, `edit/retail_tag.rs`),
`crates/apps/studio/src/ui/ui_screens/edit.rs`,
`crates/apps/studio/src/ui/ui_screens/transport.rs`,
`crates/apps/studio/src/api/ui_edit.rs`,
`crates/apps/studio/src/api/ui_retail.rs`,
`crates/extensions/game/src/ui_kit/preview.rs` (`Disc::retail_view`,
`RetailPlaces`, `retail_field`, `filmstrip`),
`crates/extensions/game/src/ui_kit/preview/retail_edit.rs`
(`RetailEdits`, `RetailBlocks`, `parked`),
`crates/extensions/game/src/ui_kit/animation.rs` (`Transition`).
Tests: `app::ui_screens::edit::tests`,
`app::ui_screens::edit::retail_tag::tests`, the `ui_edit` and
`ui_retail` tests in `crates/apps/studio/src/api/tests.rs`,
`ui_kit::preview::edit_tests`, `ui_kit::preview::retail_edit::tests`.

The UI Screens tool edits the picture it shows: click a widget to select
it, drag it, resize it, or double-click a text to retype it. The agent
API does the same with `ui.move`, `ui.set_rect` and `ui.set_text`. Every edit
changes the data the game reads. It is never a change to the preview
alone. Where that data lives depends on the screen.

```text
            canvas drag / handle / inline text / inspector / ui.* call
                                   │
                          edit::EditAction
                                   │
             ┌─────────────────────┴──────────────────────┐
             ▼                                            ▼
  ours: ScreenDoc fields                    retail: tag fields of the open
  (content/ui/screens/<name>.yaml)           mainmenu map (App::tags writes)
  text, at, size by widget id               rect, top_left, anchor by widget id
             │                                            │
  with_screens(edited docs) ─▶ View          RetailPlaces ─▶ Disc::retail_view,
             │                               Disc::render_retail_with
             ▼                                            ▼
        layout and content checks run on the edited view; ui.render shows it
             │                                            │
  Save: to_yaml into <checkout>/content/      Save: the map's own save
  ui/screens/<name>.yaml (live-reloaded)     (patches with the user's map)
```

**One edit model, two sinks.** An edit is `Rect(widget, [top, left, bottom,
right])` or `Text(widget, string)` in UI units (`edit::Change`). A move
is a rect: the shown rect plus the step. The tool picks the sink from the
pick:

- **Our screens** (`edit::ours`). Presentation lives in
  `content/ui/screens/<name>.yaml`, one `ScreenDoc` per screen
  (`ui_kit::data`). The sink holds the edited docs, the docs as last loaded
  or saved, and an undo stack of whole-doc snapshots. A rect sets the
  widget's `at` (its top-left, `[left, top]`) and `size` (`[width,
  height]`). A text sets `text`, or `rows.<row id>.text` for a row's
  label (`row.N.label`). The view is made with the edited docs standing
  in for the baked ones (`data::with_screens`), so the preview, the checks,
  `ui.widgets`, `ui.render` and `ui.animate` show the edit at once.
- **Retail screens** (`edit::retail`). A retail screen is a `wgit` tag of
  the user's `mainmenu.map`. Its edits are tag field writes through the
  map's tag document (`App::tags`, `tag_api::set_field`, `TagAction::Write`).
  That is the path the Tags tool uses, with its undo and its save, which
  patches the user's map. Nothing disc-derived goes in the repository. The
  map open in the editor must be `mainmenu`. Otherwise the sink refuses
  and says so. The picture is drawn from the disc, then the edited fields
  are read back as `RetailPlaces` (widget id to rect) and the pane's
  text, button and bitmap blocks as `RetailBlocks`, and applied before
  drawing (`Disc::render_retail_edited`). "Retail tag edits" has the
  edits past a rect.

**What can be edited, and the refusals.** A refusal is an error that
names the owner. It is never a silent no-op.

| Widget | Ours | Retail |
|---|---|---|
| a widget with an entry in its YAML (`title`, `body`, `legend`, a panel text, art) | rect (`at`, `size`), text | - |
| a text block `text.N`, a button `button.N` | - | rect (`panes[0].text_blocks[N].rect`, `buttons[N].rect`) |
| a bitmap `bitmap.N`, a list `list.N`, a table `table.N` | - | move only (`top_left`, `anchor`): the image or skin sets the size |
| a row label `row.N.label` | text (`rows.row.N.text`) | - |
| a settings dialog's `header.name`, `header.about` | text (`settings.name`; `settings.about`, or `settings.caller.help` when the page takes its about from its caller) | - |
| a settings dialog's `description` | text: the help of the row it shows (`rows.row.N.help`) | - |
| a settings dialog's `header.*`, `art.*`, `description`, moved | refused: `design.yaml` `components.settings.*` places it | - |
| any row widget, moved | refused: rows sit where the list's `layout` puts them | - |
| a widget with no YAML entry (`row.N.value`, `description`, `status`, `cell.N.*`) | refused: Rust fills it, and the refusal names the kind of value | - |
| a text with `{slots}` | the template is edited, and its slots must stay as they are (the refusal names a slot that went or came) | - |
| a legend made of `prompts` | text refused: edit its prompts | - |
| any retail text | - | text refused: retail strings live in the map's localized string table (`unic`), which the editor does not write. Its string ID is set with `ui.set_string` |
| `title` of a retail screen | - | refused: the header rectangle is the shared `wigl` header layout's |

**Undo.** Edits are undo steps of the editor's Undo and Redo while the UI
Screens tool is shown (`App::undo_redo`). Ours have their own stack. A
retail edit is a step of the map's tag document. A drag or a run of
nudges merges into one step, and so does typing in an inspector field
(`merge`).

**Saving.** Ours write `data::to_yaml(doc)` to
`<checkout>/content/ui/screens/<name>.yaml` for every edited screen. The
editor finds the first checkout at or above the working directory, then
above the executable, that has `content/ui/package.toml`. Save requires
the screen to be listed in that package's `screens` inputs. The game
bakes the package when it is built. The editor
polls that folder (once a second) and reloads a file that changed on disk
when its screen has no unsaved edits. A screen with unsaved edits keeps
them, and the tool says the file changed. Retail edits save with the map
(`doc.save`, File > Save).

**Shipping a retail edit.** A retail edit saved with the map is the
user's alone, since the map is disc-derived. `ui.export_edits {path,
screen?}` writes the open map's retail edits as retail edit files, one
per edited screen pane. A `screen-edits` item in `content/ui/package.toml`
applies the named file to the user's map at boot ([ui-in-maps.md](ui-in-maps.md), "Retail
edits as data").

**Checks on the edited view.** `UiScreens::draw_native` makes the view
inside `with_screens`, so `layout::check` and `content::check` run on the
edited rects and texts. A drag past the title-safe area shows its finding
on the next frame.

**Canvas interaction** (`ui/ui_screens/edit.rs`). Click selects the
topmost visible widget. A selection draws an outline and, on a resizable
widget, eight handles. A drag moves it, and a handle resizes it. Moves snap
to the 640 by 480 unit grid and to other widgets' edges and centres within
4 units. Hold Shift to place freely. Arrow keys nudge by 1 unit, with
Shift by 10. A double-click on a text opens an input over its rect: Enter
commits and Esc cancels. The inspector's **Selected** section shows the
id, the rect as numbers you can type, the text, the font and the
justification. Tint, alpha and layer are shown read-only. It also has
**Copy JSON**, **Revert widget** and **Revert screen**. The widget list
lights a widget in the picture while you hover its row.

**Animation preview** (`ui/ui_screens/transport.rs`, `animation::Transition`).
The kit defines two transitions, **Open** (`Phase::In`) and **Close**
(`Phase::Out`). Focus moves and page changes play the skin's native
tracks in game, and the builder does not sample them. The transport plays,
pauses and loops one transition, and you can scrub its timeline by tick.
The speed runs from 0.25 to 2 times, at the builder's 30 ticks a second
(`animation::TICKS_PER_SECOND`); at 1 times an open plays its 333 ms
and a close its 200 ms, the lengths the game holds a screen of ours to
("Transitions"). An optional onion skin
outlines the widgets two ticks before and after. `ui.animate {phase,
frames?, out?}` writes a filmstrip PNG, the frames side by side at half
size, each labelled with its time (`preview::filmstrip`), and returns
`{path, frames: [{t, tick, x, w}]}`.

**Agent API** (`api/ui_edit.rs`, docs/editor/agent-api.md). Each method
works on the screen `ui.pick` shows, of either kind, and runs the same
`EditAction` as the window, so its edits are the window's undo steps and
its save. Rects are `[top, left, bottom, right]` in UI units.

| Method | Params | Result |
|---|---|---|
| `ui.select` | `{id?}` (none or null: nothing) | `{selected, widget}` |
| `ui.move` | `{id, dx, dy}` | `{widget, findings, sink, dirty}` |
| `ui.set_rect` | `{id, rect}` | `{widget, findings, sink, dirty}` |
| `ui.set_text` | `{id, text}` | `{widget, findings, sink, dirty}` |
| `ui.hide` | `{id}` (retail) | `{hidden, widget: null, findings, sink, dirty}` |
| `ui.duplicate` | `{id, rect?, grow?}` (retail) | `{copy, of, widget, findings, sink, dirty}` |
| `ui.set_string` | `{id, string, shows?}` (retail) | `{string, widget, findings, sink, dirty}` |
| `ui.revert` | `{id?}` (none: the whole screen) | `{line, edits}` |
| `ui.undo`, `ui.redo` | `{}` | `{done, can_undo, can_redo, edits}` |
| `ui.edits` | `{}` | `{sink, selected, ours: [{screen, file, yaml}], ours_dirty, retail: {tag: {id: rect}}, retail_blocks: {tag: {added, edited, header}}, stand_ins: {string: text}, retail_dirty, checkout, changed_on_disk}` |
| `ui.save` | `{}` | ours `{sink, written: [path]}`; retail `{sink, save}` (`doc.save`'s answer) |
| `ui.animate` | `{phase: open\|close, frames?: 1..32 (5), out?}` | `{path, width, height, frames: [{t, tick, x, w}]}` |

A refusal answers `REFUSED` with the owner named. A widget that is not
shown, a rect whose top is below its bottom or right left of its left,
and a phase that is neither `open` nor `close` answer `INVALID_PARAMS`.
`ui.animate` and the default output path follow `ui.render`: under
`studio/ui/` in the cache directory, never inside a Git checkout.

### Retail tag edits

Status: Built, not compiled or run in the editor yet.
Code: `crates/apps/studio/src/app/ui_screens/edit/retail_tag.rs`,
`crates/apps/studio/src/api/ui_retail.rs`,
`crates/extensions/game/src/ui_kit/preview/retail_edit.rs`,
`crates/formats/tags/src/ui.rs` (`TextBlock::from_element`,
`Button::from_element`, `BitmapBlock::from_element`).

A retail screen takes three edits past a rect. Each is a write of its
`wgit` tag in the open `mainmenu` map's tag document, one undo step,
saved with the map as a patch. They are for changing a retail screen
with its own parts: a line of text the screen already has, moved or
repeated, and named by a string of ours.

| Edit | Tag write | Refused for |
|---|---|---|
| Hide (`ui.hide {id}`, the Selected section's Hide) | the block's rect to top and bottom 9999 with its left and right kept, or a bitmap's `top_left` to `9999, 9999`: where the disc parks its own spare blocks (`preview::parked`) | a button (it keeps its place in the focus order wherever its rect is), a list, a table, the title (the tag's `no_header` flag hides it) |
| Copy (`ui.duplicate {id, rect?, grow?}`, Duplicate) | one element appended to `text_blocks`, `buttons` or `bitmaps`, then the copied element's bytes written into it | a list, a table, the title; a full block while Edit > Allow growing full blocks is off |
| Set string (`ui.set_string {id, string, shows?}`) | the string ID field: the screen's `header` for `title`, a text block's `text`, a button's `label`. A name the map lacks is added to it (`TagDoc::set_string`) | a bitmap, a list, a table; a name that is not lowercase letters, digits and `_` |

- **A copy goes to the end of its block.** The game's code finds a
  pane's widgets by block index (the lobby's panel update reads text
  blocks 4 to 8), so no block before the copy changes its index. The
  copy's widget id is its index: `text.48` in a pane that had 48 text
  blocks. `ui.duplicate` answers it as `copy`.
- **A hidden block is still built.** The game makes a widget for every
  block of the pane and draws nothing of one parked off screen, as it
  does for the disc's spares. `ui.revert {id}` shows it again.
- **A string of ours has no text in the map.** The localized text of a
  retail string is in the map's `unic` table, which the editor does not
  write. A string ID added by `ui.set_string` names a slot: the game's
  code answers it at run time (a text set on the widget, or our string
  table). The picture draws the stand-in given as `shows` for that
  string name (`UiEdit::stand_ins`, `RetailEdits::texts`). The stand-in
  is the tool's: it is in `ui.edits` and is not saved.
- **The picture follows the document.** The pane's text, button and
  bitmap blocks and the header string ID are read back whole from the
  tag document after every edit, undo and pick
  (`App::ui_retail_read_blocks`, `RetailBlocks::decode`) and drawn in
  place of the disc's (`RetailEdits::apply`). An edit saved with the map
  shows when the map is opened again.
- **Revert** (`ui.revert`) writes a block's element back as the loaded
  map has it, puts the title's string ID back and removes a copy; a
  copy removed from the middle moves the copies after it down one.
- **Pane 0 only.** The tool shows and edits pane 0 of a retail screen
  (`edit::retail::PANE`). A screen with more panes, as the pregame lobby
  has for its three retail modes, takes the same edit on each pane it
  should show on.

## Retail screens in a believable state

Status: Built. Not measured against the game (see "The fidelity loop").
Code: `crates/extensions/game/src/ui_kit/preview.rs`
(`Disc::render_retail_edited`, `Disc::retail_view_edited`,
`Canvas::text_boxed`),
`crates/extensions/game/src/ui_kit/preview/retail_sample.rs`,
`crates/extensions/game/src/ui_kit/preview/text_layout.rs`,
`assets/ui/retail-samples/`.

A retail screen's tag lays out every block the game might show. The
pregame lobby's pane has the two buttons the leader gets and, in the
same place, the line everyone else gets; a status line whose own string
is for a party with no session; sixteen speaker icons; six empty lines
for the variant's options; a table of sixteen player cells. The game's
code picks among them, writes the empty ones, replaces the substitution
units in the strings (docs/formats/ui.md "Substitution units") and adds
the rows of a list. A picture of the tag alone shows all of it at once,
with a square for every unit.

So the picture is drawn in a **sample state**: a file in
`assets/ui/retail-samples/` that stands in for the game's code on one
screen. `render_retail` and `retail_view` look the sample up by the
screen tag's path; every caller gets it (the UI Screens tool's picture,
its thumbnails, `ui.render`).

```yaml
screen: ui\screens\game_shell\pregame_lobby\pregame_lobby
tokens:                       # substitution unit (hex) to what it reads as
  e416: Kestrel
  e425: "4"
footer: { string: button_key_squad_leader_teams_disabled }
cells:                        # the tables' filled cells, first cell first
  - text: Kestrel
    blocks:
      ui\global_bitmaps\rank_icons_sm: { image: 14 }
panes:
  0:
    blocks:
      party_leader: { hide: true }
      game_cant_start_no_session: { string: game_start_when_you_get_off_your_ass }
      text.4: { text: "Score to Win: 50" }
      ui\screens\game_shell\pregame_lobby\speaker: { hide: true }
    lists:
      0: { focus: 0, rows: [{ string: change_map }] }
```

| Key | What it says |
|---|---|
| `variant` | the state's name among the screen's samples. A file without one is the screen's plain sample, the one every caller draws; a file with one is drawn only when a caller names it (`Retail::sample`, `ui.pick {retail, sample}`). Two files for one screen and variant, or two plain ones, fail to load |
| `over` | the screen tag this one opens over (a dialog's opener). That screen's pane 0 is drawn first, in its own plain sample state, in place of the game shell background, then dimmed as the game dims it ("A dialog is an overlay on its opener") |
| `tokens` | what each substitution unit reads as, for every pane |
| `footer` | the legend in the header layout's second rectangle |
| `cells` | the tables' filled cells: `text` is the cell skin's first text block, `blocks` its bitmaps, `emblem` the player's emblem. A cell with no entry is not drawn |
| `panes.N.blocks` | what the game does to a block of pane `N` |
| `panes.N.lists.M` | the rows the game's code gives list `M`, and the focused one, drawn on the list's skin from its anchor as a screen of ours lays its rows |

A block is named by its widget id (`text.4`, `button.0`, `bitmap.21`,
`title`), by the name of its string, or by its bitmap's tag path; a
widget id wins. What it takes:

| Field | Meaning |
|---|---|
| `hide: true` | the game does not show the block in this state. A hidden button's art goes with its text |
| `string: name` | the game writes another string of the screen's own table there. The text is read from the disc when the picture is drawn |
| `text: "..."` | the game writes text of its own making there |
| `image: N` | the image of the block's bitmap the game picks (a rank, a game type's icon) |
| `tint: [r, g, b]` | the colour the game draws the bitmap in (a player's colour on a white bar) |
| `level: stem` | the game puts a map's picture there in place of the block's bitmap (the lobby's map picture). `stem` is the map file's name without `.map`; the picture is the one the disc's level list gives that map (`Disc::art`, `Art::Level`). A map with no picture fails the render |

A cell's `emblem` is `{ foreground: N, background: M, colours: [...] }`:
an image of each of the disc's small emblem bitmaps
(`ui\global_bitmaps\emblems\foreground_small`, `background_small`) and
four colours, the foreground's two and then the background's two. It is
drawn in each shader bitmap of the cell's skin. The game moves a skin
item's shader bitmap up by its own height, as it does an item's bitmap
(docs/formats/ui.md "Screen and list classes (code, not tags)"), so the lobby's 32 by 32 rectangle has its
top on the bar's top. The art is the top-left 19 by 19 of its 32 by 32
image, which is the bar's height. Each layer is two masks, red and blue
(`emblem_rules` in `crates/apps/cli/src/live_art.rs`): the layer's
colour is its first colour by red plus its second by blue, and the
foreground covers the background by the larger of its masks
(`preview.rs` `emblem`). A pixel no mask covers is left clear. That last
rule is the preview's own: the `ui_small_emblem` shader is not read.

Rules:

- **Nothing of the disc is in a sample.** The names, numbers and options
  are made up. A retail string is named by `string:`, never quoted. A
  sample that names a string the screen's table lacks fails the render
  with the names (`State::missing`).
- **Without a sample, the game's blocks are hidden.** A string that holds
  a substitution unit is not drawn, nor is one with a unit the sample
  has no text for (`retail_sample::substitute`). What the tag says
  outright still shows.
- **One pane.** The picture is pane `Retail::pane`'s blocks alone; a
  sample's `panes.N` applies to that pane only.
- **The title and the legend are in the screen's own header layout**
  (`Screen::header_layout`, the shared globals' four layouts): the
  lobby's title and Game Setup's are in different rectangles.
- **A dialog is drawn over its opener** when its sample names one with
  `over`, one level deep: the opener's own `over` is not followed. The
  opener is under the game's backdrop: the shared globals' overlay
  colour, then the blur (`Disc::backdrop`).
- **A variant is named.** `assets/ui/retail-samples/` holds
  `squad_settings_dialog.switch-to.yaml`, Game Setup with retail's
  Switch To rows and two of ours (Switch To: Forge, Switch To:
  Firefight) between Quick Options and Co-Op: seven rows. The plain
  `squad_settings_dialog.yaml` is what every other caller draws.
- **The editor's edits come first.** A stand-in for a string set with
  `ui.set_string` is drawn ahead of the sample, then `Retail::text`'s
  answer, then the sample, then the tag's own string.
- **Text is laid out as the game does** (docs/formats/ui.md "Text
  layout"): broken at the rectangle's width, justified line by line,
  and the lines under the rectangle's bottom left out.
- **The view keeps every block.** `retail_view` has a widget for a
  hidden block, with no text, so the editor can still select and move
  it.

Not drawn: model scenes, and a shader bitmap of a cell with no `emblem`.

Not traced: which code hides the lobby's per-slot bars and speaker
icons, and when it shows the help text under the player column. The
lobby's update (`0x24fd..`) looks up text widgets 1, 3, 4, 0x2a and 0x2d
and bitmaps 2 and 4 only. The sample hides the co-op and matchmaking
panes' slot bars, as the custom game's pane parks them off screen.
Game Setup's panel bitmap is another screen's art, and its image index
is out of range: the block's image (`+0xa`) is 8 and `xbox_live` has
images 0 to 7, so the tag alone draws nothing there. The tag preloads
`difficulty_options` and `mp_games`, and the code that picks the
panel's picture per focused row is not read, so the sample's `level`
stands in for it.

## The fidelity loop

Status: Design. `h2 ui compare` and the agent API's `ui.screenshot` are not built.

The preview is only worth trusting once it has been measured against the game:

1. Pick retail screens the preview can draw from tags alone: Controls, the main menu, `error_dialog_ok_cancel`, `dialog_live_password`. Render them with the same renderer as our screens.
2. Capture the same screens in game with `ui.screenshot` (it writes the presented frame and the native screens' widget rects). The user runs one command against the game they have up. Agents never start the game or `h2 serve`.
3. `h2 ui compare <capture.png> <screen> [--state NAME]` reports, per widget, the rect error in units (where the capture has rects) and the pixel difference inside each rect, and writes a diff image.
4. Fix the renderer until those numbers are small. Then our own screens' previews are trusted to the same margin.

References, captures and diffs live outside the repository, never in git.

## Not verified

A game run has to show:

- draw order: our dialog on top of the screen beneath, and the footer widget keeping our legend (`0x2300cf` may refill it from the tag);
- alpha 0 hiding a widget, and tint changes taking effect while the screen is up;
- the backing's blend: whether `framing_center` and `dialog1`/`dialog2` draw as opaque as the preview shows;
- the native animation timing and the panel, rows and selection in both directions on the game shell and paused game, and what holding a screen's delay and length to the kit's budgets does to its widgets (a quicker track, or the screen freed before its tracks end);
- input timing: key repeat, the opener-press guard, the close guard and a closing dialog's presses reaching the screen beneath, and the keyboard's return;
- a dialog's description drawn over its backing at `dialog_tag::description_bias`, and a settings dialog's header lines at that bias too;
- a settings dialog's one-line about and two-line help in the game's font: the help clear of the eighth row and the legend;
- that the Controls constructor accepts a tag recipe whose list holds one item and whose pane has more text blocks than the Controls tag;
- the "Not verified" items of the modal, picker, header and Game Setup sections above, through their game runs.

Each new screen's run checklist lists exactly these for it.
