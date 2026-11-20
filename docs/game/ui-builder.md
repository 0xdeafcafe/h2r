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
| `Dialog` | a retail dialog's tag (`DialogBase`): its backing art and header layout, our rows in its list, a body text or none ("The Dialog kind") | the base's: layout 1 (Quick Options) or 2 (large: title 498 wide, panel 536) | settings pages over the lobby (Clankers, Halo 1 options), questions, errors |
| `CodeEntry` | `Dialog` large, plus N cells: the Live passcode dialog's box art (screen 21's `password` blocks) with a digit text over each, the cursor cell tinted bright | layout 2 | the sign-in code |
| keyboard handoff | any kind; `Effect::Keyboard(field)` opens the game's keyboard (`signin_keyboard`), the typed text comes back as an `Input::Typed` | — | gamertag, invite |

**Backing.** A `Dialog` or `CodeEntry` is `Over` (drawn over the screen
beneath, as the game's own dialogs are) or `Own` (it carries the game shell
backing, `framing_center` from screen 5, under its panel, so the screen
beneath doesn't matter).

**Sign-in is a modal.** The sign-in
prompt and its new-account gamertag and invite pages are `Over` dialogs,
not full screens. They sit over whatever is beneath (the title screen, the
main menu, in game) the way the game's own dialogs do, in the large dialog
layout of `error_dialog_large` and `dialog_live_password` (header layout
2, about 536 wide): that dialog's own `dialog1`/`dialog2` panel art, the
digit cells and the legend. What the game's dialogs draw behind the panel
(a dim layer or nothing) is copied from the retail dialog tags, read off
them rather than assumed. A modal may cover the logo, as retail dialogs
do, so for a modal the reserved-area check ignores what the panel covers.
Everything in it must still sit inside the panel and inside the safe
area. The preview draws a modal over the screen that opened it (its
`Opener`, below), so the sheet shows it in place.

**The runtime tag recipe.** Our screens are still made by the Controls
constructor and adopted by `list_class`. The screen-tag function
(`0x22f871`) already serves them a tag copy (`list_class::copy_tag`); the
builder writes that copy from the `View` instead: the root flags pick the
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
h2 ui preview <screen> [--state NAME|all] [--focus N] [--synthetic] [--out DIR]
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

The art checks need the images' pixels, so the synthetic source skips
them, and says so (`Report::skipped`). The provenance check runs without a
disc for the screens our art comes from (Controls 39, level select 11).

The checks are `ui_kit::layout::check(view, source, rules)`, and
`h2 ui preview` prints them per state. Page-model tests check bounds
after every input from every named state, including the visible window
of a scrolling dialog. The agent's `ui.screen` applies the same
rectangle checks to instantiated widgets. The builder also uses font
metrics to catch text that exceeds its text box. The tests:

- unit tests with synthetic metrics, plus one with a solid-image source
  for the two pixel checks;
- every screen in every state passes with synthetic metrics (CI);
- every screen in every state passes against the disc with nothing
  skipped (the disc test).

No finding is left on purpose.

What the checks hold the screens to:

- **Unfocused rows stay readable.** Unfocused rows draw at about 40% alpha (`view::UNFOCUSED_ALPHA`, observed from a game screenshot, not traced); contrast on the real bars is 2.9 to 3.1.
- **The skulls page keeps none of the Controls art.** It copies the level select's own bracket (`ul_02`) at run time (`ListClass::frame`); the frame provenance check names a bracket borrowed from the wrong list. Its view draws the skull picture (`ScreenSpec::art`), untinted.
- **Legends sit in the footer**, not in the description block: the account menu's, and the co-op screens' (`A SELECT`, `START START GAME` or `READY`, `B BACK`, set in game from the co-op text table by `coop_live::prompts`).

**The full-height Game Options list.** The Game Options page lays out
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
geometries (synthetic and disc) draw that many rows. The bracket plays its own animation from the level select (2, read off the disc,
`skulls_menu::FRAME_ANIMATION`), so it slides out with the page and isn't
left drawn over the screen coming back on B. The sheet's files are named
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
`DialogSpec`'s `Opener`. On the disc, the preview draws the opener
(`Disc::opener_view`) and then the dialog over it, with nothing between
them, as the tags have nothing there. Synthetic previews draw the
opener's rows as boxes and text blocks.

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
hidden and no dim layer. So a `DialogSpec` names its `Opener`: the
opener's screen tag, the pane shown, its list's rows (the game's code
fills them) and the focused row. `Disc::opener_view` makes it a `View`:
the game shell, the pane's bitmaps and texts in the screen's own
strings, its header, and its rows on its own list skin with the focused
row lit (`view_of`'s list). The preview draws that under the dialog. The
openers:

- `Opener::MAIN_MENU`: the main menu on the Xbox Live row (rows as
  `ui_watch` reads them: Campaign, Xbox Live, Split Screen, System
  Link, Settings).
- `Opener::XBOX_LIVE_MENU`: the Xbox Live menu (screen `0xba`, pane 0)
  on Quickmatch, with our five rows.

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
  class with a description shows them.
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
settings dialog's description goes; its game type picture (bitmap 1) is
the retail dialog's content and is left out (`content_bitmaps`). A
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
animation); the rows show `ValueStyle::Setting` (the value alone in the
value column); and the focused row's description goes in a text block
equal to Quick Options' help line (`dialog::QUICK_HELP_TEXT`, held to the
tag's text block 0 by the disc test), which `KitScreen::dialog` and the
Halo 1 class use unless told otherwise. A description is one line there,
52 units at most (retail's is 51). The variant-name line stays out; the
game type picture is retail's content and stays out of the base, and a
page shows the focused row's icon there ("A page's header"). The pages'
titles are in capitals, as Quick Options' header is.

The test `ui_kit::screens::tests::our_settings_dialogs_are_quick_options_inside`
holds both pages to that (dialog base, list place, value style, help
block, one-line helps, capital title, footer) and draws each beside the
kit's Quick Options sample (`kit.dialog`): every panel, title, row bar,
label, value, help-line and legend widget the sample has is at the same
rectangle, with the same art or font and justification. The disc test
repeats that drawing on the disc's geometry.

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
   no game type picture. Compare with `kit.dialog` in the sheet, and with
   the lobby's own Quick Options side by side: the same row bars and
   focus highlight, the same two blues, the value alone ("Normal", not
   "< Normal >"), the focused row's description in the one line where
   Quick Options says "Temporarily change settings...", the legend in the
   same footer place.
3. The d-pad moves and changes rows; the lobby beneath doesn't move.
4. B closes the dialog (stderr shows no close by slot), and the lobby
   takes input again at once. Open it again: it opens the same way.
5. The title is ours (slot 18 sets it on the header after the pane is
   built, as on every class); note it if it reads "Quick Options".

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
description is the row's explanation (`Row::explain`,
`ChoiceRow::explain`, `OptionRow::explain`; "A page's header"), else
its description:

- `KitScreen` value rows (`Row::value`, so every `ChoiceRow`): the
  Clankers page, the kit's examples;
- the Halo 1 Options screen's two rows (`h1_options::pick`).

A on a value sets it through the row's own setter (the settings store:
saved, its change callbacks run) and closes; B closes and changes
nothing; X, Start, left and right do nothing there. Left and right on
the row itself still step. A row with one value, or more than
`picker::MAX_CHOICES` (61, its string-id family), keeps stepping on A.
The legend is `(A) SELECT (B) CANCEL`; the pages' own legends include
`(A) SELECT`.

Theater uses `picker::offer_list` for Sort, Map and Game type. Each opens
over the library with A, including a filter whose only entry is All.
Long lists reuse text slots as new rows scroll into view. The preview
composites the dialog over the library with its tab, rows and focus intact.

Not on the picker: the Video and Voice screens (`ui::Model` rows,
`Model::press`), the skulls (toggles, not settings) and the variant
editor's More Options page (`variant_page`, its own rows). They step
on A.

`h2 ui preview kit.picker` and `kit.picker.game` draw it (the kit's
Count, nine values, 4 held, over Game Setup).

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

The Clanker Options and Halo 1 Options pages
have Quick Options' header at their top: its picture and its one line
of help, both for the row that has the focus.

**Quick Options'** (see the table in "Quick Options inside"): bitmap
block 1, `mp_games2.bitm` at (-267, 166), depth 3, animation 31, the
variant's game type icon (eleven icons, 40 x 40); text block 1, the
variant's name in white `[163 -224 143 0]`; text block 0, the help line
`[143 -224 123 270]`, 494 units wide, one line. The bitmap widget's
constructor (`0x2b01eb`) zeroes its image index (`+0x88`) whatever its
block holds; code sets it after (`0x2b0a14`).

`KitScreen::header(icon)` (the Clankers page) and the Halo 1 Options
class do the same:

- the help line (`QUICK_HELP_TEXT`, already the pages' description
  block) shows the focused row's one line (`Rows::description`, a slot
  per row); the value picker A opens says more (below);
- the picture is Quick Options' block 1, copied and placed as Quick
  Options has it (`dialog::QUICK_PICTURE`, through `ListClass::picture`),
  so every row's icon sits where Quick Options' own does. Its image is
  the first row's icon once the widget is made (`Rows::picture_image`)
  and the focused row's each time the focus moves (`Rows::row_image`,
  both through `list_class::set_image`). `KitScreen::header`'s `icon` is
  for a row without one (`Row::icon`). Quick Options' own copy of the
  block stays out of the base (`content_bitmaps`), so only ours shows
  there.

`dialog::icon` names all eleven `mp_games2` icons by what they show:
Spartan, crown on a hill, lightning bolt, skull, helmet in a sunburst,
sack, flag, bomb, four squares, banner of three stars, question mark.

| Page | Row | Icon | Line in the help line |
|---|---|---|---|
| Clankers | Clankers | `SPARTAN` | Bots join this game and show in the lobby. |
| Clankers | Count | `SQUARES` | How many bots join, up to 15. |
| Clankers | Difficulty | `SKULL` | How well they play. Mixed varies it around Normal. |
| Clankers | Teams | `FLAG` | Which team they join in a team game. |
| Clankers | Aggression | `BOMB` | How eagerly they push into a fight. |
| Clankers | Aim style | `HELMET` | Steady tracks smoothly; Twitchy flicks. |
| Clankers | Taunts | `CROWN` | Whether they taunt after a kill. |
| Clankers | Glitches | `LIGHTNING` | Whether they BXR, sword fly and super bounce. |
| Halo 1 Options | Sandbox | `BOMB` | Weapons, vehicles and health, from the next game. |
| Halo 1 Options | HUD | `HELMET` | Shields, health, motion tracker and ammo counter. |

(Halo 1's Assets row, on the campaign's Game Options page, has
`SPARTAN`.) Each line is 52 characters at most, inside the 494-unit
help line on the kit's measure and in the disc's font 1. The white
variant-name line stays empty: the title bar already names the page.

**The picker's copy.** A on a row opens the value picker, whose
description says what the option does and what each choice means in a
few short sentences: `ChoiceRow::explain` (a kit value row's
`Row::explain`) and `h1_options::OptionRow::explain`, 200 characters at
most, so it fits the picker's description block (235 by 154 units, ten
lines of font 1).

The spec carries each row's icon as `RowSpec::art`
(`dialog::quick_icon`), and `ScreenSpec::shown_art` adds the focused
row's to the screen's own art, which `dialog_view` draws; the help line
is the focused row's description (`ScreenSpec::help` is unset). The disc
test holds `QUICK_PICTURE` to the tag's block, checks `mp_games2`'s
eleven icons are 40 high and 40 or 41 wide, and measures every row's
line in the help line's font; `our_settings_dialogs_are_quick_options_inside`
checks, for each row focused, its line, its icon and where the icon is
drawn.

### Its game run

From a custom game lobby (on a Halo 1 map for Halo 1 Options), Game
Setup: open Clanker Options. At its top left, left of the help line,
the Spartan icon where Quick Options shows its game type, and "Bots
join this game..." in the help line. Move down: the icon and the line
change with each row (Count's four squares, Difficulty's skull, and so
on), the icon never moving from its place. A on a row: the value
picker's right side explains the option. Halo 1 Options: Sandbox's bomb
and HUD's helmet. Open Quick Options to compare the places.

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
| Clanker Options... | image 1, the roster no retail row uses | `clankers::ROW_HELP`, a slot of the page's (`KitScreen::row_help`) |
| Halo 1 Options... | image 2, Quick Options' own | `h1_options::ROW_HELP` (`ROW_HELP_ID`, served with the other Halo 1 strings) |

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

The enabled lines follow the retail options that fit. When all five
rows are filled, each enabled line replaces a bottom retail option. The hook sets each extra
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
- **`ListPage`**: every list screen (Game Options, the Halo 1 options and
  multiplayer skulls, the three co-op screens, the kit's example) is its
  real `ListClass` stepped offline. An event goes through the class's own
  hook, then `list_class::route` (the same pure function the runtime's
  `event_with` uses: action buttons, then sideways steps), then the
  Controls handler: up and down move the focus, A presses the row
  (`Rows::press`), B and Back close. Effects: `Focus`, `Hook`, `Action`,
  `Stepped(line)`, `Open(screen)`, `Retail(line)`, `Close`. The rows keep
  their own state (an in-memory profile store, a lobby without Live), so
  each table case builds a fresh page. Whether the game wraps the focus
  at either end is not traced; the page stops it there, and the fidelity
  loop will settle it.
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
row of the list above, plus Game Options (Start Level's A runs the action,
X and Start don't, left, right and A step a skull, a locked skull stays
locked), the co-op screens (Start starts or readies, Invite a Friend opens
the friends list), the account menu, and every screen taking every input
in every state with its spec still passing the kit's checks.

Rules the tables hold:

- **An error clears on the next edit** (`CodeEntry::error`), and shows again with the next answer, the same error included.
- **Back cancels sign-in.** The hook takes Back as B, so the HLE's logon does not stay pending.
- **Only A and Start submit the code**, matching the legend (`signin_screen::ACTION_BUTTONS`, Start alone); X does nothing.

`h2 ui preview <screen> [--state S] --press up,right,a` steps a page,
prints each effect and the summary, and draws where it ends as
`<state>-pressed`.

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
- transitions and animations (the preview draws everything at rest);
- input timing: key repeat, the opener-press guard, and the keyboard's return;
- that the Controls constructor accepts a tag recipe whose list holds one item and whose pane has more text blocks than the Controls tag;
- the "Not verified" items of the modal, picker, header and Game Setup sections above, through their game runs.

Each new screen's run checklist lists exactly these for it.
