# The HUD kit: HUD elements of ours, drawn by Halo 2's own HUD

Status: Design. Nothing here is built; the superbounce HUD in `crates/apps/app/src/superbounce_hud.rs` is the host overlay this replaces.
Code: `crates/tools/hud-kit` (the model, the compiler, the preview), `crates/extensions/game/src/hud_kit.rs` (the runtime), `crates/apps/cli/src/hud_cmd.rs` (`h2 hud`). Tests: `crates/tools/hud-kit/src/tests.rs`, `crates/tools/hud-kit/tests/disc.rs`.
Switch: none (the runtime is the feature `hud-kit`).

A feature of ours that wants something on the in-game HUD describes it as
kit elements: a text line, a number, a meter, an icon, a pointer, a
message, a prompt, a training box. The kit turns the elements into
records of the game's own HUD format and hands them to the game's HUD
code, which draws them with the disc's bitmaps, shaders and fonts. It is
to the HUD what the UI kit ([ui-screens.md](ui-screens.md),
[ui-builder.md](ui-builder.md)) is to the menus: the game's own widgets,
a description as data, an offline preview and checks.

The one rule: **the kit draws nothing itself in the game.** Every pixel of
a kit element comes out of a retail draw routine reading retail assets. An
element the game has no routine for is not a kit element.

Evidence labels are those of [docs-style.md](../meta/docs-style.md):
**E** read from the retail XBE with `h2 disasm`, **V** checked on the
owner's disc, **Code** read from this repository, **S** supposed (with
what would verify it), **Design** our choice.

## What Halo 2's HUD is made of

The HUD has four parts, each with its own draw routine. All are called
per user from `0x001917eb` (`points::UI_HUD_DRAW`), in a 640 by 480 frame
(E, [native-post-hud.md](../graphics/native-post-hud.md), "The UI and
HUD").

| Part | Draw | Data |
|---|---|---|
| Widgets: meters, panels, counts, reticles, warnings | `0x0013954b`, each `nhdt` through `0x0013a532` | `nhdt` tags named by the unit (`unit +0x1a0`) and the weapons (`weap +0x2b0`) |
| Messages: the prompt line, pickup notifications, objective and help text | `0x0024cf66` | `hudg`, the per-user message state at `[0x005023f4] + 0x4e0 * user` |
| Training: a framed box with one or two lines | the routine holding `0x001e70c3`..`0x001e73a3`, its frame `0x001e9140` | `hudg +0x420`..`+0x484`, the training state at `[0x0051e9c0]` |
| Motion tracker blips, waypoints, damage markers | `0x001a2823`, `0x0024da7d` | `hudg`, a centre table at `0x0047ff40` |

### Widgets (`nhdt`)

[hud-editor.md](../editor/hud-editor.md) is the home of the widget
format and its placement rules, and `crates/formats/tags/src/defs/nhdt.txt`
of the fields. What the kit depends on:

- **Two widget kinds.** Bitmap widgets (`nhdt +0x08`, `0x64` bytes each,
  drawn by `0x0022aff1`) and text widgets (`+0x10`, `0x54` bytes, drawn
  by `0x0022aa16`) (E `0x0013a532`).
- **The root is read every frame.** The draw takes the count and the
  address of each widget block from the loaded tag, and the unit's and
  weapon's `nhdt` datums from their tags (E `0x0013a532`, `0x0013b071`;
  Code `crates/extensions/game/src/hud_swap.rs`, "Why a swap works").
- **A widget is a bitmap, a shader and four inputs.** The inputs are one
  byte each (`+0x04`); `0x0013ad48` (`points::HUD_WIDGET_INPUTS`) turns
  them into four floats the shader reads as `hud_input1`..`4` (16 shield,
  17 body, 20 and 21 grenade counts, `0x31` heat, `0x36` rounds, `0x38`
  battery) (E, [health.md](../halo1/health.md), "HUD").
- **A state gate.** `0x0013ac87` (`points::HUD_WIDGET_GATE`) draws a
  widget when one of its four show words meets the state and no hide
  word does (E).
- **Text.** A text widget shows a localized string (`+0x28`) or, with
  flag bit 0, an input as a number; `0x0022a871`
  (`points::HUD_WIDGET_STRING`) builds the string. No shader or no
  string, no text (E `0x0022aa26`, `0x0022aa2f`).

`crates/extensions/game/src/h1_hud_text/weapon.rs` already answers the
gate, the inputs, the string and the sprite of marked widgets from Rust,
on those four points. The kit uses the same points.

### Messages (`0x0024cf66`)

Read for this page (E, `h2 disasm 0x24cf66 530`). One routine draws, top
to bottom from one starting point, then two centred lines:

1. **Font.** Font id 6 with one local player, 5 with more
   (`0x0024cfaf`..`0x0024cfc9`).
2. **Start point.** Anchor 1 (top left) through `0x001396c7`, then 60
   pixels down, 35 in split screen (`0x0024cfda`..`0x0024d002`). At 640
   by 480 fullscreen that is (48, 96) (S: the frame's top is 36).
3. **The first line**, one of three sources, in this order:
   - the objective (`hud +0x1390`, while its time `+0x1394` runs): colour
     `hudg +0xcc`, alpha the time over `hudg +0xea` (`0x0024d13e`);
   - the help text (`hud +0x138c` with the help flag): colour
     `hudg +0xa4`, or `+0xa8` by `hudg +0xb6` bit 0 and the flash byte
     `hud +0x1384` (`0x0024d194`);
   - the user's prompt (`+0x444` shown, text at `+0x220` when the id at
     `+0x440` is 0): colour `hudg +0xa4`. For a quarter of a second
     after the prompt changes (`+0x446` against a quarter of the tick
     rate) the colour runs from white (`[0x00468718]`) to its own, with
     a per-glyph callback `0x0024cedf` (`0x0024d0a6`..`0x0024d23f`).
   The line's box runs from the start point to the frame's right edge,
   five lines high (`0x0024d244`..`0x0024d277`).
4. **Notifications.** Up to four entries (three in split screen) of
   `0x88` bytes at the head of the user's state: start tick `+0`, UTF-16
   text `+4`, active `+0x82`, a count flag `+0x84` and count `+0x86`.
   Each is drawn in the user's HUD colour (below) and alpha, held for
   `hudg +0x58` seconds and faded over `hudg +0x5c`; the next line starts
   the drawn height times `hudg +0x80` lower
   (`0x0024d33a`..`0x0024d516`).
5. **Two centred lines** 40 pixels above the frame's bottom, 18 apart:
   a timed text at `+0x454` (start `+0x4d8`, duration `+0x4dc`) and the
   text of `0x0015f120` (`0x0024d51f`..`0x0024d6bb`).

Who writes the state (E):

| Routine | What it does |
|---|---|
| `0x0024c9e9` (user, key, sound) | looks the key up in `hudg +0x3fc` (`0x0013925f`) and shows it as the prompt through `0x0024ca1d`. Its one caller is the interaction code at `0x00191e42` (pick up, swap) |
| `0x0024ca1d` (EAX user, ECX text, key, sound) | copies the text to `+0x220`, sets `+0x444`, clears `+0x440`; a new key (`+0x448`) restarts the fade at `+0x446` and plays the sound |
| `0x0024c98c` (EAX user, shown) | shows or hides the prompt each frame, from `0x00191976` |
| `0x0024cbbf` (EDI string id, user) | queues a notification; called for an item's pickup text (`0x00191fcf`) and by `0x001df40d` |

Code that already uses them: `h1_hud_text/prompts.rs` (`show`) writes the
prompt fields, and `skulls_custom.rs` queues "our text" through
`0x0024cbbf`.

### Training (`hudg +0x420`)

Read for this page (E, `h2 disasm 0x1e70c3 190`, `0x1e9140 300`).

- **The prompts.** `hudg +0x420` is a block of `0x14`-byte rows: a string
  id, durations and flags (`crates/formats/tags/src/defs/hudg.txt`,
  `training_prompts`). `0x001e8d50` returns a row and `0x001e8cb0` its
  string id. The per-player bits at `player +0x170` and `+0x174` record
  what has been taught (`0x001e8d80`).
- **A scripted text.** The training state at `[0x0051e9c0]` holds four
  users of `0x1b0` bytes, then a string id at `+0x6c0`, a show byte at
  `+0x6c4` and a byte at `+0x6c5`. With the show byte set and the id
  neither 0 nor -1, the draw shows that string in place of a prompt row
  (`0x001e70c3`..`0x001e70d8`). Three script evaluators write the three
  fields (`0x002aa7cd`, `0x002aa81d`, `0x002aa86d`).
- **The text.** The string comes from `hudg +0x3fc` through
  `0x001a0180`, then button substitution `0x0022d2ee`. Font id 6 with
  one view, 5 with more (`0x001e71d7`). Centred (`0x0013edb0` with
  style 2). Its colour is a fixed pair of floats with blue at 1, from
  one of two sets (`0x0045e518`, `0x0045e51c` and `0x0045e520`,
  `0x0045e524`, chosen by `0x0022acb4`), at the user's HUD alpha.
- **The box.** The text is measured without a limit (`0x0013e9c0`), and
  the box is centred on the centre of the HUD frame, half the text's
  width plus 5 and half its height plus 1 each way
  (`0x001e72e9`..`0x001e7373`). `0x001e9140` draws the box as ten shader
  quads (`0x0022a664`) from four bitmaps (upper right, upper centre,
  upper left, middle) with the shader at `hudg +0x444`. The bitmaps are
  `hudg +0x44c`, `+0x454`, `+0x45c`, `+0x464`, or `+0x46c`..`+0x484`
  when the player's byte `+0x88` is 1 or 3.

So Halo 2's "press X to..." training prompt is a small framed box in the
middle of the screen holding centred text in the HUD font, with the
button drawn as the font's own glyph.

### Colours

No colour value is recorded here: the kit reads each one from the user's
disc or the running game.

| Colour | Where it is | Evidence |
|---|---|---|
| The HUD text colour, per user | `0x0013927e` returns one of two static RGB triples, `0x00468c84` or `0x00468c94`, by `0x0022acb4` (S: the Elite's HUD; the same test picks a sequence's second image) | E |
| The HUD alpha, per user | `[0x004e69c0 + 4 * user]` through `0x001392a9`; multiplies message and training text | E |
| Text shadow | the four floats `[0x004686d4]` points at | E `0x0024d3ab` |
| Prompt and help text | `hudg +0xa4`, packed ARGB | E `0x0024d0a6` |
| Flash records | `hudg +0xa4`..`+0xbc` (help), `+0xcc`..`+0xe4` (objective), `+0x318`, `+0x338` (timer): a rest colour, a peak colour, period, delay, count, flags, duration | Code `defs/hudg.txt`; E the help and objective reads above |
| Widget art | the widget's shader: a meter's fill and a panel's tint are shader colours | [health.md](../halo1/health.md) |
| Text widget colour | not read: how `0x0022aa16` gets from the widget's shader to the colour it hands `0x0013edb0` | decoding needed (4) |

The flash curve of such a record is `strength` in
`crates/extensions/game/src/h1_hud_text/flash.rs` (Halo 1's, which
Halo 2's help text is drawn with on converted maps). That Halo 2's own
curve is the same is S: read the caller of `hudg +0xac`..`+0xb8`.

### Fonts

Font id N is line N of `font_table.txt`, counted from 0
([ui-screens.md](ui-screens.md), "Font ids"; [fonts.md](../formats/fonts.md)).

| Use | Font id | File (V: the table on the disc) |
|---|---|---|
| Messages, prompts, training text; a text widget's font index 0 | 6 (5 in split screen) | `conduit-13` (`conduit-12`) |
| A text widget's font index 1: the counts | 8 | `MSLCD-14` |

V, `h2 font <iso> <font> <text> --out`: a line of `conduit-13` renders 21
pixels high and of `MSLCD-14` 17; "17.9" is 44 wide in `MSLCD-14`. Button
pictures are glyphs of every font at U+E100 and up (X is U+E102), and
action tokens are replaced by the bound button in `0x0022d2ee`
([button-icons.md](../input/button-icons.md)).

### Bitmaps

The kit names retail art by the widget that draws it, never by pixels.

| Art | Where it is |
|---|---|
| Shield meter, its mask, the tracker's frame, the grenade panel | bitmap widgets of the player's `nhdt` (V: eight on the multiplayer player's, the shield meter the widget with special type 5, [health.md](../halo1/health.md)) |
| A thin strip meter | the energy sword's battery meter: a 92 by 8 strip with the legacy meter shader, which fills where the texture's blue is below `hud_input1` (V, [health.md](../halo1/health.md)) |
| Weapon panels, ammunition counts, reticles | each weapon's `nhdt` |
| Waypoint arrows and markers | `hudg +0x408` (image, shader, sequence) and `+0x410` |
| Message icons, button glyph sprites | `hudg +0x8c`, `+0x9c` |
| The training box | `hudg +0x444`..`+0x484` |

### Anchors, the safe area and scaling

- A widget's point is a corner of the view's HUD frame plus its offset
  for the screen mode: anchor 0 top right, 1 top left, 2 bottom left, 3
  bottom right, 4 the centre (E `0x001396c7`). The frame is the i16
  rectangle at `0x004b9dd8` (top, left, bottom, right), and the view's
  origin is at `0x004b9dd0`.
- Fullscreen at 640 by 480 the frame is the screen inset 7.5%: the
  bottom-left corner is (48, 444) (V), the rest S
  ([hud-editor.md](../editor/hud-editor.md), "Placement").
- Art is 1:1 in HUD pixels and is placed by its registration point; a
  text's point is its top, and its justification says which end (E).
- Each widget has three offsets, sequences and fonts: fullscreen, half
  and quarter screen.
- **Our renderer.** The guest always draws in 640 by 480. The host fits
  each HUD draw scope to the window: Centered keeps the corners in a
  4:3 area, Pinned moves each corner group to its viewport edge at the
  same scale ([high-resolution.md](../graphics/high-resolution.md),
  "HUD and UI"; `points::render_aspect::GROUPS`). A kit widget is inside
  the widget scopes, so it moves and scales with the tracker and the
  shield meter. A kit message is inside the message scope.
- **What is where on the bottom left** (V, 640 by 480 fullscreen,
  [health.md](../halo1/health.md)): the tracker at x 47..130, y 360..443;
  the shield meter's visible band at x 47..129, y 346..370 (its bitmap
  reaches x 174); on converted Halo 1 maps a health strip at y 336..343.

### Animation

| Behaviour | How the game does it | Evidence |
|---|---|---|
| A meter fills | the shader compares the texture with `hud_input1`; the shield by its own code (special type 5), up to five layers | V, [health.md](../halo1/health.md) |
| A meter flashes | a second widget over it (the shield's mask) whose input is a state (22, shield low) | V |
| A widget pulses, scales or turns | the widget's effects block (`+0x58`, text `+0x4c`), through `0x0022bcaf` | E; not decoded (5) |
| A warning appears | the state gate, with thresholds at `+0x18` | E |
| A prompt changes | a quarter-second run from white, and a sound | E above |
| A notification leaves | hold, then a fade with a fixed exponent | E above |
| Help and objectives flash | the `hudg` flash records | E above |

### Decoding still needed

1. **The training routine's entry and gates.** Where the routine holding
   `0x001e70c3` starts, what it checks before that point, whether it
   runs in multiplayer, and what the byte at `+0x6c5` does. Read back
   from `0x001e70c3` and from its caller.
2. **The script evaluators' names.** That `0x002aa7cd`, `0x002aa81d`
   and `0x002aa86d` are the show, set and enable training builtins: find
   their slots in the evaluator table as `h1scripts::hud::BUILTINS` does.
3. **The training box's pieces.** The ten quads of
   `0x001e9140`..`0x001e95c5`: each one's rectangle and which bitmap,
   for the preview. The game draws them without this.
4. **A text widget's colour.** `0x0022aa16` to its `0x0013edb0` call.
5. **Widget effects.** The `0x68`-byte effect record and `0x0022bcaf`,
   for a retail widget's own flash and scale.
6. **The notification insert.** `0x0024cbbf` and `0x0024cb54`: which
   slot a new entry takes and how a repeat becomes a count.
7. **The quad's corners.** `0x0022a664` reads four (x, y) float pairs at
   ECX (E `0x0022a6ca`..); their order is not read.
8. **Waypoints.** `0x0024da7d` and the arrow styles, for a marker with a
   distance drawn on a world point.
9. **The split-screen frames** and the right and top edges of the
   fullscreen frame.

## Where the kit draws

Three places were possible.

| | Host overlay (`app::superbounce_hud`) | Host draws the disc's art and fonts | The game draws records of ours |
|---|---|---|---|
| Fonts, shaders, colours | invented | ours to reimplement, shader by shader | the game's |
| Hides with the HUD (pause, cinematics, death, Blind, Theater) | no | must be copied | yes (`0x0013939b`) |
| Follows the HUD fit, split screen, the Elite's HUD | no | must be copied | yes |
| In films, captures, the headless host | no | no | yes |
| Frame cost | an upload and a quad when it changes | the same | a few more HUD quads |
| Needs | nothing | a second HUD renderer | hooks on five HUD points, tag memory edits |

The kit takes the third, as the UI kit does for menus and as the Halo 1
health strip does (a widget added to the player's `nhdt` by the
converter, [health.md](../halo1/health.md), "The health meter in the
HUD"). The second is used only offline, by the preview, where
`hud-edit` already draws an `nhdt` from the disc.

## How it works

### The model (`crates/tools/hud-kit`)

A HUD of ours is a `Hud`: a name and a fixed list of elements. A state
is a `Values`: per element its text, fill, angle, style and whether it
shows. One layout per HUD: a state never adds, removes or moves an
element (the UI kit's rule, for the same reasons: the runtime only
answers hooks, and the checks cover every state).

```rust
pub struct Hud { pub name: &'static str, pub elements: Vec<Element> }

pub struct Element { pub id: &'static str, pub kind: Kind, pub at: Place }

pub enum Kind {
    /// A line in a HUD font. `longest` is the widest text any state gives it.
    Text { font: Font, justify: Justify, longest: &'static str },
    /// A strip that fills from the left with `Value::fill`, 0 to 1.
    Meter { art: Art },
    /// A sprite of a retail HUD bitmap.
    Icon { art: Art },
    /// An icon turned to `Value::angle` about its centre.
    Pointer { art: Art },
}

/// Anchor and offsets in HUD pixels, per screen mode. `None` hides the
/// element in that mode.
pub struct Place {
    pub anchor: Anchor,
    pub full: (i16, i16),
    pub half: Option<(i16, i16)>,
    pub quarter: Option<(i16, i16)>,
    pub registration: [f32; 2],
}

pub enum Font { Hud, Lcd }            // a text widget's font index 0 and 1
pub enum Art {
    StripMeter,                       // the sword's battery strip and its shader
    WaypointArrow,                    // hudg arrow_assets[0]
    Widget { hud: HudRef, name: &'static str }, // a retail widget's bitmap, shader and sequence, by its name string
}
pub enum Style { Normal, Dim, Flash(Pulse) }
pub enum Pulse { Help, Objective, TimeWarning, TimeExpired } // the hudg records

pub struct Value { pub shown: bool, pub text: String, pub fill: f32, pub angle: f32, pub style: Style, pub since_tick: u32 }
pub struct Values(pub Vec<(&'static str, Value)>);

pub trait Describe {
    fn hud(&self) -> Hud;
    /// The named states the preview sheet draws.
    fn states(&self) -> Vec<(&'static str, Values)>;
}
```

`compile(&Hud, &Templates) -> Compiled` turns the elements into widget
records: a `[u8; 0x64]` per meter, icon and pointer and a `[u8; 0x54]`
per text, with the element id of each. It builds them from the `nhdt`
definition (`tags::defs`), as `h1objects::h1hud` does. `Templates` holds
what must come from the loaded map: for each `Art` the bitmap and shader
datums and the sequences, and for text the shader of a retail text widget
of the same `nhdt` (the grenade count: the first text widget whose first
input is 20). The same `Compiled` bytes go to the game and to the
preview, so what is checked offline is what the game is given.

Three more elements are not widgets. They are channels into the message
and training parts, one text each:

| Element | What the player sees | Through |
|---|---|---|
| `Message` | a notification in the pickup list, top left, that fades | the notification queue (`0x0024cbbf`) |
| `Prompt` | the prompt line, top left, as "Hold X to pick up" | the user's prompt fields (`+0x220`, `+0x440`, `+0x444`), while the game shows no prompt of its own |
| `Training` | the framed box in the middle of the screen | the scripted training text (`+0x6c0`, `+0x6c4`) |

Their texts are UTF-16 with the font's button glyphs or action tokens.

### The runtime (`crates/extensions/game/src/hud_kit.rs`)

A feature registers its `Hud` once and publishes a `Values` whenever its
state changes (from its tick hook). The feature `hud-kit` does the rest:

1. **Install**, before `GAME_ACTIVATE` and after `hud_swap`: for every
   `nhdt` a `bipd`'s HUD interfaces name, resolve the `Templates`,
   compile every registered `Hud`, copy the tag's two widget blocks into
   memory from `Builder::guest_alloc` with the kit's records after them,
   and point the root's two block fields at the copies. The retail
   records are byte for byte the same and draw first. With no `Hud`
   registered the tags are not touched.
2. **Gate** (`HUD_WIDGET_GATE`, before): a widget whose address is in the
   kit's memory is drawn when its `Value::shown` is set, its `Hud` is on
   and the mode has an offset; the retail gate is skipped for it.
3. **Inputs** (`HUD_WIDGET_INPUTS`, before): `hud_input1` is the
   element's `fill`.
4. **String** (`HUD_WIDGET_STRING`, before): the element's `text`.
5. **Colour** (`HUD_TEXT_SETUP`, before, from the text widget's call):
   `Dim` scales the alpha the game chose; `Flash` runs the colour through
   the `hudg` record with `flash::strength` from `since_tick`. `Normal`
   changes nothing.
6. **Turn** (`HUD_SHADER_QUAD`, before): a pointer's four corners are
   turned about their centre by `angle`.
7. **Channels.** `message` queues through the game's routine at the
   tick, the way `skulls_custom.rs` does. `prompt` writes the prompt
   fields in a hook on `HUD_INTERACTION_VISIBLE` when the game hides its
   own, the way `h1_hud_text/prompts.rs` does. `training` writes the id
   and the show byte, and a hook on the string lookup (`0x001a0180`, as
   `community_variants.rs` hooks it) answers the kit's string ids
   (family `hud-kit` in `ui_kit::ids`).

The draw hooks are `context_only` and read one shared snapshot of the
published values, so they run on the render thread
(`crates/extensions/game/src/render_thread.rs`); the tick hooks are
listed there as refused. The feature is presentation-only and holds
nothing a snapshot needs: its guest memory is rebuilt at each install.

### Anchoring next to retail art

An element is placed in HUD pixels from an anchor, like the retail
widget beside it. The crate has the retail rectangles as named
constants, checked on the disc: `TRACKER`, `SHIELD_BAND`, `H1_HEALTH`,
`PROMPT_LINE`, `TRAINING_BOX`. `Place::above(SHIELD_BAND, gap)` and the
like give offsets from them, so "above the motion tracker" is written
once and is right in each screen mode.

### Cost

S, to be measured with `profile` on the lab Mac: one address-range test
per retail widget on hook points other features already use,
and for a HUD of ten elements about ten more HUD quads and five text
draws a frame, in the game's own batches. No host texture, no upload.

## The preview and the checks

```sh
h2 hud preview list                                   # HUDs and their states
h2 hud preview <hud> [--state NAME|all] [--mode full|half|quarter] [--map NAME] [--synthetic] [--out DIR]
h2 hud sheet [--synthetic] [--out DIR]                # every HUD x state x mode, and sheet.png
```

`hud_kit::preview` draws on `hud_edit::render`'s 640 by 480 canvas: the
player's retail `nhdt` at a `HudState` (so the tracker, the shield meter
and the grenade panel are in the picture), then the compiled kit widgets
through the same `hud_edit::render::draw`, then the channels: the prompt
line and a notification at their points in font 6, and the training box
around its text. Output is `<hud>-<state>-<mode>.png` and a `.json` with
every element's id, kind, rectangle, text and style. A `DiscSource`
reads the map, the fonts and the XBE's colour constants from the user's
disc; a `SyntheticSource` uses flat boxes and a fixed-advance font, so
`cargo test` draws the sheet without a disc. Output goes to a directory
you name, never the repository.

Checks, over each state and mode (`hud_kit::check`), each finding naming
the element and the state:

| Check | Rule |
|---|---|
| fit | every text's `longest`, and every state's text, fits between its point and the next element or the frame edge |
| glyphs | every character is in the element's font |
| frame | every rectangle is inside the HUD frame and the screen |
| retail | no element's opaque pixels meet a retail widget's at any `HudState` of the sheet (overshield, both grenades, zoomed, a weapon in each hand), nor the reserved rectangles (`PROMPT_LINE`, `TRAINING_BOX`, `H1_HEALTH`, the reticle) |
| own | no two elements of any registered HUDs overlap, texts included |
| one layout | every state has a value for every element and for nothing else |
| templates | every `Art` and the text shader resolve on the map (disc only; on every map of the disc in the disc test) |
| prompt | a `Prompt` or `Training` text has at most two lines and fits the frame's width less the box |
| modes | an element with no half or quarter offset is hidden there, and says so in the JSON |

`hud_edit::checks` supplies the pixel tests. The art checks need pixels,
so the synthetic source skips them and says so.

The agent API gets one read method, `hud.kit`: the registered HUDs, and
per element the id, kind, widget address, placed rectangle, text, fill
and whether the gate last drew it. It is how a run is checked against
the preview's JSON, with `render.frame` for the pixels.

## The superbounce HUD on the kit

`crates/extensions/game/src/superbounce_hud.rs` describes it and
publishes a `Values` from `superbounce_mode`'s `State` each tick. It is
shown to the first local player, fullscreen only.

### The block

Bottom left, above the shield meter and the tracker, left edges in line
with theirs. Anchor 2 (bottom left, (48, 444) at 640 by 480); offsets in
HUD pixels; rectangles at 640 by 480 fullscreen.

| Element | Kind | Offset | Rectangle | Disc assets |
|---|---|---|---|---|
| `sb.hint` | Text, `Hud`, start | (-1, -194) | x 47.., y 250..270 | font 6; the X glyph U+E102 |
| `sb.status` | Text, `Hud`, start | (-1, -172) | x 47.., y 272..292 | font 6; the `hudg` help and objective flash records |
| `sb.pointer` | Pointer, registered at its centre | (7, -140) | centre (55, 304), within 16 by 16 (S) | `hudg` waypoint arrow image, shader and sequence |
| `sb.distance` | Text, `Lcd`, end | (62, -148) | x ..110, y 296..312 | font 8 |
| `sb.distance.unit` | Text, `Hud`, start | (65, -150) | x 113.., y 294..314 | font 6 |
| `sb.drop.label` | Text, `Hud`, start | (86, -150) | x 134.., y 294..314 | font 6 |
| `sb.drop` | Text, `Lcd`, start | (128, -148) | x 176.., y 296..312 | font 8 |
| `sb.launch.meter` | Meter, registered top left | (-1, -124) | x 47..138, y 320..327 | the sword's battery strip and the legacy meter shader |
| `sb.launch` | Text, `Lcd`, start | (97, -128) | x 145.., y 316..332 | font 8 |

Words are in the HUD font and numbers in the count font, as the weapon
panels do. The rows are 22 apart for a 21-pixel line. The block's bottom
is y 332; the Halo 1 health strip starts at 336 and the shield band at
346. The x positions on the middle row are from measured widths ("17.9"
is 44 wide) and are S until the `fit` check runs on the disc.

### States

Each drawing is the screen's bottom-left corner; one column is about 8
HUD pixels. `(X)` is the X button's glyph, `>` the pointer, `#` the
meter's fill.

`scanning`: the status line pulses with the help record.

```text
y272  SCANNING


y346  [=shield=========]
y360  (    tracker     )
```

`not-primed`, a strip in range: the status is `Dim`; the pointer, the
distance, the drop and the launch show at full strength.

```text
y272  NOT PRIMED
y294  >   12.4 m   DROP 1.1
y320  [######------]  17.9
y346  [=shield=========]
y360  (    tracker     )
```

`primed`: the status flashes with the objective record for its count
when the pair word becomes -3, then stays `Normal`.

```text
y272  PRIMED
y294  >   12.4 m   DROP 1.1
y320  [######------]  17.9
y346  [=shield=========]
y360  (    tracker     )
```

`primed-airborne`: the hint is added above; nothing else moves.

```text
y250  Hold (X) to steer
y272  PRIMED
y294  >    3.1 m   DROP 0.4
y320  [#########---]  24.6
y346  [=shield=========]
y360  (    tracker     )
```

`no-strip` (the scan is done and the map has none, or the scan failed):
the status alone, `Dim`: `NO STRIPS HERE` or `SCAN FAILED`.

| Piece | Rule |
|---|---|
| Primed | `PRIMED` when the level pair's word is -3, else `NOT PRIMED`. The number -3 is in `superbounce.state`, not on the HUD |
| Launch | the meter is `gauge_fill`: the nearest strip's launch over 30 units a second, full past 30; the number is the launch to one decimal |
| Distance | metres to the strip's nearest edge: one decimal under 100, whole above |
| Direction | the pointer's angle is the bearing from the unit's aim yaw; hidden without an aim or standing on the strip |
| Drop | the fall the strip needs, world units to one decimal |
| Hint | shown while primed and airborne |
| Scan | `SCANNING` until the scan is done; then the rows appear |

### The tutorial

The four steps are `Training` texts: the game's own training box, two
centred lines of font 6 in the middle of the HUD frame, drawn by the
game with its frame bitmaps and its training colour. There are no
diagrams: a retail training prompt has none.

| Step | Text | Moves on when |
|---|---|---|
| 1 | Crouch under a cyan crouch spot, / then stand up. | primed |
| 2 | You are primed. / Do not crouch again. | a bounce spot within 8 m |
| 3 | Find a strip. Green is a standing jump. / Red needs a big drop. | a bounce, or 20 s |
| 4 | Land on the strip. / Hold (X) while falling. | a bounce, or 15 s |

```text
                 x 163                              x 477
          y 218  +----------------------------------+
                 |  Crouch under a cyan crouch spot, |
                 |          then stand up.           |
          y 262  +----------------------------------+
                          (HUD frame centre 320, 240)
```

The box is the text's size plus 5 each side and 1 above and below, plus
the frame art (S: decoding 3 gives its outer edge). The widest first
line is 304 pixels in font 6 (V, measured as above), inside the frame's
544. The step machine is `Tutorial` (its thresholds, F4, the
`superbounce.tutorial_seen` setting and `H2_SUPERBOUNCE_TUTORIAL`),
moved beside the HUD. A game training prompt of its own takes the box
while it shows; the tutorial's step waits.

## Interfaces

- A feature calls `hud_kit::register(hud) -> Handle`, then
  `Handle::publish(values)`, `Handle::message(user, text)`,
  `Handle::prompt(user, Option<text>)` and
  `Handle::training(Option<text>)`.
- Guest points hooked: `HUD_WIDGET_GATE`, `HUD_WIDGET_INPUTS`,
  `HUD_WIDGET_STRING`, `HUD_TEXT_SETUP`, `HUD_SHADER_QUAD`,
  `HUD_INTERACTION_VISIBLE`, `GAME_ACTIVATE`, `GAME_TICK`, and the string
  lookup `0x001a0180`. New research points: the training state
  `0x0051e9c0` and its three fields, the notification routine
  `0x0024cbbf`.
- Guest memory written: the two block fields of each player `nhdt`
  root, the kit's own allocation, a user's prompt fields, the training
  id and show byte.
- Agent API: `hud.kit`.

## Rules and limits

- No pixels, strings or colour values from the disc are in the
  repository. An `Art` is a reference; a colour is an address or a tag
  field.
- A HUD may use only the styles above. A new colour is a new retail
  source, named in "Colours", not a constant of ours.
- A kit widget is drawn only while the game draws the HUD.
- Fullscreen first: an element without split-screen offsets is hidden in
  split screen.
- The prompt line and the training box are the game's, one text at a
  time, and the game's own text wins.
- The training id, the prompt fields and the notification queue are game
  state. A feature writes them only for a local player and never from a
  dedicated host.
- A map whose player `nhdt` lacks a template (no text widget with a
  shader, no strip meter art in the map) gets no kit HUD, and the
  install logs which template was missing. Nothing falls back to other
  art.

## Evidence

Every E above is from `h2 disasm` on the retail `default.xbe`; the
callers of `0x0024c9e9`, `0x0024ca1d`, `0x0024cbbf`, `0x0024c98c` and
`0x001e9140` are from a scan of the code section for `call` targets. The
font table and the measured text sizes are V, read from the owner's disc
with a byte search and `h2 font`. No game was run for this page: the
agent API had no game to attach to, and a design page starts none.

## Not verified

- That the scripted training text draws in a multiplayer game, and what
  enables it (decoding 1).
- That a kit text widget drawn through the grenade count's shader takes
  the HUD colour, and that the alpha at `HUD_TEXT_SETUP` is the one to
  scale for `Dim`.
- That turning the corners at `HUD_SHADER_QUAD` turns the sprite in both
  the D3D path and the native HUD pass
  (`crates/rewrites/render-bridge/src/native_hud.rs`), and the hook
  order against it.
- Whether `0x0013a532` or the HUD state caps the widget count of one
  `nhdt`.
- Whether a `bipd` naming one `nhdt` twice draws it twice.
- That the training draw is inside a HUD fit scope
  (`points::render_aspect::GROUPS` has none for it), so the box sits
  right under Pinned and Centered at wide aspects.
- Whether lockstep and co-op hashes cover the tag memory and the
  training state the kit writes.
- The waypoint arrow's sprite: which sequence is the plain pointer, and
  its size.
- The frame cost.

## Build plan

Each slice is code and tests only; the coordinator compiles and runs.

1. **The model and compiler.** New crate `crates/tools/hud-kit`
   (`Cargo.toml`, `BUCK`, `README.md`, a row in
   `scripts/crate-domains.toml`; `scripts/crate-table.py` after):
   `model.rs`, `compile.rs`, `templates.rs` (from a `map::Map`: the
   player `nhdt`s, the text shader, each `Art`), `rects.rs` (the retail
   rectangles), `hudg.rs` (the flash records, the message fields, the
   training references, through `tags::defs`). Tests: a compiled widget
   reads back through `hud_edit::model::Widget` with the anchor, offsets,
   registration, font and inputs asked for; one layout per HUD; template
   lookup on `hud-edit`'s fixture map. Disc (`#[ignore]`): templates
   resolve on every map; the retail rectangles match `hud-edit`'s
   placement of the retail widgets.
2. **The preview and checks.** `preview.rs`, `check.rs`, the two
   sources; `crates/apps/cli/src/hud_cmd.rs` and its line in `main.rs`.
   Tests: the synthetic sheet draws; each check fires on a HUD built to
   break it; the JSON's rectangles equal the compiler's. Disc: every
   registered HUD passes every check with nothing skipped; the X glyph
   and every character used are in fonts 6 and 8.
3. **The widget runtime.** `crates/extensions/game/src/hud_kit.rs`:
   install, gate, inputs, string; registration in `boot`; the
   `render_thread.rs` entries; `hud.kit` in the agent crate and
   `docs/runtime/agent-api.md`. Tests on `FlatMemory`: the root points at
   a copy whose head is the retail bytes; a retail widget's hooks pass
   through; a kit widget's gate, input and string are the published
   values; a second install leaves one copy. Verify on the disc and in a
   run: the widget cap, the text shader's colour.
4. **Styles and the pointer.** The colour hook and the flash (move
   `strength` into `hud-kit` and have `h1_hud_text/flash.rs` call it);
   the corner turn. Before coding, decoding 4 and 7. Tests: a `Dim`
   text's alpha; a flash at the record's phases; four corners turned by
   a quarter turn.
5. **Channels.** The `hud-kit` string family in `ui_kit/ids.rs`, the
   lookup hook, `message`, `prompt`, `training`; a fit scope for the
   training draw in `points/render_aspect.rs` if decoding 1 shows it has
   none. Before coding, decodings 1, 2 and 6. Tests on `FlatMemory`: the
   prompt fields as `0x0024ca1d` leaves them; the game's own prompt is
   not overwritten; the training id and byte set and cleared; the lookup
   answers only the family's ids.
6. **The superbounce HUD.**
   `crates/extensions/game/src/superbounce_hud.rs` (the `Hud`, the
   `Values` from `State`, the tutorial's step machine with its tests
   moved from the shell). Delete
   `crates/apps/app/src/superbounce_hud.rs`, `hud_shapes.rs`, their
   uses in `shell.rs`, `lib.rs`, `play_cmd.rs` and
   `superbounce_cmd.rs`'s `hud-preview` (it becomes
   `h2 hud preview superbounce`). Rewrite "The HUD" and "The tutorial"
   in [superbounce.md](superbounce.md) and the overlay paragraphs of
   `docs/runtime/app.md`; add the glass panel to
   `docs/game/notes/not-adopted.md`. Tests: each `State` gives the
   documented values; the preview's five states and four tutorial steps
   pass the checks. Verify in a run with `hud.kit` and `render.frame`
   against the preview's JSON, at 4:3 and 16:9, Pinned and Centered.
7. **Preview fidelity.** Decodings 3 and 5, so the preview draws the
   training box's frame and a retail widget's own flash; `h2 hud
   compare` against a `render.frame` capture, as the UI builder's
   fidelity loop.

Slices 1 and 2 need no game. Slice 6 can be written against slice 1's
model while 3 to 5 are built.

## Related

- [superbounce.md](superbounce.md), "The superbounce game type": the first user.
- [hud-editor.md](../editor/hud-editor.md): the `nhdt` format, placement rules and the renderer the preview uses.
- [health.md](../halo1/health.md), [hud.md](../halo1/hud.md), [text.md](../halo1/text.md): the Halo 1 HUD, which adds widgets and message text the same way.
- [ui-screens.md](ui-screens.md), [ui-builder.md](ui-builder.md): the UI kit.
- [fonts.md](../formats/fonts.md), [button-icons.md](../input/button-icons.md), [high-resolution.md](../graphics/high-resolution.md).
