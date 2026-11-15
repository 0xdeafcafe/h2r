# Expanded settings menu (design)

Status: design only. Nothing in this document is built. It says how an
in-game menu for our opt-in enhancements should work, what we know about
Halo 2's own settings screens, and in which order to build it.

The menu will hold, in the order the README lists them:

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

[ui.md](ui.md) has the full data model. What matters here:

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
  `unic`, see [localization.md](localization.md)) and animation indices. It
  does not say what pressing a row does. `0x00148098` finds a screen by its
  `i16` screen ID, but nothing in the tag names a constructor. The widget
  classes and their event handlers are code. A new screen therefore cannot be
  made from tags alone.

### Where option values live (Local, partly traced)

[engine-map.md](engine-map.md) records two persistence roles that matter:

- a **preferences** path (`0x00121350`): an async file read, signature
  calculation, version and size checks, and defaults when the data is missing
  or invalid;
- a **profile and configuration cache** (`0x0007fa40`, `0x0007fb50`), with
  profile and variant type naming at `0x00216b60`.

Saves go through XAPI's save-game calls and are signed with the game's own
`XCalculateSignature` ([xapi.md](xapi.md), [checkpoints.md](checkpoints.md)),
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
  [localization.md](localization.md)). A miss returns an empty string.
- Tag memory is guest memory. Whether the patched bytes end up in a checkpoint
  or in anything the game saves is not known. Checkpoints are raw snapshots of
  game-state memory ([checkpoints.md](checkpoints.md)); whether the tag cache is
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
  ([ui.md](ui.md)), but not the list and screen class vtables or the event
  system.
- Guest code that runs in the middle of the simulation tick can change game
  state. A hook must keep to UI state only.
- Strings, fonts and layout still have to work in nine languages, and in the
  CJK font tables, with the existing glyph coverage ([fonts.md](fonts.md)).
- The recompiled game is built from the XBE ahead of time. A hook point is a
  guest function we replace through dispatch overrides (the same mechanism that
  replaces D3D, DirectSound and XAPI), so the mechanism exists. The cost is
  reverse engineering, not plumbing.

### C. A host-side overlay menu

Draw the menu in the shell, on top of the game frame, with a host immediate
mode UI such as egui. The shell already composites the guest frame into the
window surface and draws a banner in a second alpha-blended pass
([app.md](app.md): `Compositor`, `app::overlay`). egui's wgpu renderer is
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
- The shell already intercepts keys (F5, F6, Cmd+F, Cmd+P), so a chord and a
  hotkey have a precedent.

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
| Survives a title update (1.1+) | needs address map | needs address map | yes ([title-updates.md](title-updates.md)) |

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

- Path: next to the existing data root,
  `~/Library/Application Support/h2recomp/settings.toml`, from the same
  `vfs::Config::for_title` root ([storage.md](storage.md)). It is not under
  `disk/Partition1/UDATA`, which is the guest's.
- Reasons: the guest profile is signed and sized by the game, and writes to it
  go through retail code we should not perturb. Enhancements are per install,
  not per gamertag. A host file is easy to edit, diff, delete and share, and it
  does not travel with a save someone else loads.
- Format: TOML, the same as `bindings` ([input.md](input.md)), which is already
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

Sketch (invented, not a committed schema):

```toml
[display]
vsync = true
fullscreen = false
aspect = "4:3"        # as --aspect
scaling = "fit"       # as --scaling
filter = "linear"     # as --nearest
render_scale = 1.0    # 1.0 = native 640x480; 2.0, 3.0 ... up to 4K

[timing]
fps = 30              # 30, 60, 120 (future)

[graphics]
merge_passes = true
lighting = "retail"   # "retail" or "e3" (future)
fov = 0               # 0 = the game's value; otherwise degrees (future)

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
| bindings, deadzones, mouse sensitivity, invert Y | `--bindings`, [input.md](input.md) TOML | live once `Bindings` can be replaced on a running manager (not implemented) |

**Graphics backend (runtime variables, read at launch).**

| Setting | Variable | Applies |
|---|---|---|
| merged passes | `H2_GFX_MERGE_PASSES=0` turns off; `=exact` for the exact rule | `GfxBackend::set_merge_passes` and `set_merge_discards` exist, so live |
| render thread | `H2_RENDER_THREAD=0` (off; on by default) | restart; it moves the backend to another thread ([render-thread.md](render-thread.md)) |
| pipeline cache | `H2_PIPELINE_CACHE` (path or `off`) | restart |
| shader compile threads | `H2_GFX_COMPILE_THREADS` | restart |
| frame stats | `H2_FRAME_STATS`, `--frame-stats` | live (a HUD or log toggle) |

**Build-time (change generated C).**

| Flag | Notes |
|---|---|
| `H2_FASTMEM`, `H2_X87_NATIVE`, `H2_ACCESS_COUNT` | They change the generated library and its cache key ([BUILD-CACHE.md](BUILD-CACHE.md)). The menu must not offer a toggle that takes minutes to rebuild. Show them as read-only status ("native x87: on"), with the `h2 build` command in a tooltip. |

**Developer-only (never in a menu).** `H2_BOOT_*`, `H2_DISC`, `H2_XBE`,
`H2_PERF*`, `H2_PROFILE*`, `H2_TIMELINE*`, `H2_FAULT*`, `H2_TRACE`, scripted
input and the rest of the boot runner's variables. They are for reproduction
and measurement. The settings file has no keys for them.

**New settings with no existing flag.** Resolution scale, 60 and 120 fps, E3
lighting and FOV have no control today. Each needs its own design before the
menu can switch it on (see phases). Until then the menu shows them greyed out
with a one-line reason, so the list is honest about what exists.

Notes on specific settings:

- **Mouse sensitivity.** The host value is deflection per mouse count; Halo
  then applies its own deadzone and its own per-profile look sensitivity, which
  the retail option screens set ([input.md](input.md)). The menu scales the
  first and does not touch the second, and the labels must say so, or users will
  look for one number and find two.
- **Resolution scale and 4K.** The D3D layer reports 640x480 surfaces to the
  guest and renders through wgpu. Scaling means rendering at a multiple and
  keeping every guest-visible size and every sampled render target consistent.
  Effects that sample the back buffer at a fixed texel size, and render
  targets with 1:1 pixel assumptions, are where it will break
  ([d3d.md](d3d.md), [graphics.md](graphics.md)). Separate document and test
  corpus.
- **60 and 120 fps.** The engine's simulation runs on a tick rate read from game
  options ([engine-map.md](engine-map.md)), 30 Hz in retail. The settings
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
  the retail renderer can do here. This is a research item, so the first menu
  shows it only as "coming".
- **FOV.** Community H2V code applies a scale to the camera's vertical field of
  view after the game computes it
  ([`render_cameras.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/render/render_cameras.cpp),
  it even notes that H2V's projection differs from the nominal value). For us
  the clean place is the projection we hand to the host renderer, but the guest
  also uses its camera for culling and for some effects, so a host-only FOV can
  show geometry the game culled. Needs a trace of the camera and cull path.

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
  reads back pixels, like the existing banner test ([app.md](app.md)).
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
  (the combined pad stays connected, [input.md](input.md)).
- A text-entry path inside the overlay, for names, shares work with
  [keyboard-charset.md](keyboard-charset.md).

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
