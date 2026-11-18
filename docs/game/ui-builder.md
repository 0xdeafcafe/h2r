# The UI builder: native Halo 2 screens built, seen and proven offline

A native screen is one we write in Rust and the game draws with its own
widgets ([ui-screens.md](ui-screens.md) is the kit it grows from). The
builder's job: an agent writes a screen, sees it in every state, and proves
how it looks and behaves, all without starting the game. A game run then
only confirms what offline work can't reach (the last section).

The sign-in prompt is the first screen built on it. The account, co-op and
skulls screens follow.

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
| `List` | the Controls tag (today's `KitScreen`) | header layout 0 | settings-style lists over the game shell |
| `Dialog` | our tag recipe: dialog backing art, a body text, a skin 0 list of buttons | layout 3 (small: title 244 wide, panel 278) or 2 (large: title 498 wide, panel 536) | questions, progress, errors |
| `CodeEntry` | `Dialog` large, plus N cells: the Live passcode dialog's box art (screen 21's `password` blocks) with a digit text over each, the cursor cell tinted bright | layout 2 | the sign-in code |
| keyboard handoff | any kind; `Effect::Keyboard(field)` opens the game's keyboard (`signin_keyboard`), the typed text comes back as an `Input::Typed` | — | gamertag, invite |

**Backing.** A `Dialog` or `CodeEntry` is `Over` (drawn over the screen
beneath, as the game's own dialogs are) or `Own` (it carries the game shell
backing, `framing_center` from screen 5, under its panel, so the screen
beneath doesn't matter).

**Decision (2026-11-18, the user): sign-in is a modal.** The sign-in
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
area. The preview draws a modal over a sample screen beneath (the main
menu), so the sheet shows it in place. This replaces the earlier plan of
an `Own` sign-in screen on the shell backing.

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
the checks treat the same way.

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

`preview` writes to `$TMPDIR` by default, and `sheet` to
`$RUSH_KEEP/ui-builder/`. `--synthetic` draws without a disc.

**Built (slice 2).**

- `ui_kit::view` holds `View`, `Widget`, `Art`, `Metrics` and `Synthetic`,
  plus `Geometry` and its disc-free `Geometry::fallback`. The disc test
  checks the fallback against the disc, field for field.
- `view_of` turns today's `ScreenSpec`s into views.
- `preview::Source` has two implementations: `Disc` and `SyntheticSource`.
  `render_view` draws a view from either; `write_states` and `write_sheet`
  write the files.
- `screens::states` names each screen's states: a list's first, middle and
  last focus, and the sign-in prompt's 11 pages.
- Sheet labels use a built-in 3 by 5 pixel font, so a synthetic sheet is
  readable too. `cargo test` draws the synthetic sheet; the disc test with
  `H2_UI_SHEET_DIR=<dir>` draws the real one.

Today's screens come in as `ScreenSpec`s, from their `Page` (slice 4,
below). A spec with `ScreenSpec::dialog` set is drawn as a modal
(slice 6, "The modal dialog" below).

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
| safe area | texts and row art stay inside the title-safe area: x ±272, y 204 down to -211 (the middle 85%, down to the bottom of retail's footer rectangle, where the game's own legends sit); frame art is exempt |
| contrast | every text against what is drawn under it (the view rendered without its texts), WCAG ratio at least 2.5 |
| legend | a screen that takes inputs past up, down, A and B shows a button legend (a text with a button glyph) |
| focus | the focused row's text is drawn on screen, and more opaque than the other rows' |
| glyphs | every character exists in its font (the kit's `Finding::Glyph`) |
| ids | every string id is in a registered family (the kit's `Finding::Id`) |

The art checks need the images' pixels, so the synthetic source skips
them, and says so (`Report::skipped`). The provenance check runs without a
disc for the screens our art comes from (Controls 39, level select 11).

**Built (slice 3)**: `ui_kit::layout::check(view, source, rules)`, and
`h2 ui preview` prints it per state. The tests:

- unit tests with synthetic metrics, plus one with a solid-image source
  for the two pixel checks;
- every screen in every state passes with synthetic metrics (CI);
- every screen in every state passes against the disc with nothing
  skipped (the disc test).

No finding is left on purpose. Sign-in's missing legend was the last,
and slice 6 gave it one.

The checks' first findings, fixed:

- **Unfocused rows nearly invisible.** The preview dimmed row text to 55%
  of the skin's blue. The game draws row text white, and unfocused rows
  at about 40% alpha: the 2026-11-16 screenshot peaks at (229, 230, 231)
  focused and (103, 109, 118) unfocused over a (27, 37, 56) bar. Rows now
  measure a contrast of 2.9 to 3.1 on the real bars, and the old model
  about 1.5. `view::UNFOCUSED_ALPHA` is observed, not traced.
- **The skulls page's frame.** It kept the Controls bracket (`ul_03`)
  under a list moved 39 units right on another skin. The bracket framed an
  empty box, and the first row's end cap cut its arm off. The frame
  provenance check names it. The page now keeps none of the Controls art
  and copies the level select's own bracket (`ul_02`) at run time
  (`ListClass::frame`). Its view also draws the skull picture now
  (`ScreenSpec::art`), untinted.
- **The account menu's legend** was in the description block, on top of
  rows 1 and 2. It is in the footer now.
- **The co-op screens had no legend.** They now have footers (`A SELECT`,
  `START START GAME` or `READY`, `B BACK`), set in game from the co-op
  text table (`coop_live::prompts`).

**The full-height Game Options list (master's 13 rows).** The skulls
page became the Game Options page, and its list now lays out 13 rows
(`skulls_menu::VISIBLE_ROWS`), down to y -156. The level select's bracket
(`ul_02`, `[177 -259 -79 -131]`) ends at -79, so rows 8 to 13 run past
the bottom of its image, and its visible art ends sooner, about the fourth
row. Nothing is flagged, and that is right: the bracket is a top-left
corner piece, not a box. The level select itself lays out 16 rows at the
same anchor under the same bracket (its list block's visible count), so
rows running past it are retail's own composition. A "list longer than
its frame" check would flag the game's screen, so there is none.
`ListPlace::Moved` carries the layout's visible count, and both
geometries (synthetic and disc) draw that many rows. The bracket now also
plays its own animation from the level select (2, read off the disc,
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

## The modal dialog (slice 6)

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
rectangle. The view's backing is `Backing::Over(Beneath::MainMenu)`. On
the disc, the preview draws the retail main menu (`render_retail`) and
then the dialog over it, with nothing between them, as the tags have
nothing there. Synthetic previews draw boxes for the menu's list.

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

`spec_for` keeps the one-row list the game shows today and adds the
dialog, which the preview, the sheet and the editor draw. All 11 states
pass every check, synthetic and on the disc. X no longer submits: only A
and Start do, matching the legend (`signin_screen::ACTION_BUTTONS`).

**The screen beneath is drawn in full (the user, 2026-11-18).** A
retail dialog over the main menu has the whole menu under it: its logo,
its list and its scrolling track lines, with no dim layer. The preview
draws exactly that, so the backdrop stays as it is.

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

**The tweak, built.** The body starts at 72, as retail's text blocks
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

**Not built yet: the runtime.** In game, the prompt is still the
one-row list on the Controls layout. Serving the dialog needs the tag
recipe ("Screen kinds" above): a tag copy with header layout 2, the
panel and cell art, and our text blocks. It also needs the dialog opened
the way the game opens its own (the window, and whether the screen
beneath stays drawn). Both need a game run.

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

**Built (slice 4)**: `ui_kit::page`.

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
  `event_with` now uses: action buttons, then sideways steps), then the
  Controls handler: up and down move the focus, A presses the row
  (`Rows::press`), B and Back close. Effects: `Focus`, `Hook`, `Action`,
  `Stepped(line)`, `Open(screen)`, `Retail(line)`, `Close`. The rows keep
  their own state (an in-memory profile store, a lobby without Live), so
  each table case builds a fresh page. Whether the game wraps the focus
  at either end is not traced; the page stops it there, and the fidelity
  loop will settle it.
- **`AccountPage`**: the account screen (`AccountScreen`, already pure),
  with typed text and server answers as events.
- **`SigninPage`** (`signin_screen`): the prompt plus the code being
  typed. A press goes through `CodeEntry::press` as on the screen; what the
  HLE then does follows `live_hle::signin`'s own transitions, now pure
  functions the HLE uses too: `typed` (keyboard text on a page), `refused`
  (a code that isn't six digits) and `settled` (the page after a server
  call ends). Typed text and server answers are events.

The tables (`ui_kit/page/tests.rs`, `live_hle::signin` tests) cover every
row of the list above, plus Game Options (Start Level's A runs the action,
X and Start don't, left, right and A step a skull, a locked skull stays
locked), the co-op screens (Start starts or readies, Invite a Friend opens
the friends list), the account menu, and every screen taking every input
in every state with its spec still passing the kit's checks.

Behaviour the tables changed:

- **An error clears on the next edit** (`CodeEntry::error`): the code page
  hid nothing before, so "Wrong code" stayed up while a new code was typed.
  It shows again with the next answer, the same error included.
- **Back cancels sign-in.** Back closed the screen through the Controls
  handler without telling the HLE, so the logon stayed pending. The hook
  now takes Back as B.
- **X no longer checks the code.** X and Start were both the class's
  action buttons, so a stray X sent a half-typed code. Now only A and
  Start submit, matching the legend (`signin_screen::ACTION_BUTTONS`,
  Start alone), and X does nothing.

`h2 ui preview <screen> [--state S] --press up,right,a` steps a page,
prints each effect and the summary, and draws where it ends as
`<state>-pressed`.

## In the editor

**Built (editor slice)**: the editor's fourth tool, **UI Screens**
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

The preview is only worth trusting once it has been measured against the
game:

1. Pick retail screens the preview can draw from tags alone: Controls, the
   main menu, `error_dialog_ok_cancel`, `dialog_live_password`. Render
   them with the same renderer as our screens.
2. Capture the same screens in game with `ui.screenshot` (agent API phase
   2; it writes the presented frame and the native screens' widget rects).
   The user runs one command against the game they have up. Agents never
   start the game or `h2 serve`.
3. `h2 ui compare <capture.png> <screen> [--state NAME]` reports, per
   widget, the rect error in units (where the capture has rects) and the
   pixel difference inside each rect, and writes a diff image.
4. Fix the renderer until those numbers are small. Then our own screens'
   previews are trusted to the same margin.

References, captures and diffs live under `$RUSH_KEEP/ui-builder/`, never
in git.

## What stays unverifiable offline

A game run has to show:

- draw order: our dialog on top of the screen beneath, and the footer
  widget keeping our legend (`0x2300cf` may refill it from the tag);
- alpha 0 hiding a widget, and tint changes taking effect while the
  screen is up;
- the backing's blend: whether `framing_center` and `dialog1`/`dialog2`
  draw as opaque as the preview shows;
- transitions and animations (the preview draws everything at rest);
- input timing: key repeat, the opener-press guard, and the keyboard's
  return;
- that the Controls constructor accepts a tag recipe whose list holds one
  item and whose pane has more text blocks than the Controls tag.

Each new screen's run checklist lists exactly these for it.
