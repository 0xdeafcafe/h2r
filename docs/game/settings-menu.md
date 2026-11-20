# Expanded settings menu

Status: Built. Our settings are native game screens (a Video screen, extra Controls rows, Video and Voice rows in both pause menus), driven by a schema of `settings::Def`s. Per-player mute is not built (see "Per-player mute").
Code: `crates/boot/src/video_screen.rs`, `crates/boot/src/pause_menu.rs`, `crates/boot/src/voice_settings.rs`, `crates/boot/src/fov_row.rs`, `crates/boot/src/setting_defs.rs`, `crates/settings`, `crates/cli/src/settings_cmd.rs`. Tests: in those files.
Switch: one variable per setting, in `setting_defs.rs` and listed in [switches.md](../runtime/switches.md).

The screens are the game's own list class with our rows, so they look native and
take pad navigation for free. They are not a host overlay. The rule that
holds: a setting is one `settings::Def`, and nothing on a screen writes guest
simulation memory except through a hook that says so (the clamp global in
"Field of view").

- The top-level Settings list has a **Video** row that opens our Video screen:
  field of view, frame rate, resolution, lighting, mute button and aim
  assist ("Video screen", "Field of view").
- The Controls screen has rows for the controller speaker, the Steam
  Controller (speaker, trackpad aim, left trackpad) and, on iOS, the touch
  controls ([playstation-audio.md](../audio/playstation-audio.md) "In-game
  setting", [ios.md](../runtime/ios.md#controls)).
- Both pause menus (campaign and multiplayer) have Video and Voice rows in
  their Settings dialog ("Pause menu", "Voice screen").
- The button layouts are rows on the Button Layout screen
  ([controller-layouts.md](../input/controller-layouts.md)).
- Every setting is a `settings::Def` in `boot::setting_defs`, and
  `h2 settings` lists, gets and sets them ("Settings as a schema").
- Halo 1 options are not in Settings. They are chosen per game on the Halo 1
  options screens (`boot::h1_options`; [multiplayer.md](../halo1/multiplayer.md) "Halo 1 options").

No game data is in this document. Screen layouts below were read with our own
`h2 ui`, `h2 tags` and `h2 strings` commands and are described in words. Guest
addresses are named only as hook points.

## Evidence labels

- **Local**: found by reading the retail XBE or the disc with this repo's tools
  (for example `h2 ui`), or already recorded in another doc here.
- **Community**: stated by a public source. It may describe Halo 2 for Windows
  Vista (H2V), a different build with different addresses and structures. Used
  as a comparison only, never as a layout we depend on.
- **Speculation**: my reasoning. It needs a test or a trace before use.


## How Halo 2's own settings screens work

### Screens, panes, lists (Local)

[ui.md](../formats/ui.md) has the full data model. What matters here:

- A screen is a `wgit` tag. A screen's widgets are built from its panes, and a
  pane holds buttons, one list, text blocks, bitmaps and so on.
- A **list** widget is declared in the tag with an anchor, a skin index, a
  visible-item count and an animation. The rows are not in the tag. A row is
  drawn from the list's skin (a `skin` tag: item text, item background, end
  cap), and what goes in each row is filled by code that belongs to that
  screen.
- The root settings screen has a list of six visible items in each of two
  panes (one for the player profile and one for game variants), with flavour
  text blocks that fade in per selection. Controller-style option screens use
  a different skin with two text blocks per row: a label on the left and a
  right-justified value on the right. Such a screen has a fixed visible-item
  count in its tag (six, eight and so on). Confirm/choice screens such as
  "invert look" are small dialog screens with a two-row list.
- The tag only supplies geometry, string IDs (resolved through the screen's own
  `unic`, see [localization.md](../formats/localization.md)) and animation indices. It
  does not say what pressing a row does. `0x00148098` finds a screen by its
  `i16` screen ID, but nothing in the tag names a constructor. The widget
  classes and their event handlers are code. A new screen therefore cannot be
  made from tags alone.

### Where option values live (Local, partly traced)

[engine-map.md](../engine/engine-map.md) records two persistence roles that matter:

- a **preferences** path (`0x00121350`): an async file read, signature
  calculation, version and size checks, and defaults when the data is missing
  or invalid;
- a **profile and configuration cache** (`0x0007fa40`, `0x0007fb50`), with
  profile and variant type naming at `0x00216b60`.

Saves go through XAPI's save-game calls and are signed with the game's own
`XCalculateSignature` ([xapi.md](../runtime/xapi.md), [checkpoints.md](../runtime/checkpoints.md)),
which our runtime provides. A profile the game writes is therefore a normal
game save from the game's point of view.

Not traced: which individual option (look sensitivity, invert look, vibration,
subtitles, brightness, audio levels) lives in the profile and which in the
preferences, and the field layout of either. Per-player screens in the settings
tree cover controller options, thumbstick, button layout and vibration, so
those are probably profile fields. That is a guess from screen grouping.
Mapping each option to a field is a prerequisite for reading or writing the
retail values; our settings do neither.

### What Cartographer did on Windows (Community)

Project Cartographer, the H2V online project, is the closest prior art. Its
public source (not Xbox) adds native screens: it declares C++ classes for the
settings, video, audio, controller, brightness, gamma, LOD, vsync and
virtual-keyboard screens, and adds its own list item and pane to the settings
screen by tag injection plus function hooks. See
[`screen_settings.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/interface/screens/screen_settings.cpp)
and the [`screens/`
directory](https://github.com/pnill/cartographer/tree/master/xlive/Blam/Engine/interface/screens).
Adding even one row needed a larger list-data allocation, a new pane and code
for the press handler. It also has an ImGui overlay for "Advanced Settings".
We use only the first technique, in our own code: native screens by function
hooks on the retail UI code, with no tag edited.

## How our screens get in

Our runtime replaces guest functions through dispatch overrides (the mechanism
that replaces D3D, DirectSound and XAPI). A row needs a press handler and a
value renderer, and those are code, so each screen is a hook on the retail
list's row text and row chosen functions, registered by `boot::layout_glue::install`.
The tables under "Video screen" and "Pause menu" give the addresses. Rows,
labels and values come from the Rust model `boot::ui`; strings take ids in our
own private range (English).
Guest code that runs in the tick can change game state, so a hook keeps to UI
state and to the one camera global named under "Field of view".

## Where settings persist

A host-side file, not the guest profile.

- Path: `settings.toml` in the data root the run uses (`h2 play --data`,
  default `~/Library/Application Support/h2recomp/data`;
  [storage.md](../runtime/storage.md)). It is not under `disk/`, which is the
  guest's. One writer, `settings::Store`, writes it and keeps other keys
  (`crates/settings/src/file.rs`).
- The guest profile is signed and sized by the game, and writes to it go
  through retail code. Our settings are per install, not per gamertag, and
  do not travel with a save someone else loads.
- Format: TOML. An unknown key or an out-of-range value is reported and names
  the key. A file that is not TOML is reported and never overwritten.
- Writes are atomic: a temporary file, then a rename (`settings::file`).
- Precedence, highest first: flag, environment variable, settings file,
  default. Flags and variables win so scripts, benchmarks and `--follow` runs
  stay reproducible. A locked row shows the value and does nothing.
- Per-profile values are kept apart ("Settings as a schema").

## Settings and switches

Not every switch is a setting. A setting has a `Def` in
`boot::setting_defs`, which names its file key, its variable and its flag.
Each setting has its variable there, and the variables are listed once, in
[switches.md](../runtime/switches.md). Developer-only variables (`H2_BOOT_*`,
`H2_PERF*`, `H2_PROFILE*`, `H2_FAULT*`, scripted input and the like) are
not settings and have no file key. Build-time switches change the generated
C and its cache key ([BUILD-CACHE.md](../lifter/BUILD-CACHE.md)), so they are
not offered as rows.

Settings that have no row of ours: `live.visibility` (the title's own party
privacy screen sets it; [live-custom-games.md](../live/live-custom-games.md)),
`coop.max_players`, `coop.join_in_progress` and `coop.look_prediction`
([live-coop.md](../live/live-coop.md)), and `graphics.single_copy`
([high-resolution.md](../graphics/high-resolution.md)).

## Video screen

The top-level Settings list (Player Profiles, Game Variants) has a third
row, **Video**. A on it opens a screen titled "Video" with up to six rows:

| Row | Values | Saved as | Applies | Locked by |
|---|---|---|---|---|
| Field of View | 70 to 120 in steps of 5 | `[graphics] fov` | at once | `H2_FOV` |
| Frame Rate | 30, 60, 120, Uncapped | `[graphics] fps` | at once | `--fps`, `H2_FPS` |
| Resolution | 640x480, 1280x960, 1920x1440, 2560x1920 (scale 1 to 4; shown as the size, saved as the scale) | `[graphics] render_scale` | next frame | `H2_RENDER_SCALE` |
| Lighting | Retail, E3 (default Retail) | `[graphics] lighting` (`"retail"`, `"e3"`) | next frame | `H2_LIGHTING` |
| Mute Button | On, Off | `[pad] mute_button` | at once | `H2_PAD_MUTE` |
| Aim Assist | Auto, Default, Strong, Very Strong (default Auto: Strong while the touch controls show) | `[controls] aim_assist` | next map load ([aim-assist.md](aim-assist.md)) | `H2_AIM_ASSIST` |

A steps the row's value and saves it to `<data>/settings.toml` at once,
keeping every other key. The window follows the frame rate at its next turn
(`app::fps::FpsControl`), the view backend makes its targets again at the
next frame boundary on the render thread (`ScaledBackend::follow_scale`,
[high-resolution.md](../graphics/high-resolution.md)), and so does the lighting
(`ScaledBackend::follow_lighting`, [e3-lighting.md](../graphics/e3-lighting.md)),
and the HID thread gets the mute
button (`PadHid::mute_switch`). A locked row shows the variable's value and A
does nothing. A bad variable is reported and ignored, so the saved value is
used. Above 30, the frame rate redraws the newest game frame more often
([high-framerate.md](high-framerate.md)). Headless runs (`h2 run`) have none of it (`H2_LIGHTING=e3`
works there as a variable).

Code: the rows are `Def`s in `boot::setting_defs` with a `menu` naming
the screen; `boot::video_screen::model` builds the screen as
`ui::Screen::new("video").rows_from_settings(..)`, the Video row as
`ui::extend(List::Settings, 2)` and the speaker row as
`ui::extend(List::Controls, 6)` ([Settings as a schema](#settings-as-a-schema));
the hooks registered by `layout_glue::install` ask that model for counts,
texts and presses. `h2 play` reads the saved frame rate, scale, lighting
and mute button when no flag or variable sets them. Tests:
`setting_defs::tests::lighting_is_retail_unless_asked_and_has_a_video_row`
(default Retail, the row on this screen, live, the file's names, the
variable's lock) and `video_screen`'s row tests (the row between
Resolution and Mute Button, showing Retail, then E3 after a press, saved).

### How the screen is made (Local, static reading of the retail XBE)

The Video screen is our own list class (docs/engine/feature-engine.md "UI step
b", `boot::list_class`): a screen built by the Controls constructor (screen
39, so the Controls tag's layout) whose screen and list handler point at our
copies of the Controls vtables. No tag is added and no retail function is
patched for it.

| Address | Role | Evidence and what we do |
|---|---|---|
| `0x2b1cca` | the top-level Settings list's handler constructor (screen 19, `ui\screens\game_shell\settings_screen\settings`, loader `0x230691`; vtable `0x45b060`, at `screen + 0x614`) | clears byte `+0x220`, makes the datum array "settings list" (`0x45b128`) with 2 rows, and leaves room for 3 item widgets of 0x80 at `+0x88`. The list hook makes it 3 rows and sets `+0x220` |
| `0x2b1d60` | its slot 19, the item widget count | `2 + (byte [this+0x220] != 0)`: 3 once the hook has set the byte |
| `0x2b1d6e` | its row text (thiscall, item at `[esp+4]`, `ret 8`; one text, index 0) | row 0 `0x17000196` Player Profiles, row 1 `0x10000197` Game Variants; row 2 is ours: the "Video" label |
| `0x2b1da9` | its row chosen (thiscall, `ret 8`) | row 0 opens `0x2b19dc` (Player Profiles), row 1 `0x2b1b01` (Game Variants), through `0x149f1e`; any other row does nothing. Row 2 is ours: `0x149f1e(1 << player, 0x2b7714)` |
| `0x230427` | the pane for a row (thiscall, the row, `ret 4`; no bounds check) | the tag has one pane per retail row; for row 2 the hook returns at once, so the shown pane stays (as for the Skulls row) |
| `0x2b79f9` | the per-profile Settings list's row chosen (thiscall, `ret 8`) | row 0 calls `0x149f1e(1 << player, 0x2b7714)`: this is how Controls opens |
| `0x149f1e` | open a screen: build a 0x20-byte request (`0x149f49`) and call its loader with it (stdcall, `ret 8`) | request `+0` 0, `+2` the player mask, `+4` 5, `+8` 4, `+0xc` 0, `+0x10..+0x1b` -1, `+0x1c` the loader. Controls' own dialogs (`0x2c8719`) build the same request. Our Video row calls it with our class's loader, an entry at a feature-arena address |
| `0x2b7714` | Controls loader (stdcall, the request, `ret 4`) | allocates 0x9b4 bytes (`0x1a47fd`), constructs, sets `+0x6c = 1`, pushes the screen (`0x147f6d`). Our loader does the same steps and, between construct and push, adopts the screen: vtables, the press record's function, the rows trimmed to the model's |
| `0x2b7752` | Controls screen constructor (EDI = screen; request `+4`, `+8` and the `+2` word) | base screen `0x230451(39, ...)` with its list handler at `screen + 0x614`, vtable `0x45be30`, then the list handler constructor `0x2c84b8` |
| `0x1a480d` | datum array constructor | the Controls list hook may grow the array (the speaker row); adopting deletes the rows past ours. |
| `0x2c8548`, `0x2c8719` | Controls row text and row chosen | never reached for a Video screen: list slot 20 and the press record point at our entries. The row text is ported (`boot::controls_text`) |
| `0x24bb60` | "add items" | the speaker hook skips any handler whose vtable is not the Controls one, so ours keeps its six item widgets |
| `0x22fda6` | build a screen's header | runs as retail; our screen slot 18 then sets our title on the header text widget (`screen + 0x80`, `0x253b1a`). |

**B returns to Settings.** The Video screen is opened by the same call,
with the same request, that the Settings list uses for Player Profiles
and a profile uses for Controls, so it is pushed on the same screen stack
and closes the same way. The Controls class itself never touches the
Controls edit buffer on close: only the profile screen's back handler
(`0x2b7abe` → `0x148bff`) and the generic `0x230b32` commit and clear it,
and no profile is being edited from the Settings list.

**Settings root as the entry.** Video is a row of the top-level Settings
list, next to Player Profiles and Game Variants, because none of its
settings belongs to a profile. It is added the way the Controls rows are: a
`ui::extend(List::Settings, 2)` row in the model, the list hook growing
the count (plus the one handler byte that gives the list a third item
widget), and wrappers on the list's own row text and row chosen that
answer for row 2 and leave rows 0 and 1 to the originals. No tag is
changed: the list already shows up to six items, and the missing pane is
skipped.

## Pause menu

The pause menu's **Settings** (the in-game Controller Settings dialog,
screen 200) gets two rows at its end, after its own rows and the
Controller Speaker row ([playstation-audio.md](../audio/playstation-audio.md)):
**Video** opens the Video screen above, and **Voice** opens the Voice
screen below. Each opens over the dialog, and B comes back to it. The
top-level pause menus are retail. Code: `boot::pause_menu`, the model in
`video_screen::model` (`ui::extend(List::GameSettings, 7)`), and the
dialog's hooks in `layout_glue::install`, the same three the speaker row
uses. The trace below is a static reading of the retail XBE with
`h2 disasm`, `h2 ui` and `h2 tags`.

### How Start opens it (Local, static)

A button event that no screen takes goes to `0x14954a` (from the UI
event dispatch at `0x147dcc` and `0x147eaa`), which switches on the UI's
game kind byte at `0x54d5a8`: 0 (campaign) goes to `0x149366`, 1
(multiplayer) to `0x1493ef`, and 2 (the main menu) to `0x1494af`. Both
game handlers act on event kind 5, button 12 (Start), for a player whose
record at `0x54e8e4 + player * 0xc70` is set:

| | Campaign (`0x149366`) | Multiplayer (`0x1493ef`) |
|---|---|---|
| a screen already up (`0x147d13`) | `0x236299(2)` | the same |
| blocked when | `0x13cb40`: byte `+5` of the globals at `0x510c50`, which the tick budget `0x146660` also reads [not named] | `0x68290` [not read] |
| opens | `0x149f1e(1 << player, 0x2320c4)`: kind 5, slot 4 | a request built at `0x14945a`: kind 5, slot `0x1910b8(player)` (0 to 3), loader `0x1475c7` |
| pauses | yes, in the screen's constructor | no |

### The two screens (Local, static)

| | Campaign | Multiplayer |
|---|---|---|
| screen | 18, `ui\screens\singleplayer\dialog_pause\dialog_pause` (one pane, a five-item list, the objectives text) | 195, `ui\screens\multiplayer\mp_menu\mp_menu` (one pane, a five-item list) |
| loader, size | `0x2320c4`, 0xb34 bytes | `0x1475c7`, 0x934 bytes |
| constructor | `0x23203e`: `0x230451(18, ...)`, vtable `0x458fa8` | `0x147605`: `0x230451(195, ...)`, vtable `0x453898` |
| list handler | `screen + 0x614`, built by `0x231e88`, vtable `0x459018` | `screen + 0x614`, built by `0x232708`, vtable `0x459298` |
| item widgets | five of 0x80 at `+0x88` | five of 0x80 at `+0x88` |
| rows' datum array | name at `0x459134`, five rows, every one made | name at `0x4593a8`, room for six, a datum per row shown with its action word (0 to 5) at datum `+2` |
| which rows | always all five | actions 0, 1 and 2 always; 3 when `0x232d06`; 4 when `0x592f0` and not `0x1999b3`; 5 when `0x592f0` and the lobby mode `0x19989d` is not 6 |
| row text | `0x231f6c`: a switch on the item's row (`+0x70`), 0 to 4; other rows get the null id | `0x23284e`: the action looked up in a table of six (`0x24c75c`) |
| row chosen | `0x231fe3` (the press record at `+0x308`): rows 0 to 3 have their own press; **every other row runs row 4's** | `0x2328b5`: actions 0 to 4 have their own press; **every other action runs action 5's** |
| B, Back | close (`0x232102`, `0x22e957(3)`) | B, Back and Start close (`0x2326ce`) |

The campaign rows, by what a press does (their texts are the game's own):
row 0 closes the menu (`0x22e957(3)`); row 1 sets the byte at `0x547f6f`
and closes it [what reads the byte is not traced]; row 2 opens a
confirmation (`0x19b5af(3, 0xaa, 4, mask, 0x2323b7)`); row 3 closes the
menu and opens the in-game Controller Settings (screen 200, loader
`0x231db5`, [playstation-audio.md](../audio/playstation-audio.md)); row 4
opens screen 168, or 183 when `0x18f911` says more than one. The
multiplayer menu's action 1 opens Controller Settings (screen 200) or the
player settings screen (196, loader `0x23252e`) when `0x2b505c(player)`,
and leaves the menu up; actions 0 and 2 to 5 go to `0x2329a7`, `0x232b63`,
`0x232bb4`, `0x232c05` and `0x232c4d` [not read further].

**Which stack a sub-screen goes on.** Both menus open Controller Settings
with a request built from the menu screen's own `+0x74` and `+0x78`
(screen slots 20 and 21, `0x23012c` and `0x230130`): `0x2321ab` and
`0x232aff`. For the campaign menu those are what `0x149f1e` writes (kind 5,
slot 4); for the multiplayer menu the slot is the player's own.

### What pausing means (Local, static)

The paused byte is `[0x510c54] + 1`; `0x146840` reads "initialised and
paused", and the tick budget `0x146660` runs no tick while it is set.

- **Campaign.** The pause menu's constructor sets the byte (and pauses
  the sound, `0x125a90(0)`) unless it is already set. Nothing in the
  screen clears it. Each UI update `0x147aa8` calls `0x1482c4`, which
  clears it (and resumes the sound, `0x125a90(1)`) only while no screen is
  up on any stack (`0x149227`: `0x14917c` over every window kind and
  slot). So a screen of ours on top of the pause menu keeps the game
  paused, B back to the pause menu changes nothing, and the game resumes
  when the last screen closes. Settings' row closes the pause menu and
  opens the dialog, which keeps the game paused the same way; ours open
  over the dialog. Our screens and rows never write the byte.
- **Multiplayer.** Nothing on this path writes the byte: `mp_menu` sets
  nothing, so the simulation runs while it and our screens are up, and
  the network game is never held. A press on a Video or Voice row writes
  `settings.toml` on the game thread (`settings::Store::set`, one small
  file); nothing else of ours waits.
- **Co-op over Live** is a campaign game (kind 0) on the retail
  synchronous path, so Start opens `dialog_pause` and its constructor
  sets the local paused byte, as without our rows. [not traced] Whether
  the synchronous client and server honour one console's paused byte. Our
  rows add nothing to it either way.

### The Settings dialog (Local, static)

Screen 200, `ui\screens\multiplayer\mp_menu\controller_settings`, in
`shared`, `single_player_shared` and every level: one pane, a five-item
list whose items have one text each. The campaign menu's row 3 closes the
pause menu and opens it (loader `0x231db5`, constructor `0x231df3`,
vtable `0x4590b8`); the multiplayer menu's action 1 opens it on the
player's stack and leaves the menu up. Its list handler (`screen +
0x614`, built by `0x2b48c2`, vtable `0x45b778`) makes a datum array
named at `0x45b8b0`, six rows, and gives each datum a value word at `+2`,
0 to 5. Row text (`0x2b49e6`, slot 20) looks the value up in a table of
six string ids; row chosen (`0x2b4a4d`) switches on it (the press
record's player at `[[esp+4]] + 4`, the row's handle at `[[esp+8]]`) and
does nothing for any other value.

### How our rows are added

- **Rows.** `layout_glue`'s datum array hook (`0x1a480d`) grows the array
  named at `0x45b8b0` by one for the speaker row and by one per row of
  ours (`pause_menu::list_count`). An override on the list constructor
  `0x2b48c2` runs the original, then adds a datum per extra row
  (`0x16b840`) and writes its value: 6 for the speaker, then 7 for Video
  and 8 for Voice (`pause_menu::FIRST_VALUE`). Value words, not indexes,
  tell the rows apart, so the retail rows keep theirs and ours come last.
  Without the speaker row ours are still 7 and 8.
- **Text.** The row text override draws our label for a value of ours
  (`pause_menu::text_row`) and passes every other value to the original.
- **Press.** The row chosen override opens our screen for a value of ours
  (`pause_menu::chosen_row`, `open_request`): a request built from the
  dialog's own `+0x74` and `+0x78` (window and slot, as the retail menus
  open their sub-screens) and the pressing player's mask, handed to the
  class's loader. Every other value goes to the original.
- **One set of hooks.** The speaker row and ours share the three
  overrides, so nothing else hooks the same addresses. The rows are fixed
  at install (`pause_menu::settings_rows`): the hooks never read the
  settings store with guest memory held.
- **Scrolling.** The list has five item widgets; with nine rows it
  scrolls, as it already did with seven.

### In a game, our screens use screen 200

The Controls constructor's tag (screen 39,
`ui\screens\game_shell\settings_screen\player_profile\controller_settings`)
is in `mainmenu` only: `h2 tags` lists it there and in no in-game map, so
built on it, a screen in a game would get no widgets. In a game (the UI's game kind byte
`0x54d5a8` not 2) the loader runs the Controls constructor's own
steps on screen 200's tag instead (`list_class::construct_on`): the
generic screen constructor `0x230451(200, ...)`, the Controls screen
vtable, the Controls list constructor `0x2c84b8`. Screen 200's items have
one text each and the same header string id, so a row there is one line,
label and value (`ui::Model::line`), drawn from a text slot of the class's
own (`list_class::Lines`, the `ui lines` id family: two ids a row, taken
in turn when the text changes so the widget sets it again). In the main
menu the loader builds on the Controls constructor.

### Per-player mute

The game already has it: the player menu's Mute and Unmute (actions 14 and
15 of `0x2b55ae`, handler `0x2b5d3b`) call `XOnlineMutelistAdd` and
`XOnlineMutelistRemove`, and the mute matrix the game builds from the list
gates voice both ways ([live-voice.md](../live/live-voice.md) sections 1.4, 1.5 and 3.6). That player menu sits
beside the multiplayer pause menu's player settings screen (196)
[reachability from the pause menu not traced]. So there is no row of ours
and no list in `settings.toml`: the retail rows work once the mute list HLE stores
the list per account. A host row would call that same HLE.

### Tests

`boot::pause_menu::tests`: the dialog gets Video then Voice at values 7
and 8 and its array grows by two (`list_count`, `settings_rows`); a press
opens our screen and no other value is ours; the switches take rows out;
our datums are found by value with the speaker row or without it, and
the retail ones go to the original; our screen opens on the dialog's own
window and slot for the pressing player; the Voice and Video pages step,
apply at once and close on B (`ui_kit::page::ListPage` over the real list
class); the pause rule (`pause_menu::resumes`, `0x1482c4`) keeps the game
paused while a screen of ours is up over the dialog; a host with many
devices still fits the model's string ids. `boot::list_class::tests`: in
a game the loader builds on screen 200 and in the main menu on the
Controls constructor; in a game a row draws as one line in the class's
slot; a line changes id when its text changes.
`boot::ui::tests::a_row_reads_as_one_line`.

## Voice screen

Opened from the Voice row of the pause menu's Settings. Every setting whose `Def` names
the screen `voice` (`boot::voice_settings`), in order; every one applies
at once, touches only the host (`Scope::Host`) and is read by the voice
chat (the voice chat) through `voice_settings::VoiceHandles`, one
atomic load each. Muting is `VoiceHandles::mic_muted.get()`.

| Row | Key | Values | Locked by |
|---|---|---|---|
| Microphone | `voice.input_device` | System default, then the host's inputs | `H2_VOICE_INPUT` (an id, part of a name, or `default`) |
| Voice Output | `voice.output_device` | Same as Game (mixed into the game's output), then the host's outputs | `H2_VOICE_OUTPUT` |
| Talk | `voice.mode` | Open Mic (`open`), Push to Talk (`push`) | `H2_VOICE_MODE` |
| Talk Button | `voice.push_to_talk_button` | D-Pad Down, D-Pad Up, D-Pad Left, D-Pad Right, Back | `H2_VOICE_PTT_BUTTON` |
| Mute My Mic | `voice.mic_muted` | On, Off (default Off) | `H2_VOICE_MIC_MUTED` |
| Mute Everyone | `voice.incoming_muted` | On, Off (default Off) | `H2_VOICE_INCOMING_MUTED` |
| Mic Volume | `voice.input_volume` | 0% to 100% in tens (default 100) | `H2_VOICE_INPUT_VOLUME` |
| Voice Volume | `voice.output_volume` | 0% to 100% in tens (default 100) | `H2_VOICE_OUTPUT_VOLUME` |

**Devices.** `audio::voice::Devices` lists a host's inputs and outputs
(`audio::voice::Device`: the host id and the name); `HostDevices` is cpal's
default host (CoreAudio on macOS: the device UID as the id), `FakeDevices` a
fixed list for tests. `h2 play` and `h2 settings` list them once at start
(`voice_settings::use_host_devices`), and the two device settings become
choices of the first choice and up to twelve devices, each name cut to 24
characters, a repeated name numbered. A device's value is
`audio::voice::key(id)` (FNV-1a, positive, never 0), so the handle is an
`i64`; `audio::voice::find` gives the device back, and 0 means the first
choice. The file keeps the id (`input_device = "coreaudio:<UID>"`). A
device in the file but not plugged in at launch is reported once and the
row shows the first choice; the file keeps it until another is chosen.
The list is not followed while the game runs: a device plugged in later
appears at the next launch.

**String ids.** The `ui` model's family (`ui model`, 191 ids) is not enough for every
text, so the model also takes ids from a second
family, `ui model more` (`0x27fff000`, 256 ids, `ui_kit::ids`), through
`ui::Model::with_ids`.

## Field of view

The Video screen's **Field of View** row (above).
Its value is the first-person camera's unzoomed horizontal field of view in
degrees. Pressing A steps it 70, 75, ... 120 and wraps back to 70. 70 is
retail: the hook then hands the game its own number back, bit for bit.
Left and right do nothing on this list (the Controls list's own input code
is not traced; A is the only press we see, through the row-chosen handler).

### Where the game sets it (Local, static reading of the retail XBE)

| Address | Role | Evidence |
|---|---|---|
| `0x187130` | per-local-player field of view. `eax` = player-control index (array `[0x4ed284]`, stride `0x94`, unit at `+0x14`), result in `xmm0`, plain `ret` | starts from the observer default `[0x54e854]`; with a unit, takes the unit definition's camera field of view (`+0xcc`, found through the object header table `[0x4e0300]+0x44` and the tag instances `[0x4e3b44]`), calls `0xc8880` for the zoom magnification of the player's zoom level (`[pc+0x2e]`), and returns `fov / magnification` when that is between `0x45e25c` and `0x45e258` (about 1.8 and 178 degrees), else the unzoomed value. So zoom divides the angle |
| `0x23c110` | first-person camera update (director callback, [engine-map.md](../engine/engine-map.md)) | the **only** call to `0x187130` (`0x23c202`; a scan of every `call`/`jmp` rel32 in `.text` and every absolute dword finds no other reference). Multiplies by 1.0, clamps to `[0x55e484, 0x55e488]` and stores the director camera's field of view at `+0x28`; a change sets the observer's 0.18 s transition (`+0xa4`, `+0x90`) |
| `0x55e484` / `0x55e488` | clamp globals, set by static initialisers `0x3782a0` / `0x3782c0` from `0x45dc54` / `0x45e098` | 1 and 110 degrees. `0x55e488` has one reader, `0x23c225` |
| `0x171d90` | observer: camera result from the command | vertical FOV `+0x4c = 2 atan(tan(h/2) / aspect)` from the horizontal `+0x38`; the scale `+0x50 = h / [0x54e854]` |
| `0x2f800` | render camera from the observer camera (called by `0x223080`, per view, from `render_game_views` `0x2234e0`) | copies position, forward, up, vertical FOV (`+0x4c` to `+0x28`) and the scale (`+0x50` to `+0x2c`), then the split-screen factors 0.5 / 0.8. The renderer's projection and its frustum come from this camera |
| `0x54e854` and the other `70` copies | 70 degrees (`0x45e628`) copied into about 40 per-file globals by static initialisers in `0x3763b0`..`0x378894` | only `0x54e854` (camera code), `0x5234dc` (render, the no-observer fallback) and `0x557c28` are read; we change none of them |

Community comparison: Cartographer (H2V) replaces the unit's camera field of
view and keeps the same `[1, 110]` clamp in its first-person camera. Nothing
is copied, and H2V's addresses and structures differ.

### What the hook does

`boot::fov_row` (logic over `Gm`), registered by `layout_glue::install`:

- **Camera** (`0x187130`, `fov:camera`): run the original, then
  `result * target / unzoomed`, where `unzoomed` is the unit's camera
  field of view read the way the function reads it (or the result itself
  when there is no unit). Unzoomed you get the target; a 2x scope at 100
  gives 50, so the magnification stays what the weapon's tag says. When the
  target is above retail's 110-degree clamp, `0x55e488` is raised to the
  target, and put back to the value of `0x45e098` when the setting drops
  below it or goes back to retail.
- **Row**: a row of the Video screen (above): the row text fills our
  label and the degrees as our own strings (English digits; ids in
  `layout_glue::is_ours`'s range); the row-chosen handler steps the value
  and returns.
- **Setting** (`boot::setting_defs::FOV`): `[graphics] fov = <degrees>` in
  `settings.toml` in `h2 play`'s data root, next to `[pad_speaker]`,
  written atomically and keeping every other key. 70 is retail (0 in the
  file or `H2_FOV` reads as 70); the file may hold 30 to 150. A bad value
  is reported, treated as retail and the file is not overwritten when it
  is not TOML. `H2_FOV=<degrees>` (30 to 150) or `H2_FOV=0` locks
  the value for the run; the row shows it and pressing it does nothing. A
  bad `H2_FOV` is reported and ignored. Headless runs (`h2 run`) install
  neither the row nor the camera hook.

### Why culling stays right and the simulation does not change

- The value is changed where the game computes it, before the director,
  the observer and the renderer see it, so the projection, the frustum the
  renderer culls with, the observer's FOV scale (which particles read at
  `0x248a31`) and the LOD all follow the same camera. Nothing on the host
  side knows about FOV.
- `0x187130` has one caller, the first-person camera callback, which runs
  from `director_update` in the per-frame, variable-`dt` part of the main
  loop ([high-framerate.md](high-framerate.md), step 9), not in the 30 Hz
  tick. Its result goes only into the director's camera.
- Aim does not read it: the camera reads the player's yaw and pitch from
  player control (`[0x4ed284]+0x24/0x28`) and never writes them. The zoom
  level, the magnification and look sensitivity are not touched; only the
  angle that results from them is. Auto-aim and aim assist are weapon and
  globals tag values; the paths above do not lead there. The Aim Assist
  row scales those tag values at map load instead
  ([aim-assist.md](aim-assist.md)).
- Readers of the observer array (`0x4e9c8c`, stride `0x358`; through
  `0x16f3a0` and direct) were checked for reads of the FOV fields
  (`+0x38`, `+0x4c`, `+0x50`): the render camera (`0x2f800`), the
  particles' scale (`0x248970`) and camera code. The others read
  position. `0xbd610` reads `+0x38` / `+0x4c` of a pointer whose origin was
  not traced; that is the one open item.
- Networked state: the observer and the director are per local player and
  never serialised by the game; nothing here writes simulation memory. The
  clamp global `0x55e488` is the one guest write, and only the camera
  callback reads it.

Only the first-person camera follows the setting; the following camera
(vehicles; `0x23bd70` stores `[0x54e854]` itself), dead and scripted
cameras do not.

## Settings as a schema

The schema is the settings layer of [feature-engine.md](../engine/feature-engine.md). A setting is one
`settings::Def` (`crates/settings`): its `table.key` in `settings.toml`, its
label and help, its kind (bool, whole number with a valid range and row
steps, or a choice), its default, the variable and flag that override it,
and whether a change applies at once (`Live`) or at the next launch
(`Restart`). Ours are in `boot::setting_defs`.

- **Precedence:** a flag (`--fps`, `h2 play --set key=value`), then the
  variable, then the file, then the default. A value from a flag or the
  variable, or one the run forces (the controller speaker with audio off),
  is locked: its row shows it and A does nothing.
- **Bad values** anywhere are reported once, naming the key, and the next
  source is used. A file that is not TOML is reported and never
  overwritten.
- **Handles:** code reads a setting through `Setting<T>`, one atomic load,
  so the camera hook reads the field of view with guest memory held. Row
  presses go through the store after guest memory is dropped; a `Live`
  setting's change callbacks (the controller speaker's router) run after
  the store's lock is released.
- **Rows:** `boot::ui` is the Rust model of our rows. `rows_from_settings`
  takes every `User` setting whose `menu` names the screen, in order; the
  model gives each text it can show a string id in our private range
  (shared texts share an id) and refuses more texts than ids.
- **CLI:** `h2 settings [--data DIR] list [--all] | get KEY | set KEY VALUE`
  shows each value a play from this shell would use and where it comes
  from, and writes the file through the same writer.

A new setting with a Video row is one `Def` with
`menu: Some(Menu { screen: "video", order })`. `H2_FOV=0` means retail,
`"uncapped"` is written as a string, and `H2_PAD_SPEAKER` may name a routes
file. A bad `H2_PAD_MUTE` is ignored (the file or on). The log line after
a press names the key (`graphics.fps: 60`, or for a `Restart` setting
`<key>: <value> from the next launch`).

**Per-profile values.** A setting can also hold a value per player profile
(`Store::profile_get` / `profile_set`), written as
`[profiles."<name>".<table>] key = value` and kept apart from the run's own
value. The campaign skulls use it: which skulls each profile has collected
and which it wants on ([skulls.md](skulls.md)).

## Not verified

- The pause menus: what `0x13cb40` (byte `+5` at `0x510c50`) and the byte
  campaign row 1 sets (`0x547f6f`) are; whether the pause lists' pane byte
  (`+0x7e`) is clear; whether the retail synchronous co-op path honours one
  console's paused byte.
- Aim Assist is `Scope::Sim` and reachable from the pause menu during a co-op
  over Live game; it applies at the next map load on that console only.
- Which retail option lives in the profile and which in the preferences file,
  and their layouts. Nothing here reads or writes the retail values.
- Which row-level behaviours only a game run shows: the Video rows apply as
  the tables say (frame rate, resolution and lighting without a flash or a
  black frame); the Video and Voice screens open over the pause dialog with
  the world frozen in a campaign and running in multiplayer; B returns to the
  dialog; in split screen a screen opens in the pressing player's quarter.
- Field of view: the first-person weapon model, the HUD scope overlays and
  the sniper's zoom ring at wide values, and whether the following camera
  (vehicles) should follow; the read of `0xbd610`.
- The Voice screen in a networked game, kept up for a while while rows are
  pressed (no stall, no disconnect).

## Sources

Community, cited and not copied:

- Project Cartographer: <https://github.com/pnill/cartographer>.
