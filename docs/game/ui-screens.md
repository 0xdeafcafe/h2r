# Screens of ours: the UI kit

Every screen we add to Halo 2's menus is a list screen of our own class
(`crates/boot/src/list_class.rs`, docs/formats/ui.md "Screen and list
classes"). Each one we built by hand failed in the same few ways first. The
kit (`crates/boot/src/ui_kit/`) is there so those mistakes can't be made
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
   registry.add(&KitTexts(Arc::clone(&screen)))?; // its texts, as a locale source
   registry.add(&screen.class())?;                 // the list class
   ```

   Row kinds: `Row::action` (A runs it; it returns `Then::Stay(log)`,
   `Then::Open(screen)`, `Then::Close` or `Then::CloseThen(work)`),
   `Row::value` (left and right step through the values and wrap; A steps
   right; a toggle is two values), `Row::info` (read-only), and `Row::text`
   (A opens the game's keyboard; the typed text comes back once the keyboard
   closes). `.layout(ListLayout, ValueStyle::Inline)` moves the list onto a
   one-text skin and puts values in the label (`Mythic: < On >`), as the skulls
   page does. `.description(TextBlock)` adds the focused row's description. The
   prompts go in the footer (below). `.scrolls()` says more rows than the list shows is
   meant.
   **Choice rows.** A row of many choices over a setting is a
   `ui_kit::choice::ChoiceRow`, the sibling of the Halo 1 options'
   two-choice `OptionRow`. It names a settings `Def`, a label and a
   description; its choices come from the setting (a `Kind::Choice`'s
   labels, a `Kind::Int`'s steps or else every number of its range, Off and
   On for a `Kind::Bool`). `ChoiceRow::row(store)` gives the value row:
   left and right step and wrap, A steps right, and each step is saved in
   the store at once, so its change callbacks see it. A bots page's Count
   and Difficulty are two of them:

   ```rust
   static COUNT: ChoiceRow = ChoiceRow::new(&BOTS_COUNT, "Count", "How many bots join.");
   static LEVEL: ChoiceRow = ChoiceRow::new(&BOTS_LEVEL, "Difficulty", "How well they play.");
   let page = KitScreen::new("bots", "bots", ids::family("bots"), "Clankers")
       .layout(skulls_menu::LIST_LAYOUT, ValueStyle::Inline)
       .description(skulls_menu::DESCRIPTION_TEXT)
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
   screen in every state on one page (docs/game/ui-builder.md). The test
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

The skulls page logs `skulls page: ...` lines (docs/game/skulls.md "What it
logs"); the sign-in prompt logs `signin prompt: ...` lines (docs/live/live.md).

## Pitfalls we hit, and why

- **A continue on X, or on A over any row.** The skulls page went on with
  A on any row, X or Start, and X read as a mystery. It now does what the
  game's own lists do: a visible Start Level row first, A selects the
  focused row (Start Level goes on, a value row changes), B goes back, and
  the class takes no extra buttons (`ListClass::action_buttons(&[])`).
- **A widget of ours left on screen as its screen leaves.** A text or
  bitmap block with animation 0 has no transition: it stays fully drawn
  through its screen's out transition. Give it the animation the retail
  screen it copies uses (`TextBlock::animation`, `PictureSpec::animation`).
- **A list that stops a third of the way down.** Our screens are built on
  the Controls tag, whose list lays out 6 rows (5 seen). A layout that
  wants the page's height sets `ListLayout::visible`; the rows past the
  Controls handler's six item widgets come from item widgets of ours
  (`list_class::extra_items`).
- **A level start that waits for a close.** Running a retail press from
  the destructor of a closed screen of ours makes the player watch the
  screen underneath come back first, and races the stack when the close
  comes early. When the press itself pushes the next screen (the
  difficulty's loading screen), run it from the press with our screen up.

- **Start "stuck" on the skulls page.** The kept difficulty press
  (`0x2c9d38`) ran while the page was closing. It stored the difficulty, and
  the closing page's teardown reset it. The fix: close first (`0x14800c`) and
  run the press from the destructor slot, once the stack has freed the page.
  `lifecycle::AfterClose` and `Then::CloseThen` do this for every screen.
- **The press that opened a screen, seen again.** A button event is raised
  only on the frame the button goes down, so a held A gives no second event.
  But a continue that comes within 50 ms of opening is the opener's press,
  and is dropped (`input::OpenerGuard`; `H2_SKULLS_GUARD_MS` for the skulls
  page).
- **The skull labels over the campaign picker's ids** (`0x22fff0xx`). Both
  were hand-picked bases. The registry now fails the build on an overlap.
- **Co-op texts showing "Zombies" and "SWAT".** The co-op ids were
  `0x20ffe0xx`, and the community variants' lookup then matched only the low
  24 bits (`0x??ffe0xx`), so it answered first. The co-op texts moved to
  `0x20ffd0xx`, and the variants now match the whole id. A family whose lookup
  compares fewer bits has to say so with `mask`, and the registry then
  checks that masked range against every other family.
- **The d-pad as buttons 8 to 11.** Directions never arrive that way. The
  controller update raises event kinds 1 up, 2 left, 3 down and 4 right (d-pad
  or left stick), first when pushed and then every 250 ms while held. Buttons
  are kind 5, only on the frame they go down (A 0, B 1, X 2, Y 3, Start 12,
  Back 13). `input::Input::from_event` is the one place an event is read.
- **Raw code points in prompts.** `\u{e10a}` reads as nothing in review.
  Prompts are `Glyph`s (U+E100 A, U+E101 B, U+E102 X, U+E103 Y, U+E108 to
  U+E10B the d-pad, U+E10C Start, U+E10D Back). `input::footer_legend` lays
  them out as retail's footers do (below), and the skulls page's footer is
  checked against it glyph for glyph.
- **A legend under the description.** On the skulls page it pushed 10 of
  the 21 descriptions past the 5 lines their rectangle holds. Retail never
  puts a legend there: every game shell screen has a footer text widget
  (`screen +0x338`, made with the header at `0x22ff53`, in the `wigl`
  header layout's second rectangle, filled from the tag's `panes[0] +2`
  string by `0x2300cf`), and retail's footer strings read
  `<glyph> SELECT  <glyph> BACK` (glyph, space, capitals; two spaces between
  prompts). Kit screens and the skulls page now set the footer
  (`ListClass::footer`, `list_class::setup_pane_footer`) and keep the
  description to the description.
- **Shipping blind** on row count, text width, where the description sits,
  which glyphs draw, and what's on top. The checks and the preview cover all
  but the last.

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

**Font ids.** A text block's font index (`+0xa`, read at `0x2537d7`; flag
bit 3 forces 1) becomes the widget's font id. Every text measure and draw
maps an id through the 11-entry table at `0x4e28f4` (`0x1407d8`,
`0x122540`) to a slot (`0x1224c0`, under 10, stride `0x1d0` from
`0x4e2920`), and the font init (`0x121fc0`) fills those slots line by line
from `font_table.txt`. So id N is line N: 1 conduit-12, 2 the header font,
4 conduit-13. The widget's draw call itself was followed only as far as that
table.

**Item spacing.** An item's height is its bounding box over its texts and
its bitmaps without flag bit 0 (`0x2bafa4`). Inside an item a bitmap covers
its point's `y` up to `y` plus its height (Y up): the Controls skin's row
bar (508 by 26 at 0,0) then gives 26, the spacing in the game's screenshot.
Taking the point as the bitmap's top (as screen-level bitmaps are) would
give 49 for the Controls skin, which the screenshot rules out. Skin 0
(the skulls page) gives 22 by the same rule.

**Side by side.** With `H2_UI_COMPARE=<png>` (a 640 by 480 frame of the game)
and `H2_UI_PREVIEW_DIR`, the disc test also writes `coop-vs-game.png`: the
co-op screen focused on row 4 beside the frame. Rows, bars, frame and
header line up. What differs:

- the game fills above and below `framing_center` with dark blue (we
  leave it black), and the track lines scroll;
- unfocused rows' dimming is a guess (55%);
- the D-pad left and right glyphs are drawn from the header font as the
  font has them (a white diamond each), not checked in game;
- the skulls page's picture and the list's scroll arrows are not drawn.

## Open (needs a game run)

- 5 or 6 Controls rows. `0x24bc12` lays out up to the list block's
  visible count, which the Controls tag gives as 6, and the Controls handler
  embeds 6 items. Yet in game our co-op screen (6 rows on the Controls tag)
  shows 5, and the Xbox Live menu keeps room for five. The checks use the 5
  seen; what takes one away is not found.
- Whether the footer widget keeps our legend, or the screen's tag string
  (`0x2300cf`) overwrites it after the pane is set up.
- Whether an overlong description clips or runs past its rectangle.
- Whether a screen opened from inside another call draws on top.
- The kit's keyboard row: it opens the game keyboard for controller 0 with
  context 20 (no check on Done). The sign-in prompt's keyboard path is the
  proven one.
