# Screens of ours: the UI kit

Status: Built. The kit and its static checks and offline preview are tested; the in-game checks are listed under "Not verified".
Code: `crates/extensions/game/src/ui_kit/`, `crates/extensions/game/src/list_class.rs`. Tests: `ui_kit::screens::tests`, `ui_kit::page::tests`, `string_ids.rs`.
Switch: none (a kit screen's texts are the feature `ui-kit-texts.<name>`).

Every screen we add to Halo 2's menus is a list screen of our own class
(`crates/extensions/game/src/list_class.rs`, [ui.md](../formats/ui.md) "Screen and list
classes"). Each one we built by hand failed in the same few ways first. The
kit (`crates/extensions/game/src/ui_kit/`) is there so those mistakes can't be made
again, and so a screen can be seen and checked before anyone runs the game.

| Module | What it gives a screen |
|---|---|
| `ui_kit::ids` | String ids by family name, from one registry checked at compile time |
| `ui_kit::input` | Events read as inputs, glyphs by name, legends, the opener-press guard |
| `ui_kit::lifecycle` | Closing as B does, and work that waits until the screen is freed |
| `ui_kit::spec` | A screen as data (`ScreenSpec`, the `Describe` trait) and the static checks |
| `ui_kit::screen` | `KitScreen`: a declared screen on the list class |
| `ui_kit::screens` | The skulls, co-op, sign-in and account screens described, with sample states |
| `ui_kit::preview` | A screen drawn offline from the disc's tags and fonts |

## Ids

String ids come from one registry, `ui_kit::ids::FAMILIES`, checked at compile time: no two families share an id, and a family whose lookup compares fewer than 32 bits says so with `mask`, so the registry checks that masked range against every other family (the co-op texts and the community variants once collided that way). A screen takes its ids by name (`ids::family("my.screen")`); "Adding a screen" has the steps.

## Input

`ui_kit::input::Input::from_event` is the one place an event is read. The screen's event slot (screen slot 10) is handed an event whose kind word (`+0`) is a direction or a button: the controller update raises kind 1 up, 2 left, 3 down and 4 right (d-pad or left stick), first when pushed and then every 250 ms while held. Buttons are kind 5 with the button in `+8` (A 0, B 1, X 2, Y 3, Start 12, Back 13), raised only on the frame the button goes down. Directions never arrive as buttons 8 to 11.

- **The opener's press.** The press that opens a screen can be seen again by it. `input::OpenerGuard` drops a confirm that comes within 50 ms of opening (`input::OPENER_GUARD`).
- **Glyphs.** Prompts are `Glyph`s, not raw code points (U+E100 A, U+E101 B, U+E102 X, U+E103 Y, U+E108 to U+E10B the d-pad, U+E10C Start, U+E10D Back). `input::footer_legend` lays them out as retail's footers do (below).

## Lifecycle

`ui_kit::lifecycle` opens a screen with `0x149f1e(player mask, loader)` and closes one with `0x14800c` (EAX the screen's slot `+0x78`, ECX its window `+0x74`), as the generic back handler closes on B. The close starts the out transition; the stack frees the screen when that ends, calling the destructor (screen slot 0) first.

Anything that must run on the screen underneath (a retail press, a level start) waits for the destructor: `lifecycle::AfterClose`, which `Then::CloseThen` uses for every kit screen. Running it while the page is still closing lets the closing page's teardown undo it (the skulls page's difficulty press stored the difficulty and the teardown reset it, so Start seemed stuck).

## Describe

A screen is data a check can read. `ui_kit::spec::ScreenSpec` says what a screen shows and what each row does, and the `Describe` trait (`fn spec(&self) -> ScreenSpec`) is what an existing screen that keeps its own `Rows` implements to get the checks and the preview. The static checks, the offline preview, the open-screen log line and the agent API's UI stack all read the same spec.

## Adding a screen

1. **Ids.** Add a family to `ui_kit::ids::FAMILIES`, with a free range and a
   name. The build fails if it overlaps another family, or if a family whose
   lookup compares fewer than 32 bits (`mask`) would claim one of its ids.
   Take the ids with `const IDS: Family = ids::family("my.screen");`. A
   `KitScreen` needs `2 + 3 × rows` slots (the title, a label, a value and
   a description per row, then the footer legend); `KitScreen::class` panics
   at install if the family is too small.
2. **Declare it.**

   ```rust
   const IDS: Family = ids::family("kit.example");
   let screen = KitScreen::new("kit-example", "kit.example", IDS, "Example")
       .row(Row::value("Grunts", &["On", "Off"], get, set).describe("Grunts' heads pop."))
       .row(Row::info("Status", || "Ready".into()))
       .row(Row::text("Name", 15, get_name, |t| format!("typed {t:?}")))
       .row(Row::action("Start", || Then::close_then(start_level)).describe("Start the level."))
       .prompts(&[
           Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
           Prompt::new(&[Glyph::A], "Start"),
           Prompt::new(&[Glyph::B], "Back"),
       ]);
   let screen = Arc::new(screen);
   registry.add(&KitTexts(Arc::clone(&screen)))?; // its texts, as a locale source (feature ui-kit-texts.<name>)
   registry.add(&screen.class())?;                 // the list class
   ```

   Row kinds: `Row::action` (A runs it; it returns `Then::Stay(log)`,
   `Then::Open(screen)`, `Then::Close` or `Then::CloseThen(work)`),
   `Row::value` (left and right step through the values and wrap; A opens
   the value picker on them, retail's `low_level_settings`, see
   [ui-builder.md](ui-builder.md) "The value picker"; a toggle is two values), `Row::info` (read-only), and `Row::text`
   (A opens the game's keyboard; the typed text comes back once the keyboard
   closes). `.layout(ListLayout, ValueStyle::Inline)` moves the list onto a
   one-text skin and puts values in the label (`Mythic: < On >`), as the skulls
   page does. `.description(TextBlock)` adds the focused row's description.
   Use `.live_help(block, get_text)` for a line shared by all rows, such as
   the Clanker profile. It redraws when the text changes even if focus stays
   on the same row.
   `.dialog(DialogBase::QuickOptions, opener)` makes the screen a dialog of its
   rows over whatever opened it, Quick Options inside and out: its panel and
   header, its own list (`setting_lists`, eleven rows from (-254, 87)), each
   row's name left and its value alone in the value column
   (`ValueStyle::Setting`: `Count  4`, no arrows), and the focused row's
   description in its one-line help line (`dialog::QUICK_HELP_TEXT`, 52
   units at most) unless `.description` names another block. A settings
   page that should look like the game's own (Clankers, Halo 1 Options)
   is this and nothing more: no `.layout`, no `.description`, plus
   `.header(icon)` for Quick Options' game type picture, which shows the
   focused row's `Row::icon` ([ui-builder.md](ui-builder.md) "A page's
   header"). `Row::explain` is what the value picker says about a value
   row, when it should say more than the one-line description. Or
   `DialogBase::Large`, the one the game maps have; see
   [ui-builder.md](ui-builder.md) "The Dialog kind". The prompts go in the footer
   (below). `.scrolls()` says more rows than the list shows is meant.
   **Choice rows.** A row of many choices over a setting is a
   `ui_kit::choice::ChoiceRow`, the sibling of the Halo 1 options'
   two-choice `OptionRow`. It names a settings `Def`, a label and a
   description; its choices come from the setting (a `Kind::Choice`'s
   labels, a `Kind::Int`'s steps or else every number of its range, Off and
   On for a `Kind::Bool`). `ChoiceRow::row(store)` gives the value row:
   left and right step and wrap, A opens the value picker, and each change
   is saved in the store at once, so its change callbacks see it.
   `.explain(text)` gives the value picker its fuller text and
   `.icon(icon)` the row's header icon. A bots page's Count and
   Difficulty are two of them:

   ```rust
   static COUNT: ChoiceRow = ChoiceRow::new(&BOTS_COUNT, "Count", "How many bots join.")
       .explain("How many bots join this game, from 1 to 15.")
       .icon(icon::SQUARES);
   static LEVEL: ChoiceRow = ChoiceRow::new(&BOTS_LEVEL, "Difficulty", "How well they play.")
       .icon(icon::SKULL);
   let page = KitScreen::new("bots", "bots", ids::family("bots"), "CLANKERS")
       .dialog(DialogBase::QuickOptions, Opener::GAME_SETUP_CLANKERS)
       .header(icon::SPARTAN)
       .row(COUNT.row(Arc::clone(&store)))
       .row(LEVEL.row(store))
       .prompts(&[
           Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
           Prompt::new(&[Glyph::B], "Back"),
       ]);
   ```

   The kit's own example is `kit.choices` (`h2 ui preview kit.choices`,
   over example settings no profile holds); its table test is
   `ui_kit::page::tests::kit_choices_page`.
3. **Open it** from an `Open(name)` row on another screen of ours, or with
   `list_class::open(cx, chain, name)` (`0x149f1e(player mask, loader)`).
   `0x149f1e` always opens on window 5. A screen opened from inside a
   lobby dialog (window 3) has to open on the pressing screen's own
   window and slot instead, or it ends up under the lobby. For that, a
   `Rows::press` returns `Chosen::Push(name)` (`list_class::push`), with a
   request built the way the Halo 1 Options row builds its own.
4. **Check it.** Add the screen to `ui_kit::screens::NAMES` and `sample`. The
   tests then check it without the disc, and against the disc when it's
   there (below).
5. **Preview it.**
   `h2 ui preview <screen> [--state NAME|all] [--focus N] [--press up,a,...] [--disc disc.iso] [--out DIR]`
   prints the log line and every check, then draws each state as a PNG
   with a JSON of its widget rects beside it; `h2 ui sheet` draws every
   screen in every state on one page ([ui-builder.md](ui-builder.md)). The test
   `ui_kit::screens::tests::preview_and_check_against_the_disc` with
   `H2_UI_PREVIEW_DIR=<dir>` draws every screen at its first and last row.
   Previews go to a scratch or keep directory, never into git (they come from
   the disc).
6. **Then the game run** (the user does it): go through the in-game checklist
   below.

An existing screen that keeps its own `Rows` gets the checks and previews by
implementing `ui_kit::spec::Describe` (see `ui_kit/screens.rs` for the skulls
and co-op ones, and `signin_screen::spec_for`).

## The checklist

Before a game run (all of these are tests):

- [ ] **Ids**: the family is in `ids::FAMILIES` and the build passes. Every
      id the screen shows is in a family (`Finding::Id`). `string_ids.rs`
      also checks each family's real lookup function against every other
      family's ids.
- [ ] **Rows versus slots**: a list built on the Controls tag shows 5 rows
      (`preview::SHOWN`, seen in game; the tag says 6, see "Open"). More
      rows than that needs `scrolls` (`Finding::Rows`).
- [ ] **Widths**: every label and every value a row can show (not just the
      current one) fits its skin text: 245 units for the Controls skin's label
      and value columns in conduit-13, 190 for skin 0's single text in
      conduit-12 (`Finding::Wide`).
- [ ] **Description**: the focused row's description fits the description
      rectangle at the font's line height (`Finding::Description`).
- [ ] **Footer**: the legend fits the header layout's second rectangle in
      the header font (`Finding::Footer`).
- [ ] **Glyphs**: every character is drawn by the font. The icon range's
      filler record counts as missing (`Finding::Glyph`).
- [ ] **Presses**: an action row does something, a value row has at least two
      values and the screen takes left and right, and a text row opens the
      keyboard (`Finding::Press`).
- [ ] **Lifecycle**: anything that must happen on the screen underneath runs
      from `Then::CloseThen`, never straight after `close`.
- [ ] **Preview**: look at the PNG. Red outlines mark overflowing texts; a red
      bar under the list marks rows past the visible ones with no scroll
      intent.

In game, in stderr, in this order:

1. `ui: opening <name>` (from an `Open` row), or the opener's own line.
2. `<feature>: opened with N rows (our list class)`. N must be the row
   count; fewer means the loader didn't size the array.
3. `<feature>: <name>: "<Title>"; rows: [0] "Label" = "value" ...; focus 0; prompts [A] ...`.
   This is `ScreenSpec::log_line`, the same text the checks and the preview
   saw (kit screens). The co-op screens log their own `title ...; rows:` line.
4. `<feature>: row text`, once: the list drew.
5. Left or right on a value row: `<name>: right on row R: <set's line>`.
6. A on an action row that closes: `<name>: closing`, or
   `<name>: closing; the rest runs once 0x... is freed`, then
   `<name>: 0x... is freed; running what waited`. B logs
   `<name>: 0x... is freed (B)`.
7. A within 50 ms of opening:
   `<name>: a confirm N ms after opening: the press that opened it; ignored`.
8. A text row: `<name>: opening the keyboard for row R (N characters)`, then
   the row's `typed` line once the keyboard closes.

Look at the screen itself for: the title in the header, every row's text
whole, the focus moving over every row including past the visible ones, the
description updating with the focus, the legend's glyphs drawn as buttons, B
going back to the screen underneath, and nothing left on screen after it
closes.

The Mods browser and pack use the level-select bracket, list and picture.
A star marks a new map, a tick marks an installed map, and a down arrow
shows download progress. A failure says `Failed` on the row. A cold list
opens with one waiting row. Its class reserves forty slots through
`Rows::growing_capacity` and adds the returned rows without changing the
focused handle. Later catalogue updates wait for the next visit. A pack
has room for Download All and all forty maps. The action count includes
only maps eligible to start or retry. The browser uses the base map's
bitmap or the neutral lobby picture. Change Map and the lobby use the
selected slot's art. [Map selection](map-select.md)

The detail page uses the pregame lobby's action row, status panel, picture
and table. Facts and changes are `ScreenSpec::texts` over
`ScreenSpec::panel`. Each fact has separate label and value rectangles.
The action row retains focus while the download progresses or removal
awaits confirmation. The Mods classes return a `Rows::focused_footer`
slot when focus or download state changes. The list class sets it on the
footer widget when it sets the description. The preview reads the same
commands from the focused `RowSpec::prompts`.

The skulls page logs `skulls page: ...` lines ([skulls.md](skulls.md) "What it
logs"); the sign-in prompt logs `signin prompt: ...` lines ([live.md](../live/live.md)).

## Rules and limits

- **Continue on A over the focused row only.** A selects the focused row (Start Level goes on, a value row changes), B goes back, and the class takes no extra buttons (`ListClass::action_buttons(&[])`). A list has a visible Start Level row first. X, Start or A over any row as a continue reads as a mystery.
- **Give a widget of ours the animation of the retail screen it copies.** A text or bitmap block with animation 0 has no transition: it stays fully drawn through its screen's out transition (`TextBlock::animation`, `PictureSpec::animation`).
- **Our screens are built on the Controls tag, whose list lays out 6 rows (5 seen).** A layout that wants the page's height sets `ListLayout::visible`; the rows past the Controls handler's six item widgets come from item widgets of ours (`list_class::extra_items`).
- **Run a level start from the press with our screen up when the press itself pushes the next screen** (the difficulty's loading screen). Running a retail press from the destructor of a closed screen of ours makes the player watch the screen underneath come back first, and races the stack when the close comes early. Otherwise run it after the close (Lifecycle).
- **Family ids are never hand-picked.** The registry fails the build on an overlap, including a masked family's range (Ids).
- **Read an event only through `Input::from_event`** (Input).
- **The footer holds the legend; the description holds the description.** Retail never puts a legend under the description: every game shell screen has a footer text widget (`screen +0x338`, made with the header at `0x22ff53`, in the `wigl` header layout's second rectangle, filled from the tag's `panes[0] +2` string by `0x2300cf`), and retail's footer strings read `<glyph> SELECT  <glyph> BACK` (glyph, space, capitals; two spaces between prompts). Kit screens and the skulls page set the footer (`ListClass::footer`, `list_class::setup_pane_footer`), and the skulls page's footer is checked against `input::footer_legend` glyph for glyph.
- **Check row count, text width, where the description sits and which glyphs draw before a game run.** The checks and the preview cover all of that; what is on top of what they do not.

## The preview

`preview::Disc::render` draws a screen the way the game shell does, from
`mainmenu.map` and the disc fonts:

- the `game_shell_background` screen's bitmaps (the header and footer
  bands of `framing_center`, the track lines), then the Controls tag's
  frame art (left of the cut for a moved list), lowest render depth first,
  each at its image's size with its top-left at the block's point;
- the header text and the footer legend in the `wigl` header layout's two
  rectangles, header font and colour;
- each item: the skin's bitmaps, then its texts with their justification
  (`tags::ui::justification`: 0 left, 1 right, 2 centred), focused bright,
  others at 55%; the frame (flag bit 0) on every row, lit on the focused
  one; text with a drop shadow, button glyphs in the font's own colours.

### Font ids

A text block's font index (`+0xa`, read at `0x2537d7`; flag
bit 3 forces 1) becomes the widget's font id. Every text measure and draw
maps an id through the 11-entry table at `0x4e28f4` (`0x1407d8`,
`0x122540`) to a slot (`0x1224c0`, under 10, stride `0x1d0` from
`0x4e2920`), and the font init (`0x121fc0`) fills those slots line by line
from `font_table.txt`. So id N is line N: 1 conduit-12, 2 the header font,
4 conduit-13. The widget's draw call itself was followed only as far as that
table.

### Item spacing

An item's height is its bounding box over its texts and
its bitmaps without flag bit 0 (`0x2bafa4`). Inside an item a bitmap covers
its point's `y` up to `y` plus its height (Y up): the Controls skin's row
bar (508 by 26 at 0,0) then gives 26, the spacing in the game's screenshot.
Taking the point as the bitmap's top (as screen-level bitmaps are) would
give 49 for the Controls skin, which the screenshot rules out. Skin 0
(the skulls page) gives 22 by the same rule.

### Differences from the game

Drawn beside a 640 by 480 frame of the game, the co-op screen focused on row 4 lines up in rows, bars, frame and header. What differs (not verified further):

- the game fills above and below `framing_center` with dark blue (we leave it black), and the track lines scroll;
- unfocused rows' dimming is a guess (55%);
- the D-pad left and right glyphs are drawn from the header font as the font has them (a white diamond each);
- the skulls page's picture and the list's scroll arrows are not drawn.

## Not verified

- 5 or 6 Controls rows. `0x24bc12` lays out up to the list block's
  visible count, which the Controls tag gives as 6, and the Controls handler
  embeds 6 items. Yet in game our co-op screen (6 rows on the Controls tag)
  shows 5. The Xbox Live menu (a retail list, not ours) has one child item
  widget per row, six with the co-op row, laid out in its own tag's room
  ([live-custom-games.md](../live/live-custom-games.md) §5). The checks use the 5 seen; what
  takes one away is not found.
- Whether the footer widget keeps our legend, or the screen's tag string
  (`0x2300cf`) overwrites it after the pane is set up.
- Whether an overlong description clips or runs past its rectangle.
- Whether a screen opened from inside another call draws on top.
- The kit's keyboard row: it opens the game keyboard for controller 0 with
  context 20 (no check on Done). The sign-in prompt's keyboard path is the
  proven one.

## Related

[ui-builder.md](ui-builder.md), [ui.md](../formats/ui.md), [skulls.md](skulls.md), [map-select.md](map-select.md).
