# UI builder: the settings dialog and Game Setup

The settings dialog, the value picker, a page header, Game Setup picture and description, Quick Options and behaviour. Part of [ui-builder.md](ui-builder.md).

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
`variant-quick` (Quick Options for a script variant, eight slot rows and a
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
held value, description, its setter) and opens `kit.picker`. The pane
claims that offer when it sets its opening focus. The destructor releases
the offer if it still belongs to this screen. The description is the row's explanation (`Row::explain`), else its
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

**Closing.** The first occupied window receives the input dispatch.
When a closing kit screen sees its closing press again within 50 ms,
`lifecycle::while_closing` consumes it. For a later press, it checks the
active screen on the same slot, starting with that window and continuing
underneath the closing screen. A pending replacement or an unsettled
active screen consumes the press. A settled screen can receive it when
the channel's focused widget belongs to that screen. The first widget
on the focus path that accepts the pressing controller receives the event
through its own slot (`lifecycle::hand_down`). The tests
`lifecycle::tests::a_closing_dialog_only_forwards_to_the_next_active_widgets`
and `list_class::tests::a_press_while_a_dialog_closes_goes_to_the_screen_beneath`
cover this dispatch.

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
