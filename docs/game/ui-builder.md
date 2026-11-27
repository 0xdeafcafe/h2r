# The UI builder: native Halo 2 screens built, seen and proven offline

Status: Built. The preview, sheet, checks and page tables are tested offline; the in-game items are under "Not verified". The fidelity loop (`h2 ui compare`, `ui.screenshot`) is design.
Code: `crates/extensions/game/src/ui_kit/` (`view.rs`, `preview.rs`, `layout.rs`, `dialog.rs`, `dialog_tag.rs`, `picker.rs`, `page/`), `crates/apps/cli/src/ui.rs`. Tests: `ui_kit::screens::tests`, `ui_kit::page::tests`.
Switch: `H2_SIGNIN_DIALOG`, `H2_UI_SHEET_DIR`, listed in [switches.md](../runtime/switches.md).

A native screen is one we write in Rust and the game draws with its own
widgets ([ui-screens.md](ui-screens.md) is the kit it grows from). The
builder's job: an agent writes a screen, sees it in every state, and proves
how it looks and behaves, all without starting the game. A game run then
only confirms what offline work can't reach (the last section).

The sign-in prompt, the account, co-op and skulls screens are built on it.

## The model

A screen is three pure parts and one runtime part:

```text
Page   ── state + input ──▶ step() ──▶ (new state, Effect)    pure, table-tested
  │
  └── view(state) ──▶ View: widgets with rects, texts, sources  pure
                        │
        ┌───────────────┼──────────────────┬──────────────────┐
        ▼               ▼                  ▼                  ▼
   preview (PNG     checks (fit,       agent API          runtime: the tag
   + rects JSON)    overlap, safe      (ui.screen,        recipe and the
                    area, legend,      native: true)      widget updates
                    focus)
```

- **`Page`** (trait): `type State`, `fn step(&self, &mut State, Input) ->
  Effect`, `fn view(&self, &State) -> View`, `fn states(&self) -> Vec<(name,
  State)>` (the named states a preview sheet draws). Effects are the
  kit's: `Redraw`, `Close`, `CloseThen`, `Open(screen)`, `Keyboard(field)`,
  and screen-specific ones the glue carries out (`Submit(code)`).
- **`View`**: the whole screen as data, in UI units (640 by 480, origin at
  the centre, Y up). Every widget has a stable id (`title`, `body`,
  `cell.3`, `row.0`, `legend`) used by the checks, the rects JSON and the
  agent API.

  | Widget | What it carries |
  |---|---|
  | `Text` | rect, font id, justification, colour, the string, `shadow` |
  | `Art` | a retail bitmap by reference (screen id, pane, index, image), its top-left, tint and alpha |
  | `List` | anchor, skin index, visible count, the rows' cells, focus |
  | `Legend` | prompts (`Glyph`s and words), drawn in the footer |

  `Art` never holds pixels or paths: it names a bitmap block of a retail
  screen. The runtime copies that block from the loaded map
  (`list_class::picture_block`), and the preview decodes it from the
  user's disc. Nothing disc-derived is in the repository.
- **One layout per screen.** A state changes texts, tints and alpha, never
  the widget set. That keeps the runtime to `TEXT_SET` and tint writes,
  and the checks cover every state by running over each state's `View`.
  Pages with different content (the code page and the gamertag page) share
  the layout; a widget a page doesn't use has alpha 0.

## Screen kinds

| Kind | Built from | Header | Use |
|---|---|---|---|
| `List` | the Controls tag (`KitScreen`) | header layout 0 | settings-style lists over the game shell |
| `Dialog` | a retail dialog's tag (`DialogBase`): its backing art and header layout, our rows in its list, a body text or none ("The Dialog kind") | the base's: layout 1 (Quick Options) or 2 (large: title 498 wide, panel 536) | settings pages over the lobby (the shared settings dialog, "The settings dialog"), questions, errors |
| `CodeEntry` | `Dialog` large, plus N cells: the Live passcode dialog's box art (screen 21's `password` blocks) with a digit text over each, the cursor cell tinted bright | layout 2 | the sign-in code |
| keyboard handoff | any kind; `Effect::Keyboard(field)` opens the game's keyboard (`signin_keyboard`), the typed text comes back as an `Input::Typed` | - | gamertag, invite |

**Backing.** A `Dialog` or `CodeEntry` is `Over` (drawn over the screen
beneath, as the game's own dialogs are) or `Own` (it carries the game shell
backing, `framing_center` from screen 5, under its panel, so the screen
beneath doesn't matter).

A HUD over the running game is not a screen kind: it is the HUD kit's
([hud-kit.md](hud-kit.md)), drawn by the game's own HUD pass. The Forge
monitor's HUD and the Forge screens are the game's
([forge-firefight-ui.md](forge-firefight-ui.md)).

The Forge screens are the game's: list classes of ours on the large
dialog (the object menu, Tools, Link, the questions, and the Waves
screens' Rounds, Squads and Problems lists) and on the in-game
Controller Settings host (Properties, Tool settings, the Forge menu,
Controls, Map limits, the Waves screens' WAVES, ROUND, WAVE and SQUAD
pages, Enemy types), opened over the running world
(`crates/extensions/game/src/forge_screens.rs`,
[forge-firefight-ui.md](forge-firefight-ui.md), "The screens" and "The
Waves screens"). A value row opens `kit.picker.game` over them. Their
preview pages are `forge.objects`, `forge.properties`, `forge.tools`,
`forge.settings`, `forge.link`, `forge.question`, `forge.menu`,
`forge.controls`, `forge.games`, `forge.variants`, `forge.variant`,
`forge.map-limits`, `forge.waves`, `forge.rounds`, `forge.round`,
`forge.wave`, `forge.squads`, `forge.squad`, `forge.wave-problems` and
`forge.enemy-types`, drawn over the main menu, which stands in for the
game.

**Sign-in is a modal.** The sign-in
prompt and its new-account gamertag and invite pages are `Over` dialogs,
not full screens. They sit over whatever is beneath (the title screen, the
main menu, in game) the way the game's own dialogs do, in the large dialog
layout of `error_dialog_large` and `dialog_live_password` (header layout
2, about 536 wide): that dialog's own `dialog1`/`dialog2` panel art, the
digit cells and the legend. The retail dialog tags draw nothing behind
the panel; the window stack's render dims the screen under a dialog
("A dialog is an overlay on its opener", below). A modal may cover the logo, as retail dialogs
do, so for a modal the reserved-area check ignores what the panel covers.
Everything in it must still sit inside the panel and inside the safe
area. The preview draws a modal over the screen that opened it (its
`Opener`, below), so the sheet shows it in place.

**The runtime tag recipe.** Our screens are made by the Controls
constructor and adopted by `list_class`. List screens open on their own
`wgit` tag, generated into the maps ([ui-in-maps.md](ui-in-maps.md));
dialogs, and list screens in a map that was not rewritten, are served a
tag copy by the screen-tag function (`0x22f871`, `list_class::copy_tag`).
Either tag is written from the `View`: the root flags pick the
header layout (bits 4, 3, 0 select layouts 1, 2, 3), the pane gets our
text blocks and the copied `Art` blocks in draw order, and the list block
is moved, reskinned and given the visible count. Tints go to `+0x28` of
the bitmap widget, and alpha to `+0x64`.

**Legend.** It goes in the screen's footer widget (`screen +0x338`, the
header layout's second rectangle) as `<glyph> WORDS` with two spaces
between prompts, as retail does. When a header layout has no second
rectangle, the legend is a text block on the panel's bottom edge, which
the checks treat the same way. For a list with commands that depend on
focus, a row can supply its own `RowSpec::prompts`. In game its
`Rows::focused_footer` returns a string slot to set on the widget after
focus moves or `live_text` asks for a redraw. Changing the string in
memory alone leaves the previous legend on screen.

**Code entry input.** Up and down change the digit under the cursor,
left and right move the cursor, A submits, B cancels. The list's one row is
the submit button, so A reaches it through the list as on any screen;
directions are taken before the list sees them (`list_class::Buttons`).

## The offline render path

`ui_kit::preview` grows into the renderer for any `View`:

1. **Sources**, behind one trait: `DiscSource` decodes bitmaps and fonts
   from the user's disc (`mainmenu.map`, `shared.map`, `font_table.txt`),
   and `SyntheticSource` uses flat boxes of known size and a fixed-advance
   font (8 units per character, 16 high). The synthetic source lets CI
   draw and check without a disc.
2. **Draw order**: the game shell background (for `List` and `Own`), the
   panel art by render depth, then the header, list items, texts and
   legend. Text gets the game's drop shadow, and glyphs keep the font's
   colours.
3. **Output**: `<screen>-<state>.png` plus `<screen>-<state>.json`, which
   holds every widget's id, kind, rect (UI units and pixels), text and
   state.

Commands (output only to a directory you name, never the repository):

```sh
h2 ui preview list                                       # screens and their states
h2 ui preview <screen> [--state NAME|all] [--focus N] [--animation PHASE] [--synthetic] [--out DIR]
h2 ui sheet [--synthetic] [--out DIR]                    # every screen x state, and sheet.png
```

`preview` writes to the temp directory by default, and `sheet` to
`$RUSH_KEEP` when `--out` is not given. `--synthetic` draws without a disc.

**The pieces.**

- `ui_kit::view` holds `View`, `Widget`, `Art`, `Metrics` and `Synthetic`,
  plus `Geometry` and its disc-free `Geometry::fallback`. The disc test
  checks the fallback against the disc, field for field.
- `view_of` turns a `ScreenSpec` into views.
- `preview::Source` has two implementations: `Disc` and `SyntheticSource`.
  `render_view` draws a view from either; `write_states` and `write_sheet`
  write the files.
- `screens::states` names each screen's states: a list's first, middle and
  last focus, and the sign-in prompt's 11 pages.
- Sheet labels use a built-in 3 by 5 pixel font, so a synthetic sheet is
  readable too. `cargo test` draws the synthetic sheet; the disc test with
  `H2_UI_SHEET_DIR=<dir>` draws the real one.

Screens come in as `ScreenSpec`s, from their `Page` ("Behaviour"). A spec
with `ScreenSpec::dialog` set is drawn as a modal ("The modal dialog").

## Transitions

The kit uses the retail pane, list and skin animation tracks. The pane's
track moves and fades its panel art, title, description, footer and other
text together. Rows and the selection bar use the list and skin tracks.
Dialogs and pickers animate over the existing page, including their body,
status and icons. `ListClass::animation` and `KitScreen::animation` set a
page's preference. `TransitionStyle::Default` inherits the source tracks,
`Off` disables the pane and list transitions, and `Retail { pane, rows }`
selects one-based slots from the active map's interface tag. A zero slot
disables that track. An unavailable override falls back to the source pane
and list.

`AnimatedView` captures the resting widget bounds and opacity once.
`seek(Phase)` reuses those buffers and computes the same pose for the same
tick. The builder samples a page slide and fade, staggered rows and
selection, and a modal fade. The game plays the native curves from the
active map. Both paths move the text with the other widgets on its track.

**How long.** A page of ours opens in 10 ticks (333 ms) and closes in
6 (200 ms) at the builder's 30 ticks a second (`animation::OPEN_TICKS`,
`CLOSE_TICKS`; `Transition::ticks`). Closing is quicker than opening: a
page leaves as soon as it is dismissed. The game holds a screen of ours
to the same lengths, whatever the retail tracks it borrows ask for
(`Transition::millis`, `lifecycle::cap_transition`): a screen's
transition is its delay (`+0x44`) and length (`+0x54`) in milliseconds
from its start (`+0x4c`), with state bit 0 set while it comes in and
bit 1 while it goes out (`+0x42`), and the window frees a closing
screen once they have passed. Row text (asked at every draw) holds an
opening or closing screen of ours to its budget, and so do a close
through `dialog_tag::close` and a press that closes one; a transition is
only ever shortened (`lifecycle::capped`). So the editor's animation
player (`ui.animate`, the **Transition** selector) ends where the game
does.

Use `--animation in:6`, `--animation rest`, `--animation out:4` or
`--animation all` to append the sampled phase to each PNG and JSON name.
For example, `h2 ui preview theater.playback --animation all --synthetic`
writes both ends and middle samples of playback's transitions. The UI
Screens tab in the editor offers the same fixed samples in its
**Transition** selector. Layout and content checks use the resting pose,
so a deliberate slide never produces a false overflow.

## The checks

They run over a `View` and a `Metrics` (text widths, line heights, image
sizes) from either source, so `cargo test` runs them with synthetic
metrics, and a disc-backed test (`#[ignore]` unless `H2_DISC` is set) runs
them with the real fonts and art. Each finding names the widget id and the
state.

| Check | Rule |
|---|---|
| fit | every text fits its rect at its font (width per line, lines times line height) |
| overlap | no two texts' ink overlaps; a text that belongs to no row (description, legend, title) sits on no row's art (a row's frame reaches into its neighbours by design) |
| frame provenance | frame art borrowed from a retail screen with a list sits with that list where that screen puts it (same anchor and skin) |
| covered (art) | frame art from a screen without a list is covered by other art or text over at most 24 visible pixels (alpha at least 24) |
| off screen (art) | no art's visible pixels fall past the screen edge |
| reserved | nothing overlaps the reserved areas of the screen beneath (its logo and header), unless the screen is `Own`; a modal may cover them with its panel, but everything in it stays inside the panel |
| safe area | the full rectangles of visible texts and rows stay within x ±272, y 204 down to -211, including the footer |
| container | modal text and rows stay inside the usable panel. Bitmap padding does not extend that area. |
| over the opener (modals, flagged only) | a dialog text whose ink meets a text of the screen that opened it is a `Note::OverOpener` in `Report::notes`, never an issue: retail's dialogs sit over their opener as it is, texts included |
| contrast | every text against what is drawn under it (the view rendered without its texts), WCAG ratio at least 2.5 |
| legend | a screen that takes inputs past up, down, A and B shows a button legend (a text with a button glyph) |
| focus | the focused row's text is drawn on screen, and more opaque than the other rows' |
| glyphs | every character exists in its font (the kit's `Finding::Glyph`) |
| ids | every string id is in a registered family (the kit's `Finding::Id`) |
| on drawn | a text's glyphs meet another group's drawn row or cell art, picture (`Art::Level`) or text over more than 24 pixels (`Issue::OnDrawn`); the pairs `overlap` already names are left out |
| panel shared | a panel of the screen's own (not a modal's, and not a backdrop over more than half the title-safe area) holds the list's rows and another group's texts (a card carousel's `card.*` and `cards.*` texts are its list row's own card, not another group): at least half of each one's drawn pixels inside the panel's drawn box (`Issue::PanelShared`). A screen that draws the lobby's panel as retail's lobby does, the list and its status lines in one pane, is listed in `layout::drawn::SHARED_PANELS` with its retail precedent |
| covers under | a modal over a kit screen (`View::underlay`) draws its text over more than 24 drawn pixels of that screen's content (the modal's panel hiding it is what a modal does): its pictures, and texts that are neither the list's (rows, cells, `description`, `name`) nor chrome (`title`, `legend`, `body`, `status`) (`Issue::CoversUnder`). A modal over a retail opener (`Backing::Over`) covers it as retail's do. A screen whose modal text may lie over it is listed in `layout::drawn::COVERS` with its retail precedent |
| drawn outside | art drew more than `Rules::max_outside` (2) units past its declared rectangle (`Issue::DrawnOutside`): fix the rectangle |

The last four read what the preview really drew. `preview::render_traced`
draws the view and records each widget's `Footprint`: the pixels it
touched with alpha above zero, their bounding box in UI units (past the
canvas edge too, for art), and a mask of the touched canvas pixels. Art's
footprint is its image's own opaque pixels where the image put them, so
a skin, frame or picture whose image is wider than its declared rect is
seen at its real size; a text's is its glyphs, drawn a second time on a
clear canvas; a solid fill's, and the box the synthetic source draws for
art it lacks, is the rectangle. `From::draw_traced` gives the view and
the trace together.

A widget's group is its id's first part, the spec's own naming:
`button.start` is the buttons', `player.2` the roster's, `game` its own,
and every row and cell is the list's (`layout::drawn::group`). A mode list
that sits on a status panel beside the game's lines is what `panel
shared` catches when no two rects meet; a picker whose text lies over the
lobby's map picture is what `covers under` catches.

The art checks need the images' pixels, so the synthetic source skips
them, and says so (`Report::skipped`). The provenance check runs without a
disc for the screens our art comes from (Controls 39, level select 11).

The checks are `ui_kit::layout::check(view, source, rules)`.
`ui_kit::findings::of_state` gathers every check's findings for one state
(the spec against the disc, content, layout, and the consistency rules
that read one view), each with its check, kind, widget ids and line.
`h2 ui preview` prints them per state, writes them into the state's JSON
as `findings` (with `notes`, and each widget's footprint as `drawn`), and
with `--strict` exits non-zero when any state has one, so a run shows
the problems without opening the PNG. The editor's picture and gallery
use the same list. Page-model tests check bounds
after every input from every named state, including the visible window
of a scrolling dialog. The agent's `ui.screen` applies the same
rectangle checks to instantiated widgets. The builder also uses font
metrics to catch text that exceeds its text box. The tests:

- unit tests with synthetic metrics, plus one with a solid-image source
  for the two pixel checks;
- every screen in every state passes with synthetic metrics (CI);
- every screen in every state passes against the disc with nothing
  skipped (the disc test).

### Content findings

`ui_kit::content::check` checks the authored spec and visible widgets.
CLI previews and the editor display these findings alongside the layout
findings. `ui.screen` reports them separately in `content_findings`.

| Finding | What it checks |
|---|---|
| broken strings | Required empty text, unresolved keys and format tokens, raw hexadecimal IDs, editing markers, invalid encoding, repeated or trailing spaces, angle selectors and inconsistent row-label casing |
| font and fit | Characters missing from the selected font when its metrics are available, and visible text wider or taller than its box |
| dead controls | Action rows without a confirm action, prompts without a supported input, and offline focus routes or input probes that cannot reach or operate a control |
| missing content | Empty lists without a message, pickers without choices, absent descriptions for rows that share a help slot, missing icon or emblem widgets or unresolved required preview images, and sentinel values without an explicitly unavailable source |

Use `ScreenSpec::content` to declare empty-state and required icon
widgets, shared description slots, or a value awaiting its source.
The content tests visit every registered page and named state, traverse
focus in both directions, and probe advertised buttons on fresh offline
models. Worker-backed or external actions use the authored action
contract. The live watcher examines the current widget tree and registered
Rust list models without sending input. It can inspect resolved text from
stock screens that have no authored model and uses installed font
metrics for authored text when available.

What the checks hold the screens to:

- **Unfocused rows stay readable.** Unfocused rows draw at about 40% alpha (`view::UNFOCUSED_ALPHA`, observed from a game screenshot, not traced); contrast on the real bars is 2.9 to 3.1.
- **The skulls page keeps none of the Controls art.** It copies the level select's own bracket (`ul_02`) at run time (`ListClass::frame`); the frame provenance check names a bracket borrowed from the wrong list. Its view draws the skull picture (`ScreenSpec::art`), untinted.
- **Legends sit in the footer.** The account and co-op screens use a footer separate from their description. Co-op offers Start and Back when applicable and leaves confirm off informational rows. The skulls pages' footer follows the focused row: Start Level, a row whose A opens the picker, or a locked skull (`skulls_menu::RowLegend`), the same on the campaign page and the multiplayer one.

**The full-height skulls list.** The campaign Skulls page lays out
13 rows
(`skulls_menu::VISIBLE_ROWS`), down to y -156. The level select's bracket
(`ul_02`, `[177 -259 -79 -131]`) ends at -79, so rows 8 to 13 run past
the bottom of its image, and its visible art ends sooner, about the fourth
row. Nothing is flagged, and that is right: the bracket is a top-left
corner piece, not a box. The level select itself lays out 16 rows at the
same anchor under the same bracket (its list block's visible count), so
rows running past it are retail's own composition. A "list longer than
its frame" check would flag the game's screen, so there is none.
`ListPlace::Moved` carries the layout's visible count, and both
geometries (synthetic and disc) draw that many rows. The bracket joins
the pane's track, so it exits with the title and descriptions. The sheet's files are named
by the preview's screen name, not the spec's: `skulls.h1` and `skulls`
are both `skulls` specs, and the Halo 1 page's tiles overwrote the
campaign page's.

Calibrations the checks needed, each against the game's own screens:

- the safe area's bottom is retail's footer rectangle (-211);
- the contrast floor is 2.5 (the game's own rows sit at about 2.9);
- synthetic text is 6 units per character (about the disc fonts' mean),
  so the CI checks don't flag text that fits on the disc.

## Pages

| Page | What |
|---|---|
| [UI builder: dialogs](ui-builder-dialogs.md) | The modal dialog, panel texts and the Dialog kind. |
| [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md) | The settings dialog, the value picker, a page header, Game Setup picture and description, Quick Options and behaviour. |
| [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md) | Screens in YAML, the design system, the layout gallery, editing in the editor and retail-screen fidelity. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## The modal dialog

See [UI builder: dialogs](ui-builder-dialogs.md#the-modal-dialog).

### The modal dialog in game

See [UI builder: dialogs](ui-builder-dialogs.md#the-modal-dialog-in-game).

### The game run

See [UI builder: dialogs](ui-builder-dialogs.md#the-game-run).

## Panel texts: information that isn't a row

See [UI builder: dialogs](ui-builder-dialogs.md#panel-texts-information-that-isnt-a-row).

## The Dialog kind

See [UI builder: dialogs](ui-builder-dialogs.md#the-dialog-kind).

## The settings dialog

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#the-settings-dialog).

## The value picker

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#the-value-picker).

## A page's header

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#a-pages-header).

## Game Setup's picture and description

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#game-setups-picture-and-description).

## The pregame lobby's Quick Options

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#the-pregame-lobbys-quick-options).

## Behaviour

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#behaviour).

### Scroll arrows

See [UI builder: the settings dialog and Game Setup](ui-builder-settings-dialog.md#scroll-arrows).

## Screens in YAML

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#screens-in-yaml).

## The design system

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#the-design-system).

## The layout gallery

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#the-layout-gallery).

## Consistency

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#consistency).

## In the editor

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#in-the-editor).

## Editing in the editor

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#editing-in-the-editor).

### Retail tag edits

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#retail-tag-edits).

## Retail screens in a believable state

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#retail-screens-in-a-believable-state).

## The fidelity loop

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#the-fidelity-loop).

## Not verified

See [UI builder: screens in YAML, design system and editing](ui-builder-yaml-editor.md#not-verified).

#### A dialog is an overlay on its opener

See [ui-builder-dialogs](ui-builder-dialogs.md#a-dialog-is-an-overlay-on-its-opener).
