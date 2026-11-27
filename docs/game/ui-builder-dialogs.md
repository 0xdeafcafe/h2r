# UI builder: dialogs

The modal dialog, panel texts and the Dialog kind. Part of [ui-builder.md](ui-builder.md).

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

#### A dialog is an overlay on its opener

A retail dialog sits over whatever was open when
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
is 1.0 and, the call's growth being 1, each next one's is the last
times the spread factor `[0x0045dc0c]`; a draw's spread is its fraction
times its scale. Each draw blends four bilinear taps, tap `k` moved by
the spread times 0.5 (`[0x0045dbbc]`) in x for `k & 1` and in y for
`k & 2`, else times -0.5 (`[0x0045dbc4]`), with the sampler clamped at
the edges (`native_render::glow::quad`). The call's strength
(`[0x0045dc20]`) selects the combiner that blends them in pairs leaning
to the brighter tap; the alpha is their mean
([native-post-hud.md](../graphics/native-post-hud.md#the-blur-under-a-dialog)).
`Disc::backdrop` draws both at rest (fraction 1): the fill from
`SharedGlobals::overlay_color`, then `preview::backdrop::blur` on the
640 by 480 canvas. The preview's blur grows each scale by 1.25 and
takes the plain mean of the taps, so it differs from the game's: its
steps spread less and it does not lean to the brighter tap.

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
