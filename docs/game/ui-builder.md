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
| `Hud` | text and art widgets over the running game (`ScreenSpec::hud`) | supplied by the tool | the Forge monitor strip, reticle and controls |
| keyboard handoff | any kind; `Effect::Keyboard(field)` opens the game's keyboard (`signin_keyboard`), the typed text comes back as an `Input::Typed` | — | gamertag, invite |

**Backing.** A `Dialog` or `CodeEntry` is `Over` (drawn over the screen
beneath, as the game's own dialogs are) or `Own` (it carries the game shell
backing, `framing_center` from screen 5, under its panel, so the screen
beneath doesn't matter).

The Forge monitor HUD has no list or focus. The compositor draws its
widgets on a transparent surface with the preview renderer and disc fonts;
its preview pages are `forge.hud`, `forge.palette` and `forge.menu`
(`crates/extensions/game/src/ui_kit/forge.rs`), until slice U7 removes
them.

A `Hud` whose spec is not `transparent` is drawn over the game shell
instead of a live picture (`preview::From::view`): widgets that sit on
the shell's header and footer bands. Its doc says `backing: shell`.

The Forge screens are the game's: list classes of ours on the large
dialog (the object menu, Tools, Link and the questions) and on the
in-game Controller Settings host (Properties, Tool settings, Forge
options, Controls), opened over the running world
(`crates/extensions/game/src/forge_screens.rs`,
[forge-firefight-ui.md](forge-firefight-ui.md), "The screens"). A value
row opens `kit.picker.game` over them. Their preview pages are
`forge.objects`, `forge.properties`, `forge.tools`, `forge.settings`,
`forge.link`, `forge.question`, `forge.options` and `forge.controls`,
drawn over the main menu, which stands in for the game.

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
| panel shared | a panel of the screen's own (not a modal's, and not a backdrop over more than half the title-safe area) holds the list's rows and another group's texts: at least half of each one's drawn pixels inside the panel's drawn box (`Issue::PanelShared`). A screen that draws the lobby's panel as retail's lobby does, the list and its status lines in one pane, is listed in `layout::drawn::SHARED_PANELS` with its retail precedent |
| covers under | a modal over a kit screen (`View::underlay`) draws its text over more than 24 drawn pixels of that screen's content (the modal's panel hiding it is what a modal does): its pictures, and texts that are neither the list's (rows, cells, `description`, `name`) nor chrome (`title`, `legend`, `body`, `status`) nor the HUD's `reticle` (`Issue::CoversUnder`). A modal over a retail opener (`Backing::Over`) covers it as retail's do. A screen whose modal text may lie over it is listed in `layout::drawn::COVERS` with its retail precedent |
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

## The modal dialog

**What the retail dialogs are.** Read off every dialog tag in
`mainmenu.map` (`h2 ui mainmenu <tag>`): `error_dialog_ok` (8),
`confirmation_dialog` (33), `dialog_live_signin` (20), `tcr_dialog` (238),
`dialog_live_password` (21) and `error_dialog_large` (240). Each is the
same four bitmaps: the `ul_07` and `br_07` corners and the `dialog1` and
`dialog2` panel halves. Then come the header, one text block, and a list,
a button or the password cells. None has a full-screen bitmap, a dim
layer, or any widget outside its panel and corners. The small dialogs
(root flag bit 0) use header layout 3, whose title is 244 wide. The large one
(flag 8) uses layout 2: its title is at `[93 -243 73 255]` and its second
rectangle, the legend, at `[-83 -243 -103 240]`. The panel art is an edge
glow, not a fill. `dialog1` has a mean alpha of 10 and a peak of 87;
`dialog2` has a mean of 5 and a peak of 61. Whatever is drawn beneath
shows through almost untouched.

**What we draw.** `ui_kit::dialog`: a `DialogSpec` (body, optional entry
`Cells`, status line, the `Beneath` screen) on a `ScreenSpec`, drawn by
`dialog_view` with `DialogGeometry`. The geometry is the disc's
(`Disc::dialog_geometry`) or `DialogGeometry::fallback`, and the disc test
holds the two equal. It draws error_dialog_large's panel and corners
(`Layer::Panel`), the title in layout 2's header, our body under it, and
six `password` cells from screen 21. The cells sit at that dialog's own
height (top 6), 55 wide and 59 apart, with 12 more between the halves. A
digit sits over each cell. The cursor's cell and digit are lit; the
others are dimmed (the frame and row alphas). The status line (Checking...,
the error) sits above the legend, and the legend is in layout 2's second
rectangle. The view's backing is `Backing::Over(opener)`, the
`DialogSpec`'s `Opener`. On the disc, the preview draws the opener as a
retail screen, then the game's backdrop over it (`Source::backdrop`:
the overlay colour's fill and the blur, below), then the dialog.
Synthetic previews draw the opener's rows as boxes and text blocks, then
the same backdrop with the fill's colour fixed
(`preview::SYNTHETIC_DIM`, the disc's). A dialog drawn over a screen of
ours (a view's `underlay`, as `layout.dialog.over-screen` is) gets the
same backdrop over that screen (`render_composite`).

**The modal's checks.** `Rules::panel` is set for a dialog spec
(`DialogGeometry::area`, the panel's and corners' box,
`[153 -273 -161 331]`). Every text and cell must sit inside it (the new
`panel` check, `Issue::OutsidePanel`), as well as inside the safe area.
The reserved-area check skips the panel's art and anything inside the
panel. Panel art is held neither to a list nor to its pixels: content
sits on it by design. Entry cells count as groups for the focus and
overlap checks, the same way rows do (`cell.N.art`, `cell.N.text`).

**Sign-in as a dialog.** `signin_screen::dialog_for` gives each page its
title, body, cells, status and glyph legend:

- the code page: up/down Digit, left/right Move, A Sign in, B Cancel;
- Checking: the code in its cells with no cursor, "Checking...", and B
  Cancel only;
- the gamertag and invite pages: A Type it, B Cancel;
- the account pages and the end pages: A Make it, Try again or Done, or B
  Back.

`spec_for` keeps the one-row list the game shows and adds the
dialog, which the preview, the sheet and the editor draw. The 11 states
are over the main menu; two more (`code-over-live-menu`,
`code-wrong-over-live-menu`) are over the Xbox Live menu. All 13 pass
every check, synthetic and on the disc. Only A
and Start submit, matching the legend (`signin_screen::ACTION_BUTTONS`).

**A dialog is an overlay on its opener.** A retail dialog sits over whatever was open when
it opened, drawn as it was: not always the main menu, and with nothing
hidden. So a `DialogSpec` names its `Opener`: the opener's screen tag,
the pane shown, its list's rows (the game's code fills them) and the
focused row. The preview draws the opener as a retail screen in its
sample state (`Disc::render_retail_layer` with `Retail::rows`, "Retail
screens in a believable state"): units substituted, texts wrapped, the
opener's rows on its own list skin with the focused row lit.
`Disc::opener_view` makes the same opener a `View` (its pane's bitmaps,
its header and its texts as the sample state reads them, its rows) for
the over-opener notes.

The game dims the opener. Every window stack's render slot is
`0x234f8c` (vtables `0x4599a8`, `0x4599dc`, `0x459a08`; `0x459a34`
reaches it through `0x23583e`). For each of the stack's screens,
`0x23029a` asks whether a lower window holds a screen (windows 1, 3, 2,
4, 5 from the top, tested with `0x1490ec`; the screen's virtual `+0x6c`
can opt out). If one does, it adds the screen's transition fraction
(`0x230374`, clamped to 1) and fills the whole screen (`0x147cdb`) with
the shared globals' colour at `wigl +0x6c` (a, r, g, b), its alpha
times the fraction, then calls `0x235756` with the fraction, and only
then draws the stack's own widgets. On the disc the colour is (0.85,
0.0, 0.0784, 0.1725): 85% of (0, 20, 44). So under a window-3 dialog
(Game Setup) or a window-1 dialog the opener is drawn in full and then
dimmed 85%: the lobby's PREGAME LOBBY strip above the frame shows at
15%.

`0x235756` then blurs the screen with the Kawase blur `0x211a0`
(`engine::glow::blur`): `3.75 * fraction` steps (`[0x0045de14]`), so at
rest four draws with fractions 1, 1, 1 and 0.75. The first draw's scale
is 1.0 and each next one's is 1.25 times it; a draw's spread is its
fraction times its scale (1, 1.25, 1.5625, 1.465). Each draw makes every
pixel the mean of four bilinear taps, tap `k` moved by the spread times
0.5 (`[0x0045dbbc]`) in x for `k & 1` and in y for `k & 2`, else times
-0.5 (`[0x0045dbc4]`), with the sampler clamped at the edges
(`native_render::glow::quad`). `Disc::backdrop` draws both at rest
(fraction 1): the fill from `SharedGlobals::overlay_color`, then
`preview::backdrop::blur` on the 640 by 480 canvas.

The openers:

- `Opener::MAIN_MENU`: the main menu on the Xbox Live row (rows as
  the list hooks show them: Campaign, Xbox Live, Split Screen, System
  Link, THEATER, Settings).
- `Opener::XBOX_LIVE_MENU`: the Xbox Live menu (screen `0xba`, pane 0)
  on Quickmatch, with its nine rows
  ([live-custom-games.md](../live/live-custom-games.md) §5).
- `Opener::LOBBY` (`lobby` in a screen doc): the pregame lobby's custom
  game pane in its sample state, with no rows.

On the disc, the code pages' legend sits over the main menu's Xbox Live
and Split Screen rows. Over the Xbox Live menu, the title and body sit
over its rows, and the last cell's digit over "Latest news:". Those are
notes (`over the opener` above), not failures: retail's dialogs overlap
their opener in the same way.

Not verified: on the main menu path, the Xbox Live row opens the game's
own account picker, `dialog_live_signin` (screen 20, loader `0x24b4a9`),
and the logon is the picker's. So when our prompt opens, the picker may
still be up over the main menu, and then it is the real opener. The
game run below says which.

**Where retail puts dialog text, against the logo.** The main menu
(`main_menu`, screen 6) draws its logo (`start_screen`) at
`[45 -255 -19 257]`, with thin track lines at y 64 to 50, -19 to -30 and
-180 to -191. Its list starts at anchor (-178, -65), so its first item's
text is at y -37 to -65, and the rest of the list is below that. Inside
the large panel, between the header (bottom 73) and the footer (top -83),
no band is clear. The band from 73 down to 45 is the cleanest: only the
thin track lines cross it. Retail doesn't avoid the logo either. The
small dialogs' text starts at y 72 and error_dialog_large's at 74, both
running down over the logo. dialog_live_password's cells sit at top 6,
over the logo's lower half, and ours sit at the same height. So our
cells are already where retail's are. The body is the one part that
could do better.

**Text placement.** The body starts at 72, as retail's text blocks
do. A code page's body stays in the band from 72 down to 45, the strip
above the logo. When the body is one line and two lines of its font fit
in the band, the status line goes under it there (`dialog::status_place`).
Otherwise it stays above the legend. The cells stay at retail's height.
On the disc the band holds one line, not two: font 1 (`conduit-12`) is
19 units a line in a 27-unit band. So the code page's body (one line)
fills it, and Checking... and the errors stay above the legend, over the
menu's first item. Making them fit would take a smaller font or text
placed over the logo, and neither is retail's. The `placement` check
(`Issue::Placement`, run for every modal) holds all of this:

- the body at 72;
- a code page's body above 45;
- every cell at top 6;
- a one-line status under a one-line body in the band, when it fits.

Longer bodies (the gamertag page, the recovery page) still run over the
logo, as retail's own dialogs do.

### The modal dialog in game

In game the prompt is the one-row list on the Controls layout. The dialog
tag is built and tested; serving it waits on a game run.

**How the game opens its dialogs** (E).
`0x19b527` and `0x19b5af` fill a 0x20-byte request with `0x149f49`, as
`0x149f1e` does, and call the loader themselves. The only difference is
the window kind at `+4`: 1 for a dialog, 5 for `0x149f1e`. The push
(`0x147f6d`) picks the screen stack by that kind (kind 5 at `0x54d62c`,
kind 1 at `0x54d884`), so a dialog goes on a stack of its own
and the screen that opened it is left as it is. Our class loader already
hands `+4` and `+8` to the Controls constructor and the request to the
push, as the retail loaders do.

**The pieces** (`ui_kit::dialog_tag`).

- `pieces(view)` turns a dialog's `View` into what its tag carries:
  header layout 2 in the root flags (`LAYOUT_2`), the four panel blocks
  and the six cells as `PictureSpec`s (found at run time through the
  screen list, as `list_class` pictures are), and a text block each for
  the body, the six digits and the status line. The title and the legend
  are the header's. Each piece keeps its alpha (the dimmed cells), since
  a block has none and it has to be set on the widget.
- `write_tag` writes the tag copy from the Controls tag: the root with
  layout 2 (its other flag bits and header string kept), one pane with
  only our bitmaps, our text blocks (justification flags and a string id
  of ours each) and the one list, and every other block empty.
- `request(window, mask, loader)` writes the request as `0x149f49` does.
- `H2_SIGNIN_DIALOG=1` opens the sign-in prompt in the dialogs' window
  with that request, handed to our loader directly. It's off by
  default: the prompt opens through `0x149f1e`.

**Serving the tag is not built.** Two things are needed.
The sign-in class's tag function has to return the `write_tag` copy
(the panel and cell blocks resolved by screen id before the open). Its
text blocks need string ids from the sign-in family, served from its
text page, with alphas set once the pane is built. Whether a text
block's string is read again on each draw, or only when the widget is
built, decides how the digits change, and that needs the run.

### The game run

One run with `H2_SIGNIN_DIALOG=1`, signing in with an identity whose key
isn't on the machine, from the main menu's Xbox Live row. Check each
line on stderr and on screen:

1. stderr: `signin prompt: opening for controller 0 (window kind 1)`,
   then `signin-code: opened with 1 rows (our list class)`. If the
   game crashes or nothing opens, window kind 1 doesn't take a
   Controls-built screen; run again without the switch (`window kind 5`)
   to compare.
2. On screen: whatever opened the prompt stays drawn under it, with
   nothing hidden, the way a retail error dialog sits over it. Note what
   that is: the main menu, or the game's account picker
   (`dialog_live_signin`) over it; the preview's opener follows. With
   kind 5 the main menu is replaced.
3. The d-pad edits the code and A submits (`signin prompt: event ...`
   lines as in docs/live/live.md). The dialogs' stack takes the input
   while the prompt is up; the menu beneath doesn't move.
4. B cancels and the prompt goes; the menu beneath is where it was, and
   its own input works again straight away.
5. A wrong code shows the error and the next edit clears it; a right code
   ends on Done and A closes.
6. Note anything drawn differently from the sheet (`h2 ui sheet`): the
   screen is the list layout in this run, so only the backdrop and the
   input are expected to match.

This run settles whether our screen can go on the dialogs' stack, what
is drawn beneath it, and input and closing there. The tag (layout 2
header, panel, cells) and the text questions in "Not verified" need a run
once serving the tag is built.

## Panel texts: information that isn't a row

A page that shows facts (a mod's page: docs/game/maps.md "A mod's
page") keeps them out of its list: its rows are only what A does
something with, as retail's lists are. The rest is the screen's own
texts and art, from a retail screen that shows information the same
way (for a mod's page, the pregame lobby, screen 14):

- `ScreenSpec::panel` (retail backings, `Layer::Panel` widgets
  `panel.N`, held neither to a list nor to their pixels, as a modal's
  panel is) and `ScreenSpec::texts` (`PaneText`: id, rectangle, font,
  colour, left or right, the string), drawn by `view_of` after the
  rows and the art and checked like any text (fit, overlap, safe area,
  contrast).
- In game, `ListClass::panel(Panel { art, texts })`: the art made
  bitmap widgets and each `PanelText` a text block of the class's page
  (justification and the retail block's depth bias kept) made a text
  widget by the pane builder's text step when the pane is built; their
  strings come from `Rows::panel_text(i)` and are set then and again
  whenever the description is (`Rows::live_text`'s redraw). Only a
  class with a description or a panel text shows them; a class with a
  panel text and no description (a dialog's body) builds its description
  block from the first panel text (`ListClass::description_block`), which
  stays empty because no row names a string for it.
- The test `ui_kit::screens::tests::a_mods_page_is_the_lobby_layout_with_one_action`
  draws the page and holds every part where the lobby has it; the disc
  test holds the layout's constants to the lobby tag's blocks and draws
  it again on the disc's geometry.

## The Dialog kind

`KitScreen::dialog(base, opener)` (and
`ListClass::dialog(base)` under it) makes a kit screen a dialog of its
rows, the way the game's own settings dialogs are. The target look is the lobby's **Quick Options** (Game Setup, then Quick Options).

**Bases** (`ui_kit::dialog::DialogBase`), each a retail dialog tag:

| Base | Screen | Header | List | Panel | In the game maps |
|---|---|---|---|---|---|
| `QuickOptions` | 222, `quick_options` | layout 1 | skin 2 (label and value), 11 rows at (-254, 87) | `fs_dialog`, full screen | no, `mainmenu` only |
| `Large` | 240, `error_dialog_large` | layout 2 | skin 0 (one text), 2 rows at (-235, 30) | `dialog1`/`dialog2` and corners | yes |

Quick Options' help line (`QUICK_HELP`, above the list) is where a
settings dialog says what the window is for; its game type picture
(bitmap 1) is the retail dialog's content and is left out
(`content_bitmaps`). A
row's value goes in the skin's value column on Quick Options and after
the label on the large dialog (`DialogBase::style`). The disc test holds
`DialogGeometry::fallback_for` to the disc for both.

**Quick Options inside** (read off `mainmenu.map`'s `quick_options` tag
and its strings). It is not the Controls tag's list with the skulls
page's inside (the level select's one-text list, values in the label, a
five-line description pane). It is:

| Part | `quick_options` (screen 222, flags `0x10`: header layout 1) |
|---|---|
| Header | "QUICK OPTIONS" (its `header` string), layout 1's rectangle `[190 -253 170 30]`, font 2 |
| Pane | animation 31; bitmaps `null`, `mp_games2` at (-267, 166) (the game type's picture, drawn by code), `fs_dialog` at (-270, 200) |
| Text 1 | `quick_options_variant_name` (the variant's name, filled by code), white, font 1, `[163 -224 143 0]` |
| Text 0 | `quick_options_help`, "Temporarily change settings for this game variant." (51 characters, one line), font 1, pale blue (0.388, 0.580, 0.757), left, `[143 -224 123 270]`, animation 31 |
| List | skin 2 (`ui\list_skins\setting_lists\setting_lists`), 11 rows from (-254, 87), flags 1, animation 32 |
| Skin 2 | label text font 4, pale blue, `[23 10 0 255]`; value text font 4, paler blue (0.561, 0.694, 0.812), `[23 256 0 500]`; bar `settings_list_bkd` and focus `hilite` at (-12, -4); items 26 high |
| Footer | layout 1's second rectangle `[-178 -200 -198 259]` |

No value is drawn with arrows: the executable has no `< %s >` format
(neither byte nor wide), the skin has no arrow art, and the only
`left_arrow`/`right_arrow` strings in the menu belong to the virtual
keyboard. A settings row there is its name and its value, `Score to Win
Round  50`.

So a page on `DialogBase::QuickOptions` takes all of it:
`write_base_tag` copies Quick Options' root (header layout 1), pane
(animation 31), `fs_dialog` and list block (skin, anchor, flags,
animation); and the rows show `ValueStyle::Setting` (the value alone in
the value column). The variant-name line and the game type picture are
retail's content and stay out of the base. A settings dialog of ours
puts its own header there, the same in every state, and the focused
row's help in a block under the rows ("The settings dialog"). The pages'
titles are in capitals, as Quick Options' header is.

The test `ui_kit::screens::tests::our_settings_dialogs_are_quick_options_inside`
holds every settings page to that (dialog base, list place, value style,
header blocks, help block, capital title, footer) and draws each beside
the kit's Quick Options sample (`kit.dialog`): every panel, title, row
bar, label, value, header, help and legend widget the sample has is at
the same rectangle, with the same art or font and justification. The
disc test repeats that drawing on the disc's geometry.

**Preview.** `DialogSpec` carries `base` and `rows` (and a `body`, empty
for a rows-only dialog). `dialog_view` draws the base's panel and header
and the rows through `view_of` at the base's list, over the opener.
`kit.dialog` (Quick Options) and `kit.dialog.large` are the samples in
the sheet.

**In game** (`ui_kit::dialog_tag`, `list_class`):

- Loading: before the constructor, `prepare` looks the base up with
  `SCREEN_TAG_BY_ID` and writes `write_base_tag`'s copy into the class's
  page: the base's root (header layout and string), one pane with its
  flags, its backing bitmaps (up to 16, less `content_bitmaps`), its
  list moved to our skin, anchor and row count, and no other blocks.
  The screen-tag hook serves that copy (`served`) and doesn't ask for the
  Controls tag for the frame. The large error dialog has a null localized string-list
  reference because retail fills its text buffers directly. For a base
  without a string list, the copy takes the Controls screen's reference
  (Controller Settings in a game). This lets the widget's string lookup
  reach the kit's locale source for every label. If the interface lacks
  the base, its list, or both string lists, the loader leaves the host
  screen's layout in place.
- Opening: `list_class::open` and `push` open a dialog class with
  `open_over`: the request with window kind 1 (`DIALOG_WINDOW`) handed to
  the loader, as `0x19b527` does. The opener stays drawn under it.
- Closing: retail's close by window and slot (`0x14800c`) has no case for
  window 1, so the generic back handler can't close it. B and Back on a
  window-1 screen send it the close message (`0x22e957`, kind 2) unless
  it is closing already (state bit 1), and so does `KitScreen`'s close.

**Pause screens.** The Controls tag (screen 39) is only in `mainmenu`, so
a Controls-built screen opened in a game has no tag; such screens build on
the in-game Controller Settings (screen 200) instead. A `Large` dialog
works there too, since screen 240 is in `shared` and
`single_player_shared`; `QuickOptions` does not.

### The Dialog kind's game run

With a dialog screen on `QuickOptions` opened from the lobby (Clankers
or Halo 1 Options, both on it; they open on the Game
Setup dialog's own window and slot, as before, not the dialogs' window):

1. stderr: `<name>: a dialog on screen 222's tag (copy at ...)`. If it
   says `no dialog screen 222 in this interface`, the lobby's interface
   doesn't have Quick Options loaded; try `Large`.
2. On screen: `fs_dialog` over the lobby, the header at the top left,
   the rows as Quick Options' (label left, value right) from (-254, 87),
   the page's icon in the game type picture. Compare with `kit.dialog` in the sheet, and with
   the lobby's own Quick Options side by side: the same row bars and
   focus highlight, the same two blues, the value alone ("Normal", not
   "< Normal >"), the window's line where Quick Options says
   "Temporarily change settings...", the focused row's help under the
   rows, the legend in the same footer place.
3. The d-pad moves and changes rows; the lobby beneath doesn't move.
4. B closes the dialog (stderr shows no close by slot) within a fifth
   of a second, and the lobby takes input again at once: a press while
   the dialog is still fading reaches Game Setup ("Behaviour"). Open it
   again: it opens the same way.
5. The title is ours (slot 18 sets it on the header after the pane is
   built, as on every class); note it if it reads "Quick Options".

## The settings dialog

Every settings page of ours is one component, `ui_kit::settings`, built
from its doc (`content/ui/screens/<name>.yaml`, `layout: settings`) and
the doc's `settings:` block. It keeps one rule:

- **Top:** static per screen. The icon, the name and a line about the
  window come from the `settings:` block and do not change with focus.
- **Bottom:** the focused setting's help, under the rows.
- **Picker:** a setting's value picker says that setting's help: the
  row's `explain`, else its `help`.

| Part | Widget id | Place | Source |
|---|---|---|---|
| Title | `title` | layout 1 header `[190 -253 170 30]` | the doc |
| Icon | `art.0` | Quick Options' picture `[166 -267 126 -227]`, `mp_games2` image N (a name, or `image-N` for an added image) | `settings.icon` |
| Name | `header.name` | `[163 -224 143 0]`, white, font 1, one line | `settings.name` |
| About | `header.about` | `[143 -224 123 270]`, pale blue, font 1, one line | `settings.about`, else `settings.caller.help` |
| Rows | `row.N.*` | skin 2 at (-254, 87), 26 apart, at most 8 (`spacing.settings_rows`) | the doc's rows |
| Help | `description` | `[-133 -244 -173 246]`, pale blue, font 1, two lines | the focused row's `help` |
| Legend | `legend` | layout 1 footer `[-178 -200 -198 259]` | the rows and `settings.actions` |

`design.yaml` places the name, the about and the help
(`components.settings.name`, `settings.about`, `settings.help`); a
settings doc gives no `description`, `legend`, `header.*` or `art.*`
widget. The name is Quick Options' variant-name block
(`dialog::QUICK_NAME_TEXT`). The about block is Quick Options' help
line, one line (`dialog::SETTINGS_ABOUT_TEXT`): a row's cap rises 33
above its item's anchor, so row 0's reaches y 120, under the help
line's bottom at 123, and a second line would sit on it. The list is
Quick Options' own at (-254, 87). The help block sits between the last
row and the legend, its left on the row labels' left
(`dialog::SETTINGS_HELP_TEXT`). The disc test holds the name to the
tag's text block 1 and the about to its text block 0.

**The doc.**

```yaml
settings:
  host: menu
  icon: spartan
  name: "Bots in this game"
  caller:
    label: "Clanker Options..."
    help: "Add bots: how many, how well they play and which team."
rows:
  row.0:
    kind: value
    text: "Clankers"
    help: "Bots join this game and show in the lobby."
    setting: clankers.enabled
    explain: "Clankers are bots that play this game with you. On adds them to the lobby like any other player. Off plays without them."
```

- `host`: `menu`, Quick Options' tag (`DialogBase::QuickOptions`). The
  schema names `game`, the in-game host; the kit has no game host and
  refuses it by name (`settings.host`).
- `icon`: one of `dialog::icon::NAMES`.
- `name`: what the window changes. A lone slot (`name: "{variant}"`)
  is the page's name binding (`Bindings::name`), read each time the
  header is drawn; a doc with one needs that binding, and a binding
  needs one.
- `about`: what the window is for. A page opened from a row elsewhere
  (Game Setup's) gives `caller` instead, the opening row's `label` and
  `help`; the about line is that help, so the row and the window say the
  same thing. A doc gives one or the other.
- `actions`: extra buttons, `{ button, words, bind }`: the button one of
  X, Y, Start or Back, the words in capitals.
- Rows: `kind` is `value`, `pick`, `open` or `action`, and every row has
  its `help`. A `value` row is over a setting of the store (`setting:`,
  its choices from the setting's `Def`, `values:` naming them) or a
  binding (`bind:`); a `pick` or `action` row names a binding; an `open`
  row names the screen it opens. `explain` is the picker's text; the
  help is when it is unset.
- Slot rows: a `pick` row whose `text` and `help` are both lone slots
  (`text: "{label}"`, `help: "{help}"`) and whose `bind` names a slot
  binding (`Bindings::slot`) takes its label, its value and its help
  from that binding each time it is drawn (`screen::Filled`), and has no
  `explain`. A slot its binding leaves empty is not drawn: the rows
  after it move up and the list's count is the rows drawn. A is the
  binding's pick, titled by the label and saying the help; left and
  right do nothing.

`data::ScreenDoc::validate` makes each rule a named error at its field.

**The Rust.** A page's Rust is its bindings alone:

```rust
pub fn page(store: Arc<Store>) -> KitScreen {
    settings::screen(FEATURE, SCREEN, IDS, Some(store), Bindings::new())
        .unwrap_or_else(|e| panic!("{e}"))
}
```

`settings::Bindings` registers a row's behaviour by name (`value`,
`pick`, `action`, `slot`) and the header's name (`name`). A binding no row names, a row naming no registered
binding, an unknown setting and a `values` list of the wrong length are
named errors. `settings::legend` derives the legend: `CHANGE` when a row
is a value row, then `SELECT` and `BACK`, then the extra buttons
(`KitScreen::extra`).

**In the game.** The header is the class's picture and panel: the icon
is `ListClass::picture(QUICK_PICTURE)` showing `Rows::picture_image`, set
once as the widget is made; the name and the about are panel texts
(`Rows::panel_text`), drawn at the description's depth bias at least
(`dialog_tag::description_bias`) so `fs_dialog` does not cover them.
Nothing row-indexed reaches them: a focus move names the same panel ids
with the same strings, and the focused row's help goes through
`Rows::description`.

**The pages.** `clankers`, `firefight`, `h1-options`, `browser.filters`,
`variant-quick` (Quick Options for a script variant, five slot rows and a
bound name; [rules-variants.md](rules-variants.md#quick-options-in-the-lobby))
and the kit's `kit.dialog`, with the specimens `layout.dialog.settings`
and `design.row.setting`. `settings::NOT_SHARED` lists the screens with
settings rows that are not the shared dialog, each with why:
`variant-more-options` (a page of retail's variant editor), the skulls
pages (the level select with the skull's picture), `forge.properties`
and `forge.settings` (Forge pages on the in-game Controller Settings
host, screen 200), and the kit's other examples. Forge's pages have no
icon: `mp_games2` is in `mainmenu.map` only. One arrives with map
injection, when the tag and its art are generated into the shared maps.

The tests are `ui_kit::settings::tests`
(`every_settings_list_is_the_shared_dialog_or_says_why`,
`the_top_is_the_same_under_every_focus`,
`the_bottom_is_the_focused_rows_help`, `a_rows_picker_carries_its_help`,
`a_setting_row_reads_and_writes_its_def`, `the_legend_follows_the_rows`,
`a_slot_row_is_its_bindings_and_an_empty_slot_is_not_drawn`,
`a_slot_and_a_bound_name_attach_or_name_their_field`),
the consistency rule `settings` ("Consistency") and
`data::tests::a_settings_doc_round_trips_and_its_rules_name_their_field`.
The retail measurements behind the places are in
[notes/2026-11-22-retail-settings-dialogs.md](notes/2026-11-22-retail-settings-dialogs.md).

## The value picker

A setting with a range of values opens a list of
them, as retail's Game Options do, rather than stepping on A.

**Retail's dialog** (read off `mainmenu.map`). The variant editor and Quick Options open
`low_level_settings` (screen 223,
`ui\screens\game_shell\settings_screen\variant_settings\editing_format_screens\low_level_settings`)
for a setting row:

| Part | `low_level_settings` |
|---|---|
| Root | flags 8: header layout 2, the large dialog's (title `[93 -243 73 255]`, legend `[-83 -243 -103 240]`); its title is the setting's name in capitals, set by code |
| Pane | animation 23; `error_dialog_large`'s four panel blocks (`ul_07`, `br_07`, `dialog1`, `dialog2`) at the same places |
| List | skin 22 (`ui\list_skins\default\default6`: `default`'s one text, row bar and end cap, its scroll arrows at (-27, 24) and (-27, -88)), 6 rows from (-231, 30), flags 1, animation 24 |
| Text 0 | `description`, filled by code: `[74 15 -80 250]`, font 1, the pale blue, left |

Every setting of the game's variant editor is a `sily` tag with a block
of options, label string ids included; 49 of the 92 have two (On and
Off), and they open the same dialog. So our two-value rows open it too.
The tag is in `mainmenu.map` only (not in `shared.map` or
`single_player_shared.map`); skin 22 is in every interface's `wigl`.

**The picker** (`ui_kit::picker`). `DialogBase::ValuePicker` is the dialog
(`DialogGeometry::fallback_for`, `PICKER_LIST`, `PICKER_HELP_TEXT`,
held to the disc by the disc test). Two classes show it: `kit.picker`
on `low_level_settings` and `kit.picker.game` on the large dialog with
the same list, skin and description block; `list_class::open` takes the
second in a game. A row sets the process-wide `Pick` (title, values,
held value, description, its setter) and opens `kit.picker`. The
description is the row's explanation (`Row::explain`), else its
description. On a settings dialog both come from the doc (a row's
`explain`, else its `help`), and the kit titles a pick row's picker by
the row and gives it that text, whatever the binding's pick says
("The settings dialog"). `KitScreen` value rows (`Row::value`, so every
`ChoiceRow` and every `setting:` row) open it: the settings pages and
the kit's examples.

A on a value sets it through the row's own setter (the settings store:
saved, its change callbacks run) and closes; B closes and changes
nothing; X, Start, left and right do nothing there. Left and right on
the row itself still step. A kit value row can open a picker with one
option. Pickers with many options reuse text slots as the list scrolls.
The legend is `(A) SELECT (B) CANCEL`; the pages' own legends include
`(A) SELECT`.

Theater uses `picker::offer_list` for Sort, Map and Game type. Each opens
over the library with A, including a filter whose only entry is All.
Long lists reuse text slots as new rows scroll into view. The preview
composites the dialog over the library with its tab, rows and focus intact.

Theater's playback window draws the bar, pause menu and its choice dialogs
from the same specs used by `h2 ui preview theater.playback` and
`theater.pause-menu`. The compositor renders the kit's view using the
owned disc's fonts and row art, then blends it over the published film
picture. The pause menu's pickers keep their parent focused behind them.
`ui.screen` and `ui.input` use that same playback model.

The Video and Voice screens use `ui::Model::press`. More Options
(`variant_page`), the skulls pages' collected skulls (On and Off) and the
Halo 1 Assets and HUD rows open the kit picker on A. Back dismisses a picker without
changing the held value.

`h2 ui preview kit.picker` and `kit.picker.game` draw it (the kit's
Count, nine values, 4 held, over Game Setup).

`kit.picker.mode` (`screens::MODE_PICKER`) is the picker of the lobby's
modes, a sample that no row of the game opens: GAME MODE, with
Multiplayer, Custom Games, Forge and Firefight as its rows (the lobby
tabs' labels in title case, `lobby_tabs::Tab::title_case`), Custom
Games held and focused when it opens (its first state is `focus-1`),
"Choose what this lobby plays." on the right, over the lobby under the
backdrop (`Opener::LOBBY`). Its choice only logs. The concepts for choosing a
lobby's mode pick it to show the list their button or prompt opens.

**Strings are in guest memory before the screen is built.** `TEXT_SET`
(`0x253b1a`) copies the string as it sets it: it resolves the id into a
buffer of its own (`0x230134`) and hands the widget that, so a slot of
ours must hold its text before the call names it. The picker fills every
slot of the pick (title, description, each value) in `Rows::count`, which
the loader asks first and follows with `Rows::write_texts`; and row text
writes the texts again after the description and panel texts are named,
before their `TEXT_SET`. That second step is in `list_class` itself, so
every class's description and panel texts get it.
`list_class::tests::the_value_picker_opens_with_its_pick_on_the_games_path`
runs the game's path (the class taken at build, a row's A, the loader,
the pane set-up, row text for each item) on the runner, and reads each
`TEXT_SET`'s string from the slot at the moment of the call, as the game
does.

After the pane builds its list, the picker focuses the held value with
the retail list cursor setter (`list_class::LIST_SET_ROW`). Each opening
applies this once, including when the row is reopened after a change.

### The value picker's game run

From a custom game lobby, Game Setup, Clanker Options:

1. A on Difficulty: stderr `ui: opening kit.picker over the screen
   beneath` and `kit.picker: a dialog on screen 223's tag`. On screen:
   the large dialog's glow over the Clankers page, "DIFFICULTY" as its
   title, Easy, Normal, Heroic, Legendary, Mixed down the left, "How well
   the Clankers aim..." on the right, `(A) SELECT (B) CANCEL` in the footer.
2. Check that the held value has the focus.
3. Down to Heroic, A: stderr `kit.picker: clankers.difficulty -> Heroic`
   then `kit.picker: chosen; closing`; the Clankers page shows Heroic.
4. A on Count (more than six values): the list scrolls, its arrows show.
   B: it closes, Count unchanged.
5. Halo 1 Options (on a Halo 1 map): A on HUD shows Halo 1 and Halo 2
   with "Which HUD you play with..." on the right.

## A page's header

A settings dialog's header is the window's, the same in every state
("The settings dialog"): Quick Options' picture showing the page's
icon, its variant-name line saying what the window changes, and its
help line saying what the window is for.

**Quick Options'** (see the table in "Quick Options inside"): bitmap
block 1, `mp_games2.bitm` at (-267, 166), depth 3, animation 31, the
variant's game type icon (eleven icons, 40 x 40); text block 1, the
variant's name in white `[163 -224 143 0]`; text block 0, the help line
`[143 -224 123 270]`, 494 units wide, one line. The bitmap widget's
constructor (`0x2b01eb`) zeroes its image index (`+0x88`) whatever its
block holds; code sets it after (`0x2b0a14`).

The icon is Quick Options' block 1, copied and placed as Quick Options
has it (`dialog::QUICK_PICTURE`, through `ListClass::picture`), so the
icon sits where Quick Options' own does. Its image is the doc's
`settings.icon`, set once the widget is made (`Rows::picture_image`,
through `list_class::set_image`); nothing sets it after. Quick Options'
own copy of the block stays out of the base (`content_bitmaps`), so
only ours shows there.

`dialog::icon` names all eleven `mp_games2` icons by what they show:
Spartan, crown on a hill, lightning bolt, skull, helmet in a sunburst,
sack, flag, bomb, four squares, banner of three stars, question mark.

| Page | Icon | Name | About |
|---|---|---|---|
| Clankers | `spartan` | Bots in this game | Add bots: how many, how well they play and which team. |
| Firefight | `skull` | Firefight rules | Fight waves of enemies: set the waves, difficulty and lives. |
| Halo 1 Options | `helmet` | This Halo 1 map | Play this Halo 1 map with Halo 1's weapons and HUD, or Halo 2's. |
| Game filters | `squares` | Custom games list | Show only the games that match these filters. |

The first three pages open from Game Setup, so their about line is the
Game Setup row's help (`settings.caller.help`): the row and the window
always agree.

**A new icon.** A page's icon is one of `mp_games2`'s images: the eleven
the disc has, or one added to the user's `mainmenu` map in Studio
(`bitmap.image.add`, docs/editor/modeling.md "Adding images and
sequences"; `image.generate` with `image: "new"` adds the slot and makes
the picture in one call). Studio's **Generate image...** makes a picture
in their style from the
icons the user picks as references
([editor-guide.md](../editor/editor-guide.md#generating-an-image),
`image.generate` in [agent-api.md](../editor/agent-api.md#generating-images)).
It goes into an image of the user's `mainmenu` map, fitted
to its 40 by 40 and its format, and shows in the UI Screens picture and a
paired game before it is kept; a bitmap that gained an image saves with
its whole tag written again, so the saved map still loads. A picture
made at `size [40, 40]` with no slot is kept as art with its
provenance (`image.accept {save}`). The doc names a stock icon
(`settings.icon`, one of [`NAMES`]) or an added one as `image-N`; an
index the map does not have leaves the picture's image as the widget
made it (0, the Spartan), `list_class::set_image` refusing it. A game
running without the edited `mainmenu` (Forge's pages, a host map) still
has no icon for the page: one arrives with map injection, which
generates the tag and its art into the shared maps.

**Drawn over the panel.** The draw sorts a pane's widgets by depth, then
render depth bias, then type, text (6) before bitmap (8)
(docs/formats/ui.md "What a screen, pane and widget are"). A text block
at bias 0 over `fs_dialog` at bias 0 or more is drawn first, and the panel
covers it, while the picture (bias 3) shows. On a dialog's copy the help
block and the header's panel texts take `dialog_tag::description_bias`:
the base's own help line's bias (Quick Options' text block 0,
`DialogBase::help_block`) and one more than every backing bitmap the
copy keeps. stderr says `<name>: description at render depth bias N,
over the dialog's backing` and `<name>: panel texts at render depth bias
N at least, over the dialog's backing` as the page opens. The
consistency rule `empty` holds the header lines and the help to saying
something in every state ("Consistency").

**The picker's copy.** A on a row opens the value picker, whose
description says what the option does and what each choice means in a
few short sentences: the row's `explain`, 200 UTF-16 units at most
(`settings::EXPLAIN_UNITS`), so it fits the picker's description block
(235 by 154 units, ten lines of font 1).

The spec carries the icon as the screen's art (`ScreenSpec::art`,
`dialog::quick_icon`) and the name and the about as its texts
(`ScreenSpec::texts`, ids `header.name` and `header.about`), the same at
every focus; `RowSpec::art` is for pages whose art follows the focus
(Theater's library). The disc test holds `QUICK_PICTURE` to the tag's
block, checks `mp_games2`'s eleven icons are 40 high and 40 or 41 wide,
and measures each page's name and about (one line each) and every
row's help (two lines at most) in the disc's font 1.

### Its game run

From a custom game lobby (on a Halo 1 map for Halo 1 Options), Game
Setup: open Clanker Options. At its top left, the Spartan icon where
Quick Options shows its game type, "Bots in this game" in white beside
the icon's upper half, and "Add bots: how many..." in pale blue
beneath it, both over the panel. Move down: the icon, the name and the
about stay as they are; the help under the rows changes with each row.
A on a row: the value picker's title is the row's name and its right
side explains the option. Halo 1 Options: the helmet. Open Quick Options
to compare the places.

## Game Setup's picture and description

Clanker Options and Halo 1 Options get a picture
and a description, as Quick Options has.

**Retail's** (E). The Game Setup dialog's
update (`SQUAD_SETTINGS_UPDATE`, `0x2bbbf6`, vtable `0x45c524`) asks the
focused row's action (`0x2bbaad`, EAX the list at screen `+0x610`), gets
its text widgets 2, 3, 4 and bitmap widget 4 (`0x22edb8`), and by the
action (jump table `0x2bbea8`, actions 0 to 9) sets text 2 (the
description, `change_map_help`'s block `[-40 -241 -173 0]`), texts 3
and 4 (a "current" label and value beside the picture) and the picture:
the map's for Change Map, the game type's for Change Rules, else image N
of `xbox_live.bitm` (`0x2b0a14` stores N at bitmap widget `+0x88` when it
is under the bitmap's image count, `bitm +0x44`). The images, by row:
Matchmaking 0 (and action 9's 4, the same roster), Quick Options 2,
Switch To: Custom Game 6, Switch To: Co-Op 7; images 3 and 5 are 4x4
blanks, and 1 (a Spartan over a roster of players) no row uses. Our
actions (10 and up) fall to the default: no texts, image 0.

**Ours** (`boot::game_setup_help`). A before hook on the update runs
retail's (passing its own call through once), then for one of our
actions sets text 2 to our description and the picture to our image:

| Row | Picture | Description |
|---|---|---|
| Clanker Options... | image 1, the roster no retail row uses | `settings.caller.help` of `clankers.yaml`, a slot of the page's (`KitScreen::row_help`) |
| Firefight Options... | image 6, Switch To: Custom Game's | `settings.caller.help` of `firefight.yaml`, a slot of the page's (`KitScreen::row_help`) |
| Halo 1 Options... | image 2, Quick Options' own | `settings.caller.help` of `h1-options.yaml`, a slot of the page's (`KitScreen::row_help`) |

The row's label is `settings.caller.label` (`KitScreen::row_label`), and
the same help is the page's about line, so the row and the window it
opens say the same thing.

Texts 3 and 4 stay empty, as retail leaves them for a row with no value.

### Its game run

On a custom game lobby on a Halo 1 map, Game Setup: move the focus onto
Quick Options (its picture and "Temporarily..." text), then Halo 1
Options (the same picture, our text under the list), then Clanker
Options (the Spartan over the roster, our text), then back to Change
Map (its map picture comes back). Note any frame where the text flickers
between empty and ours.

## The pregame lobby's Quick Options

The custom game information panel on Xbox Live, System Link and Split
Screen uses pane 0 of screen 14 (`pregame_lobby`). In the lobby update,
`LOBBY_VARIANT_PANEL` (`0x2501c0`) calls the formatter at `0x25092c` for
five option rows. Text widgets 6 through 10 use the pane's text blocks
4 through 8. The last of these rows ends at y −171, above the footer.
The next widget sits below the Quick Options box and is excluded from
the summary. `ui.screen` checks both the five rows and the spare widget
against the Quick Options box when they are visible.

`crates/extensions/game/src/lobby_options.rs` adds the enabled options in
this order: Clankers with the configured count and difficulty on the
hosting console, Halo 1 Options when either multiplayer choice uses
Halo 1 on a converted map, and the multiplayer skull count. Hosted games
use the signed-in profiles' skull choices. Xbox Live guests read the last
cached host advertisement for skulls.

For a variant whose record names a rules script this machine has, the
formatter's rows give way to the script's Quick Options marks
(`crate::variant_quick`, switch `variant-quick`;
[rules-variants.md](rules-variants.md#quick-options-in-the-lobby)): one
"Label: value" line a mark, in the script's order, from the top, and the
slots past the last mark hidden. A script with no `quick` line shows no
rows. A variant with no script, or naming one this machine lacks, keeps
the formatter's rows. The labels and value texts are ours, from
`rules::settings::TABLE` and the script's options. The lines are five
ids of the `variant quick` family (`ui_kit::ids`), written to their
slots at each panel update (`variant_quick::panel_ids`).

The enabled lines follow the retail options, or the marked rows, that
fit. When all five rows are filled, each enabled line replaces a bottom
row (`lobby_options::plan`). The hook sets each extra
text widget through `TEXT_SET` and the shared locale table
(`layout_glue::StringsFeature`, IDs from `ui_kit::ids`). The read-only
[`ui.lobby_options` agent method](../runtime/agent-api.md#ui-what-the-menus-show)
reads those widgets in display order.

## Behaviour

Each native screen's `Page` is proven by table tests: an input sequence
from a named state, then the expected state summary and effects. For
sign-in that covers:

- digits wrapping and the cursor stopping at either end;
- A submitting, and B cancelling from every page;
- an error showing, and clearing on the next edit;
- Checking ignoring edits;
- the new-account pages handing off to the keyboard;
- a taken gamertag turning into the code page.

The glue (`list_class` events, `TEXT_SET`, tints) keeps its runner tests
(`engine::entry::testing::run`).

`ui_kit::page`:

- **`Page`**: `step(&mut State, Event) -> Effect`, `spec(&State)` (the
  `ScreenSpec` the preview turns into a `View`), `states()` and a
  `summary(&State)` line for tables. Events are at least the pad's
  `Input`s; pages that wait on the keyboard or a server take those too.
  `Live` drives any page behind one type (`Driven<P>`), for the editor and
  `h2 ui preview --press`. `screens::live(name)` is every native screen's
  page, and `screens::states` and `sample` come from it, so the sheet
  draws exactly the states the tables start from.
- **`ListPage`**: every list screen (the skulls pages, the Halo 1
  options, the three co-op screens, the kit's example) is its real
  `ListClass` stepped offline. An event goes through the class's own
  hook, then `list_class::route` (the same pure function the runtime's
  `event_with` uses: action buttons, then sideways steps), then the
  Controls handler: up and down move the focus, A presses the row
  (`Rows::press`), B and Back close. Effects: `Focus`, `Hook`, `Action`,
  `Stepped(line)`, `Open(screen)`, `Retail(line)`, `Close`. The rows keep
  their own state (an in-memory profile store, a lobby without Live), so
  each table case builds a fresh page. The page stops the focus at
  either end. The game stops it there too, or wraps it to the other end
  when the list block's flag bit 0 is set (`0x24c1c5`, below); the
  value picker's `default6` list has the flag.
- **`StackPage`**: list screens that open one another, walked as the
  player walks them: an `Open(screen)` it has pushes that screen, B or Back
  pops it, and the summary names the screen on top (`closed` once the
  first one closes). A state is the presses since the first screen, and
  `spec` replays them on the rows first, so a named state (given as
  presses, `.state(name, &[Down, A])`) shows what its presses chose. That
  holds for rows whose presses only choose; rows that toggle a store are
  stepped through their own `ListPage`. The map select's model is the first
  ([map-select.md](map-select.md)).
- **`AccountPage`**: the account screen (`AccountScreen`, already pure),
  with typed text and server answers as events.
- **`SigninPage`** (`signin_screen`): the prompt plus the code being
  typed. A press goes through `CodeEntry::press` as on the screen; what the
  HLE then does follows `live_hle::signin`'s own transitions, pure
  functions the HLE uses too: `typed` (keyboard text on a page), `refused`
  (a code that isn't six digits) and `settled` (the page after a server
  call ends). Typed text and server answers are events.

The tables (`ui_kit/page/tests.rs`, `live_hle::signin` tests) cover every
row of the list above, plus the skulls pages (Start Level's A runs the
action, X and Start don't, A opens the picker on a collected skull and
does nothing on a locked one, left and right do nothing, each row kind's
legend the same on both pages), the co-op screens (Start starts or readies, Invite a Friend opens
the friends list), the account menu, and every screen taking every input
in every state with its spec still passing the kit's checks.

**Closing.** A closing screen still holds its window, and the input
dispatcher (`0x147dbe`) gives every press to the first window holding a
screen, so until the window freed a closing dialog of ours the lobby
under it took no input. A press on a closing screen of ours
(`list_class::event_with`, state bit 1) is the screen beneath's
(`lifecycle::while_closing`): the next screen down its own window's list
(the value picker's page), else the first screen of the first lower
window on its slot (Game Setup under a dialog), with a controller in
common and not closing itself. A press inside `lifecycle::CLOSE_GUARD`
(50 ms, the opener guard's reading) of the close starting is the
closing press seen again and is dropped; every later one goes down
through that screen's own event slot (`lifecycle::hand_down`). The
tests `lifecycle::tests::the_screen_beneath_is_the_next_down_the_list_then_the_next_window`
and `list_class::tests::a_press_while_a_dialog_closes_goes_to_the_screen_beneath`
hold it.

Rules the tables hold:

- **An error clears on the next edit** (`CodeEntry::error`), and shows again with the next answer, the same error included.
- **Back cancels sign-in.** The hook takes Back as B, so the HLE's logon does not stay pending.
- **Only A and Start submit the code**, matching the legend (`signin_screen::ACTION_BUTTONS`, Start alone); X does nothing.

`h2 ui preview <screen> [--state S] --press up,right,a` steps a page,
prints each effect and the summary, and draws where it ends as
`<state>-pressed`.

### Item count

A list shows one row per item widget linked to its handler, and the
game is given exactly the rows our list lays out
([ui.md](../formats/ui.md) "Item widgets, scrolling and spacing"):

- **Linking.** "Add items" (`0x24bb60`, list slot 17) links as many of
  the handler's embedded items as slot 19 says. The Controls list's
  slot 19 (`0x2b198a`) says six whatever its tag lays out. Our list
  vtable's slot 19 (`list_class::item_count`) reads the visible count
  of the list block the screen's pane was built from (the tag the
  screen-tag function `0x22f871` serves, `list_class::laid_out`) and
  links that many, less the item widgets of ours a list taller than six
  gets (`list_class::item_widgets`). Embedded items an earlier call
  linked past that count are unlinked first.
- **What was wrong.** The layout (`0x24bc12`, called from the pane
  builder at `0x22f828` with its "drop the rest" argument 0) places
  children up to the visible count and leaves the rest. Rows go to
  every child in order (`0x24c0c4`), so on a list laid out shorter than
  six a row landed on a child never placed: not drawn, but Down reached
  it, A pressed it, and past it the list scrolled. Every list of ours
  built on the in-game Controller Settings tag (screen 200 lays out
  five) with six rows or more had one; so did the theater film page
  (four laid out, six actions).
- **Focus.** Down (`0x24c1c5`) moves to the next child when it has a
  row (`0x24ac76`); at the last child it scrolls while a row follows.
  At the end it wraps to the first row (`0x24c0c4`) when the list
  block's flag bit 0 is set (`0x24bc12` copies it to `+0x7c`), else it
  stops. Up is the same back. A presses the focused child's row
  (`0x24c024`); Start does too.
- **Model.** `list_class::game_list::GameList` is the list as the game
  holds it: item widgets, laid-out rows, rows, focus, scroll and arrows.
  `game_list::tests::a_list_laid_out_shorter_than_its_six_items_had_a_phantom_row`
  shows the retail count's phantom row, and
  `consistency::tests::every_list_links_the_rows_it_lays_out_and_the_focus_stays_on_them`
  holds every list of `NAMES` in every state: the item widgets are the
  rows laid out, the focus stays on placed rows, A presses only a real
  row, and a list whose rows all fit never scrolls.

A dialog with no layout of its own takes its visible count from its
row count when the class is built. A dialog whose rows outgrow that
count scrolls to the rest.

### Scroll arrows

The list draw (`0x24bda2`) shows the up arrow while the first item
widget's row has a row before it (`0x24c63e`) and the down arrow while
the last one's has a row after it (`0x24c676`), so a list whose rows all
fit shows neither. It places both from the list's top-left at the
skin's two points (`skin +0xc` and `+0x10`), at the size of the skin
`bitm`'s images 0 and 1. The down point is fixed: the `default` family
puts it beside the last row of the skin's own list, `(rows - 2) × 22`
below the anchor (`default5` -66 for five rows, `default6` -88 for
six, `default` -44 for four), and `setting_lists` puts it under the
Controls list's six. `list_class::SKIN_ARROWS` holds those numbers from
the disc. On a moved list of ours (`list_class::ScrollArrowLayout`,
through `UI_QUAD_TO_SCREEN`) the down arrow moves by the rows the list
lays out more or fewer than the skin's own (`SkinArrows::placed`), so it
keeps its place by the last row. The up arrow stays where the skin puts
it. A class with arrows of its own (`ListClass::scroll_arrows`, the
skulls page) keeps those. A list left where its tag has it keeps the
game's.

The preview draws them the same way (`view::scroll_arrows`, widgets
`scroll.up` and `scroll.down`, art `Art::Arrow`), and the `items` and
`scroll` consistency rules hold them.

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
  `code_entry`, `picker`, `hud`), `layout` (a list's `controls`,
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
`message`, `code_entry`, `picker` and `hud`, with `row`, `legend`,
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
`layout.dialog.over-screen`, `layout.code-entry`, `layout.picker` and
`layout.hud` (`ui_kit::components::specimens::PAGES`). A new screen
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
| a widget with an entry in its YAML (`title`, `body`, `legend`, a panel text, art) | rect (`at`, `size`), text | — |
| a text block `text.N`, a button `button.N` | — | rect (`panes[0].text_blocks[N].rect`, `buttons[N].rect`) |
| a bitmap `bitmap.N`, a list `list.N`, a table `table.N` | — | move only (`top_left`, `anchor`): the image or skin sets the size |
| a row label `row.N.label` | text (`rows.row.N.text`) | — |
| a settings dialog's `header.name`, `header.about` | text (`settings.name`; `settings.about`, or `settings.caller.help` when the page takes its about from its caller) | — |
| a settings dialog's `description` | text: the help of the row it shows (`rows.row.N.help`) | — |
| a settings dialog's `header.*`, `art.*`, `description`, moved | refused: `design.yaml` `components.settings.*` places it | — |
| any row widget, moved | refused: rows sit where the list's `layout` puts them | — |
| a widget with no YAML entry (`row.N.value`, `description`, `status`, `cell.N.*`) | refused: Rust fills it, and the refusal names the kind of value | — |
| a text with `{slots}` | the template is edited, and its slots must stay as they are (the refusal names a slot that went or came) | — |
| a legend made of `prompts` | text refused: edit its prompts | — |
| any retail text | — | text refused: retail strings live in the map's localized string table (`unic`), which the editor does not write. Its string ID is set with `ui.set_string` |
| `title` of a retail screen | — | refused: the header rectangle is the shared `wigl` header layout's |

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
