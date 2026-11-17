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
   `KitScreen` needs `1 + 3 × rows` slots (the title, then a label, a value
   and a description per row); `KitScreen::class` panics at install if the
   family is too small.
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
   page does. `.description(TextBlock)` adds the focused row's description
   with the legend under it. `.scrolls()` says more rows than the list shows is
   meant.
3. **Open it** from an `Open(name)` row on another screen of ours, or with
   `list_class::open(cx, chain, name)` (`0x149f1e(player mask, loader)`).
4. **Check it.** Add the screen to `ui_kit::screens::NAMES` and `sample`. The
   tests then check it without the disc, and against the disc when it's
   there (below).
5. **Preview it.**
   `h2 ui preview <screen> [--focus N] [--disc disc.iso] [--out file.png]`
   prints the log line and every check, then draws the screen. The test
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
- [ ] **Rows versus slots**: the Controls list shows `visible` rows (the tag
      says 6, `List +6`). More rows than that needs `scrolls` (`Finding::Rows`).
      Without the disc the check assumes 5, to be safe.
- [ ] **Widths**: every label and every value a row can show (not just the
      current one) fits its skin text: 245 units for the Controls skin's label
      and value columns in conduit-13, 190 for skin 0's single text in
      conduit-12 (`Finding::Wide`).
- [ ] **Description**: the focused row's description plus the legend fits the
      description rectangle at the font's line height (`Finding::Description`).
- [ ] **Glyphs**: every character is drawn by the font. The icon range's
      filler record counts as missing (`Finding::Glyph`).
- [ ] **Presses**: an action row does something, a value row has at least two
      values and the screen takes left and right, and a text row opens the
      keyboard (`Finding::Press`).
- [ ] **Lifecycle**: anything that must happen on the screen underneath runs
      from `Then::CloseThen`, never straight after `close`.
- [ ] **Preview**: look at the PNG. Red outlines mark overflowing texts; a red
      bar under the list marks rows past the visible ones with no scroll
      intent (blue when they're meant).

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

- **A "Start" row at the top of the skulls page.** The user didn't want a row
  that does nothing but continue. On the kit, A on any row can run the
  screen's continue (`Row::action` returning `Then::CloseThen`), and value
  rows keep left and right for themselves. The skulls page does this: left
  and right toggle, A starts.
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
  U+E10B the d-pad, U+E10C Start, U+E10D Back), and `input::legend` lays them
  out with retail's three-space gap. The skulls page's legend is checked
  against it glyph for glyph.
- **Shipping blind** on row count, text width, where the description sits,
  which glyphs draw, and what's on top. The checks and the preview cover all
  but the last.

## What the previews showed (2026-11-17)

Drawn from the disc's `mainmenu.map` and fonts by
`preview_and_check_against_the_disc`:

- **The skulls page's descriptions overrun.** With the legend under them,
  10 of the 21 skulls' descriptions wrap to 6 or 7 lines in conduit-12, and
  the level select's description rectangle (`[-30 39 -133 260]`, 103 units)
  holds 5. The legend is what spills. Shorter descriptions, or a legend on one
  line elsewhere, would fix it. Whether the game clips or overflows needs a
  run.
- **Every row label fits.** "Grunt Birthday Party: < Off >" is under skin 0's
  190 units in conduit-12.
- **The co-op host and join screens have 6 rows.** The Controls list's tag
  shows 6 (`visible items` 6), so they fit. The "5 visible, the sixth
  scrolls" seen in game doesn't match the tag, and needs a run to settle.
  Until then, the fallback check (no disc) assumes 5 and flags both screens.
- **Item spacing.** The skin's bounding box from its text blocks is 28 units
  for skin 0 and 23 for the Controls skin. docs/formats/ui.md gives 43 for
  skin 0, counting its bitmaps at their image size, which the preview doesn't
  read yet. Rows in the preview sit closer together than in game.

## Open (needs a game run)

- Which engine font slot a text block's font index names. The preview takes
  it as that line of `font_table.txt`: index 1 is conduit-12, index 4 is
  conduit-13.
- Whether 5 or 6 Controls rows show before the list scrolls.
- Whether an overlong description clips or runs past its rectangle.
- Whether a screen opened from inside another call draws on top.
- The kit's keyboard row: it opens the game keyboard for controller 0 with
  context 20 (no check on Done). The sign-in prompt's keyboard path is the
  proven one.
