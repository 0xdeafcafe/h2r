# Expanded settings menu

Status: Built. Video, Voice, Player Settings and the extra Controls rows use native game screens driven by `settings::Def`s.
Code: `crates/extensions/game/src/video_screen.rs`, `crates/extensions/game/src/pause_menu.rs`, `crates/extensions/game/src/voice_settings.rs`, `crates/extensions/game/src/fov_row.rs`, `crates/extensions/game/src/setting_defs.rs`, `crates/formats/settings`, `crates/apps/cli/src/settings_cmd.rs`. Tests: in those files.
Switch: see [switches.md](../runtime/switches.md).

The screens are the game's own list class with our rows, so they look native and
take pad navigation for free. They are not a host overlay. The rule that
holds: a setting is one `settings::Def`, and nothing on a screen writes guest
simulation memory except through a hook that says so (the clamp global in
"Field of view").

- **Video** on the main menu's Settings list opens the field of view,
  frame rate, resolution, lighting, HUD position and LOD distance options.
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

### Theater keyframe cache

`theater.cache_cap_gib` limits the disk space used by Theater keyframe
sidecars. The default is 4 GiB. To change the limit, run
`h2 settings set theater.cache_cap_gib 8` or set `cache_cap_gib = 8` under
`[theater]` in the viewer's `settings.toml`. The setting takes effect
at launch. Values are whole GiB, with `0` disabling the cache.

## Video screen

The top-level Settings list reads Player Profiles, Game Variants, **Video**
and **Voice**. Selecting Video opens the display options below.

| Row | Values | Saved as | Applies | Locked by |
|---|---|---|---|---|
| Field of View | 70 to 120 in steps of 5 | `[graphics] fov` | at once | `H2_FOV` |
| Frame Rate | 30, 60, 120, Uncapped | `[graphics] fps` | at once | `--fps`, `H2_FPS` |
| Resolution | Native (render width x height), then fixed sizes at the viewport aspect | `[graphics] resolution` (`"native"` or `"WxH"`) | next frame | `H2_RESOLUTION`, legacy `H2_RENDER_SCALE` |
| Lighting | Retail, E3 (default Retail). A [BSP review launch](../graphics/relighting-review.md) displays its locked lamp state | `[graphics] lighting` (`"retail"`, `"e3"`) | next frame | `H2_LIGHTING` |
| HUD Position | Pinned (default), Centered | `[graphics] hud_position` (`"pinned"`, `"centered"`) | live | `H2_HUD_POSITION` |
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
In Live, System Link, co-op and Theater, the native renderer can apply
the choice to supported display draws while the exact reference keeps
its own model records. Guest-only draws in exact mode retain the
reference detail.
[Projected size and level of detail](../graphics/occlusion-culling.md#projected-size-and-level-of-detail).

Pinned keeps corner-anchored HUD groups near the edges of each player's
viewport. Centered keeps those groups inside a centred 4:3 safe area.
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

## Pause menu

Open **Player Settings** from the campaign or multiplayer pause menu. It
opens the game's Controller Settings or Player Settings list for the
pressing player. Both native lists include **Video** and **Voice**
after the game's settings. Those rows open the same pages as the main
menu.

| Menu | Rows, top to bottom |
|---|---|
| Campaign (GAME PAUSED) | Continue, Revert to Last Checkpoint, Restart Level, Player Settings, Save and Quit |
| Multiplayer (GAME MENU) | Leave Game, Player Settings, Handicap, then Change Teams, Party Privacy, **Change Map** and End Game when the game shows them |

B or Back from Video or Voice returns to its row in the native
list. Another B returns to the pause menu. Campaign stays paused through
both steps. Multiplayer continues to run.

Code: `game::pause_menu` and `layout_glue::install`. The
`features.pause-rows` and `features.pause-mp-rows` switches control the
campaign and multiplayer extensions. Each list includes the Video and
Voice pages whose classes are enabled. When neither page is enabled, the
game handles its original Settings row directly. Both features are
stateless for snapshots: they draw their rows from guest memory and hold
no host state, so they never forbid a machine keyframe.

**Game Setup** shows above End Game for the party leader of a custom
game ("Game Setup from the pause menu" below).

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
multiplayer menu's action 1 calls `0x232aff`. This opens Controller
Settings (screen 200) when `0x2b505c(player)` is true and the player
settings screen (196, loader `0x23252e`) otherwise. The window saves the
pause menu as a reopen request. Actions 0 and 2 to 5 go to `0x2329a7`,
`0x232b63`, `0x232bb4`, `0x232c05` and `0x232c4d` [not read further].

The row names, from the row text functions and the game's strings: the
campaign menu's rows 0 to 4 are Continue (`0x08000144`), Revert to Last
Checkpoint (`0x130002ea`), Restart Level (`0x0d0002eb`), Controller
Settings (`0x130002ef`) and Save and Quit (`0x0d0002ec`; Quit,
`0x040002ed`, when `0x18f911` says more than one). The multiplayer
menu's actions 0 to 5 are Leave Game, Settings, Handicap, Change Teams,
Party Privacy and End Game (the table built at `0x23284e`).

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
  slot). Settings and its pages keep the campaign paused until the last
  screen closes. The UI hooks leave the paused byte to the game.
- **Multiplayer.** Nothing on this path writes the byte: `mp_menu` sets
  nothing, so the simulation runs while it and our screens are up, and
  the network game is never held. A press on a row of the Video or Voice
  screen writes
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
does nothing for any other value. The shared count and row hooks append
Controller Speaker (value 6), a read-only Aim Assist row (value 7) on
iOS, and Lightbar (value 8) on macOS. **Aim Assist: Change in Main Menu**
directs the player to the other touch options on the main menu's Controls
screen. Campaign applies aim assist at map load. The local touch player
in multiplayer uses the setting during play.

On macOS, **Lightbar: Team** matches each PlayStation pad to its local
player's team colour in a team game or their primary armour colour in
menus, free-for-all and campaign. Press A to choose **Off** and restore
the controller's default colour. Both Controls screens use the same
host setting, `controls.lightbar` in `settings.toml`, which defaults to
`true` and applies during play (`setting_defs::PAD_LIGHTBAR`).

### How our rows are added

The pause menus retain their native row counts and action words.
`pause_menu::Settings` labels campaign row 3 and multiplayer action 1
**Player Settings**. In campaign it opens Controller Settings directly on
the pause screen's window and slot so the window can restore the pause
menu. In multiplayer the game's original action 1 picks the native list
for the pressing player.

The player list (196) uses constructor `0x2b4f5c`, row text `0x2b508c`
and row chosen `0x2b50d9`. The constructor has four possible native
actions and may omit some of them. `layout_glue::install` extends the
datum capacity of each native list and appends one datum per enabled page
after its normal rows. The action words are 9 for **Video**
and 10 for **Voice**. They are independent of the number of visible
widgets, so the existing list scrolls to show them. Native rows keep
their original labels and actions.

`list_class::push` opens Video and Voice on the native list's
window and slot. The game-shell window saves the parent loader and focus.
The normal Back handler restores them, including in the pressing player's
quarter in split screen.

### Game Setup from the pause menu

The party leader of a custom game can change the next game from GAME
MENU. **Game Setup** opens a list of ours with the lobby's own rows:
**Change Map...**, **Change Rules...**, **Quick Options...** and
**Clanker Options...**, each shown when it has something to offer.

- **Change Map** offers the same categories and maps as Game Setup's
  Change Map ([map-select.md](map-select.md)). A Forge or Firefight
  lobby opens its own category's maps, and a single category opens its
  maps directly. The maps are a value picker; picking one closes the
  picker and the categories.
- **Change Rules** offers the game types and their variants the lobby's
  Change Rules lists, including Research and Devolution, Party Games, Firefight, Siege,
  the community variants and the game-variant store's (Live, imported
  and ported variants, rules-script variants among them). A Firefight
  lobby has no Change Rules, as in the lobby.
- **Quick Options** offers Score to Win, Time Limit, Lives per Round and
  Respawn Time of the variant the next game plays: the one Change Rules
  picked in this visit, else the lobby's.
- **Clanker Options** offers the Clankers page's settings.

Map, rules and Quick Options changes wait in the visit's draft, and the
row shows what was picked (`Change Map  Lockout`). Nothing changes in
the running game. **B** on Game Setup with a draft opens a dialog with
**Restart with these settings** and **Keep playing** (focused). Restart
ends the game for everyone. Back in the lobby, the lobby takes the map
and the variant and the countdown starts with no further press. Keep
playing drops the draft and closes Game Setup. B on the dialog goes back
to Game Setup with the draft kept. B with no draft closes Game Setup.
What the draft leaves out stays the lobby's: a map alone keeps the
lobby's variant, rules alone keep its map.

Clanker Options' changes apply at once, in the running game: the
Clankers driver follows the settings store, as it does for the lobby's
page (`clankers::LIVE_KEYS`, `pause_change_map::clanker_rows::applies`).
A count change adds or takes off bots, and a difficulty or style change
reaches the bots at their next frame. Teams has no reader, so changing
it changes nothing yet.

Quick Options never applies live (`pause_change_map::quick::applies`).
A game plays the copy of the variant made at its start (`G+0x13c`), the
other consoles get the record only with the game's parameters
([game-options.md](game-options.md#how-a-variant-reaches-other-machines-confirmed)),
and which code reads each setting from the copy during a game is not
traced. A change written there would reach this console only, so every
Quick Options change waits for the restart. The page is ours, not
retail's: Quick Options' screen (222) is in `mainmenu.map` only.

Code: `game::pause_change_map` (`PauseChangeMap`, `install`, from
`boot::registration`; `setup`, `game_types`, `quick`, `clanker_rows`).
The `features.pause-change-map` switch turns the row, Game Setup and the
restart off. Each screen's class is its own feature
(`pause-game-setup`, `pause-map-categories`, `pause-game-types`,
`pause-quick-options`, `pause-clanker-options`); a row whose screen is
off is left out.

- **Who gets the row.** Retail shows End Game (action 5) only on the
  console that leads the party (`0x592f0`) outside matchmaking (lobby
  mode `0x19989d` not 6). An `after` hook on the list's constructor
  (`MP_MENU_LIST_CTOR`) adds our row (action word `0x30`) just above End
  Game when the list has End Game, the life cycle is not matchmaking's,
  this console hosts the squad session (session `+0x40` equal to
  `+0x72d8`) or the game is offline, and Game Setup has a row
  (`pause_change_map::offered`, `avail`, `setup_rows`). Every other
  player's GAME MENU is retail's. The rows' array (name `0x4593a8`) is
  made one row larger while the feature is built
  (`pause_change_map::list_count`, from the datum array hook in
  `layout_glue`), on top of the Forge rows' room.
- **One row, not four.** GAME MENU's list is not shown to scroll past its
  item widgets, so the four rows sit on Game Setup and GAME MENU gains one
  row, as with Change Map alone.
- **Text and press.** `MP_MENU_ROW_TEXT` draws **Game Setup** from our
  string slot. `MP_MENU_ROW_CHOSEN` runs End Game's press for any word it
  does not know, so ours is always answered before the original. It
  reads the built-in variants and the lobby's variant, starts a visit
  with an empty draft, opens Game Setup on GAME MENU's own window and
  slot (`list_class::push`) and remembers the pressing controller.
- **The screens** are list classes of ours built on screen 200 in a game
  ("In a game, our screens use screen 200"), with texts from the
  `pause change map` string family. Game Setup, Quick Options and
  Clanker Options draw one line a row (`ListClass::lines`), so a value
  shows after its label. Game Setup takes B and Back as its action
  (`setup::back`). The maps, variants, values and the confirm dialog are
  the kit's value picker (`ui_kit::picker`, the large dialog in a game).
  A pick from the categories or game types closes that list once the
  picker has closed (`pane_ready` notes it). The retail Select Map and
  SELECT GAME screens are not used in a game: their lists are filled
  from `mainmenu.map`. Mod pack folders are not listed in the pause
  picker.
- **The variants** are the game's built-in list (`[0x51ea14]`, a count
  and `0x1e4`-byte records with the variant at `+4`), into which
  `community_variants` and `game_variants::inject` merge theirs at each
  main menu load (`game_types::read`). A record is filed under our row
  when its name or the rules script its block names says so
  (`tweakers::record_row`), else under its game type, in the game type
  list's order. Saved variants are listed as the store's ported copies.
- **Quick Options' values** are the ones the variant editor offers for
  the setting on this disc, kept from the last built-in load
  (`community_variants::last_offered`), else the editor's steps from
  [game-options.md](game-options.md); the held value is always listed. A
  value the clean-up would refuse is not stored (`quick::apply`,
  `record::refused`).
- **Ending the game.** `firefight::guest::end_game`, the engine's
  round-over path: engine state over, the end latch through `GAME_END`
  once, the engine's changed flag. The game ends as at its time or score
  limit, with the postgame carnage report.
- **The next game** (`pause_change_map::Pending`). A hook before
  `UI_UPDATE` reads the lobby each frame. It does nothing until the main
  menu is back with a lobby in its pregame life cycle (`[0x527334]` 1 or
  2, or an offline lobby). With a map, it sets the lobby's map with
  `LOBBY_SET_MAP` (map id in ESI, campaign -1), as retail's Change Map
  does, so the change reaches the other consoles. With a variant, it
  then writes the record to a page of ours and calls the lobby variant
  setter (`host_lobby::SET_VARIANT`, `0x19a728`, stack the record, AL
  nonzero when taken), as the playerless host does. Each call retries
  every 0.5 s while refused. Two seconds after the last setting is set it
  asks retail's start check (`LOBBY_START_STATE(0)`). On 9 (ready) it
  asks for the countdown for the pressing controller (`0x19a78e`, EAX
  the controller, stack 10 and 3, as the lobby's Start Game asks). On 10
  (a countdown runs) it stops. On 8 (this console does not lead the
  party) it gives up. Other states wait. After five minutes it gives up
  and the lobby keeps what was set, so the leader presses Start. Each
  step is logged with a `pause game setup:` line.

[not verified] Statically unverifiable, to check in a run:

- `LOBBY_SET_MAP`, the variant setter and the countdown are called only
  in the pregame lobby, where retail calls them. Nothing is set during
  the game.
- That the lobby's own Change Rules sets its variant through `0x19a728`,
  and that a record set there reaches the other consoles as a Change
  Rules pick does. That it keeps the record's `hx` block
  ([rules-variants.md](rules-variants.md#not-verified)).
- That the built-in list at `[0x51ea14]` is still filled in a game, after
  `mainmenu.map`'s tags are unloaded. With it empty, Game Setup has no
  Change Rules row.
- That a bot the Clankers driver adds during a game joins it, as a
  remote peer joining in progress.
- The postgame carnage report is not skipped. Whether the life cycle
  leaves the postgame (4) for the lobby (1 or 2) by itself, or only after
  a press, is not traced. The next game starts once the lobby is back.
- The pause menu's player index is the controller port that
  `0x19a78e` takes in EAX (both index the records at `0x54e8e0 + port *
  0xc70`).
- The round-over path reaches the other consoles through the engine's
  changed flag, as it does for Firefight.
- B and Back reach Game Setup's event handler as button words 1 and 13
  (`list_class::BUTTON_EVENT`), before the generic close.
- Closing the categories or game types list from the picker's `after`,
  while the picker closes on the dialogs' window.
- Our screens and GAME MENU stay up while the game ends. The map unload
  takes them down, as it takes down a retail GAME MENU left open when a
  game ends.

`ui.screen` and `ui.stack` show GAME MENU's rows and the Game Setup
screens ([agent-api.md](../runtime/agent-api.md#ui-what-the-menus-show)),
so the API needs no new method for this.

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

`game::pause_change_map::tests` covers who gets the Game Setup row, its
place above End Game, Game Setup's rows in order per lobby mode and
what each offers, Change Map's opening per lobby mode, the game types
and the built-in list read from guest memory, the draft (a map, a
variant, both, Quick Options changes and refusals), the confirm dialog,
Quick Options' values, which changes apply live (Clanker Options) and
which wait for the restart (Quick Options), the switch, the plan with
the Forge rows and the next game's steps (map, variant, both,
countdown, give up). `game::pause_menu::tests` covers the pause row labels, the native
multiplayer Settings action, both destination lists' new action words,
feature switches, and live Video and Voice changes.
`game::list_class::tests` covers the window and slot passed to a child
loader. `game::video_screen::tests` checks the display options, Mute
Button in Voice and Aim Assist on the iOS Controls list.
`game::speaker_row::tests` checks the read-only iOS Aim Assist row and the
pause Lightbar row. `game::layout_glue::tests` checks Lightbar's default,
the Controls row and persistence.

### To check in game

Use `H2_UI_WATCH=1`, `h2 agent press` for buttons and
[`h2 agent ui`](../runtime/agent-api.md#ui-what-the-menus-show) to check the
screen after each step.

1. Start in campaign or multiplayer, select **Player Settings** and press
   A. Check for screen 200 in campaign or screen 196 or 200 in multiplayer.
2. Scroll to **Video** and press A. Check for `class: "video"` on
   the same window and slot and the display options. Press B and check
   that the native list returns at **Video**.
3. Select **Voice** and press A. Check for `class: "voice"` on the same
   window and slot and **Mute Button** below **Voice Volume**. Press Back
   and check that the native list returns at **Voice**.
4. Press B to return to the pause menu. In campaign, Continue resumes the
   game. In multiplayer, play continues behind the menus. Check the
   same navigation from the second seat in split screen. Use `h2 agent
   ui.stack` to inspect that seat's channel.
5. On iOS, check **Aim Assist: Change in Main Menu** in the in-game
   Controller Settings list. Check **Aim Assist** in the main menu's
   profile Controls and step its value there. On other platforms Controls
   has no Aim Assist row.

## Voice screen

Open **Voice** from the main menu's Settings list (after **Video**) or
from **Player Settings** in a game. The rows are host settings (`Scope::Host`) and apply at
once. The voice chat reads its settings through
`voice_settings::VoiceHandles`. The DualSense HID thread follows **Mute
Button** through `PadHid::mute_switch`.

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
| Mute Button | `pad.mute_button` | On, Off (default On) | `H2_PAD_MUTE` |

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

## Edit Profile

Edit Profile is retail. Nothing of ours adds a row to it or changes its
panes: the Voice settings are the install's (`Scope::Host`), not a
profile's, so Voice is a row of the main menu's Settings list, not of a
profile ("Video screen", "Voice screen").

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

- The director and observer apply the setting before the renderer builds
  the view. The guest projection, portal frustum, particle scale
  (`0x248a31`) and LOD follow that camera. The native BSP stage uses the
  captured projection for its part bounds. A widescreen display can widen
  the horizontal lens after the guest gathers clusters. The extra strip
  can therefore show only geometry the guest submits.
  [Display aspect and live visibility](../graphics/high-resolution.md).
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
  position. In `0xbd610`, the `+0x38` load at `0xbd67f` reads the object
  definition. The `+0x4c` load at `0xbd8d2` reads render-model node data.
  Both pointers come through tag instances.
- The observer and director are local to each player. The camera hook's
  only guest write is the clamp at `0x55e488`. Object setup during the
  guest visibility walk can consume shared animation RNG, so the display
  aspect correction runs after that walk.
  [Render determinism](../graphics/high-resolution.md#determinism).

Only the first-person camera follows the setting; the following camera
(vehicles; `0x23bd70` stores `[0x54e854]` itself), dead and scripted
cameras do not.

## Settings as a schema

The schema is the settings layer of [feature-engine.md](../engine/feature-engine.md). A setting is one
`settings::Def` (`crates/formats/settings`): its `table.key` in `settings.toml`, its
label and help, its kind (bool, bounded whole number, resolution or choice), its default, the variable and flag that override it,
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
- Which retail option lives in the profile and which in the preferences file,
  and their layouts. Nothing here reads or writes the retail values.
- Which row-level behaviours only a game run shows: the Video rows apply as
  the tables say (frame rate, resolution, lighting and LOD distance without a flash or a
  black frame); the Video and Voice screens return to the native settings
  list with B; in split screen each page opens in the pressing player's
  quarter.
- Field of view: the first-person weapon model, the HUD scope overlays and
  the sniper's zoom ring at wide values, and whether the following camera
  (vehicles) should follow; the read of `0xbd610`.
- The Voice screen in a networked game, kept up for a while while rows are
  pressed (no stall, no disconnect).

## Sources

Community, cited and not copied:

- Project Cartographer: <https://github.com/pnill/cartographer>.
