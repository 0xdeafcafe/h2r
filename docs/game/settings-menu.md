# Expanded settings menu

Status: built as native game screens (approach B below), not as the host
overlay (approach C) this page first recommended. Nothing has been built of
C: there is no egui overlay, hotkey or pad chord, and "Opening the menu",
"Testing" and "Phased plan" are the original design, kept for the
reasoning. What exists:

- a "Video" row on the top-level Settings list opens our Video screen with
  the field of view, the frame rate, the resolution, the lighting, the mute
  button and aim assist ("Video screen" and "Field of view (built)" below);
- the Controls screen gains rows for the controller speaker, the Steam
  Controller (speaker, trackpad aim, left trackpad) and, on iOS, the touch
  controls ([playstation-audio.md](../audio/playstation-audio.md) "In-game
  setting", [ios.md](../runtime/ios.md#controls));
- the button layouts are rows on its Button Layout screen
  ([controller-layouts.md](../input/controller-layouts.md));
- every setting is a `settings::Def` in `boot::setting_defs`, and
  `h2 settings` lists, gets and sets them ("Settings as a schema").

The rest says what we know about Halo 2's own settings screens and why the
menu is built this way.

The menu was meant to hold, in the order the README lists them:

- resolution scale and 4K;
- 60 and 120 fps;
- E3 2003 lighting;
- merged render passes and the other performance toggles;
- mouse sensitivity and input bindings;
- field of view;
- vsync.

None of these exist as game options. The retail game has no concept of most of
them, so the question is where our code puts the controls, not how to extend a
retail list.

No game data is in this document. Screen layouts below were read with our own
`h2 ui`, `h2 tags` and `h2 strings` commands and are described in words. Guest
addresses are named only as hook points, and only when an existing doc already
names them.

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
Mapping each option to a field is a prerequisite for any approach that wants to
read or write the retail values, and is not needed for the recommended design
below.

### What Cartographer did on Windows (Community)

Project Cartographer, the H2V online project, is the closest prior art. Its
public source (not Xbox) shows both techniques the next section weighs:

- Native screens: it declares C++ classes for the settings, video, audio,
  controller, brightness, gamma, LOD, vsync and virtual-keyboard screens, and
  adds its own list item and pane ("guide") to the settings screen by tag
  injection plus function hooks. See
  [`screen_settings.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/interface/screens/screen_settings.cpp)
  and the [`screens/`
  directory](https://github.com/pnill/cartographer/tree/master/xlive/Blam/Engine/interface/screens).
  Adding even one row needed a larger list-data allocation, a new pane and
  code for the press handler.
- An ImGui overlay for "Advanced Settings", opened from the settings screen, a
  hotkey, or a controller chord (select plus d-pad up). See
  [`H2MOD/GUI/imgui_integration/AdvancedSettings.cpp`](https://github.com/pnill/cartographer/tree/master/xlive/H2MOD/GUI/imgui_integration),
  the [DeepWiki overview](https://deepwiki.com/pnill/cartographer) and the
  [install notes](https://www.halo2pc.com/install/).

So the community answer to "native or overlay" was "both": native screens for
things that belong with the profile, an overlay for the long tail. That fits
what we find below.

## Where our runtime can inject entries

Our runtime owns guest memory, the file system the game reads, the window and
the GPU output. That gives three places to act.

### A. Tag patching in memory at load

Edit the tags after the game loads them, or serve a modified map from the VFS
so the game reads modified tags. Add a list item, a text block and string IDs.

- It looks native, uses the game's skin, and inherits controller navigation for
  free, because the list code already handles d-pad and A/B.
- It cannot add behaviour. A row needs a press handler and a value renderer, and
  those are code. Raising the visible-item count on a two-column list gives an
  empty row that does nothing. Without a code hook the best case is a
  display-only row.
- New strings must go into a `unic` for each of the nine language slots, or the
  lookup misses (there is no English fallback in the lookup path, see
  [localization.md](../formats/localization.md)). A miss returns an empty string.
- Tag memory is guest memory. Whether the patched bytes end up in a checkpoint
  or in anything the game saves is not known. Checkpoints are raw snapshots of
  game-state memory ([checkpoints.md](../runtime/checkpoints.md)); whether the tag cache is
  inside that region has not been checked (Speculation: probably not, but test
  it).
- Serving a modified map from the VFS means writing derived retail data
  somewhere. That must stay in the user's cache, never in the repo, and every
  retail map would need a patch step.

### B. Hooking the guest UI functions

Replace or wrap the screen's code, as Cartographer does: add a screen class,
register its loader, and detour the press handler.

- It looks native and can do real work in the game's own widgets, including
  pad navigation, sounds and animations.
- It is the most fragile option. It depends on exact function addresses and
  object layouts of this one executable, and every screen needs its own
  discovery work. We have the widget header and widget type constructors
  ([ui.md](../formats/ui.md)), but not the list and screen class vtables or the event
  system.
- Guest code that runs in the middle of the simulation tick can change game
  state. A hook must keep to UI state only.
- Strings, fonts and layout still have to work in nine languages, and in the
  CJK font tables, with the existing glyph coverage ([fonts.md](../formats/fonts.md)).
- The recompiled game is built from the XBE ahead of time. A hook point is a
  guest function we replace through dispatch overrides (the same mechanism that
  replaces D3D, DirectSound and XAPI), so the mechanism exists. The cost is
  reverse engineering, not plumbing.

### C. A host-side overlay menu

Draw the menu in the shell, on top of the game frame, with a host immediate
mode UI such as egui. The shell already composites the guest frame into the
window surface and draws a banner in a second alpha-blended pass
([app.md](../runtime/app.md): `Compositor`, `app::overlay`). egui's wgpu renderer is
designed for this: it records into a render pass that loads the existing frame
([egui-wgpu](https://docs.rs/egui-wgpu/latest/egui_wgpu/), and a [working
example](https://github.com/emilk/egui/discussions/3067)). The window thread
owns it. The guest thread never sees it.

- It does not look native. It can be styled to be close (dark panel, Halo-like
  palette), but it will not use the game's fonts and skins unless we add that
  work.
- Controller navigation is ours to build. `egui-winit` translates window
  events, not gamepads. The input layer must go into a "menu mode" that routes
  pad, keyboard and mouse to the overlay and keeps them away from XInput, then
  feed arrows, Tab and Enter into egui. (Speculation on the egui side: its
  focus navigation is keyboard driven, so mapping pad events to keys should
  work; prototype it before committing.)
- Localization is ours: our own table, or English only at first. The retail
  `unic` data is not involved.
- Save compatibility is a non-issue: the guest profile is not read or written.
- It stays out of the simulation by construction, provided opening it pauses
  the guest the same way Cmd+P does. `host.tick` already blocks while paused,
  and guest time is logical, so a pause does not change what the guest sees.
- The shell already intercepts keys (F3, F5, F6, Cmd+F, Cmd+P), so a chord
  and a hotkey have a precedent. The development-build watermark (F3) is a
  small instance of this design: host text drawn over the game image after
  the compositor, never in guest memory.

### Trade-offs

| | A. Tag patch | B. Guest hook | C. Host overlay |
|---|---|---|---|
| Looks native | yes | yes | no, unless styled |
| Controller navigation | free (game list code) | free | we build it |
| Can do real work (apply a setting) | no, display only | yes | yes |
| Localization | per-language `unic` edits, 9 slots | same, plus font coverage | our table, our font |
| Save compatibility | tag memory may leak into snapshots (unchecked) | game state risk if a hook runs mid-tick | none |
| Stays out of the simulation | yes | only with care | yes |
| Works with no retail knowledge of a screen | no | no | yes |
| Cost | low for a dead row | high, per screen | moderate, one time |
| Survives a title update (1.1+) | needs address map | needs address map | yes ([title-updates.md](../runtime/title-updates.md)) |

### Recommendation

1. Build **C**. Every setting on the list can be applied from the host side or
   needs a host-side change anyway (swap chain, render target size, render
   passes, bindings). C has no dependency on understanding more of the engine.
2. Treat **B** as a later nicety: one "Enhancements" row on the retail settings
   screen that opens the overlay, so the entry point is discoverable. That is a
   single hook and a single string, not a native settings UI. It needs a trace
   of the settings screen's list code first, and it is optional. A hotkey and a
   pad chord (Back plus Start, say) work without it.
3. Skip **A** except for cosmetic cases, such as relabelling, where it is the
   cheapest tool.

## Where settings persist

A host-side file, not the guest profile.

- Path: `settings.toml` in the data root the run uses (`h2 play --data`,
  default `$TMPDIR/h2boot-data`; `vfs::Config::for_title` would give
  `~/Library/Application Support/h2recomp`, [storage.md](../runtime/storage.md)). It is
  not under `disk/`, which is the guest's. Every setting below is written
  there by one writer, `settings::Store` (below), keeping other keys.
- Reasons: the guest profile is signed and sized by the game, and writes to it
  go through retail code we should not perturb. Enhancements are per install,
  not per gamertag. A host file is easy to edit, diff, delete and share, and it
  does not travel with a save someone else loads.
- Format: TOML, the same as `bindings` ([input.md](../input/input.md)), which is already
  a config with validated values and unknown-key errors. Keep that policy: an
  unknown key or an out-of-range value is an error that names the key.
- The existing bindings file stays valid and is referenced from the settings
  file (`bindings = "path"`), or its tables are accepted inline. Do not make
  users migrate.
- Writes are atomic (write a temp file, rename). A failed write shows an error
  in the menu and keeps the old file.
- Precedence, highest first: command-line flag, environment variable, settings
  file, default. Flags and variables win so scripts, benchmarks and `--follow`
  runs stay reproducible and the file never silently changes a measurement.
  The menu shows when a value is overridden and by what.
- A per-profile override can be added later, keyed by a host-chosen name rather
  than the guest gamertag.

Sketch (the original idea, not the schema: the built keys are `graphics.fov`,
`graphics.fps`, `graphics.render_scale`, `graphics.lighting` and the rest in
`boot::setting_defs`):

```toml
[display]
vsync = true
fullscreen = false
aspect = "4:3"        # as --aspect
scaling = "fit"       # as --scaling
filter = "linear"     # as --nearest
render_scale = 1.0    # 1.0 = native 640x480; 2.0, 3.0 ... up to 4K

[timing]
fps = 30              # 30, 60, 120 (built as [graphics] fps)

[graphics]
merge_passes = true
lighting = "retail"   # "retail" or "e3" (built: the Video screen's Lighting row)
fov = 0               # 0 = the game's value; otherwise degrees (built as [graphics] fov)

[input]
bindings = "bindings.toml"
mouse_sensitivity = 0.04
invert_y = false

[performance]
render_thread = false
pipeline_cache = "default"   # a path or "off"
compile_threads = 0          # 0 = default
```

## How the existing H2_* flags map onto settings

Not every flag belongs in a menu. They fall in four groups.

**Shell and window (live, already `AppConfig` or an `h2 play` flag).**

| Setting | Existing control | Applies |
|---|---|---|
| vsync | `--no-vsync`, `AppConfig::vsync` | live (reconfigure the surface; today set at creation, so needs a small change) |
| fullscreen | `--fullscreen`, Cmd+F | live |
| aspect, scaling, filter | `--aspect`, `--scaling`, `--nearest` | live |
| mouse capture, pause on blur | `AppConfig` | live |
| audio, gamepads | `--no-audio`, `--no-gamepads` | restart |
| controller speaker | **Built:** a "Controller Speaker" row on Settings, Controls, saved as `[pad_speaker] enabled` in `settings.toml` in `h2 play`'s data root. `H2_PAD_SPEAKER` (on, or a TOML file) locks it when set; `H2_PAD_SPEAKER_DEVICE`, `H2_PAD_SPEAKER_EXCLUSIVE` ([playstation-audio.md](../audio/playstation-audio.md)) | on/off live (the router and the HID speaker path, `boot::speaker_setting`); routes and exclusive could go live the same way; device needs the output reopened |
| field of view | **Built:** a "Field of View" row on the Video screen, saved as `[graphics] fov` in `settings.toml`. `H2_FOV` (degrees, or `0` for retail) locks it when set; `H2_NO_FOV_ROW=1` removes the row and the camera hook | live (the camera hook reads the setting every frame) |
| frame rate | **Built:** a "Frame Rate" row on the Video screen (30, 60, 120, Uncapped), saved as `[graphics] fps` (`30`, `60`, `120` or `"uncapped"`). `--fps` or `H2_FPS` locks it when set ([high-framerate.md](high-framerate.md)); above 30 it only redraws the newest frame until high-fps phase 2 | live (the window follows it at its next turn) |
| resolution scale | **Built:** a "Resolution" row on the Video screen (640x480, 1280x960, 1920x1440, 2560x1920: the game's 640x480 back buffer times the scale), saved as `[graphics] render_scale` (the file may hold 1 to 6). `H2_RENDER_SCALE` locks it when set ([high-resolution.md](../graphics/high-resolution.md)) | live (the next frame is at the new size) |
| lighting | **Built (phase 1):** a "Lighting" row on the Video screen (Retail, E3), saved as `[graphics] lighting` (`"retail"` or `"e3"`). E3 draws the game's shadow maps at the render scale (at least 2x) in the shown image only and compares them with one tap; the guest sees retail. `H2_LIGHTING` locks it when set ([e3-lighting.md](../graphics/e3-lighting.md), "Phase 1 as built") | live (the next frame) |
| Halo 1 options | **Built:** not in Settings. Chosen per game on the Halo 1 options screens (`boot::h1_options`): the custom game lobby's "Halo 1 Options" (Sandbox, HUD) and a Halo 1 level's Game Options page (Assets, HUD). Saved as `[multiplayer] halo1_sandbox`, `[campaign] halo1_assets`, `[halo1] hud` (`"h1"` or `"h2"`); `H2_HALO1_SANDBOX`, `H2_HALO1_ASSETS`, `H2_HALO1_HUD` lock them ([h1-multiplayer.md](h1-multiplayer.md) "Halo 1 options") | the next map load (sandbox, assets) or game start (HUD) |
| aim assist | **Built:** an "Aim Assist" row on the Video screen (Auto, Default, Strong, Very Strong), saved as `[controls] aim_assist`. Auto is Strong while the iOS touch controls show; in multiplayer only the touch player gets it, per player, and the weapon tags stay retail. `H2_AIM_ASSIST` locks it when set ([aim-assist.md](aim-assist.md)) | next map load |
| touch controls (iOS) | **Built:** five rows on Controls on iOS, in place of the Steam Controller's: "Touch Look Speed" (1 to 10, `[touch] look`, `H2_TOUCH_LOOK`), "Touch Crouch" (Toggle, Hold; `[touch] crouch`, `H2_TOUCH_CROUCH`), "Gyro Aim" (0 off to 10; `[touch] gyro`, `H2_TOUCH_GYRO`), "Look Acceleration" (0 to 10; `[touch] look_accel`, `H2_TOUCH_LOOK_ACCEL`) and "Touch Auto Fire" (Off, On; `[touch] auto_fire`, `H2_TOUCH_AUTO_FIRE`). `H2_NO_TOUCH_ROWS=1` removes them ([ios.md](../runtime/ios.md#controls)) | live |
| Steam Controller | **Built:** three rows on Controls with a Steam Controller: "Steam Pad Speaker" (`[steam_pad] speaker`, `H2_STEAM_SPEAKER`), "Trackpad Aim" (1 to 10, default 5; `[steam_pad] sensitivity`, `H2_STEAM_SENSITIVITY`) and "Left Trackpad" (D-Pad, Stick; `[steam_pad] left_pad`, `H2_STEAM_LEFT_PAD`) | live (`boot::setting_defs::steam_tuning`) |
| controller mute button | **Built:** a "Mute Button" row on the Video screen, saved as `[pad] mute_button`. `H2_PAD_MUTE` (`1` default, `0` off; [playstation-audio.md](../audio/playstation-audio.md)) locks it when set | live (sent to the HID thread) |
| Live game visibility | **Built:** `live.visibility` (`public`, `friends`, `invite`; `H2_LIVE_VISIBILITY`). No row of ours: the title's own party privacy screen sets it, and the HLE updates an open lobby ([live-custom-games.md](../live/live-custom-games.md)) | at once |
| Co-op players | **Built:** `coop.max_players` (2 to 16, default 2; `H2_COOP_MAX_PLAYERS`). No row: the most players a co-op campaign this console hosts over Live takes, one per console. Above 2 is experimental ([live-coop.md](../live/live-coop.md) section 13) | next launch |
| Join in progress | **Built:** `coop.join_in_progress` (on/off, default on; `H2_COOP_JOIN_IN_PROGRESS`). No row: whether a friend can join a co-op campaign this console hosts while it runs (the host sends its game state; nobody reverts). Wired through the game's own synchronous join (the host's join check lets a console in while the game runs); `H2_COOP_JIP_LIFT=0` keeps retail's check, `H2_COOP_JIP_LOG=0` drops the `coop-jip:` lines ([live-coop.md](../live/live-coop.md) section 14.9) | live |
| bindings, deadzones, mouse sensitivity, invert Y | `--bindings`, [input.md](../input/input.md) TOML | live once `Bindings` can be replaced on a running manager (not implemented) |

**Graphics backend (runtime variables, read at launch).**

| Setting | Variable | Applies |
|---|---|---|
| merged passes | `H2_GFX_MERGE_PASSES=0` turns off; the loose rule is the default, `=fast` or `=exact` for the stricter rules | `GfxBackend::set_merge_passes` and `set_merge_discards` exist, so live |
| render thread | `H2_RENDER_THREAD=0` (off; on by default) | restart; it moves the backend to another thread ([render-thread.md](../graphics/render-thread.md)) |
| pipeline cache | `H2_PIPELINE_CACHE` (path or `off`) | restart |
| shader compile threads | `H2_GFX_COMPILE_THREADS` | restart |
| frame stats | `H2_FRAME_STATS`, `--frame-stats` | live (a HUD or log toggle) |
| watermark | on; `--no-watermark`, `H2_WATERMARK=0`, F3 ([app.md](../runtime/app.md) "Watermark") | live: `app::Watermark::set_visible` |

**Build-time (change generated C).**

| Flag | Notes |
|---|---|
| `H2_FASTMEM`, `H2_X87_NATIVE`, `H2_ACCESS_COUNT` | They change the generated library and its cache key ([BUILD-CACHE.md](../lifter/BUILD-CACHE.md)). The menu must not offer a toggle that takes minutes to rebuild. Show them as read-only status ("native x87: on"), with the `h2 build` command in a tooltip. |

**Developer-only (never in a menu).** `H2_BOOT_*`, `H2_DISC`, `H2_XBE`,
`H2_PERF*`, `H2_PROFILE*`, `H2_TIMELINE*`, `H2_FAULT*`, `H2_TRACE`, scripted
input and the rest of the boot runner's variables. They are for reproduction
and measurement. The settings file has no keys for them.

**New settings with no existing flag.** None left: E3 lighting's phase 1
(sharper retail shadows) is the Lighting row above, and its later phases
(stencil shadows, per-pixel lights) will change what "E3" draws, not add a
row.

Notes on specific settings:

- **Mouse sensitivity.** The host value is deflection per mouse count; Halo
  then applies its own deadzone and its own per-profile look sensitivity, which
  the retail option screens set ([input.md](../input/input.md)). The menu scales the
  first and does not touch the second, and the labels must say so, or users will
  look for one number and find two.
- **Resolution scale and 4K.** The D3D layer reports 640x480 surfaces to the
  guest and renders through wgpu. Scaling means rendering at a multiple and
  keeping every guest-visible size and every sampled render target consistent.
  Effects that sample the back buffer at a fixed texel size, and render
  targets with 1:1 pixel assumptions, are where it will break
  ([d3d.md](../graphics/d3d.md), [graphics.md](../graphics/graphics.md)). Separate document and test
  corpus.
- **60 and 120 fps.** The engine's simulation runs on a tick rate read from game
  options ([engine-map.md](../engine/engine-map.md)), 30 Hz in retail. The settings
  decision is "interpolate render, keep the tick", because changing the tick
  changes physics and scripts. Community 60 fps patches for the Xbox version
  exist ([XPSemu release notes](https://github.com/ZiZc3/XPSemu/releases),
  [a ModDB patcher](https://www.moddb.com/mods/halo-2-reclaimed/downloads/halo-2-reclaimed-r2-visuals-gameplay-patcher),
  [a 720p/60 effort](https://www.tomshardware.com/video-games/console-gaming/determined-modder-gets-halo-2-running-at-720p-on-the-original-xbox-after-tweaks-to-the-console-hardware-kernel-and-game)),
  but the search results did not say how they treat the tick, so treat those as
  proof that it can be done, not as a method. Separate design.
- **E3 lighting.** The README item is the E3 2003 stencil-shadow work
  (`pstencil`). The public record is contested: [Halopedia's
  page](https://www.halopedia.org/Pstencil) records that developers disagree
  about whether the E3 demo used it, and the [oral
  history](https://medium.com/@Oozer3993/an-oral-history-of-the-halo-2-e3-2003-demo)
  has an engineer saying it was used and then dropped for lightmaps plus shadow
  maps. Press on the 2025 leak
  ([Vice](https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/))
  says the leaked build includes it. A Steam comment claims retail removed the
  stencil buffer use entirely; that is unverified. We have not checked what
  the retail renderer can do here. Since then phase 0 of
  [e3-lighting.md](../graphics/e3-lighting.md) found no stencil-shadow path
  in retail, and phase 1, harder and sharper retail shadows, is the Lighting
  row (above).
- **FOV.** Community H2V code applies a scale to the camera's vertical field of
  view after the game computes it
  ([`render_cameras.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/render/render_cameras.cpp),
  it even notes that H2V's projection differs from the nominal value). For us
  the clean place is the projection we hand to the host renderer, but the guest
  also uses its camera for culling and for some effects, so a host-only FOV can
  show geometry the game culled. Built in the guest's own camera instead:
  see "Field of view (built)".

## Video screen (built)

The top-level Settings list (Player Profiles, Game Variants) has a third
row, **Video**. A on it opens a screen titled "Video" with up to seven rows:

| Row | Values | Saved as | Applies | Locked by |
|---|---|---|---|---|
| Field of View | 70 to 120 in steps of 5 | `[graphics] fov` | at once | `H2_FOV` |
| Frame Rate | 30, 60, 120, Uncapped | `[graphics] fps` | at once | `--fps`, `H2_FPS` |
| Resolution | 640x480, 1280x960, 1920x1440, 2560x1920 (scale 1 to 4; shown as the size, saved as the scale) | `[graphics] render_scale` | next frame | `H2_RENDER_SCALE` |
| Lighting | Retail, E3 (default Retail) | `[graphics] lighting` (`"retail"`, `"e3"`) | next frame | `H2_LIGHTING` |
| Mute Button | On, Off | `[pad] mute_button` | at once | `H2_PAD_MUTE` |
| Aim Assist | Auto, Default, Strong, Very Strong (default Auto: Strong while the touch controls show) | `[controls] aim_assist` | next map load ([aim-assist.md](aim-assist.md)) | `H2_AIM_ASSIST` |

A steps the row's value and saves it to `<data>/settings.toml` at once,
keeping every other key, and every row applies at once: the window follows the frame rate at its next turn
(`app::fps::FpsControl`), the view backend makes its targets again at the
next frame boundary on the render thread (`ScaledBackend::follow_scale`,
[high-resolution.md](../graphics/high-resolution.md)), and so does the lighting
(`ScaledBackend::follow_lighting`, [e3-lighting.md](../graphics/e3-lighting.md)),
and the HID thread gets the mute
button (`PadHid::mute_switch`). A locked row shows the variable's value and A
does nothing. A bad variable is reported and ignored, so the saved value is
used (before this, a bad `H2_RENDER_SCALE` meant 1x). Above 30, the frame
rate only redraws the newest game frame more often until high-fps phase 2
([high-framerate.md](high-framerate.md)). `H2_NO_VIDEO_SCREEN=1` removes
the row and the screen; `H2_NO_FOV_ROW=1` removes the field of view row and
the camera hook. Headless runs (`h2 run`) have none of it (`H2_LIGHTING=e3`
works there as a variable).

Code: the rows are `Def`s in `boot::setting_defs` with a `menu` naming
the screen; `boot::video_screen::model` builds the screen as
`ui::Screen::new("video").rows_from_settings(..)`, the Video row as
`ui::extend(List::Settings, 2)` and the speaker row as
`ui::extend(List::Controls, 6)` ([Settings as a schema](#settings-as-a-schema-built));
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
patched for it; the rows below marked "before" describe what it replaced.

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
| `0x1a480d` | datum array constructor | the Controls list hook may grow the array (the speaker row); adopting deletes the rows past ours. Before: the open call named the array "h2r video settings" to mark a Video list |
| `0x2c8548`, `0x2c8719` | Controls row text and row chosen | never reached for a Video screen: list slot 20 and the press record point at our entries. Before: both hooks branched on the array name. The row text is now ported (`boot::controls_text`; switch `features.controls-text`) |
| `0x24bb60` | "add items" | the speaker hook skips any handler whose vtable is not the Controls one, so ours keeps its six item widgets |
| `0x22fda6` | build a screen's header | runs as retail; our screen slot 18 then sets our title on the header text widget (`screen + 0x80`, `0x253b1a`). Before: the text hook swapped the id while the header was built |

**B returns to Settings.** The Video screen is opened by the same call,
with the same request, that the Settings list uses for Player Profiles
and a profile uses for Controls, so it is pushed on the same screen stack
and closes the same way. The Controls class itself never touches the
Controls edit buffer on close: only the profile screen's back handler
(`0x2b7abe` → `0x148bff`) and the generic `0x230b32` commit and clear it,
and no profile is being edited from the Settings list.

**Settings root vs Controls as the entry.** Video used to be the last row
of a profile's Controls. It is now a row of the top-level Settings list,
next to Player Profiles and Game Variants, because none of its settings
belongs to a profile. It is added the way the Controls rows are: a
`ui::extend(List::Settings, 2)` row in the model, the list hook growing
the count (plus the one handler byte that gives the list a third item
widget), and wrappers on the list's own row text and row chosen that
answer for row 2 and leave rows 0 and 1 to the originals. No tag is
changed: the list already shows up to six items, and the missing pane is
skipped.

### To check in game

1. Settings (main menu): the rows are Player Profiles, Game Variants,
   Video. Moving onto Video keeps the Game Variants pane (description)
   up. stderr: `video screen: settings row text active`. If only two rows
   show, the third item widget did not appear (`+0x220`).
2. A on Video opens a screen titled "Video" with Field of View, Frame
   Rate, Resolution, Lighting, Mute Button, and no retail row. stderr: `video
   screen: opening`, `video screen: list built with 5 rows`, `video
   screen: header active`, `video screen: row text active`. If the title
   still says Controls, the header id differs from `0x06000233` on this
   screen. The empty item widgets below the rows must not draw anything.
3. B returns to Settings. Player Profiles and Game Variants still open
   their screens. A profile's Controls has no Video row any more.
4. Frame Rate: A steps 30, 60, 120, Uncapped, 30, with no "(restart)".
   stderr: `app: frame rate 60` and so on at each press, and the watermark's
   shown rate follows; Uncapped tears (vsync off), and going back to 30
   brings vsync back. With `H2_FPS=60` the row shows 60 and does not move.
5. Resolution: A steps 640x480, 1280x960, 1920x1440, 2560x1920. The very
   next frame is sharper or softer: stderr `d3d: render scale 1x -> 2x`,
   no `showing the 1x frame` line and no flash or black frame. Check in a
   level and on a menu (the main menu's blur and the texaccum passes),
   and back down to 640x480 (the view sleeps; the run report's
   `render scale` line counts `1x frames`, not fallback frames). The run
   report's `fallback frames` stays 0. `H2_RENDER_SCALE=2` locks it.
6. Lighting: A steps Retail, E3, Retail. stderr `d3d: lighting Retail ->
   E3` at the next frame; in a level with dynamic-light shadows (below
   "Phase 1 as built" in [e3-lighting.md](../graphics/e3-lighting.md)) the
   shadow edges turn hard and sharp, with no flash. At 640x480 E3 wakes the
   view (the run report counts view frames, not `1x frames`).
   `H2_LIGHTING=e3` locks it.
7. Mute Button (DualSense): Off puts the mute LED out and the button stops
   doing anything; On makes the button toggle again, at once, with no
   relaunch. Turning it Off while muted unmutes.
8. Field of View: as below, now on this screen.

## Field of view (built)

The Video screen's **Field of View** row (above; before this it was a row
on Controls).
Its value is the first-person camera's unzoomed horizontal field of view in
degrees. Pressing A steps it 70, 75, ... 120 and wraps back to 70. 70 is
retail: the hook then hands the game its own number back, bit for bit.
Left and right do nothing on this list (the Controls list's own input code
is not traced; A is the only press we see, through the row-chosen handler).

### Where the game sets it (Local, static reading of the retail XBE)

| Address | Role | Evidence |
|---|---|---|
| `0x187130` | per-local-player field of view (`player_control_get_field_of_view` in Cartographer's H2V names). `eax` = player-control index (array `[0x4ed284]`, stride `0x94`, unit at `+0x14`), result in `xmm0`, plain `ret` | starts from the observer default `[0x54e854]`; with a unit, takes the unit definition's camera field of view (`+0xcc`, found through the object header table `[0x4e0300]+0x44` and the tag instances `[0x4e3b44]`), calls `0xc8880` for the zoom magnification of the player's zoom level (`[pc+0x2e]`), and returns `fov / magnification` when that is between `0x45e25c` and `0x45e258` (about 1.8 and 178 degrees), else the unzoomed value. So zoom divides the angle |
| `0x23c110` | first-person camera update (director callback, [engine-map.md](../engine/engine-map.md)) | the **only** call to `0x187130` (`0x23c202`; a scan of every `call`/`jmp` rel32 in `.text` and every absolute dword finds no other reference). Multiplies by 1.0, clamps to `[0x55e484, 0x55e488]` and stores the director camera's field of view at `+0x28`; a change sets the observer's 0.18 s transition (`+0xa4`, `+0x90`) |
| `0x55e484` / `0x55e488` | clamp globals, set by static initialisers `0x3782a0` / `0x3782c0` from `0x45dc54` / `0x45e098` | 1 and 110 degrees. `0x55e488` has one reader, `0x23c225` |
| `0x171d90` | observer: camera result from the command | vertical FOV `+0x4c = 2 atan(tan(h/2) / aspect)` from the horizontal `+0x38`; the scale `+0x50 = h / [0x54e854]` |
| `0x2f800` | render camera from the observer camera (called by `0x223080`, per view, from `render_game_views` `0x2234e0`) | copies position, forward, up, vertical FOV (`+0x4c` to `+0x28`) and the scale (`+0x50` to `+0x2c`), then the split-screen factors 0.5 / 0.8. The renderer's projection and its frustum come from this camera |
| `0x54e854` and the other `70` copies | 70 degrees (`0x45e628`) copied into about 40 per-file globals by static initialisers in `0x3763b0`..`0x378894` | only `0x54e854` (camera code), `0x5234dc` (render, the no-observer fallback) and `0x557c28` are read; we change none of them |

Community comparison: Cartographer (H2V) replaces the unit's camera field of
view inside its `player_control_get_field_of_view` and keeps the same
`[1, 110]` clamp in its first-person camera
([`player_control.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/game/player_control.cpp),
[`first_person_camera.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/camera/first_person_camera.cpp));
its vehicle FOV patches the following camera's default separately
([`following_camera.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/camera/following_camera.cpp)).
Cited for the names and the shape; nothing is copied, and H2V's addresses
and structures differ.

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

Unverified until played: whether the first-person weapon model, the HUD
scope overlays and the sniper's zoom ring look right at wide values (the
weapon is drawn with its own projection, commit 1a5aa8d), and whether the
following camera (vehicles, `0x23bd70` stores `[0x54e854]` itself), dead
and scripted cameras should follow too. They do not today: only the
first-person camera changes.

### What to check in game

1. Settings, Video: the first row reads "Field of
   View" with "70". A steps it 75, 80, ... 120, then 70.
2. In a level, at 90 or more: the view is wider at once (no restart);
   nothing pops in or out at the screen edges while turning (culling uses
   the same camera).
3. Zoom with the battle rifle (2x) and the sniper (5x, 10x): each zoom
   level is the same magnification as retail, relative to the wider view.
4. 120: wider than 110, the old cap.
5. Back to 70: identical to retail. Restart `h2 play`: the value is kept.
6. `H2_FOV=100 h2 play ...`: 100, and the row does not change it.
   `H2_FOV=0`: retail.
7. Split screen: each player's view widens; vehicles keep retail's view.

## Settings as a schema (built)

Phase 0 of [feature-engine.md](../engine/feature-engine.md). A setting is one
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
`menu: Some(Menu { screen: "video", order })`; the Mute Button row was added
that way. Behaviour kept from the hand-written settings: the same file keys
and variables, `H2_FOV=0` for retail, `"uncapped"` written as a string,
`H2_PAD_SPEAKER` naming a routes file. Changed: a bad `H2_PAD_MUTE` is now
ignored (the file or on) where it used to turn the button off, and the log
line after a press names the key (`graphics.fps: 60`, or for a `Restart`
setting `<key>: <value> from the next launch`).

**Per-profile values.** A setting can also hold a value per player profile
(`Store::profile_get` / `profile_set`), written as
`[profiles."<name>".<table>] key = value` and kept apart from the run's own
value. The campaign skulls use it: which skulls each profile has collected
and which it wants on ([skulls.md](skulls.md)).

## Opening the menu

- Hotkey: a key the guest pad does not use (F1 or Cmd+comma, to match macOS).
  Pad chord: Back plus Start held for about a second.
- Opening pauses the guest, like Cmd+P: `host.tick` blocks, logical audio time
  stops, and the device plays out what is queued. Closing resumes. Nothing
  reaches the guest meanwhile.
- While open, the input layer is in menu mode: pad, keyboard and mouse go to
  the overlay and the guest sees a neutral pad. On close, held buttons are
  released so the guest does not see a stuck press.
- Applying a setting that needs a restart records the value and marks the
  panel "applies on restart"; it does not restart the game.
- Because the menu pauses the guest, none of this can change a deterministic
  run: timeline-driven and headless runs have no window and no overlay.

## Testing

Per the project rule, prove behaviour in tests, not by running the game.

- Settings file: parse, defaults, unknown key, out-of-range value, precedence
  between flag, variable, file and default, atomic write, failure keeps the old
  file. Plain unit tests in a new small crate or in `app`.
- Each setting's applier behind a trait so the menu state machine is tested
  with a fake (vsync reconfigures, merge passes calls the setter, bindings
  replace the manager's table).
- Menu state machine: open and close pause and resume, input routed in menu
  mode, held buttons released on close, a pad chord opens it. No GPU.
- Overlay rendering: a GPU test that draws the menu over a known frame and
  reads back pixels, like the existing banner test ([app.md](../runtime/app.md)).
- Guard test: the guest-visible state hash is identical with the menu opened
  and closed during a scripted run.

## Phased plan

**Phase 0: settings file, no UI.** Add the loader, the schema, precedence
rules and tests. `h2 play` reads it and the existing flags override it. Move
`vsync`, `aspect`, `scaling`, filter and `bindings` onto it. Make vsync
changeable at run time. Deliverable: a user can configure the shell by file.

**Phase 1: host overlay with the host-only settings.** egui in the shell,
paused guest, menu mode in the input layer, controller navigation. Panels for
display, input (sensitivity, invert, bindings with a "press a key" capture) and
performance (merge passes live; render thread, pipeline cache and compile
threads marked restart). Frame stats toggle. Deliverable: everything in the
list that is already implemented can be set in-game.

**Phase 2: backend settings that need new graphics work.** Resolution scale
and 4K, designed and tested against the shader and render-target corpus.
The menu entry appears when the work lands.

**Phase 3: engine-coupled enhancements.** FOV, 60 and 120 fps (render
interpolation), E3 lighting. Each has its own design document and its own
acceptance tests. The menu has the entries greyed out from phase 1 so nothing
about the layout changes later.

**Phase 4 (optional): native entry point.** One "Enhancements" row on the
retail settings screen that opens the overlay. Needs a trace of that screen's
list-filling and press code, a single hook, and a string for each language.
Skip it if the hotkey and chord prove enough.

## Open questions

- Which retail option lives in the profile and which in the preferences file,
  and their layouts. Not needed for the recommended design.
- Whether the tag cache is inside a checkpoint's captured region (only
  relevant to approach A).
- Whether egui's keyboard focus navigation is good enough for a pad, or the
  overlay needs a small custom focus model.
- How the keyboard-and-pad "first player" combination interacts with menu mode
  (the combined pad stays connected, [input.md](../input/input.md)).
- A text-entry path inside the overlay, for names, shares work with
  [keyboard-charset.md](../input/keyboard-charset.md).

## Sources

Community, cited and not copied:

- Project Cartographer: <https://github.com/pnill/cartographer>,
  [overview](https://deepwiki.com/pnill/cartographer),
  [install notes](https://www.halo2pc.com/install/).
- egui wgpu: <https://docs.rs/egui-wgpu/latest/egui_wgpu/>,
  <https://github.com/emilk/egui/discussions/3067>.
- 60 fps and widescreen prior art: [XPSemu](https://github.com/ZiZc3/XPSemu/releases),
  [Halo 2 Reclaimed patcher](https://www.moddb.com/mods/halo-2-reclaimed/downloads/halo-2-reclaimed-r2-visuals-gameplay-patcher).
- E3 lighting: [Halopedia](https://www.halopedia.org/Pstencil),
  [Vice](https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/),
  [oral history](https://medium.com/@Oozer3993/an-oral-history-of-the-halo-2-e3-2003-demo).
