# Expanded settings menu

Status: Built. Video, Voice, Player Settings and the extra Controls rows use native game screens driven by `settings::Def`s.
Code: `crates/extensions/game/src/video_screen.rs`, `crates/extensions/game/src/pause_menu.rs`, `crates/extensions/game/src/voice_settings.rs`, `crates/extensions/game/src/fov_row.rs`, `crates/extensions/game/src/setting_defs.rs`, `crates/formats/settings`, `crates/apps/cli/src/settings_cmd.rs`. Tests: in those files.
Switch: see [switches.md](../runtime/switches.md).

The screens are the game's own list class with our rows, so they look native and
take pad navigation for free. They are not a host overlay. The rule that
holds: a setting is one `settings::Def`, and nothing on a screen writes guest
simulation memory except through a hook that says so (the clamp global in
"Field of view").

- **Video** on the main menu's Settings list opens the resolution,
  frame rate, field of view, lighting, HUD position and LOD distance options.
  See "Video screen" and "Field of view". **Voice**, the next row, opens
  the voice chat options ("Voice screen").
- The Controls screen has rows for the controller speaker, the Steam
  Controller (speaker, trackpad aim, left trackpad) and, on iOS, the touch
  controls and Aim Assist ([playstation-audio.md](../audio/playstation-audio.md)
  "In-game setting", [ios.md](../runtime/ios.md#controls)), and last the
  superbounce **Fall Assist**, Off until a press turns it on
  ([superbounce.md](superbounce.md#hold-x-the-fall-assist)). On macOS,
  **Lightbar** sits next to **Controller Speaker**.
- **Player Settings** in the campaign and multiplayer pause menus opens the
  native settings list, which includes **Video** and **Voice**
  ("Pause menu").
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
  (`crates/formats/settings/src/file.rs`).
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

A setting has a `Def` in `boot::setting_defs` with its file key and any
variable or flag overrides. See [switches.md](../runtime/switches.md).
Developer-only variables (`H2_BOOT_*`, `H2_PERF*`, `H2_PROFILE*`,
`H2_FAULT*`, scripted input and the like) are
not settings and have no file key. Build-time switches change the generated
C and its cache key ([BUILD-CACHE.md](../lifter/BUILD-CACHE.md)), so they are
not offered as rows.

Settings that have no row of ours: `live.visibility` (the title's own party
privacy screen sets it; [live-custom-games.md](../live/live-custom-games.md)),
`coop.max_players`, `coop.join_in_progress` and `coop.look_prediction`
([live-coop.md](../live/live-coop.md)), and `graphics.single_copy`
([high-resolution.md](../graphics/high-resolution.md)).

## Placement and naming

Video and Voice follow one scheme (`setting_defs.rs`, `voice_settings.rs`).

| What | Rule | Video | Voice |
|---|---|---|---|
| Screen | a setting's row is on the screen its file table belongs to | `[graphics]` | `[voice]` |
| Row order | the device or display, then how it behaves, then levels, then On/Off rows | Resolution, Frame Rate, then the view rows | Microphone, Voice Output, Talk rows, volumes, mutes |
| Label | Title Case, the retail screens' wording | Field of View | Talk Mode |
| File key | the label in lower case with underscores, without the screen's name; `fov` and `fps` are the two short forms | `hud_position` | `microphone_volume` |
| Value name | the value's label in lower case with underscores | `pinned`, `uncapped` | `open_mic`, `push_to_talk` |

The only other key the file reads is the saved integer
`graphics.render_scale` ("Video screen"). Variables keep their own names
([switches.md](../runtime/switches.md)).

Video has no widescreen, vsync or brightness row. The picture follows the
window's shape ([high-resolution.md](../graphics/high-resolution.md#widescreen-projection)),
Frame Rate paces the display, and brightness stays the game's own.

## Video screen

The top-level Settings list reads Player Profiles, Game Variants, **Video**
and **Voice**. Selecting Video opens the display options below.

| Row | Values | Saved as | Applies | Locked by |
|---|---|---|---|---|
| Resolution | Native (render width x height), then fixed sizes at the viewport aspect | `[graphics] resolution` (`"native"` or `"WxH"`) | next frame | `H2_RESOLUTION`, legacy `H2_RENDER_SCALE` |
| Frame Rate | 30, 60, 120, Uncapped | `[graphics] fps` | at once | `--fps`, `H2_FPS` |
| Field of View | 70 to 120 in steps of 5 | `[graphics] fov` | at once | `H2_FOV` |
| Lighting | Retail, E3 (default Retail). A [BSP review launch](../graphics/relighting-review.md) displays its locked lamp state | `[graphics] lighting` (`"retail"`, `"e3"`) | next frame | `H2_LIGHTING` |
| HUD Position | Pinned (default), Centred (drawn as `Centered`) | `[graphics] hud_position` (`"pinned"`, `"centered"`) | live | `H2_HUD_POSITION` |
| LOD Distance | Retail, 0.5x, 1x, 1.5x, 2x, 3x, 4x, Highest (default) | `[graphics] lod_distance` (number, `"retail"` or `"highest"`) | next view | `H2_LOD_DISTANCE` |

A on Resolution, HUD Position or LOD Distance opens a select dialog. A confirms the
highlighted option and saves it to `<data>/settings.toml`. B cancels. Native
follows the physical viewport on each frame, including the backing pixels
on Retina. Choosing a numbered size fixes the render target at that size.
At 5120x2880 the select dialog offers Native, 1280x720, 1920x1080, 2560x1440, 3840x2160 and
5120x2880. The saved integer `graphics.render_scale` and its launch override
select the corresponding fixed multiple of 640x480. In 5K fullscreen, the
4K pixel budget gives **Native (3840x2160)**. The explicit **5120x2880**
option uses the full panel raster.

**Highest** requests the finest model section at any distance. At **1x**, the
model distance multiplier follows the active render height. Higher
multipliers keep detail farther away. **Retail** uses the original
480-line distances at any resolution.

Choose Profile and Appearance sample the same setting as a world view.
The setting changes only what is drawn: the guest walk, its pose
evaluation and the lockstep and film digests stay retail whatever the
choice, so no mode forces retail for it. The native renderer applies the
choice to supported display draws, and in single-copy rendering the D3D
fallback swaps its drawn geometry at the draw boundary; the exact
reference, menu model widgets and every readback keep the retail
records.
[Projected size and level of detail](../graphics/occlusion-culling.md#projected-size-and-level-of-detail).

Pinned keeps corner-anchored HUD groups near the edges of each player's
viewport. Centred keeps those groups inside a centred 4:3 safe area.
HUD Position does not change menus. Menu widgets stay centred and keep
their proportions. Full-width menu background art and the scene backdrop
fill the viewport. See
[widescreen projection](../graphics/high-resolution.md#widescreen-projection).

The frame rate applies at the window's next turn (`app::fps::FpsControl`).
The render thread applies resolution and lighting at a frame boundary
(`ScaledBackend::follow_resolution`, `ScaledBackend::follow_lighting`).
See [high resolution](../graphics/high-resolution.md) and
[E3 lighting](../graphics/e3-lighting-setting.md).
A locked row shows the variable's value and A
does nothing. A bad variable is reported and ignored, so the saved value is
used. Above 30, the frame rate redraws the newest game frame more often
([high-framerate.md](high-framerate.md)). Headless runs (`h2 run`) have none of it (`H2_LIGHTING=e3`
works there as a variable).

Code: the rows are `Def`s in `crates/extensions/game/src/setting_defs.rs` with a `menu` naming
the screen; `game::video_screen::model` builds the screen as
`ui::Screen::new("video").rows_from_settings(..)`, the Video and Voice rows as
`ui::extend(List::Settings, 2)` and the speaker row as
`ui::extend(List::Controls, 6)` ([Settings as a schema](#settings-as-a-schema));
the hooks registered by `layout_glue::install` ask that model for counts,
texts and presses. `h2 play` reads the saved frame rate, resolution, lighting, HUD position,
LOD distance and mute button when no flag or variable sets them. Tests:
`setting_defs::tests::lighting_is_retail_unless_asked_and_has_a_video_row`
(default Retail, the row on this screen, live, the file's names, the
variable's lock) and `video_screen`'s tests for the Video rows, their exact
labels and values, and saving live changes.

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
| `0x2b1d6e` | its row text (thiscall, item at `[esp+4]`, `ret 8`; one text, index 0) | row 0 `0x17000196` Player Profiles, row 1 `0x10000197` Game Variants; row 2 shows "Video", row 3 "Voice" |
| `0x2b1da9` | its row chosen (thiscall, `ret 8`) | row 0 opens `0x2b19dc` (Player Profiles), row 1 `0x2b1b01` (Game Variants), through `0x149f1e`; any other row does nothing. Row 2 is ours: `0x149f1e(1 << player, 0x2b7714)` |
| `0x230427` | the pane for a row (thiscall, the row, `ret 4`; no bounds check) | the tag has one pane per retail row; for rows 2 and up the hook returns at once, so the shown pane stays (as for the Skulls row) |
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

**Settings root as the entry.** Video and Voice are rows of the top-level Settings
list, next to Player Profiles and Game Variants, because none of its
settings belongs to a profile. It is added the way the Controls rows are: a
`ui::extend(List::Settings, 2)` with the Video row then the Voice row in
the model, the list hook growing the count to four (plus the one handler
byte that gives the list a third item widget; the handler embeds room for
three, so the fourth is an item widget of ours, linked after them by the
"add items" hook, `video_screen::settings_extra_items`), and wrappers on
the list's own row text and row chosen that answer for rows 2 and 3 and
leave rows 0 and 1 to the originals. No tag is changed: the list already
shows up to six items, and the missing pane is skipped.

The Voice settings are the install's (`Scope::Host`), not a profile's, so
Voice is here and not on Edit Profile ("Edit Profile").

## Pages

| Page | What |
|---|---|
| [Settings menu: the screens](settings-menu-screens.md) | The pause menu, voice, profile and field-of-view screens, and settings as a schema. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## Pause menu

See [Settings menu: the screens](settings-menu-screens.md#pause-menu).

### What pausing means (Local, static)

See [Settings menu: the screens](settings-menu-screens.md#what-pausing-means-local-static).

### Game Setup from the pause menu

See [Settings menu: the screens](settings-menu-screens.md#game-setup-from-the-pause-menu).

## Voice screen

See [Settings menu: the screens](settings-menu-screens.md#voice-screen).

## Edit Profile

See [Settings menu: the screens](settings-menu-screens.md#edit-profile).

## Field of view

See [Settings menu: the screens](settings-menu-screens.md#field-of-view).

## Settings as a schema

See [Settings menu: the screens](settings-menu-screens.md#settings-as-a-schema).

## Not verified

See [Settings menu: the screens](settings-menu-screens.md#not-verified).

## Sources

See [Settings menu: the screens](settings-menu-screens.md#sources).
