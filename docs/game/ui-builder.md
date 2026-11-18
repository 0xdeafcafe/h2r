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
| `Dialog` | our tag recipe: dialog backing art, a body text, a skin 0 list of buttons | layout 3 (small, 256 wide) or 2 (large, 536 wide) | questions, progress, errors |
| `CodeEntry` | `Dialog` large, plus N cells: the Live passcode dialog's box art (screen 21's `password` blocks) with a digit text over each, the cursor cell tinted bright | layout 2 | the sign-in code |
| keyboard handoff | any kind; `Effect::Keyboard(field)` opens the game's keyboard (`signin_keyboard`), the typed text comes back as an `Input::Typed` | — | gamertag, invite |

**Backing.** A `Dialog` or `CodeEntry` is `Over` (drawn over the screen
beneath, as the game's own dialogs are) or `Own` (it carries the game shell
backing, `framing_center` from screen 5, under its panel, so the screen
beneath doesn't matter). The sign-in prompt is `Own`: it opens over the
title screen, in-game menus or the main menu, and must not depend on any
of them.

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
h2 ui preview <screen> [--state NAME|all] [--out DIR]   # PNG + JSON per state
h2 ui sheet [--out DIR]                                  # every screen x state, one contact sheet
```

The sheet goes to `$RUSH_KEEP/ui-builder/` by default.

## The checks

They run over a `View` and a `Metrics` (text widths, line heights, image
sizes) from either source, so `cargo test` runs them with synthetic
metrics, and a disc-backed test (`#[ignore]` unless `H2_DISC` is set) runs
them with the real fonts and art. Each finding names the widget id and the
state.

| Check | Rule |
|---|---|
| fit | every text fits its rect at its font (width per line, lines times line height) |
| overlap | no two widgets' rects overlap, except declared layers (a digit over its cell, text over its row bar) |
| reserved | nothing overlaps the reserved areas of the screen beneath (its logo and header), unless the screen is `Own` |
| safe area | every widget is inside the title-safe area: the middle 85%, 544 by 408 (retail headers start at its edge, y 204, x -272) |
| legend | a screen that takes a button has a legend naming it, and the legend fits |
| focus | the focused element (row or cell) is inside the visible window and drawn bright |
| glyphs | every character exists in its font (the kit's `Finding::Glyph`) |
| ids | every string id is in a registered family (the kit's `Finding::Id`) |

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
