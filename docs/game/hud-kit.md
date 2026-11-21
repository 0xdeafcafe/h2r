# The HUD kit: HUD elements of ours, drawn by Halo 2's own HUD

Status: In progress. Built: the model, the compiler, the preview and the checks, the widget runtime (install, gate, inputs, string, crop), the prompt, message (notification), training and navpoint channels, and the superbounce HUD on them with its medals ("The medals"). Not built: the colour hook and the flash, the pointer's turn, the navpoint's number, and the `hud.kit` agent method (build plan). Nothing of the runtime has been run in a game ("Not verified").
Code: `crates/rewrites/hud-kit-core` (the model, the HUDs, the templates, the record compiler), `crates/tools/hud-kit` (the preview, the checks), `crates/extensions/game/src/hud_kit.rs` (the runtime), `crates/extensions/game/src/superbounce_hud.rs` (the superbounce HUD's feed), `crates/apps/cli/src/hud_cmd.rs` (`h2 hud`). Tests: `crates/rewrites/hud-kit-core/src/tests.rs`, `crates/tools/hud-kit/src/tests.rs`, `crates/tools/hud-kit/tests/disc.rs`, `crates/extensions/game/src/hud_kit_tests.rs`, `crates/extensions/game/src/superbounce_hud_tests.rs`.
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
| `0x0024cbbf` (EDI string id, user) | queues a notification; called for an item's pickup text (`0x00191fcf`) and by `0x001df40d`. It looks the id up (`0x0013925f`) and calls `0x0024cbee` |
| `0x0024cbee` (EAX user, text) | queues a notification from UTF-16 text: takes a slot (`0x0024ccdb`), copies up to 63 characters to `+4`, sets the start tick, `+0x82`, clears `+0x84` and `+0x85`, gives `+0x83` the next value of the byte at `[0x005023f4] + 0x1385`, and clears the user's `+0x44e`. Ten callers; the game-event one is `0x0019e9c0` (below) |
| `0x0024caac` (EAX user, singular, plural, count) | the same for a line that counts: adds the count to `+0x86`, sets `+0x84`, and shows the plural once the count is over 1 |
| `0x0024ccdb` (state, singular, plural) | the slot: the first of the four that is not active; with both texts, an active counting entry already showing that text (the repeat); else the active entry with the oldest start tick, which it ends |
| `0x0024cc73` (EAX user, ECX text, ticks) | sets the timed centred text at `+0x454` |

Code that already uses them: `h1_hud_text/prompts.rs` (`show`) writes the
prompt fields, and `skulls_custom.rs` queues "our text" through
`0x0024cbbf`.

### Game events and medals (`0x0019de80`)

Read for this page (E, `h2 disasm 0x19de80 60`, `0x19e890 120`,
`0x19e770 70`; V, `h2 strings shared` and `h2 font conduit-13`).

**A medal in a Halo 2 game is a notification.** There is no medal
widget, bitmap or queue in the HUD: the game shows "Double kill" as one
more line of the pickup list, top left, in the HUD font, and the
picture is a glyph of that font at the start of the line.

- **The event.** A game-engine event is a record with a type (`+0`), an
  index (`+4`), a cause player and team (`+0x0c`, `+0x10`), an effect
  player and team (`+0x14`, `+0x18`) and a count (`+0x1c`).
  `0x0019de80` (ESI a player, EAX the event) picks the response that
  player is the audience of, trying cause player, effect player, cause
  team, effect team, then everyone (`0x0019df10` with 0, 2, 1, 3, 4),
  and hands it to `0x0019e890`.
- **The line.** `0x0019e890` returns unless the player has a local user
  (player `+0x28`). It reads the response's display string (`+0x0c`)
  from the multiplayer string list (`0x001a0180`), fills in the player
  names (`0x0019e0f0`) and queues it with `0x0024cbee`: the notification
  insert above. A response with flags bit 0 has a plural string too
  (`+0x1c`) and goes through `0x0024caac` with the event's count. A
  second string (`+0x14`, for `+0x18` seconds) goes to the timed centred
  text through `0x0024cc73`.
- **The sound.** `0x0019e770` then takes the response's sound for the
  language (`+0x48 + 8 * language`), or one of its extra sounds
  (`+0xa0`, `0x50` bytes each, picked by weight), and queues it
  `+0x3c` seconds later through `0x0023ef80`. A response with no sound
  is silent, as the sniper and stick lines are (S: not listened to).
- **The glyph.** The multi-kill and spree strings
  (`flavor_double_kill_cause_player` and the like) are a medal glyph, a
  space, the words and `!`. They exist for the cause player only, so
  only the earner sees the line (V). The glyphs are in both message
  fonts, `conduit-13` and `conduit-12`, 24 pixels high, in their own
  colours (V):

| Code points | Glyphs | Used by a string on the disc |
|---|---|---|
| U+E033 to U+E037 | five star medals: the sprees | the shared map's |
| U+E038, U+E039, U+E040 to U+E043 | six discs with two to seven stars: the multi-kills | the shared map's |
| U+E044 to U+E059 | the kill, vehicle and objective medals | the shared map's, but U+E055 and U+E058 |
| U+E060 to U+E064 | five gold discs with a question mark: spare medals | none |
| U+E065 to U+E069, U+E070 to U+E079, U+E080 to U+E091 | the 27 medals again as white outlines, which take the text's colour | the main menu's (S: the post-game report); U+E090 and U+E091 also by the flag's and the bomb's return lines in a game |
| U+E092 to U+E099 | a white question mark | none |

The low digit of a code point is never over 9 (U+E03A to U+E03F are the
filler box).

So a line of ours with a medal glyph, queued through `0x0024cbee`, is
drawn by the routine, in the font, colour, place, hold and fade of a
retail medal. That is the `Message` channel.

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
| A widget pulses, scales or turns | the widget's effects block (`+0x58`, text `+0x4c`), through `0x0022bcaf` | E; the record not decoded (5). The kit scales its own widgets at that call (runtime step 4) |
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
6. **The notification list's order.** The insert is read (above). Not
   read: `0x0024cb54`, and whether the draw sorts the four entries with
   the comparator at `0x0024cd7a` (start tick, then the byte at `+0x83`)
   or draws them in slot order.
7. **The quad's corners.** `0x0022a664` reads four (x, y) float pairs at
   ECX (E `0x0022a6ca`..); their order is not read.
8. **Waypoints.** `0x0024da7d` and the arrow styles, for a marker with a
   distance drawn on a world point.
9. **The split-screen frames** and the right and top edges of the
   fullscreen frame.

## Where the kit draws

Three places were possible.

| | A host overlay | Host draws the disc's art and fonts | The game draws records of ours |
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

### The model (`crates/rewrites/hud-kit-core`)

The model, the HUDs of ours, the template resolver and the record
compiler are in `hud-kit-core` (`rewrites` domain, guest-free), which
both the offline preview (`crates/tools/hud-kit`) and the game's runtime
use; `hud_edit::Mode` is its `Mode`. A HUD of ours is a `Hud`: a name
and a fixed list of elements. A state
is a `Values`: per element its text, fill, angle, style and whether it
shows. One layout per HUD: a state never adds, removes or moves an
element (the UI kit's rule, for the same reasons: the runtime only
answers hooks, and the checks cover every state).

```rust
pub struct Hud { pub name: &'static str, pub elements: Vec<Element> }

pub struct Element {
    pub id: &'static str,
    pub kind: Kind,
    pub at: Place,
    /// Drawn on that element by design (a count on its pill): the fit and
    /// own checks let the two overlap.
    pub over: Option<&'static str>,
    /// Word D bits that hide it (as a retail widget's `hide_when_d`): ours
    /// gives way while the game draws a retail widget in its place.
    pub hide_when_d: u16,
}

pub enum Kind {
    /// A line in a HUD font. `longest` is the widest text any state gives it.
    Text { font: Font, justify: Justify, longest: &'static str },
    /// A strip that fills from the left with `Value::fill`, 0 to 1.
    Meter { art: Art },
    /// A sprite of a retail HUD bitmap.
    Icon { art: Art },
    /// An icon turned to `Value::angle` about its centre.
    Pointer { art: Art },
    /// The channels (below).
    Message { longest: &'static str },
    Prompt { longest: &'static str },
    Training { longest: &'static str },
    Navpoint { longest: &'static str },
}

/// Anchor and offsets in HUD pixels, per screen mode. `None` hides the
/// element in that mode.
pub struct Place {
    pub anchor: Anchor,
    pub full: (i16, i16),
    pub half: Option<(i16, i16)>,
    pub quarter: Option<(i16, i16)>,
    pub registration: [f32; 2],
    pub origin: Origin,
}

/// What the offsets are from: the anchor's corner, or where the player's
/// grenade row puts a next pill or a next count (`templates::GrenadeRow`).
pub enum Origin { Anchor, NextGrenadePill, NextGrenadeCount }

pub enum Font { Hud, Lcd }            // a text widget's font index 0 and 1
pub enum Art {
    StripMeter,                       // the sword's battery strip and its shader
    StripMeterEnd(u8),                // the strip's last n columns, cropped, with its blue ramp
    WaypointArrow,                    // hudg arrow_assets[0]
    NavpointArrow,                    // hudg arrow_assets[1], the navpoints' arrow
    GrenadePill(PillPart),            // a part of the player's frag pill (shown on word A 0x04 alone)
    Widget { hud: HudRef, name: &'static str }, // a retail widget's bitmap, shader and sequence, by its name string
}
pub enum HudRef { Player, Named(&'static str) } // the player's nhdt, or one by tag name
pub enum PillPart { Body, End }       // the pill's first PILL_BODY (20) pixels, its last PILL_END (7)
pub enum Style { Normal, Dim, Inactive, Flash(Pulse) }
pub enum Pulse { Help, Objective, TimeWarning, TimeExpired } // the hudg records

pub struct Value {
    pub shown: bool, pub text: String, pub fill: f32, pub angle: f32, pub style: Style, pub since_tick: u32,
    pub motion: Motion, // how the fill and visibility move after a change, on wall time (below)
    pub from: f32,      // the runtime's: the fill shown when the value changed
}
pub enum Motion { None, Ease, Ending, RunOut, Blink }
pub struct Values(pub Vec<(&'static str, Value)>);

pub trait Describe {
    fn hud(&self) -> Hud;
    /// The named states the preview sheet draws.
    fn states(&self) -> Vec<(&'static str, Values)>;
    /// Why the HUD is not drawn on a campaign map, if it is not.
    fn multiplayer_only(&self) -> Option<&'static str> { None }
}
```

`records::compile(&Hud, &Templates) -> Result<Compiled, Error>` turns
the elements into widget records: `0x64` bytes per meter, icon and
pointer and `0x54` per text, with the element id of each, bitmaps first
and texts after, as the game draws them (E `0x0013a532`). It writes the
fields at the offsets of the `nhdt` definition (`tags::defs`) that
`hud_edit::layout` reads, and `hud-kit`'s tests read every compiled
widget back through `hud_edit::model::Widget`. `Templates` holds
what must come from the loaded map: for each `Art` the bitmap and shader
datums and the sequences, and for text the shader and string ID of a
retail text widget of the same `nhdt` (the grenade count: the first text
widget whose first input is 20). `templates::resolve` reads the map
through a `MapView`: `hud_kit::templates::EditView` over the HUD
editor's model and renderer for the preview, and
`hud_kit_core::map_file::MapFile` over the map file and its resources
for the runtime; the disc test resolves both on every map and compares
them. The one difference is deliberate: `MapFile` resolves no arrow
(`map_file::NO_ARROWS`), as the runtime's navpoint is the game's own and
the runtime checks only the widgets' templates
(`Templates::missing_widgets`). The strip meter is found by the names
`h1objects::health_hud` uses (`disintegrator_meter`, `ammo_meter`,
sequence 0). An element placed from the grenade row takes its anchor
and offsets from the row's (`compile::placed`): the next pill or count
(below, "The pill"), plus the element's own offsets. An `ArtTemplate`
may carry a crop, `[u0, u1, v0, v1]` of its sprite: the part a
`GrenadePill` part draws. The crop is not in the widget record; the
`Compiled` widget carries it, the preview draws it
(`hud_edit::render::Answers::crop`, `Sprite::cropped`) and the runtime
narrows the quad's texture rectangle to it. A cropped meter's template
also carries its blue ramp, `[b0, b1]` (`templates::ramp`): the blue of
the crop's first column and of its last plus one column's step, read
from the strip's middle row; `records::Widget::input(fill)` is
`b0 + fill × (b1 − b0)`, the `hud_input1` the preview draws and the
runtime gives. The same `Compiled` bytes go
to the game and to the preview, so what is checked offline is what the
game is given.

What the bytes leave to the runtime's hooks: the four show words are
zero, so the retail gate (E `0x0013ac87`) never draws a kit widget on its
own; every input is 0 ("zero"), and the input hook gives `hud_input1`;
a text's string is the template's, and the string hook gives the text.
A text hidden in a screen mode has the font byte -1 there, so the game's
own text draw skips it (E `0x0022a85b`). A hidden mode keeps the
fullscreen offset.

**Motion** (`hud_kit_core::motion`). A HUD's logic stays per tick: it
publishes a target fill and a `Motion` at 30 Hz. The draw hooks run once
per drawn HUD, and each asks the curve at the milliseconds since the
value changed, so a curve is the same at 30, 60 or 120 frames a second,
only sampled more often (docs/game/high-framerate.md). The curves are
pure functions of those milliseconds: `fill_at`, `shown_at` and
`scale_at`, and `animate(&Value, ms, scaled)` for a whole value.

| Motion | Fill | Shown | Scale |
|---|---|---|---|
| `None` | the fill | yes | 1 |
| `Ease` | from `from` to the fill in 200 ms, smoothstep | yes | 1 |
| `Ending` | from `from` to full in 130 ms | off 290 to 360 ms; gone at 600 ms (at 430 ms unscaled) | 130 to 290 ms a swell to 1.12 wide and 1.30 tall and back (a half sine); from 430 ms the width closes to 0 by 600 ms, the height kept |
| `RunOut` | from `from` to 0 in 200 ms | gone at 300 ms | 1 |
| `Blink` | the fill | on and off in sixths of 500 ms, three times on; gone at 500 ms | 1 |

Every step is at least 60 ms, so at 30 frames a second a blink or the
swell still gets two frames. `motion::restamp` decides a stamp: a change
of shown, fill or motion is stamped from the fill the old value was
showing then (0 if it was not showing); the same value again keeps its
stamp and its `from`. Text and style alone are no change.

Four more elements are not widgets. They are channels into the game's
message, training and navpoint parts, one text each:

| Element | What the player sees | Through |
|---|---|---|
| `Message` | a notification in the pickup list, top left, that fades | the notification queue (`0x0024cbbf`) |
| `Prompt` | the prompt line, top left, as "Hold X to pick up" | the user's prompt fields (`+0x220`, `+0x440`, `+0x444`), while the game shows no prompt of its own |
| `Training` | the framed box in the middle of the screen | the scripted training text (`+0x6c0`, `+0x6c4`) |
| `Navpoint` | the game's arrow pointing down at a point of the world, its waypoint marker above, a whole number above that | the navpoint list (`0x0024da7d`, below in "The navpoint"), the number the kit's |

Their texts are UTF-16 with the font's button glyphs or action tokens.
A channel, like a widget, shows only in the screen modes its place has:
`Element::channel` gives it `Place::GAME`, fullscreen only.

A notification is an event, not a state: two bounces are two lines with
the same text. So a `Message` is sent with `Handle::notify(user, text)`,
once per line, for a local user, and not by `publish`. Its element is in
the `Hud` for the checks and the preview, whose states give it a text
(`Value::text`); `longest` is the widest line it is ever sent. A text
of at most 63 UTF-16 units and one line is sent whole; a longer one, or
one with a line break, is refused by the `prompt` check and by
`Handle::notify` (`hud_kit_core::model::message_fault`; E `0x0024cbee`
copies 63).

### The runtime (`crates/extensions/game/src/hud_kit.rs`)

A feature registers its `Describe` once (`hud_kit::register`, which
returns a `Handle`), then publishes a `Values` whenever its state changes
(`Handle::publish`) and the world point of its navpoint
(`Handle::navpoint`). The feature `hud-kit` does the rest:

1. **Install**, before `GAME_ACTIVATE` and after `hud_swap`: the loaded
   map is opened through `native_map::open` (builds with the `gpu`
   feature; others install nothing and say so), and for every `nhdt` a
   `bipd`'s HUD interfaces name (`hud_kit_core::map_file::player_huds`)
   the templates are resolved through `MapFile` and every registered HUD
   is compiled. A HUD that is multiplayer only is not joined on another
   map, and one with a template missing is not joined; both are logged
   with the reason. `install_records` copies each root's two widget
   blocks into the kit's memory (`Builder::guest_alloc`, `REGION_LEN`)
   with the kit's records after them and points the root's two block
   fields at the copies. The retail records are byte for byte the same
   and draw first. What each root held is kept at the head of that
   memory (`hud_kit::Header`), so the next install puts it back first
   and leaves one copy, after a machine snapshot too; a root the game
   reloaded is left alone. With no HUD registered nothing is touched.
2. **Gate** (`HUD_WIDGET_GATE`, before, at the widget loop's two calls):
   a widget in the kit's memory is drawn while its value is shown, in a
   style the runtime draws, in a screen mode its place has (one local
   view fullscreen, two half, more quarter: `player_globals +8`), and
   none of its element's `hide_when_d` bits is set in the game's word D,
   the gate's third stack argument (`[esp+12]`, its low 16 bits). The
   retail gate is skipped for it; a retail widget costs two loads.
3. **Inputs** (`HUD_WIDGET_INPUTS`, before): `hud_input1` is
   `records::Widget::input(fill)`, the other three 0.

   The gate and the inputs read a value with a motion as it draws now:
   `Handle::publish` stamps a changed one with a host `Instant`
   (`hud_kit::stamp_values`, through `motion::restamp`), and
   `hud_kit::animated` puts it on its curve at the milliseconds since.
4. **Scale** (`HUD_WIDGET_EFFECTS`, `0x0022bcaf`, before, at the bitmap
   draw's call, return `0x0022b19d`): a kit bitmap widget's basis
   factors in the draw's frame, `[ebp]` across and `[ebp+0x34]` down (the
   two `h1_hud_text::weapon`'s split-screen scale writes; S: which is
   which), are multiplied by `motion::scale_at`; the widget is
   `[ebp+0x78]` (`hud_kit::scale_basis`). A factor that is not a positive
   number is left. The first time the hook meets a kit widget it sets
   `hud_kit::EFFECTS_SEEN`: the game does call it for a widget with an
   empty effects block, and the scale applies. Until then the curves run
   unscaled and the ending is the plain one (sweep, blink, gone at
   430 ms); a crop-based close was not chosen, as it would need the
   sprite hook to narrow from both sides.
5. **String** (`HUD_WIDGET_STRING`, before): the value's text, UTF-16, in
   the element's engine font for the mode, and the four inputs copied to
   the HUD material's (`0x004e6980`), as the text draw does.
6. **Crop** (`HUD_WIDGET_SPRITE`, before): for a cropped widget the hook
   picks the sprite as the game does (`hud_kit::sprite_pick`: the mode's
   sequence and its first sprite; a sequence with no sprites, its
   `first_image` whole; no such sequence, image 0 whole; then the
   image's width and height) and narrows its bounds to the crop
   (`hud_kit::narrow`). A sprite-less sequence is cropped too: the
   battery strip and the grenade panel are drawn as whole images, and
   left to the game they drew 92 and 103 pixels wide, the meter across
   the full strip and the pill over the retail pills' art. The crop is applied to the sprite's bounds,
   not to the quad's corners at `HUD_SHADER_QUAD`, whose order is not
   read (decoding 7).
7. **Channels**, at the tick's end (`GAME_TICK`, after) and in the prompt
   hook:
   - `Prompt`: the first shown prompt text is written to the first local
     player's prompt fields (`+0x220`, `+0x440`, `+0x444`, the fade
     bytes `+0x44e`, `+0x44f`) in a hook on `HUD_INTERACTION_VISIBLE`, at
     the interaction code's call, only while the game shows no prompt of
     its own (`hud_kit::show_prompt`, as `h1_hud_text/prompts.rs` does).
   - `Training`: the first shown training text is the kit's string id
     (family `hud kit` in `ui_kit::ids`), set with the show byte in the
     scripted training slot (`[0x0051e9c0] +0x6c0`, `+0x6c4`) unless the
     game shows a scripted text of its own; the string lookup
     (`0x001a0180`, `community_variants`' override) answers the id with
     the text (`hud_kit::write_training`, `hud_kit::text`).
   - `Navpoint`: at install the scenario's point block
     (`[0x004e0350] +0x1e4`) is copied into the kit's second allocation
     with one point after it, and the scenario points at the copy
     (`hud_kit::install_point`, kept and put back as the widget blocks
     are). Each tick that point is set to the HUD's navpoint and one slot
     of the first local player's navpoint list is kept on it: the slot
     already ours, else one the game skips, else the list's end, with the
     end moved one on (`hud_kit::write_navpoint`). Without a point the
     slot is skipped (kind 15). The game draws its own arrow and marker
     there; the kit's number is not drawn.
   - `Message`: `Handle::notify(user, text)` queues the line (a user
     under 4, a text `message_fault` passes; else refused and logged).
     At the top of the next tick a before-hook on `GAME_TICK` writes
     each queued text, UTF-16 and zero ended, into a 128-byte slot of
     the kit's third allocation (`NOTES_LEN`, 32 slots, so 32 lines a
     tick at most; the rest wait), and calls the insert `0x0024cbee`
     for each with EAX the user and the slot's address its stack
     argument ("Game events and medals"), then runs the tick itself and
     answers for it, as `skulls_custom` sends its messages. A flag passes
     the hook while that tick runs. It is in the first group, after the
     agent's calls, our skulls', the map select's and the dedicated
     lobby's hooks there (`hud_kit::stage_notes`, `notify_call`).

The styles the runtime draws are `Normal`, and `Flash` as `Normal`; a
widget in `Dim` or `Inactive` is not drawn until the colour hook
(slice 4). The draw hooks are `context_only` and read one shared table,
so they run on the render thread
(`crates/extensions/game/src/render_thread.rs`); the install and the tick
are listed there as refused. The feature is presentation-only and
snapshot-stateless: what the roots and the scenario held is in guest
memory, and the draw table is rebuilt at each install (a snapshot loaded
mid-game draws no kit widget until the next game starts).

### Anchoring next to retail art

An element is placed in HUD pixels from an anchor, like the retail
widget beside it. The crate has the retail rectangles as named
constants, checked on the disc: `TRACKER`, `SHIELD_BAND`, `H1_HEALTH`,
`PROMPT_LINE`, `TRAINING_BOX`. `rects::above(SHIELD_BAND, gap, height)`
and the like give offsets from them, so "above the motion tracker" is written
once and is right in each screen mode.

### Cost

S, to be measured with `profile` on the lab Mac: one address-range test
per retail widget on hook points other features already use,
and for a HUD of ten elements about ten more HUD quads and five text
draws a frame, in the game's own batches. No host texture, no upload.

## The preview and the checks

```sh
h2 hud preview list                                   # HUDs and their states
h2 hud preview <hud> [--state NAME|all] [--mode full|half|quarter|all] [--map NAME] [--disc ISO] [--synthetic] --out DIR
h2 hud sheet [--map NAME] [--disc ISO] [--synthetic] --out DIR   # every HUD x state x mode, sheet.png
```

`hud_kit::preview` draws on `hud_edit::render`'s 640 by 480 canvas: the
player's retail `nhdt` at a `HudState` (so the tracker, the shield meter
and the grenade panel are in the picture), then the compiled kit widgets
through the same draw, `hud_edit::render::draw_answered`, with the
values given as the runtime's hooks give them (shown, fill, text,
colour, turn), then the channels: the prompt line and a notification at
their points in font 6 (5 in split screen), and the training box around
its text. The player `nhdt` is the one a biped's HUD interfaces name,
`ui\hud\masterchief` when the map has it. The retail HUD is drawn with
the state gate on at the source's retail words (`source::retail_words`):
word A `0x1000`, the retail shield meter's show word, plus the first
show bit of the widget the HUD editor places on the tracker's rim
(`rects::TRACKER`) that lets it through the gate (S: that the game sets
that bit whenever it draws the tracker; decoding 5). A HUD with no
widget there keeps `0x1000` alone. Every report notes which bit was
taken. The preview draws a navpoint at its own sample point; in a split
frame, whose stack would leave the frame's top at the sample, it moves
the point down until the stack fits (S: that the game keeps a navpoint
inside its waypoint area, the safe bounds less `hudg +0xec`..`+0xf8`).

`preview` writes `<hud>-<state>-<mode>.png` and a `.json` with every
element's id, kind, rectangle, the rectangle it may take in any state
(`reserved`: a text's at its `longest`), text, style, fill, angle, font,
whether it is drawn and why not, and the findings of that state and
mode; by default every state, fullscreen. `preview` fails on a glyphs
or drawn finding, and, from the disc, when the glyph check could not
run; its other findings are printed. `sheet` writes every HUD x
state x mode, `sheet.png` (half-size tiles, a column per mode),
`sheet.json` (where each tile is) and `checks.json`, and fails on a
finding. `Source::disc` reads the map (default `lockout`), the fonts and
the XBE's colour constants from the user's disc (`--disc`, else
`$H2_DISC`); `Source::synthetic` draws an invented map whose tracker,
shield meter, grenade pills and counts are flat boxes on the retail
rectangles and whose `hudg` has an arrow and a marker,
with `hud_edit`'s fixed-advance stand-in font, so `cargo test` draws the
sheet without a disc. Output goes to a directory you name outside the
Git checkout; one inside it is refused.

The preview's colours: the prompt's and the flash records' from the
map's `hudg`; notifications' and the training text's from the XBE
(`0x00468c84`, three floats, and `0x0045e518`, `0x0045e51c` with blue 1;
S: the order); a kit text widget's is `hud_edit`'s stand-in until
decoding 4. `Dim` halves the alpha; `Inactive` greys (a text's colour, a
bitmap's texels, each its luma) at 0.7 alpha, through the renderer's
`Answers::look` for bitmaps; and `Flash` draws the record's
colour at the flash's start (`hudg::Flash::start`): the rest colour, or
the peak with flags bit 0, as the help line picks `+0xa4` or `+0xa8`
(E `0x0024d194`); the objective line draws only its rest colour `+0xcc`
(E `0x0024d13e`), and a record's peak may be unset, all zero. The
engine takes each colour's alpha byte as it is (E `0x00131b20`). The
flash's curve over `since_tick` is slice 4's. The training
box's frame is a stand-in rectangle until decoding 3, and a pointer is
turned within its sprite's rectangle.

Checks, over each state and mode (`hud_kit::check`), each finding naming
the check, the elements and the state and mode:

| Check | Rule |
|---|---|
| fit | every state's text of a text widget is no wider than its `longest`, and the `longest` stays inside the frame's width and meets no other element but the one it is `over` |
| glyphs | every character of every `longest` and state text is in the element's font (a channel's in fonts 6 and 5, a navpoint's number in font 6): not record 0, not the record U+FFFF reads, and no copy of either's bitmap (`font::Font::has_glyph`; MSLCD-14 maps `.`, `-` and `~` to a copy of its box) |
| frame | every rectangle, reserved or drawn, is inside the HUD frame and the screen; an element may sit `FRAME_SLACK` (1) pixel outside the frame, as the tracker and the shield meter do (x 47, the frame's left 48) |
| retail | no widget element's opaque pixels meet a retail widget's at any `HudState` of the sheet (plain, overshield, plasma grenades, zoomed), and no widget element's rectangle meets the reserved rectangles (`PROMPT_LINE`, `TRAINING_BOX`, `H1_HEALTH`, `RETICLE`); the channels are the game's own text and are not checked against them |
| own | no two elements of any registered HUDs overlap, by their rectangles and their pixels, texts included; two channels are the game's, one at a time, and are not compared, nor is an element with the one it is `over` |
| one layout | every state has a value for every element and for nothing else, and every element id is used once |
| templates | every `Art` and the text template resolve on the map (both sources; on every map of the disc in the disc test) |
| prompt | a `Message` or `Training` text, and its `longest`, has at most two lines, a `Prompt` one (the game's prompt line draws no line break), and fits the frame's width (less the training box's margin) |
| modes | an element with no half or quarter offset is not drawn there (the JSON says why), a text has no font there, and the widgets of one HUD show in the same modes, so a block is never drawn in part |
| drawn | a text or channel shown in a state, in a mode it shows in, puts opaque pixels down (alpha over 64): a transparent colour or a missing shader is a finding |

The synthetic source has no fonts, so it skips the glyph check and says
so, and its retail check is against the flat boxes, not the disc's art.

The agent API gets one read method, `hud.kit`: the registered HUDs, and
per element the id, kind, widget address, placed rectangle, text, fill
and whether the gate last drew it. It is how a run is checked against
the preview's JSON, with `render.frame` for the pixels.

## The superbounce HUD on the kit

`crates/rewrites/hud-kit-core/src/huds/superbounce.rs` describes it:
the elements, `View` (what the HUD shows of `superbounce_mode`'s state),
`values(&View)`, the preview's states and the tutorial's step machine.
It is multiplayer only (`Describe::multiplayer_only`): a campaign
player HUD has no grenade row, and the disc test skips campaign maps for
it by that reason. It is shown to the first
local player. It is made only of things Halo 2 draws already: a grenade
pill, a navpoint and the training box. Launch and drop are not on the
HUD: they are on the strips' labels in the world and in the agent API.

```sh
h2 hud preview superbounce --state all --out DIR      # the disc's lockout
h2 hud preview superbounce --state all --mode all --out DIR
h2 hud preview superbounce --state all --synthetic --out DIR
```

### The pill

| Element | Kind | Place | Art |
|---|---|---|---|
| `sb.pill` | Icon | `Origin::NextGrenadePill`, every mode | `Art::GrenadePill(Body)`: the frag pill's first 20 pixels |
| `sb.pill.end` | Icon | `Origin::NextGrenadePill` plus (20, 0), every mode | `Art::GrenadePill(End)`: the frag pill's last 7 pixels |
| `sb.pill.icon` | Text, `Hud`, centred, over `sb.pill` | `Origin::NextGrenadeCount`, every mode | the text template (the frag count's shader and string); `^` in font 6 |

The grenade row is read from the player's `nhdt`
(`templates::GrenadeRow`): the frag pill is the first bitmap widget
shown on word A `0x04` alone, the plasma pill on `0x08` alone, and the
counts the first text widgets of inputs 20 and 21 shown on those bits
(V: retail `ui\hud\masterchief` widgets 0 and 1, docs/halo1/hud.md
"Grenades"). On the disc the two pill widgets are one panel: both at
offset (0, 0) from the top-left anchor, each drawing the whole 103 by
19 sprite of `grenade_bkd` with both pills in it, its own lit (sequence
0 lights the frag pill, the right one, sequence 1 the plasma, the left;
D: `h2 bitmap lockout 'ui\hud\bitmaps\new_hud\backgrounds\grenade_bkd'`).
The counts are at (12, -1) and (65, -1): the step from the plasma count
to the frag count, 53, is the pills' step too, so a pill is 103 - 53 =
50 wide and the frag pill is the panel's last 50 pixels (x 53 to 103).
Per screen mode the next pill's top left is the frag pill's plus the
step, (106, 0) from the anchor, screen x 154 fullscreen; the next count
is the frag count plus the step, (118, -1). A row whose pill widgets are
at two offsets, or whose sprite is not one size in every mode, is a
named error.

Each retail pill has its grenade baked in: "x" and the grenade's picture
from about x 22, the count at x 12. A third pill cut whole from the frag
pill would show a frag grenade. So the kit's pill is short: the frag
pill's first 20 pixels (its left end and body, `PillPart::Body`) with
its last 7 (the right end, `PillPart::End`) against them, 27 by 19,
screen x 154 to 181, both cropped from the frag pill's part of the
panel (`GrenadeRow::crop`). The icon sits where a third count would,
pill x 12.

The icon is `^`, the chevron of `conduit-13` (font 6, the counts'
font). The font's arrows (U+2191, U+25B2, U+21E7) are its missing box
(D: `h2 font conduit-13`), and its d-pad glyph U+E108 is swapped for the
bound key's picture on the other input sets
([button-icons.md](../input/button-icons.md)). No disc bitmap reads as
"up" at a count's size: `hud_waypoints`' chevrons and `default_arrow`
(15 by 24) and `hud_multiplayer`'s triangle (32 by 16) are bigger than
the pill (D: `h2 bitmap`). Which to use is open (Design, "Not verified").

| State | Pill and icon |
|---|---|
| primed | `Normal`: the pill as the frag pill draws, the icon in the text colour |
| not primed | `Inactive`: greyed (each texel its luma) at 0.7 alpha |
| scanning | `Dim`: half alpha |
| no strips, scan failed | hidden |

While the charge is about to go (the meter below at half or under:
19 still ticks of 38, the tick the prompt line says "Keep moving or lose
the charge"), the icon is `Flash(TimeWarning)`, the timer's warning
record.

### The charge meter

`sb.charge` is a `Meter` under the pill: how long the charge has left,
primed, and the empty strip of a loss. How far the player is through
priming is on the arming bar (below, "The arming bar"). The preview
draws it in every state; the runtime is in "The runtime, for this HUD".

**What a charge is.** The engine has no charge amount. The pair word is
0 through the whole crouch and −3 from the first tick of the stand-up; it
does not step ([superbounce-spots.md](notes/superbounce-spots.md), "The
pair word around a forced crouch"). What does change by degrees is the
player's place in the recipe, and after it the time a still body has
before it sleeps and the pair is rebuilt at 0. The arming bar shows the
first and the strip the second, and nothing invented:

| Phase | When (`superbounce_mode`'s state) | Arming bar | Strip | Pill and icon |
|---|---|---|---|---|
| hidden | not primed, and not crouched within 1.5 units of a crouch spot | hidden; run out if a step came before | hidden | inactive |
| near | crouched within 1.5 of a crouch spot, the standing capsule fits | 0, the empty bar | hidden | inactive |
| under | crouched, the standing capsule does not fit, crouch held | 1/3 | hidden | inactive |
| held | crouch let go and the request still set: the forced crouch | 2/3 | hidden | inactive, the icon `Flash(Help)` |
| primed | the word is −3 | the ending, then hidden | 1 less the still ticks over 38 | lit |
| lost | the word left −3 | the blink | 0 for one second, then hidden | inactive |

"The standing capsule fits" is the engine's own answer for `held` (the
request stays set with the button up only while it does not fit), and
the mode's capsule test against the map's collision for `under`: the
engine's bit (output bit 1 of the character physics update, E
`0x001e55d0`) is kept in no field found so far ([superbounce.md](superbounce.md),
"Charge").
The still ticks count up while the body's speed is under 0.03 units/s
and return to 0 when it moves; 38 is the measured time to sleep (S: the
speed limit, and that Havok counts the same ticks). A crouch while
primed is `lost` at once.

**The record.** A meter in Halo 2 is a bitmap widget with a meter
shader; the widget has no fill or colour field
([the research](notes/2026-11-21-h2-custom-hud-mods.md)). The kit's
`Art::StripMeter` is that: the energy sword's battery strip and its
shader, which lights the texels whose blue is below `hud_input1`
([health.md](../halo1/health.md)). The strip is 92 by 8 and the pill is
27 wide, so the meter is the strip's last 27 columns, cropped as the
pill is cropped from the frag pill (`Art::StripMeterEnd(27)`, a crop of
`Art::StripMeter`'s template).

| Field | Value |
|---|---|
| `input_1`..`input_4` | 0; the input hook answers `hud_input1` |
| the four show and four hide words | 0; the gate hook shows it |
| the three thresholds | 0 |
| `anchor` | the grenade row's (top left) |
| `bitmap`, `shader` | the strip meter template's |
| `sequence_full`, `_half`, `_quarter` | the template's (0) |
| `offset_*` | `Origin::NextGrenadePill` plus (0, 21): 2 pixels under the pill, screen (154, 57) to (181, 65) fullscreen |
| `registration_*` | (0, 0) |
| `effects` | empty |
| `special` | 0 (not 5: the shield meter's type takes the inputs over) |
| crop, in `Compiled` | columns 65 to 92 of 92 |

A cropped meter shows only part of the strip's blue ramp, so the hook
gives `hud_input1 = b0 + fill × (b1 − b0)`, with `b0` and `b1` the blue
of the crop's first and last columns, read from the strip's texels when
the templates resolve (on the disc the ramp runs from column 13 to the
right edge, V [health.md](../halo1/health.md)). The colours are the
shader's own, the ammunition counters'. Nothing of ours tints it.

The checks treat it as any widget: `fit` and `retail` against the
grenade panel above it (2 pixels clear) and the prompt line below (y 96);
`modes` with the pill, which it shares every screen mode with.

**The prompts and messages.** The recipe's verbs go on the game's own
prompt line, with the bound button drawn by the game, and the results in
its notification list:

| Channel | Text | When |
|---|---|---|
| `Prompt` | Hold (crouch) and walk under | near, not primed, within 1.5 of a crouch spot |
| `Prompt` | Let go of (crouch) | under |
| `Prompt` | Walk out to stand up | held |
| `Prompt` | Keep moving or lose the charge | primed and still for 19 ticks or more, the fill at half or under |
| `Prompt` | Find a crouch spot to charge again | for 150 ticks after a loss, until a crouch step begins; the reason is the loss's notification |
| `Prompt` | Press (X) to show ghost | looking at a spot with a ghost run, when no recipe step is on the line (it replaces "Hold (crouch) and walk under"; [superbounce.md](superbounce.md#ghost-runs)) |
| `Prompt` | Press (X) to stop ghost | while a ghost run plays, as above |
| `Message` | Charging... | a recipe step begins (near, under or held, not primed) after none: the arming bar comes up |
| `Message` | Primed! | the word becomes −3 |
| `Message` | Charge lost: you stood still / Charge lost: you crouched / Charge lost | lost, by its reason ([superbounce.md](superbounce.md), "Charge"): the body slept, a crouch, another reason |
| `Message` (`sb.medal`) | a medal line (below, "The medals") | a bounce, or a Bounced |
| `Message` | Not primed. / Missed the strip. / Too slow. / No bounce. | a landing within 0.5 of a strip that did not bounce, by its first failed test ([superbounce.md](superbounce.md), "Landings") |

(crouch) is U+E110, the left stick click's glyph, the default crouch
button ([button-icons.md](../input/button-icons.md)), which the button
swap redraws as the bound key's picture on the other input sets. U+E110 is one glyph, 38 by 19, that shows
both sticks: the left one lit and outlined, the right one faint (alpha
3 of 15), as every retail stick glyph does. The preview draws it as the
game does, so the prompt reads "Let go of" and a pair of sticks. A
prompt of the game's own wins the line.
The landing's result is `superbounce_mode`'s `last_landing`. A bounce's
line is its medal. "Charging..." and "Primed!" are the shape of
Assault's pair, "Bomb is arming..." and "Bomb armed!" (V:
`multiplayer\multiplayer_globals`), in the same list.

### The arming bar

`sb.arm` is Assault's bomb arming bar, reused: how far the player is
through the recipe, then a short ending when primed. Halo 2 draws it
low in the middle while a bomb carrier arms; ours is the same widget's
art at the same place.

| Field | Value |
|---|---|
| element | `Element::meter("sb.arm", Art::Widget { hud: HudRef::Named("ui\\hud\\scoreboard"), name: "bomb_defusal" }, ..)`, `hidden_on_d(0x400)` |
| place | anchor centre, (0, 110) fullscreen, (0, 27) half and quarter, registration (0.5, 0.5): the retail widget's (V) |
| art | the scoreboard `nhdt`'s bitmap widget `bomb_defusal`: `ui\hud\bitmaps\new_hud\bomb_meter`, a capsule 140 by 13 (sequence 0) and 86 by 9 (sequence 1, split screen), its blue rising 4 to 253 left to right; the shader `ui\hud\shaders\bomb_defusal_meter` on `widget_legacy_meter`, red `6d2323` to `cf6767` where the texel's blue is below `hud_input1`, white for the outline (V) |
| input | uncropped, so `hud_input1` is the fill itself |
| crop, ramp | none |

The retail widget's input is `0x43`, the bomb arming amount, and it is
shown on word D `0x400` alone (V: `zanzibar.map`; `0x40` and `0x41` are
the two score fractions, `0x42` "winning"). The kit's record has zero
words and inputs, as every kit widget, and its gate hides it while the
game's word D has `0x400`: in an Assault game the retail bar wins its
place. A map without the scoreboard `nhdt` gets no superbounce HUD, with
the reason logged; every multiplayer map has it.

The bar follows the charge's phase each tick
(`superbounce::ArmTrack`); its motion plays on wall time (runtime step
4 and "Motion", above):

| Tick | Fill | Motion |
|---|---|---|
| near, under, held | 0, 1/3, 2/3 | `Ease`: 200 ms to the step's fill from the fill shown |
| primed (the tick the word becomes −3) | 1 | `Ending`: the sweep to full (0 to 130 ms), the swell, 12% wide and 30% tall (130 to 290), one blink off (290 to 360), the close to its centre, across (430 to 600), gone |
| no charge after a step: left early | 0 | `RunOut`: drained in 200 ms, gone at 300 |
| lost | 0 | `Blink`: the empty bar three times in 500 ms, beside the loss's notification, then gone |
| no charge after the ending, the run out or the blink | as it was | the same: its curve has hidden it |

The pill lights and the strip under it fills on the primed tick itself;
"Primed!" is in the list a tick later. Without the effects hook
(`hud_kit::EFFECTS_SEEN` unset) the ending is the plain one: the sweep
and the blink, gone at 430 ms. The bar is silent. Two reds mean two
things: the bar's "charging", the world's lock-on "on course"
([superbounce.md](superbounce.md#lock-on)).

### The medals

The rule, the recording and the agent API are in
[superbounce.md](superbounce.md#medals); this is what the player sees.

A bounce shows one notification, as a retail multi-kill does ("Game
events and medals"): a medal glyph of the HUD font, a space, the words
and `!`, in sentence case. `sb.medal` is a `Message` in every screen
mode, sent to the local user of the player who bounced, so in split
screen each view gets its own; a bot's or a remote player's award is
recorded and not shown. A bounce shows the highest medal it earned and
no other line (a bounce's landing has none of its own); a Bounced shows
its own line. Each award is sent once, by its number
(`superbounce_chain::Award::seq`; `superbounce_hud::Sent`).

| Medal | Earned | Line | Glyph |
|---|---|---|---|
| Superbounce | every bounce; its line on 1 and 7 and up | `Superbounce!` | U+E2A0, a slate-blue disc, one chevron rising off a strip |
| Double Bounce | the chain's 2nd bounce | `Double bounce!` | U+E2A1, a gold disc, two chevrons |
| Triple Bounce | the 3rd | `Triple bounce!` | U+E2A2, a steel disc, three chevrons |
| Quad Bounce | the 4th | `Quad bounce!` | U+E2A3, a bronze disc, two pairs of chevrons |
| Chain Bounce | the 5th | `Chain bounce!` | U+E2A4, a lavender disc, a chevron over two pairs |
| Bouncetacular | the 6th | `Bouncetacular!` | U+E2A5, a pale gold disc in a spiked rim, three pairs |
| Bounced | a fall out of the map during a bounce | `Bounced!` | U+E2A6, a maroon disc, a chevron falling through a broken strip |

**The picture.** The disc has no superbounce medal, so the seven glyphs
are new art, drawn for this project in the retail medals' manner: 24 by
24, a shaded disc with a dark rim and the light from the upper left, in
their own 12-bit colours, and the tier told as a multi-kill tells it,
by the count of marks on the disc and the disc's colour, with a frame
for the top tier as the sprees have. The art is data,
`crates/formats/font/src/medals/art.rs` (ARGB 4444 rows, written by
`scripts/superbounce-medals.py` from its drawing, which also draws a
preview sheet beside the retail medals with `--preview DIR --h2 BIN`), and
`font::medals::replacements` adds the seven glyphs to `conduit-13` and
`conduit-12` through the font rewrite ([button-icons.md](../input/button-icons.md),
`font::patch`) at spare code points of the icon range, U+E2A0 to
U+E2A6, and an eighth at U+E2A7 for the Incel medal
([teabag.md](teabag.md)), a teabag under a red no sign drawn by
`scripts/live-medals.py`, which no HUD line shows; each placed as the font's own Double Kill (U+E038: its advance
and offsets; a font without a 24 by 24 one gets none). Every other font
is left alone. `font::medals::patch` is the same patch for tools that
read the disc's fonts (`hud-edit`'s renderer, the preview). New art is a new `art.rs`: the
line, the insert and the checks do not change. `MEDAL_GLYPHS` in
`hud_kit_core::huds::superbounce` names the code points.

**The sound.** None. A retail medal's voice is its response's sound
("Game events and medals"), and no line on the disc says "bounce" (S:
the event sounds are not listed or listened to). Playing the Double
Kill line on a Double Bounce would say the wrong thing.

**Not the game's event machinery.** The other way to show a medal is a
new row in the map's event table with its strings, raised as a
game-engine event, so the game picks the audience, plays the sound and
sends it to the other consoles.

| | A notification of ours (`0x0024cbee`) | A new game-engine event |
|---|---|---|
| Draws | as a retail medal: it is the same call | the same |
| Tag memory | none | a longer event block and new string ids, on every console |
| Network | none: each console shows its own players' bounces from its own simulation | a new event index in the game's own packets; a console without it is S |
| Who sees it | the earner, as retail's multi-kills | the earner |

The event buys nothing a retail medal shows, so the kit sends the
notification.

### The navpoint

`sb.navpoint` is a `Navpoint` channel over the next thing to do, in
every screen mode: while not primed, the nearest crouch spot's stand
point; while primed, the chosen strip's (`superbounce_mode`'s
`nav_target`: the nearest strip the player's floor is high enough for,
else the nearest, and while falling the strip the predicted landing is
on), the same strip the world's landing mark and lock-on are on
([superbounce.md](superbounce.md#floor-marks)). Its number is whole
metres: over a crouch spot the distance to it, over a strip the drop to
come from (the strip's `fall`, 0 for a standing jump), as "3" over the
crease in the design. Halo 2 draws a navpoint in
`0x0024e59f` (E, called per slot from `0x0024da7d`): it projects the
point (`0x0002b400`, `0x0002b460`), clamps it into the waypoint area
(`hudg` `waypoint_*_inset`, `+0xec`..`+0xf8`), then draws three HUD
shader quads (`0x0022a664`):

- the arrow, `hudg` `arrow_assets[1]` (`+0x408`; image, material and
  sequence), `0x0024e9c7`, drawn whether or not the point is on screen;
  it returns the arrow's length. The row is the draw record's `+4`,
  which `0x0024da7d` sets to 1 (`0x0024db90`; `hudg::NAVPOINT_ARROW`).
  Row 0 is another sprite, the one `Art::WaypointArrow` names. On the
  disc row 1 is `hud_multiplayer`'s green triangle pointing down;
- the marker, `hudg` `waypoint_markers[0]` (`+0x410`), `0x0024ecae`, the
  record's one layer: its outside sequence when the point is off screen,
  its obscured one when it is hidden, else its in-view one (on the disc
  `hud_waypoints`' double chevron). It is put out from the point along
  the arrow's direction by the arrow's length and one more pixel (S: the
  rest of the sum), so on screen it sits above the arrow and the arrow
  points down at the point;
- an outline, `arrow_assets[1]`'s `outline_image` (`+0x1c`),
  `0x0024f0fc`, only while the point is on screen.

All three are on screen together. Their colour is the user's HUD colour:
`0x0024da7d` fills the record's four colours from `0x0013927e`, which
the shader gets at `0x004e6950` and `0x004e695c`; their alpha is the
user's HUD alpha (`0x004e69c0`) times the navpoint's opacity (1.0). The
game draws no number with a navpoint: the distance is the kit's, whole
metres in font 6, as the grenade counts are whole (the count fonts have
no `.`, D: `h2 font MSLCD-14 '1.2'`).

The preview draws at a sample point (`NAVPOINT_SAMPLE`, right of and
above the frame's centre) the arrow with its tip on the point, the
marker's in-view sequence 1 pixel above it (`NAVPOINT_GAP`) and "12"
over a crouch spot or "3" over a strip (`SAMPLE_DISTANCE_M`,
`SAMPLE_DROP_M`) centred 2 pixels above the marker (`NAVPOINT_NUMBER_GAP`), the texels
times the notifications' colour (the same `0x0013927e`; S: that the HUD
shader multiplies). The outline is not drawn (decoding 8). It is
`Normal` over either target, and hidden while scanning and without a
target.

### States

| State | Pill and icon | Strip | Arming bar | Prompt, message | Navpoint | Tutorial |
|---|---|---|---|---|---|---|
| `not-primed` | inactive | hidden | hidden | | crouch spot, "12" | |
| `near` | inactive | hidden | 0, ease | Hold (crouch) and walk under; Charging... | crouch spot, "1" | |
| `under` | inactive | hidden | 1/3, ease | Let go of (crouch) | crouch spot, "1" | |
| `held` | inactive, icon `Flash(Help)` | hidden | 2/3, ease | Walk out to stand up | crouch spot, "1" | |
| `primed-ending` | lit | 1 | 1, ending | Primed! | strip, "3" | |
| `left-early` | inactive | hidden | 0, run out | | crouch spot, "12" | |
| `primed-full` | lit | 1 | hidden | Primed! | strip, "3" | |
| `primed-draining` | lit, icon `Flash(TimeWarning)` | 0.25 | hidden | | strip, "3" | |
| `primed-still-warning` | lit, icon `Flash(TimeWarning)` | 0.5 | hidden | Keep moving or lose the charge | strip, "3" | |
| `lost` | inactive | 0 | 0, blink | Find a crouch spot to charge again; Charge lost | crouch spot, "12" | |
| `lost-stood-still` | inactive | 0 | 0, blink | Find a crouch spot to charge again; Charge lost: you stood still | crouch spot, "12" | |
| `lost-crouched` | inactive | 0 | 0, blink | Find a crouch spot to charge again; Charge lost: you crouched | crouch spot, "12" | |
| `bounced` | lit | 1 | hidden | (medal) Superbounce! | strip, "3" | |
| `medal-double` | lit | 1 | hidden | (medal) Double bounce! | strip, "3" | |
| `medal-triple` | lit | 1 | hidden | (medal) Triple bounce! | strip, "3" | |
| `medal-quad` | lit | 1 | hidden | (medal) Quad bounce! | strip, "3" | |
| `medal-chain` | lit | 1 | hidden | (medal) Chain bounce! | strip, "3" | |
| `medal-bouncetacular` | lit | 1 | hidden | (medal) Bouncetacular! | strip, "3" | |
| `medal-bounced` | inactive | hidden | hidden | (medal) Bounced! | crouch spot, "12" | |
| `missed` | lit | 0.8 | hidden | Missed the strip. | strip, "3" | |
| `scanning` | dim | hidden | hidden | | hidden | |
| `no-strips` | hidden | hidden | hidden | | hidden | |
| `tutorial-1` | inactive | hidden | hidden | | crouch spot | step 1 |
| `tutorial-2`, `-3`, `-4` | lit | 1 | hidden | Primed! | strip | steps 2 to 4 |

`bounced` and the six `medal-` states are the seven medals, one line
each, with the glyph in its own colours as the game's font draw leaves
a coloured glyph (the tutorial's (X) is drawn the same way). The
preview draws them in font 6, and in font 5 in the half and quarter
modes:

```sh
h2 hud preview superbounce --state bounced --mode all --out DIR
h2 hud preview superbounce --state medal-chain --mode all --out DIR
```

The `glyphs` check covers the seven code points in both fonts (the disc
test also reads them from the patched `conduit-13` and `conduit-12`:
`the_glyphs_are_in_the_fonts`), `prompt` the line's width and its 63
units, and `drawn` that the line puts pixels down.

### The runtime, for this HUD

`crates/extensions/game/src/superbounce_hud.rs` registers the HUD and
feeds it each tick from `superbounce_mode`'s state through one adapter,
`superbounce_hud::view`:

| State (`superbounce_mode::State`) | View |
|---|---|
| `scan` | ready with spots, ready with none (`NoStrips`), failed, else scanning |
| `charge` (phase, fill) | the charge, as it is; the arming bar from the phase tick to tick (`ArmTrack`: the run out when a step gives way to no charge); on a recipe step begun, one `Handle::notify` of "Charging..." |
| `verb` | the prompt's verb (a crouch step, keep moving, charge again); in the near phase, not primed and with no verb, "Hold (crouch) and walk under" |
| `word` | on its change to −3, one `Handle::notify` of "Primed!" |
| the loss (`charge`'s while lost, `ChargeAgain`'s after) | its reason's message at its tick, when newer than the last landing; the HUD keeps the last loss past the 150 ticks the state has it, so an older landing does not take its place. One `Handle::notify` per loss, by its tick |
| `nav_target` (point, kind, distance) | the navpoint's target and distance, world units to metres; its point goes to `Handle::navpoint` |
| `last_landing` (result, tick) | the landing's message at its tick, for a landing that did not bounce; one `Handle::notify` per such landing, by its tick |
| `medals` (the awards newer than the last sent) | `sb.medal`: the last local award's highest medal at its tick; one `Handle::notify` per award of a local player, to its user: the line of its highest medal ([superbounce.md](superbounce.md#medals)) |

Charging, primed, the loss and the landing go to user 0, the first local player's,
whose state the mode tracks. The first state after the mode comes on is
not news: what it already holds is not sent (`superbounce_hud::Sent`).

With the mode off every element is hidden. The tutorial's step machine
watches primed, the nearest bounce spot in metres and a bounce landing
newer than the last it saw (`superbounce_hud::watch`).

What shows in the game: the pill and its icon while primed (`Inactive`
not drawn), the arming bar through the recipe, the charge meter while
primed, the prompt verbs, the navpoint (the game's arrow and marker, no
number) and the tutorial's training text, and in the notification list
"Charging...", "Primed!", a loss's reason, a landing that did not bounce
and the medals.
`superbounce.state` has a loss's reason as `lost_reason` and the medals
as `medals`.

### The tutorial

The four steps are `Training` texts: the game's own training box, two
centred lines of font 6 in the middle of the HUD frame, drawn by the
game with its frame bitmaps and its training colour. There are no
diagrams: a retail training prompt has none.

| Step | Text | Moves on when |
|---|---|---|
| 1 | Crouch, walk in under a lip, / let go, then walk out. | primed |
| 2 | You are primed. / Do not crouch again. | a bounce spot within 8 m |
| 3 | Follow the navpoint. Jump from the / Jump mark onto the Land mark. | a bounce, or 20 s |
| 4 | Jump or fall onto the strip. / Fall Assist (Controls) can steer. | a bounce, or 15 s |

No step asks for a button: the fall assist is opt in
([superbounce.md](superbounce.md#hold-x-the-fall-assist)), and step 4
names its Controls row once.

```text
                 x 163                              x 477
          y 218  +----------------------------------+
                 |   Crouch, walk in under a lip,    |
                 |      let go, then walk out.       |
          y 262  +----------------------------------+
                          (HUD frame centre 320, 240)
```

The box is the text's size plus 5 each side and 1 above and below, plus
the frame art (S: decoding 3 gives its outer edge). The widest first
line of the earlier wording was 304 pixels in font 6 (V); the steps are
no longer than it, inside the frame's 544. The element's `longest` is
step 3, the widest line (its unit test checks that); which
step is widest in font 6 is S until the disc's checks run. The tutorial
is fullscreen only. The step machine is
`hud_kit_core::huds::superbounce::Tutorial` (its thresholds);
`superbounce_hud` runs it once a game tick on game seconds (ticks over
30, `tutorial_seconds`), so 20 s is 600 ticks at every frame rate, and
sets the `superbounce.tutorial_seen`
setting when it closes (`H2_SUPERBOUNCE_TUTORIAL=1` shows it anyway). A
game training prompt of its own takes the box while it shows; the
tutorial's step waits.

## Interfaces

- A feature calls `hud_kit::register(describe) -> Handle`, then
  `Handle::publish(values)` and `Handle::navpoint(Option<point>)`. A
  `Prompt` or `Training` element's shown text is the channel's text.
- Guest points hooked: `HUD_WIDGET_GATE`, `HUD_WIDGET_INPUTS`,
  `HUD_WIDGET_STRING`, `HUD_WIDGET_SPRITE`, `HUD_WIDGET_EFFECTS`,
  `HUD_INTERACTION_VISIBLE`,
  `GAME_ACTIVATE`, `GAME_TICK`, and the string lookup `0x001a0180`
  through `community_variants`' override.
- Guest memory written: the two block fields of each player `nhdt` root,
  the scenario's point block field, the kit's two allocations, the first
  local player's prompt fields and navpoint list, the training id and
  show byte.
- Agent API: none yet (`hud.kit` is planned with slice 6's run checks).

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

Unverified until the lab: none of the runtime has run in a game. In
particular:

- That the kit's copied widget blocks draw as the retail ones do, and
  the kit's after them, on the D3D path and the native HUD pass.
- The gate's, inputs', string's and sprite's return addresses as the
  only calls the hooks answer, and that narrowing the sprite's bounds at
  `HUD_WIDGET_SPRITE` narrows the quad (the quad's size taken from the
  bounds times the image's size).
- That `player_globals +8` is the count of local views the HUD's screen
  mode follows.
- That writing the prompt fields at `HUD_INTERACTION_VISIBLE`'s
  interaction call shows the verb and hides it again when the kit stops,
  and that U+E110 is redrawn as the bound crouch key.
- The navpoint: that `[0x004e0350] +0x1e4` is the block a kind-0 slot
  indexes, that a point appended to it harms nothing that walks the
  block, that the list is not rebuilt over our slot each tick, and the
  slot's `+0` and height.
- That the template resolution at the match start (one map open, the
  strip's image decoded) is fast enough there.
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
- The exact spacing of a navpoint's arrow and marker (the marker's
  offset is read as the arrow's length plus one and more not read), the
  outline's art and place, and whether the HUD shader multiplies the
  green arrow's texels by the HUD colour or takes only their alpha.
- The pill's icon: `^` reads as a stray caret. The choices are a flipped
  `hud_waypoints` chevron or `default_arrow` beside the pill (not in it),
  a pill with no icon, or new art (not the kit's to make).
- Where the "x" of a retail pill starts (about x 22, from the disc
  bitmap by eye) and so whether 20 pixels of body is clear of it in
  every sequence, and which retail widget draws an unselected grenade's
  pill (the `Inactive` look greys the lit one instead).
- The charge meter: that a cropped strip fills as the remap says (the
  blue ramp between the crop's columns is taken as even), what the
  shader draws of the unlit part, and the strip's sequence in split
  screen.
- The arming bar: that the game calls `0x0022bcaf` for a widget with an
  empty effects block (else the plain ending, runtime step 4), that
  `[ebp]` is the factor across and `[ebp+0x34]` down and that they scale
  about the registration point, that the factors are set afresh for
  each widget, that word D is the gate's third stack argument and
  `0x400` its arming bit in our build, and the shader's on and off
  opacities. The retail bar's input is `0x43` on the disc, not
  Assembly's `0x44`.
- That the still-tick count matches the body's sleep: prime with
  `havok.pair.set`, stand still, and read `havok.agents` each tick until
  the pair goes, with `superbounce.state`'s charge beside it.
- That the (crouch) action token exists for the prompt line as the
  pickup prompt's (X) does.
- The frame cost.
- The preview on the disc: the tracker's show bit `source::retail_words`
  takes, that the game sets it, and that the moved navpoint sample is
  why the split-screen checks passed (the failing findings were not
  captured).
- Where a notification is drawn against the first message line (the
  preview puts it below the line's five-line box).
- The notifications: that a guest call to `0x0024cbee` at the tick's top
  draws as the game-event caller's does, that running the tick again
  from the hook (as `skulls_custom` does) leaves the frame as it was,
  that the hook's place in the first group passes the plan beside the
  other features there, and that a mutating tick hook in this
  presentation-only feature costs a film or a snapshot nothing (the
  insert writes only the user's HUD message state). The medal line: that a medal glyph keeps its
  own colours in a notification (the retail lines say it does) and
  fades with the line, and where a fifth line in four slots goes
  (decoding 6). That the preview's text draw keeps a coloured glyph's
  colours.
- The sheet's retail states draw the player's `nhdt` alone: no weapon's
  HUD is drawn with it, so the retail check does not see the weapon
  panels or the reticle's art (the reticle is a reserved rectangle).

## Build plan

Each slice is code and tests only; the coordinator compiles and runs.

1. **The run checks.** `hud.kit` in the agent crate and
   `docs/runtime/agent-api.md`: the registered HUDs, and per element the
   id, kind, widget address, text, fill and whether the gate last drew
   it. Then, on the lab Mac, each item of "Not verified" with `hud.kit`
   and `render.frame` against the preview's JSON, at 4:3 and 16:9,
   Pinned and Centered, fullscreen and split; the widget cap and the
   text shader's colour.
2. **Styles and the pointer.** The colour hook (`HUD_TEXT_SETUP`,
   `HUD_SHADER_QUAD`) and the flash (move `strength` into
   `hud-kit-core` and have `h1_hud_text/flash.rs` call it), so `Dim`,
   `Inactive` and `Flash` draw; the corner turn. Before coding, decodings
   4 and 7. Tests: a `Dim` text's alpha; a flash at the record's phases;
   four corners turned by a quarter turn.
3. **The navpoint's number.** The number drawn at the projected point
   after `0x0024e59f` returns (decoding 8). Tests on `FlatMemory`: the
   number's place.
4. **Preview fidelity.** Decodings 3 and 5, so the preview draws the
   training box's frame and a retail widget's own flash; `h2 hud
   compare` against a `render.frame` capture, as the UI builder's
   fidelity loop.

## Related

- [superbounce.md](superbounce.md), "The superbounce game type": the first user.
- [hud-editor.md](../editor/hud-editor.md): the `nhdt` format, placement rules and the renderer the preview uses.
- [health.md](../halo1/health.md), [hud.md](../halo1/hud.md), [text.md](../halo1/text.md): the Halo 1 HUD, which adds widgets and message text the same way.
- [ui-screens.md](ui-screens.md), [ui-builder.md](ui-builder.md): the UI kit.
- [fonts.md](../formats/fonts.md), [button-icons.md](../input/button-icons.md), [high-resolution.md](../graphics/high-resolution.md).
