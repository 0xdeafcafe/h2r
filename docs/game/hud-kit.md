# The HUD kit: HUD elements of ours, drawn by Halo 2's own HUD

Status: In progress. Built: the model, the compiler, the preview and the checks, the widget runtime (install, gate, inputs, string, crop), the prompt, message (notification), training, banner and navpoint channels, the superbounce HUD on them with its medals ("The medals"), and the Firefight HUD ([firefight.md](firefight.md), "The HUD"). Not built: the colour hook and the flash, the pointer's turn, and the `hud.kit` agent method (build plan). Nothing of the runtime has been run in a game ("Not verified").
Code: `crates/rewrites/hud-kit-core` (the model, the HUDs, the templates, the record compiler), `crates/tools/hud-kit` (the preview, the checks), `crates/extensions/game/src/hud_kit.rs` (the runtime), `crates/extensions/game/src/superbounce_hud.rs` (the superbounce HUD's feed), `crates/apps/h2-tools/src/hud_cmd.rs` (`h2 hud`). Tests: `crates/rewrites/hud-kit-core/src/tests.rs`, `crates/tools/hud-kit/src/tests.rs`, `crates/tools/hud-kit/tests/disc.rs`, `crates/extensions/game/src/hud_kit_tests.rs`, `crates/extensions/game/src/superbounce_hud_tests.rs`.
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
   a timed text at `+0x454` (start `+0x4d8`, duration `+0x4dc`, both in
   game ticks; drawn while the tick is from the start to the start plus
   the duration, its alpha falling over it, `0x0024d58a`..`0x0024d5cf`)
   and the text of `0x0015f120` (`0x0024d51f`..`0x0024d6bb`). Its colour
   is the user's HUD text colour (`0x0013927e`, `0x0024d557`).

Who writes the state (E):

| Routine | What it does |
|---|---|
| `0x0024c9e9` (user, key, sound) | looks the key up in `hudg +0x3fc` (`0x0013925f`) and shows it as the prompt through `0x0024ca1d`. Its one caller is the interaction code at `0x00191e42` (pick up, swap) |
| `0x0024ca1d` (EAX user, ECX text, key, sound) | copies the text to `+0x220`, sets `+0x444`, clears `+0x440`; a new key (`+0x448`) restarts the fade at `+0x446` and plays the sound |
| `0x0024c98c` (EAX user, shown) | shows or hides the prompt each frame, from `0x00191976` |
| `0x0024cbbf` (EDI string id, user) | queues a notification; called for an item's pickup text (`0x00191fcf`) and by `0x001df40d`. It looks the id up (`0x0013925f`) and calls `0x0024cbee` |
| `0x0024cbee` (EAX user, text) | queues a notification from UTF-16 text: takes a slot (`0x0024ccdb`), copies up to 62 characters and a zero to `+4` (through `0x0013fac0` with 63, `0x0024cc24`), sets the start tick, `+0x82`, clears `+0x84` and `+0x85`, gives `+0x83` the next value of the byte at `[0x005023f4] + 0x1385`, and clears the user's `+0x44e`. Ten callers; the game-event one is `0x0019e9c0` (below) |
| `0x0024caac` (EAX user, singular, plural, count) | the same for a line that counts: adds the count to `+0x86`, sets `+0x84`, and shows the plural once the count is over 1 |
| `0x0024ccdb` (state, singular, plural) | the slot: the first of the four that is not active; with both texts, an active counting entry already showing that text (the repeat); else the active entry with the oldest start tick, which it ends |
| `0x0024cc73` (EAX user, ECX text, ticks) | sets the timed centred text: copies the text to `+0x454` through `0x0013fac0` with 63 (62 units and a zero), the game tick (`[0x00510c54] +8`) to `+0x450` and `+0x4d8`, 1 to `+0x4d2`, the next value of the byte at `[0x005023f4] + 0x1385` to `+0x4d3` (stepping it), and the ticks, its one stack argument, to `+0x4dc`; `ret 4` |

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
| A team's colour | the loaded map's `matg +0x16c` is the datum of its `mulg`; the `mulg`'s first `universal` record (block at `+0`, element address at `+4`) has the team colour block at `+0x10`: one `real_rgb_color` of 12 bytes per team, 8 teams. `0x0007f720` is the game's lookup of it (`0x0007f790` gives it a player's slot 0 in a team game). `pad_lightbar::team_rgb` reads it; `Style::Team(n)` draws in it | E `0x0007f720` ([objects.md](../halo1/objects.md), "Change colours"); Code `crates/extensions/game/src/pad_lightbar.rs` (`team_palette`, synthetic-guest tests) |
| Widget art | the widget's shader: a meter's fill and a panel's tint are shader colours | [health.md](../halo1/health.md) |
| Text widget colour | the text draw hands `0x0013edb0` (`HUD_TEXT_SETUP`) the static colour `[0x004686d4]` points at, the one the message draw copies as its shadow, and the static `[0x004686cc]` as the shadow; `0x0013edb0` copies the colour to `0x004e73c4`..`0x004e73d0` and sets `0x004e73c0`. So the widget's own colour is not set at that call: the HUD material it binds (`0x00131bb0` on `0x004e6950`, `0x0022aabb`) gives it (S) | E `0x0022aa5b`..`0x0022aa6e`, `0x0013edcb`..`0x0013edf7`; decoding needed (4) |

A record's colour over its flash is `hud_kit_core::flash::colour`
(`crates/rewrites/hud-kit-core/src/flash.rs`), its curve `strength`:
Halo 1's, which Halo 2's help text is drawn with on converted maps
(`h1_hud_text/flash.rs` calls it). Where each `Pulse`'s record starts
comes from the `hudg` definition (`flash::record_at`), and a test reads
the six fields after it at the offsets the curve reads. That Halo 2's own
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
4. **A text widget's colour.** The colour `0x0022aa16` hands
   `0x0013edb0` is a static one ("Colours"), so a colour written there
   does not flash a kit text. Not read: how the HUD material bound at
   `0x0022aabb` colours the text (the shader's colour, a material
   input, or the static colour times either), and so where a kit text's
   colour can be set per widget.
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
    /// Pixels it may sit outside the HUD frame, in the screen's margin
    /// (the `frame` check); 0 for all but the charge segments.
    pub outside: i16,
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
    /// A picture of ours that fills with `Value::fill` in steps: a text
    /// widget whose one character is the picture for the fill ("Gauges").
    Gauge { glyphs: Glyphs },
    /// The channels (below).
    Message { longest: &'static str },
    Prompt { longest: &'static str },
    Training { longest: &'static str },
    Navpoint { longest: &'static str },
    WorldMark { shape: Shape, longest: &'static str },
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
    /// What the half and quarter offsets are from, when not `origin`.
    pub split_origin: Option<Origin>,
}

/// What the offsets are from: the anchor's corner, or where the player's
/// grenade row puts a next pill or a next count (`templates::GrenadeRow`).
/// A place can be from its anchor fullscreen and from the row in split
/// screen (`Place::split_from`); a widget has one anchor, so the place's
/// is then the row's (`records::placed`).
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
/// A gauge's pictures: `steps + 1` characters of ours in the fonts, from
/// `first`, the empty one, to the full one, each `size` pixels.
pub struct Glyphs { pub first: u16, pub steps: u8, pub size: (i16, i16) }
pub enum Style { Normal, Dim, Inactive, Flash(Pulse), Go, Team(u8) } // Go, Team: a world mark's alone
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
    /// The sample poses of the HUD's world marks in a state, for the
    /// preview and the checks.
    fn world(&self, _state: &str) -> Vec<(&'static str, Pose)> { Vec::new() }
}
```

`records::compile(&Hud, &Templates) -> Result<Compiled, Error>` turns
the elements into widget records: `0x64` bytes per meter, icon and
pointer and `0x54` per text and gauge, with the element id of each, bitmaps first
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
a text's string is the template's, and the string hook gives the text,
for a gauge the character of its picture (`records::Widget::text`).
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

Six more elements are not widgets. Five are channels into the game's
message, training and navpoint parts, one text each, and the sixth is a
world mark, drawn by the window ("World marks"):

| Element | What the player sees | Through |
|---|---|---|
| `Message` | a notification in the pickup list, top left, that fades | the notification queue (`0x0024cbbf`) |
| `Prompt` | the prompt line, top left, as "Hold X to pick up" | the user's prompt fields (`+0x220`, `+0x440`, `+0x444`), while the game shows no prompt of its own |
| `Training` | the framed box in the middle of the screen | the scripted training text (`+0x6c0`, `+0x6c4`) |
| `Banner` | one line centred above the frame's bottom, for three seconds, fading | each local user's timed centred text (`+0x454`, `+0x4d8`, `+0x4dc`), when the text changes |
| `Navpoint` | the game's arrow pointing down at a point of the world and its waypoint marker above | the navpoint list (`0x0024da7d`, below in "The navpoint") |
| `WorldMark` | lines lying on the level's geometry, a label over them | the window's world-line pass (`hud_kit_world`, below in "World marks") |

Their texts are UTF-16 with the font's button glyphs or action tokens.
A channel, like a widget, shows only in the screen modes its place has:
`Element::channel` gives it `Place::GAME`, fullscreen only.

A notification is an event, not a state: two bounces are two lines with
the same text. So a `Message` is sent with `Handle::notify(user, text)`,
once per line, for a local user, and not by `publish`. Its element is in
the `Hud` for the checks and the preview, whose states give it a text
(`Value::text`); `longest` is the widest line it is ever sent. A text
of at most 62 UTF-16 units and one line is sent whole; a longer one, or
one with a line break, is refused by the `prompt` check and by
`Handle::notify` (`hud_kit_core::model::message_fault`; E `0x0024cbee`
keeps 62). A sender with no HUD of its own, such as a rules script's `message`
([rules-world-api.md](rules-world-api.md#the-show-channel)), queues the
same line through `hud_kit::notify(user, text)`.

A `Banner` is a state, unlike a notification: the HUD publishes its
text while the line should show (a wave's number while the wave is
being placed), and the runtime writes the line each time the shown text
changes. The game draws it for `model::BANNER_TICKS` (90, three seconds;
Design), fading, and then it is gone; the same text published again
keeps the line it started, and a hidden value lets it run out. A text of
at most 62 UTF-16 units (`model::BANNER_UNITS`: the game's copy keeps
one fewer than the 63 it is given) and one line is written whole, as a notification's; a
longer one, or a line break, is refused by the `prompt` check and not
written (`model::banner_fault`). The preview draws it centred across the
frame with its top 40 pixels above the frame's bottom
(`preview::BANNER_UP`), in the notifications' colour.

### World marks

A world mark is an element of a HUD that lies on the level's geometry
and not on the screen: a landing marker, a box round a place to land in.
It is in the `Hud` with the widgets and the channels, so a state shows
or hides it, styles it and gives it a label like any other element, and
the checks and the preview cover it. `hud_kit_core::world` has its
shapes and its pose:

```rust
pub enum Shape { Pipper, Bracket, Arrow, Ticks, Bar, Line }

pub struct Pose {
    pub at: Vec3,          // the point on the geometry
    pub along: [f32; 2],   // the heading, flat: a bracket's long side, an arrow's way
    pub half: [f32; 2],    // the size, world units
    pub strength: f32,     // 0 to 1: the lines' alpha, for a mark that fades
}
```

| Shape | Lines | `half` |
|---|---|---|
| `Pipper` | a ring of 12 segments with a cross at its centre | the ring's radius, unused |
| `Bracket` | the four corner brackets of a box, each arm half the box's shorter half | along the heading, across it |
| `Arrow` | a shaft and a head's two wings, pointing along the heading | half the length, the wings' reach |
| `Ticks` | three dashes from the point along the heading | the run they take, unused |
| `Bar` | a line across the heading through the point; with a band, its two ends as lines half as long either side along the heading | the band's half along the heading (0: none), the line's half across |
| `Line` | one segment through the point along the heading, an edge of an outline | half its length, unused |

`world::edges(shape, &pose)` gives the line segments in world space,
flat and 0.02 above the pose's point. The game and the preview draw the
same segments: the game through the window's world-line pass, from each
view's camera, under the HUD and with no depth test
([superbounce.md](superbounce.md#the-superbounce-game-type), the pass
the mode's own marks use); the preview projected onto its canvas ("The
preview and the checks").

A mark is drawn in the user's HUD colour. `Dim` halves its alpha and
`Inactive` greys it, as for a widget, and the pose's strength scales the
alpha again. `Team(n)` draws it in the colour the game gives team `n`
("Colours", "A team's colour"): read from the loaded map's `mulg` each
present, and only while a shown mark has the style
(`hud_kit_world::colour`, `Teams`). Where the map has no colour for the
team the mark is in the user's HUD colour. The preview has no map to read
it from and draws the stand-ins `preview::PREVIEW_TEAMS` (**S**: not the
game's, apart enough to tell on the sheet). The `world` check refuses
`Team` on any other kind, and a team of `world::TEAMS` (8) or more. One
more style is a world mark's alone: `Go`, a
green (`world::GO_GREEN`, [0.35, 0.95, 0.45]) for a mark that says "this
will work": the approach guide's "this fall will bounce"
([superbounce.md](superbounce.md#the-approach-guide)). The `world` check refuses `Go` on any other kind. The
value's text is the mark's label, centred over its point in the game, and in the preview below the lines (the guide's drift arrow is on the other side), in the HUD font
(`model::WORLD_FONT`); `longest` is the widest label any state gives it.

A pose is the runtime's, each tick, as a navpoint's point is:
`Handle::world_mark(id, Option<Pose>)`. For the preview and the checks a
HUD gives sample poses per state, `Describe::world(state)`: world units
in a scene of the state's own. A mark shows only in the screen modes its
place has, like a channel: `Element::channel` gives it `Place::GAME`,
fullscreen only.

`crates/extensions/game/src/hud_kit_world.rs` is the game's side: a
world-line source (`hud_kit_world::register`, windowed hosts) that each
present takes the marks to draw now (`hud_kit::world_marks_now(mode)`:
the world mark elements of the registered HUDs whose value is shown,
that have a pose and whose place has the screen mode) and gives the
pass their lines (`hud_kit_world::lines`: 2.5 pixels wide at 1080p with
a glow, the label 0.25 under the point). One label a seam: of the marks within 0.75 of each other only the first with a label keeps it. A label that projects into the HUD's top-left or bottom-left block is left out (`hud_kit_world::HUD_BLOCKS`, `label_under_hud`, asked by the pass that projects the anchors). With no pose set it costs one
atomic load (`hud_kit::any_world_marks`). Nothing is written to guest
memory.

### The runtime (`crates/extensions/game/src/hud_kit.rs`)

A feature registers its `Describe` once (`hud_kit::register`, which
returns a `Handle`), then publishes a `Values` whenever its state changes
(`Handle::publish`), and, for a split-screen player whose widgets differ,
that local user's own (`Handle::publish_for(user, Some(values))`, `None`
to draw the shared ones again), the world point of each of its navpoints
(`Handle::navpoint` for its first, `Handle::navpoint_at(id, point)` for
any) and the pose of each of its world marks
(`Handle::world_mark(id, pose)`, "World marks"). The feature `hud-kit`
does the rest:

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
   A live edit of a retail record (the agent's `edit.*`) reaches the
   copy the game draws: the hot-edit glue's tag-write notice calls
   `hud_kit::tag_written`, and `recopy` copies each retail block the
   edit overlaps into its copy again
   ([realtime-edits.md](../editor/realtime-edits.md), "Realtime
   coverage"; test `an_edited_retail_record_is_copied_again`).
2. **Gate** (`HUD_WIDGET_GATE`, before, at the widget loop's two calls):
   a widget in the kit's memory is drawn while its value is shown, in a
   style the runtime draws, in a screen mode its place has (one local
   view fullscreen, two half, more quarter: `player_globals +8`), and
   none of its element's `hide_when_d` bits is set in the game's word D,
   the gate's third stack argument (`[esp+12]`, its low 16 bits). The
   retail gate is skipped for it; a retail widget costs two loads.
   The gate, the inputs, the string and the scale read the values of
   the local user whose HUD is being drawn: `[0x004b9ed8]`, which
   `UI_HUD_DRAW` reads and hands to its per-user draws (E `0x001917ed`,
   `0x0019186a`; `hud_kit::draw_user`): that user's own values if it has
   them, else the shared ones (`hud_kit::values_for`). The channels read
   the shared values alone.
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
   the HUD material's (`0x004e6980`), as the text draw does. A gauge's
   text is the one character of its picture for the value's fill as it
   draws now (`records::Widget::text`), so an eased fill sweeps through
   the steps at any frame rate.
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
     the text, each `\n` written `\r\n` as in the game's own strings (a
     lone `\n` draws no break and no space; `hud_kit::write_training`,
     `hud_kit::text`, `hud_kit::game_line_breaks`).
   - `Navpoint`: at install the scenario's point block, the tag block
     at `scnr +0x1e0` (count, then the address at `+0x1e4`, which is what
     the draw reads, E `0x0024db5e`; `0x38`-byte points), is copied into
     the kit's second allocation with four points after it
     (`KIT_POINTS`, as many as a player's navpoint list has slots), and
     the scenario points at the copy (`hud_kit::install_point`, kept and
     put back as the widget blocks are, and only while the loaded
     scenario is the one it changed). The kit's navpoints are the
     `Navpoint` elements of the registered HUDs, in registration order,
     the first four (`hud_kit::navpoints_now`); an element keeps its
     place in that list whatever shows, so it keeps its point. Each tick
     each point is set to its element's navpoint and one slot of the
     first local player's navpoint list is kept on it: the slot already
     that point's, else the first the game's draw passes (`+0` or `+8`
     -1, or kind 15; the draw walks all four slots and no slot ends the
     list, E `0x0024dc21`..`0x0024dc3e`) (`hud_kit::write_navpoint_n`).
     The game's own slots are not touched, so a navpoint of the kit's
     with no slot free is not drawn that tick. Without a point, or once
     the loaded scenario no longer points at the copy, the slot is
     skipped (kind 15): the draw does not check a kind-0 slot's index
     against the block's count. The game draws its own arrow and marker
     there; no number is drawn with it.
   - `Banner`: the first shown banner of the registered HUDs whose place
     has the screen mode (`hud_kit::banner_of`), compared with the last
     tick's (`hud_kit::banner_due`): a new text is written into each
     local user's timed line (users 0 to the local view count less one,
     `hud_kit::local_users`) with the fields as `0x0024cc73` leaves them
     and a duration of `BANNER_TICKS` (`hud_kit::write_banner`). With no
     game time (`[0x00510c54]`) or no HUD state yet nothing is written.
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
(slice 4). The draw hooks are `context_only` and read one shared table. The feature is presentation-only and
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

Something drawn over the tracker that no retail widget can draw (a
polygon, a map) is not a kit element. The superbounce bounce view's
blips and map are host marks in the image's HUD space
(`gfx::world_lines::HudMark`), placed at `TRACKER` from the HUD frame's
bottom-left corner and moved by the host's fit of the bottom-left group
(`render_aspect::hud_bottom_left_fit`)
([superbounce.md](superbounce.md#the-bounce-view)).

### Direction arrows

A direction on the HUD is a character. Every HUD font of a served map
carries eight arrows at U+E2B0 to U+E2B7, the first pointing up and each
the last turned 45 degrees clockwise (`crates/formats/font/src/arrows.rs`:
`glyph_for(angle)`, the angle in radians clockwise from up). They are
drawn in code, white, as tall as the font's capitals, and added where the
gear and the medal glyphs are (`bot_mark::GearRewrite` at load;
`hud-edit`'s `Fonts::as_served` for the preview and the checks), so a
text widget draws one in the text's colour as it draws a letter. The
disc's fonts have no arrows of their own.

A `Pointer` element is not used for a direction: the runtime does not
turn a widget.

### Gauges

A meter is a retail bitmap filled by its shader, so it is a rectangle:
a strip, or a crop of one. A shape no retail bitmap has, as an arc round
the motion tracker, is a `Gauge`: a text widget in font byte 0
(`GAUGE_FONT`, engine font 6) whose string is one character, the picture
of the shape at the value's fill. The pictures are drawn in code and
served in every font file by the font rewrite, as the direction arrows
are, so the game's text draw draws them in the HUD's text colour, in the
game's own batches, hidden with the HUD.

| | A bitmap of ours in the map | A host mark (`gfx::world_lines::HudMark`) | Glyphs of ours in the fonts |
|---|---|---|---|
| Gets into the game by | a new `bitm` in every level's map: the map decomposed and rebuilt at load (`tags::rgba_bitmap`, as mainmenu's level pictures are), and the level maps are over the rewrite's size limit (`vfs::MAX_REWRITE_BYTES`) | the window's own pass | the font rewrite the arrows and the medals use (`bot_mark::GearRewrite`, `font::patch`) |
| Fill | the meter shader, by a blue ramp along the shape | drawn per frame | a picture per step |
| Colour, hiding with the HUD, films, the headless host | the game's | not the game's ("Where the kit draws") | the game's |

A `Glyphs` names the run: `first`, the empty picture's UTF-16 unit, then
one unit a step to the full one at `first + steps`, each `size` pixels.
`Glyphs::step` picks the nearest step for a fill, and
`records::Widget::text` is the string for a value. The fill keeps its
`Motion`: the runtime asks the curve first and picks the step after.
A picture has no offset across and sits as far above the baseline as the
font's ascent, so its top-left corner is the widget's point in any font
(`font::ring::replacements`), and the widget is justified from its
start.

The pictures the kit has are the charge ring's, `font::ring`: nine arcs,
seven steps each, at U+E2C0 to U+E2FE, after the arrows ("The charge
segments").

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
colour, turn), a gauge's picture for its fill at its widget's point
(`preview::gauge`, from `Glyphs::picture`: the pixels the fonts are
served with, so the synthetic source draws it too; a flash as `Normal`,
as a meter's), then the channels:
the prompt line and a notification at
their points in font 6 (5 in split screen), and the training box around
its text. The player `nhdt` is the one a biped's HUD interfaces name,
`ui\hud\masterchief` when the map has it. The retail HUD is drawn with
the state gate on at the source's retail words (`source::retail_words`):
word A `0x1000`, the retail shield meter's show word, plus the first
show bit of the widget the HUD editor places on the tracker's rim
(`rects::TRACKER`) that lets it through the gate (S: that the game sets
that bit whenever it draws the tracker; decoding 5). A HUD with no
widget there keeps `0x1000` alone. Every report notes which bit was
taken. The preview draws a navpoint at a sample point, each navpoint of
a HUD at one of its own (`preview::NAVPOINT_SAMPLES`, by its order among
the HUD's navpoints), so two shown at once draw apart; in a split
frame, whose stack would leave the frame's top at the sample, it moves
the point down until the stack fits (S: that the game keeps a navpoint
inside its waypoint area, the safe bounds less `hudg +0xec`..`+0xf8`).
A world mark is drawn from its HUD's sample pose for the state
(`Describe::world`): the scene's origin is on the floor ahead, 0.22 of
the frame's height under its centre (`preview::WORLD_SAMPLE`), a world
unit is 130 HUD pixels across in a fullscreen frame, and the view is
oblique, +X to the right and +Y up the screen at half scale
(`preview::project`). It is the mark's lines in a sample view, not the
game's camera: it shows the shapes, the styles and the label, and that
they stay clear of the rest.

`preview` writes `<hud>-<state>-<mode>.png` and a `.json` with every
element's id, kind, rectangle, the rectangle it may take in any state
(`reserved`: a text's at its `longest`), text, style, fill, angle, font,
whether it is drawn and why not, a world mark's shape, pose, segment
count and bounds (`world`), and the findings of that state and
mode; by default every state, fullscreen. `preview` fails on a glyphs
or drawn finding, and, from the disc, when the glyph check could not
run; its other findings are printed. `sheet` writes every HUD x
state x mode, `sheet.png` (half-size tiles, a column per mode),
`sheet.json` (where each tile is) and `checks.json`, and fails on a
finding. `Source::disc` reads the map (default `lockout`), the fonts and
the XBE's colour constants from the user's disc (`--disc`, else
`$H2_DISC`); `Source::synthetic` draws an invented map whose tracker is
a flat disc and whose shield meter, grenade pills and counts are flat
boxes on the retail rectangles and whose `hudg` has an arrow and a marker,
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
| glyphs | every character of every `longest` and state text, and every picture of a gauge, is in the element's font (a channel's in fonts 6 and 5, a navpoint's number and a gauge's pictures in font 6): not record 0, not the record U+FFFF reads, and no copy of either's bitmap (`font::Font::has_glyph`; MSLCD-14 maps `.`, `-` and `~` to a copy of its box) |
| frame | every rectangle, reserved or drawn, is inside the HUD frame and the screen; an element may sit `FRAME_SLACK` (1) pixel outside the frame, as the tracker and the shield meter do (x 47, the frame's left 48), or its own `Element::outside` when that is more (the charge segments, left of and under the tracker) |
| retail | no widget element's opaque pixels meet a retail widget's at any `HudState` of the sheet (plain, overshield, plasma grenades, zoomed), and no widget element's rectangle meets the reserved rectangles (`PROMPT_LINE`, `TRAINING_BOX`, `H1_HEALTH`, `RETICLE`); the channels are the game's own text and are not checked against them |
| own | no two elements of any registered HUDs overlap, by their rectangles and their pixels, texts included; two channels are the game's, one at a time, and are not compared, nor is an element with the one it is `over`; two HUDs one of which names the other in `Describe::never_with` (a monitor's and a biped's, which are never shown together) are not compared with each other |
| one layout | every state has a value for every element and for nothing else, and every element id is used once |
| templates | every `Art` and the text template resolve on the map (both sources; on every map of the disc in the disc test) |
| prompt | a `Message` or `Training` text, and its `longest`, has at most two lines, a `Prompt` or `Banner` one (the game's prompt line draws no line break; the banner is one centred line), and fits the frame's width (less the training box's margin); a `Message` or a `Banner` has at most 62 UTF-16 units |
| modes | an element with no half or quarter offset is not drawn there (the JSON says why), a text has no font there, and the widgets of one HUD show in the same modes, so a block is never drawn in part |
| drawn | a text or channel shown in a state, in a mode it shows in, puts opaque pixels down (alpha over 64): a transparent colour or a missing shader is a finding. A gauge shown puts pixels down: one whose picture no code of ours draws, or draws at another size, is a finding |
| world | every sample pose is for a world mark of the HUD and has no fault (a number that is not finite, a negative size, a strength outside 0 to 1); a world mark shown in a state has a sample pose there, draws lines and keeps them inside the HUD frame; and `Go` and `Team` are on world marks alone, `Team` of a team the game has. A world mark's lines are not compared with the widgets' (`own`): they are under the HUD, on the geometry |

The synthetic source has no fonts, so it skips the glyph check and says
so, and its retail check is against the flat disc and boxes, not the disc's art.

The agent API gets one read method, `hud.kit`: the registered HUDs, and
per element the id, kind, widget address, placed rectangle, text, fill
and whether the gate last drew it. It is how a run is checked against
the preview's JSON, with `render.frame` for the pixels.

## The superbounce HUD on the kit

`crates/rewrites/hud-kit-core/src/huds/superbounce.rs` describes it:
the elements, `View` (what the HUD shows of `superbounce_mode`'s state),
`values(&View)`, the preview's states and the tutorial's step machine.
It is multiplayer only (`Describe::multiplayer_only`): the disc test
skips campaign maps for it by that reason. It is shown to the first
local player. It is made only of things Halo 2 draws already: Assault's
arming bar, the prompt line, the notification list, a navpoint and the
training box. The priming indicator, the charge segments round the
motion tracker, is the readout's ("The charge segments"). Launch and
drop are not on the HUD: they are on the strips' labels in the world and
in the agent API.

```sh
h2 hud preview superbounce --state all --out DIR      # the disc's lockout
h2 hud preview superbounce --state all --mode all --out DIR
h2 hud preview superbounce --state all --synthetic --out DIR
```

### The pill

The kit can put a pill of its own in the grenade row. No registered HUD
has one: the superbounce HUD's priming indicator is the readout's
charge segments. The pieces are held under test by `huds::pill_block`
(`crates/rewrites/hud-kit-core/src/huds/pill_block.rs`), a HUD of the
kit's own tests that is not registered (`huds::all`) and is never
compiled into a map.

| Element | Kind | Place | Art |
|---|---|---|---|
| `pb.pill` | Icon | fullscreen (90, 340) from the top-left anchor (`PILL_FULL`): screen (138, 376), beside the motion tracker; half and quarter `Origin::NextGrenadePill` | `Art::GrenadePill(Body)`: the frag pill's first 20 pixels |
| `pb.pill.end` | Icon | the pill's plus (20, 0), every mode | `Art::GrenadePill(End)`: the frag pill's last 7 pixels |
| `pb.pill.icon` | Text, `Hud`, centred, over `pb.pill` | fullscreen the pill's plus (12, −1) (`ICON_AT`); half and quarter `Origin::NextGrenadeCount` | the text template (the frag count's shader and string); `^` in font 6 |
| `pb.charge` | Meter | the pill's plus (0, 21) in every mode, 2 pixels under it | `Art::StripMeterEnd(27)`, the battery strip's last 27 columns ("The charge meter") |

A widget has one anchor for its three offsets, and the grenade row's is
the top left, so a pill's fullscreen place is written from it
(`Place::split_from`).

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
step, (106, 0) from the anchor, and the next count is the frag count
plus the step, (118, -1) (`Origin::NextGrenadePill`,
`Origin::NextGrenadeCount`): a kit pill's place in split
screen. A row whose pill widgets are at two offsets, or whose sprite is
not one size in every mode, is a named error.

Each retail pill has its grenade baked in: "x" and the grenade's picture
from about x 22, the count at x 12. A third pill cut whole from the frag
pill would show a frag grenade. So the kit's pill is short: the frag
pill's first 20 pixels (its left end and body, `PillPart::Body`) with
its last 7 (the right end, `PillPart::End`) against them, 27 by 19,
screen x 138 to 165 fullscreen (x 154 to 181 in the grenade row), both
cropped from the frag pill's part of the panel (`GrenadeRow::crop`). The
icon sits where a pill's count does, pill x 12.

The block's states are the superbounce HUD's, by name
(`pill_block::values`), so the two styles of a bitmap no registered HUD
gives one are drawn by the preview:

| State | Pill and icon |
|---|---|
| primed | `Normal`: the pill as the frag pill draws, the icon in the text colour |
| not primed | `Inactive`: greyed (each texel its luma) at 0.7 alpha |
| scanning | `Dim`: half alpha |
| no strips, scan failed | hidden |

### The charge meter

A charge is shown twice. How far the player is through priming is on
the arming bar (below, "The arming bar") and on the readout's charge
segments; how long a primed charge has left, and the empty ring of a
loss, are on the segments ("The charge segments").

**What a charge is.** The engine has no charge amount. The pair word is
0 through the whole crouch and −3 from the first tick of the stand-up; it
does not step ([superbounce-spots.md](notes/superbounce-spots.md), "The
pair word around a forced crouch"). What does change by degrees is the
player's place in the recipe, and after it the time a still body has
before it sleeps and the pair is rebuilt at 0. The arming bar shows the
first and the segments both, and nothing invented:

| Phase | When (`superbounce_mode`'s state) | Arming bar | Segments lit, of 9 |
|---|---|---|---|
| hidden | not primed, and not crouched within 1.5 units of a crouch spot | hidden; run out if a step came before | none |
| near | crouched within 1.5 of a crouch spot, the standing capsule fits | 0, the empty bar | none |
| under | crouched, the standing capsule does not fit, crouch held | 1/3 | 3, the left side |
| held | crouch let go and the request still set: the forced crouch | 2/3 | 6, the left side and the bottom; the right side `Flash(Help)` |
| primed | the word is −3 | the ending, then hidden | 9 times 1 less the still ticks over 38; at half or under, `Flash(TimeWarning)` |
| lost | the word left −3 | the blink | none; the nine blink and go |

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
([health.md](../halo1/health.md)). The strip is 92 by 8.
`Art::StripMeterEnd(n)` is its last `n` columns, a crop of
`Art::StripMeter`'s template: 64 for the readout's fall bar, 9 for a
charge segment, 27 for the test pill block's strip.

| Field | Value |
|---|---|
| `input_1`..`input_4` | 0; the input hook answers `hud_input1` |
| the four show and four hide words | 0; the gate hook shows it |
| the three thresholds | 0 |
| `anchor` | the element's |
| `bitmap`, `shader` | the strip meter template's |
| `sequence_full`, `_half`, `_quarter` | the template's (0) |
| `offset_*` | the element's place |
| `registration_*` | (0, 0) |
| `effects` | empty |
| `special` | 0 (not 5: the shield meter's type takes the inputs over) |
| crop, in `Compiled` | columns 92 − `n` to 92 of 92 |

A cropped meter shows only part of the strip's blue ramp, so the hook
gives `hud_input1 = b0 + fill × (b1 − b0)`, with `b0` and `b1` the blue
of the crop's first and last columns, read from the strip's texels when
the templates resolve (on the disc the ramp runs from column 13 to the
right edge, V [health.md](../halo1/health.md)). The colours are the
shader's own, the ammunition counters'. Nothing of ours tints it.

**The prompts and messages.** The recipe's verbs go on the game's own
prompt line, with the bound button drawn by the game, and the results in
its notification list:

| Channel | Text | When |
|---|---|---|
| `Prompt` | Hold (crouch) and walk under | near, not primed, within 1.5 of a crouch spot |
| `Prompt` | Let go of (crouch) | under |
| `Prompt` | Walk out to stand up | held |
| `Prompt` | Keep moving or lose the charge | primed and still for 19 ticks or more, the fill at half or under |
| `Prompt` | Find a crouch spot to charge again | for 150 ticks after a loss, from 60 ticks after it (the reason's notification shows first), until a crouch step begins |
| `Prompt` | Press (X) to show ghost | with the `superbounce-ghost` feature on (off by default), looking at the chosen strip when it has a ghost run, and no recipe step is on the line (it replaces "Hold (crouch) and walk under"; [superbounce.md](superbounce.md#ghost-runs)); (B) on the Bumper Jumper layout |
| `Prompt` | Press (X) to stop ghost | while a ghost run plays, as above; (B) on Bumper Jumper |
| `Message` | Primed! | the word becomes −3 |
| `Message` | Charge lost: you stood still / Charge lost: you crouched / Charge lost | lost, by its reason ([superbounce.md](superbounce.md), "Charge"): the body slept, a crouch, another reason |
| `Message` (`sb.medal`) | a medal line (below, "The medals") | a bounce, or a Bounced |

(crouch) is U+E110, the left stick click's glyph, the default crouch
button ([button-icons.md](../input/button-icons.md)), which the button
swap redraws as the bound key's picture on the other input sets. U+E110 is one glyph, 38 by 19, that shows
both sticks: the left one lit and outlined, the right one faint (alpha
3 of 15), as every retail stick glyph does. The preview draws it as the
game does, so the prompt reads "Let go of" and a pair of sticks. A
prompt of the game's own wins the line.
A landing has no notification: a bounce's line is its medal, and why
another did not bounce is the readout's line
("The superbounce readout"). A recipe step has no notification: the arming bar
and the prompt say it. "Primed!" is the shape of Assault's "Bomb armed!" (V:
`multiplayer\multiplayer_globals`), in the same list.

### The arming bar

`sb.arm` is Assault's bomb arming bar, reused: how far the player is
through the recipe, then a short ending when primed. Halo 2 draws it
low in the middle while a bomb carrier arms; ours is the same widget's
art at the bottom left, right of the motion tracker and over the
readout's rows, so nothing of the superbounce HUD sits
mid-screen.

| Field | Value |
|---|---|
| element | `Element::meter("sb.arm", Art::Widget { hud: HudRef::Named("ui\\hud\\scoreboard"), name: "bomb_defusal" }, ..)`, `hidden_on_d(0x400)` |
| place | anchor top left, (160, 325) fullscreen (`ARM_FULL`, x 138 to 278, y 355 to 368: 8 above the readout's first row); the retail widget's is centre, (0, 110) (V, `RETAIL_ARM_FULL`); (272, 67) half and (129, 67) quarter (`ARM_SPLIT`, `ARM_QUARTER`, below); registration (0.5, 0.5) |
| art | the scoreboard `nhdt`'s bitmap widget `bomb_defusal`: `ui\hud\bitmaps\new_hud\bomb_meter`, a capsule 140 by 13 (sequence 0) and 86 by 9 (sequence 1, split screen), its blue rising 4 to 253 left to right; the shader `ui\hud\shaders\bomb_defusal_meter` on `widget_legacy_meter`, red `6d2323` to `cf6767` where the texel's blue is below `hud_input1`, white for the outline (V) |
| input | uncropped, so `hud_input1` is the fill itself |
| crop, ramp | none |

**Split screen.** Retail puts the bar 27 under the centre in half and
quarter screen. There it lies in the prompt line's box (the `retail`
check: x 250 to 390, y 141 to 154 in half screen), and unlike Assault's
carrier our player has a verb on the prompt line the whole time the bar
is up. Below the box the training box and the reticle take the rest of
the 204-high frame, so ours sits 35 above the centre (`ARM_SPLIT`, (272,
67) from the top left of a frame 544 wide): under the prompt's first
line (the frame's top plus 35) and clear of the navpoint's stack at its
sample, right of and above the centre. A quarter frame has no row free
between the navpoint's arrow at its sample and the prompt's first line,
and at the centre the bar's right end meets the arrow (the `overlap`
check, x 198 to 203), so in a quarter view the bar sits 7 to the left
(`ARM_QUARTER`, (129, 67) of a frame 272 wide).

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

The charge segments are all lit on the primed tick itself;
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

**The sound.** As a retail medal's line comes with its announcer sound
("Game events and medals"), each medal line sent is handed to
`medal_announcer::announce`, which queues the medal's clip behind the
game's own announcer: every medal but Bounced has one
([audio.md](../audio/audio.md#our-medals-announcer-lines)).

| Medal | Earned | Line | Glyph |
|---|---|---|---|
| Superbounce | every bounce; its line on 1 and 7 and up | `Superbounce!` | U+E2A0, a slate-blue disc, one chevron rising off a strip |
| Double Bounce | the chain's 2nd bounce | `Double bounce!` | U+E2A1, a gold disc, two chevrons |
| Triple Bounce | the 3rd | `Triple bounce!` | U+E2A2, a steel disc, three chevrons |
| Quad Bounce | the 4th | `Quad bounce!` | U+E2A3, a bronze disc, two pairs of chevrons |
| Chain Bounce | the 5th | `Chain bounce!` | U+E2A4, a lavender disc, a chevron over two pairs |
| Bouncetacular | the 6th | `Bouncetacular!` | U+E2A5, a pale gold disc in a spiked rim, three pairs |
| Bounced | a fall out of the map during a bounce | `Bounced!` | U+E2A6, a maroon disc, a chevron falling through a broken strip |
| Monster Bounce | a bounce that launches well above a normal one, beside its other medals; its line wins | `Monster bounce!` | U+E2A8, a hot orange disc in a spiked crimson rim, one tall chevron out past the top over a V of seams |

**The picture.** The disc has no superbounce medal, so the eight glyphs
are new art, drawn for this project in the retail medals' manner: 24 by
24, a shaded disc with a dark rim and the light from the upper left, in
their own 12-bit colours, and the tier told as a multi-kill tells it,
by the count of marks on the disc and the disc's colour, with a frame
for the top tier as the sprees have. The art is data,
`crates/formats/font/src/medals/art.rs` (ARGB 4444 rows, written by
`scripts/superbounce-medals.py` from its drawing, which also draws a
preview sheet beside the retail medals with `--preview DIR --h2 BIN`), and
`font::medals::replacements` adds the eight glyphs to `conduit-13` and
`conduit-12` through the font rewrite ([button-icons.md](../input/button-icons.md),
`font::patch`) at spare code points of the icon range, U+E2A0 to
U+E2A6 and U+E2A8 (Monster Bounce), and one at U+E2A7 for the Incel medal
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
([superbounce.md](superbounce.md#floor-marks)). The navpoint carries
no number: its screen place is the game's, clamped into the waypoint
area, which the kit does not read, so nothing of the kit's can sit over
it. Halo 2 draws a navpoint in
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
game draws no number with a navpoint, and the kit draws none: the
distance is on the superbounce readout's lines (`sbr.tag`,
`sbr.target.*`), whole metres, as the grenade counts are whole (the count
fonts have no `.`, D: `h2 font MSLCD-14 '1.2'`).

The preview draws at a sample point (`NAVPOINT_SAMPLE`, right of and
above the frame's centre) the arrow with its tip on the point and the
marker's in-view sequence 1 pixel above it (`NAVPOINT_GAP`), the texels
times the notifications' colour (the same `0x0013927e`; S: that the HUD
shader multiplies). The preview alone also draws a sample number over the
marker (`SAMPLE_DISTANCE_M`, `SAMPLE_DROP_M`, `NAVPOINT_NUMBER_GAP`) to
size the stack; it is not in the game. The outline is not drawn (decoding 8). It is
`Normal` over either target, and hidden while scanning and without a
target.

### States

| State | Arming bar | Prompt, message | Navpoint | Tutorial |
|---|---|---|---|---|
| `not-primed` | hidden | | crouch spot, "12" | |
| `near` | 0, ease | Hold (crouch) and walk under | crouch spot, "1" | |
| `under` | 1/3, ease | Let go of (crouch) | crouch spot, "1" | |
| `held` | 2/3, ease | Walk out to stand up | crouch spot, "1" | |
| `primed-ending` | 1, ending | Primed! | strip, "3" | |
| `left-early` | 0, run out | | crouch spot, "12" | |
| `primed-full` | hidden | Primed! | strip, "3" | |
| `primed-draining` | hidden | | strip, "3" | |
| `primed-still-warning` | hidden | Keep moving or lose the charge | strip, "3" | |
| `lost` | 0, blink | Find a crouch spot to charge again; Charge lost | crouch spot, "12" | |
| `lost-stood-still` | 0, blink | Find a crouch spot to charge again; Charge lost: you stood still | crouch spot, "12" | |
| `lost-crouched` | 0, blink | Find a crouch spot to charge again; Charge lost: you crouched | crouch spot, "12" | |
| `bounced` | hidden | (medal) Superbounce! | strip, "3" | |
| `medal-double` | hidden | (medal) Double bounce! | strip, "3" | |
| `medal-triple` | hidden | (medal) Triple bounce! | strip, "3" | |
| `medal-quad` | hidden | (medal) Quad bounce! | strip, "3" | |
| `medal-chain` | hidden | (medal) Chain bounce! | strip, "3" | |
| `medal-bouncetacular` | hidden | (medal) Bouncetacular! | strip, "3" | |
| `medal-bounced` | hidden | (medal) Bounced! | crouch spot, "12" | |
| `medal-monster` | hidden | (medal) Monster bounce! | strip, "3" | |
| `scanning` | hidden | | hidden | |
| `no-strips` | hidden | | hidden | |
| `tutorial-1` | hidden | | crouch spot | step 1 |
| `tutorial-2`, `-3`, `-4` | hidden | Primed! | strip | steps 2 to 4 |

The charge segments in each of these are the readout's, by the state's
charge ("The charge segments").

`bounced` and the seven `medal-` states are the eight medals, one line
each, with the glyph in its own colours as the game's font draw leaves
a coloured glyph (the tutorial's (X) is drawn the same way). The
preview draws them in font 6, and in font 5 in the half and quarter
modes:

```sh
h2 hud preview superbounce --state bounced --mode all --out DIR
h2 hud preview superbounce --state medal-chain --mode all --out DIR
```

The `glyphs` check covers the eight code points in both fonts (the disc
test also reads them from the patched `conduit-13` and `conduit-12`:
`the_glyphs_are_in_the_fonts`), `prompt` the line's width and its 62
units, and `drawn` that the line puts pixels down.

### The runtime, for this HUD

`crates/extensions/game/src/superbounce_hud.rs` registers the HUD and
feeds it each tick from `superbounce_mode`'s state through one adapter,
`superbounce_hud::view`:

| State (`superbounce_mode::State`) | View |
|---|---|
| `scan` | ready with spots, ready with none (`NoStrips`), failed, else scanning |
| `charge` (phase, fill) | the charge, as it is; the arming bar from the phase tick to tick (`ArmTrack`: the run out when a step gives way to no charge) (`ArmTrack`), and the primed ending once per priming: after its 18 ticks the bar is hidden until the charge is next primed, in the air or on the ground (`EndingOnce`) |
| `verb` | the prompt's verb (a crouch step, keep moving, charge again, held back for 60 ticks after a loss so its reason shows first: `after_reason`); in the near phase, not primed and with no verb, "Hold (crouch) and walk under" |
| `word` | on its change to −3, one `Handle::notify` of "Primed!" |
| the loss (`charge`'s while lost, `ChargeAgain`'s after) | its reason's message at its tick; the HUD keeps the last loss past the 150 ticks the state has it. One `Handle::notify` per loss, by its tick |
| `nav_target` (point, kind, distance) | the navpoint's target and distance, world units to metres; its point goes to `Handle::navpoint` |
| `medals` (the awards newer than the last sent) | `sb.medal`: the last local award's highest medal at its tick; one `Handle::notify` per award of a local player, to its user: the line of its highest medal ([superbounce.md](superbounce.md#medals)). With the variant's `medals` option off no medal line is sent, and an award made then is not sent later |

Primed and the loss go to user 0, the first local player's,
whose state the mode tracks. The first state after the mode comes on is
not news: what it already holds is not sent (`superbounce_hud::Sent`).

With the mode off every element is hidden. The tutorial's step machine
watches primed, the nearest bounce spot in metres and a bounce landing
newer than the last it saw (`superbounce_hud::watch`).

What shows in the game: the arming bar through the recipe, the prompt
verbs, the navpoint (the game's arrow and marker, no number) and, when
asked for, the tutorial's training text, and in the notification list
"Primed!", a loss's reason and the medals. The charge
itself is on the readout's segments.
`superbounce.state` has a loss's reason as `lost_reason` and the medals
as `medals`.

### The tutorial

The four steps are `Training` texts: the game's own training box, two
centred lines of font 6 in the middle of the HUD frame, drawn by the
game with its frame bitmaps and its training colour. There are no
diagrams: a retail training prompt has none. The box is in the middle of
the screen, so the steps show only when `H2_SUPERBOUNCE_TUTORIAL=1` asks
for them (`superbounce_hud::install`).

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
setting when it closes. A
game training prompt of its own takes the box while it shows; the
tutorial's step waits.

## The superbounce readout

A second HUD on the kit, `superbounce-readout`
(`crates/rewrites/hud-kit-core/src/huds/superbounce_readout.rs`): nine
charge segments round the motion tracker ("The charge segments"), three
rows right of them and a fourth over the arming bar,
three navpoints and the approach guide's five world marks ("The
approach guide"), for the first
local player while the superbounce mode is on. Nothing of it is a panel
in the middle of the screen: the guide is on the geometry. It is a HUD of its own because it is fullscreen
only: a half or quarter frame has no four rows free under the prompt
line's box, and the `modes` check wants every widget of one HUD in the
same screen modes.

```text
 x 40  47     130 137  149
                     Targets: Likely (4)  (W)(W)       sbr.kinds   y 326
                     [=====  arming bar  =====]        sb.arm      y 355 to 368
        .---.
   (   /     \   )   (arrow) 7.2  (down) 12.4 m/s      sbr.speed   y 376
   (  | track |  )   [#####   ] (arrow) 0.40 m         sbr.fall, sbr.steer   y 397
    \  \     /  /    Too slow: 12.4 of 16.5 m/s        sbr.info    y 418
     \  '---'  /                                                   y 444
      ' _____ '                                        sbr.seg.4 to .6, to y 450
```

| Element | Kind | Place, from the frame's bottom left (48, 444) | Shows |
|---|---|---|---|
| `sbr.seg.1` to `sbr.seg.9` | gauge, an arc of the charge ring (`font::ring`) | round the tracker ("The charge segments") | how far the player is through priming, and how long the charge has left |
| `sbr.speed` | text, font 6 | (101, −68) | moving: an arrow for the way of travel against the aim ([Direction arrows](#direction-arrows)) and the horizontal speed, `(arrow) 7.2 m/s`; falling, the fall speed after it, `(arrow) 2.1  (down) 12.4 m/s`. Hidden under 0.2 m/s |
| `sbr.fall` | meter, the battery strip's last 64 columns | (101, −41) | on a fall toward a seam: the drop from the apex against the 3.35 to 3.60 window; full inside it, fading to empty one world unit outside (`window`) |
| `sbr.steer` | text, font 6 | (171, −47) | beside the bar: `On seam` when the predicted landing (the stick held as it is) is caught and drifting the seam's way, `(arrow) Drift` when it is caught and is not, else an arrow for the way from the landing to the seam's landing point against the aim and how far, `(arrow) 0.40 m` ([superbounce.md](superbounce.md#the-landing-point)) (`steer_text`); `Out of reach`, with no arrow and the bar kept, when the landing is off the seam and the stick cannot bring it in. Its `longest` is `Out of reach`. It carries no percentage: the approach guide's box has the one verdict ("The approach guide") |
| `sbr.info` | text, font 6 | (101, −26) | in a primed fall, `Drop 10.5 m  Edge 0.06 m`: the drop from the apex and the deciding landing's distance to its floor polygon's nearest edge (`air_text`). In the flight after a bounce, the seams it can reach, the best first: `1 (arrow) 12 m  2 (arrow) 30 m`. Otherwise why the last landing by a seam did not bounce, held three seconds and up five: `Not primed`, `Missed seam by 0.31 m`, `Too slow: 12.4 of 16.5 m/s`, `No bounce: on seam at 17.2 m/s`, `Fell through at 17.2 m/s` |
| `sbr.kinds` | text, font 6 | (101, −118): y 326, 8 over the arming bar (y 355 to 368) as the speed row is 8 under it | the kinds of seam the seam lines are on, how many seams of them the map has, and the flashlight's glyph twice, the double tap that switches them: `Targets: Likely (4)  (W)(W)` (`kinds_text`). The glyph is White (U+E105), or X (U+E102) on Bumper Jumper, which the button swap redraws as the bound key. Shown whenever the map is scanned and has a spot; for two seconds after a switch it names the new kinds and flashes the help record (`Flash(Help)`, `kinds_value`) ([superbounce.md](superbounce.md#targets)). Its `longest` is `Targets: Common (888)  (W)(W)` |
| `sbr.tag` | navpoint | the tagged seam's aim point | the tagged seam ([superbounce.md](superbounce.md#targets)) |
| `sbr.target.1`, `sbr.target.2` | navpoint | a seam's aim point | in the flight after a bounce, the two ranked seams; none otherwise (the set's seams are lines on the map, [superbounce.md](superbounce.md#seam-lines)) |
| `sbr.guide.box`, `sbr.guide.pipper`, `sbr.guide.ticks`, `sbr.guide.drift`, `sbr.guide.takeoff` | world mark | on the seam the player is going for, and the ground before it | the approach guide ("The approach guide") |

Speeds are metres a second and distances metres (a world unit is
3.048 m). The rows are 21 pixels apart, a line of font 6, the last
ending 5 above the frame's foot. They start at screen x 149, 12 right of
the segments' right side (`SEGMENTS_GAP`). Under the arming bar the
three rows fill the block, so the targets row is over the bar
(`KINDS_UP`), clear of the Halo 1 health strip and the shield band, which
end at x 139 and 130.

```sh
h2 hud preview superbounce-readout --state all --out DIR
```

The preview's states are `idle`, `moving`, `falling-off-seam`,
`falling-on-seam`, `falling-out-of-reach`, `falling-wants-drift`, the five `miss-` lines, `targets`, `second-bounce`,
`second-bounce-lined-up`, `targets-switched` (the row flashing),
`targets-common-none`, `tagged`, the guide's
nine (`guide-approach`, `guide-lined-up`, `guide-falling-caught`,
`guide-too-shallow`, `guide-off-line`, `guide-too-high`,
`guide-no-drift`, `guide-jump-now` and `guide-walk-to-line`) and the
segments' eight:
`segments-idle`, `segments-near`, `segments-under`, `segments-held`,
`segments-primed`, `segments-draining`, `segments-warning` and
`segments-lost`. `idle` has no ring, as with the variant's
`charge_segments` off; every other state has the empty one, and the
targets row at the default kinds, `Targets: Likely (4)`.

`h2 hud preview superbounce-all` draws both HUDs in one frame, for the
preview only and fullscreen only, as the readout is
(`huds::superbounce_all`, `huds::previews`): twenty-nine moments of a run, each a state of the superbounce HUD beside one of the
readout's (`MOMENTS`), the thirteenth to the fifteenth the second
bounce's ranked seams, its line-up and the targets row flashing after a
switch of the kinds of seam shown, the sixteenth to the twentieth the approach
guide's (the walk up counting to the takeoff line, lined up a tick
short of it, falling and caught, falling too shallow, and off the
line), the twenty-first to the twenty-fifth the priming states the
others lack (near, held, the primed ending, the warning and the loss),
the twenty-sixth and twenty-seventh the guide's `Too high` and
`No drift` calls, and the last two the takeoff
line: on it with `Jump now`, and standing off it with `Walk to line`. A
moment's segments are its superbounce state's charge
(`superbounce_all::moment_segments`), so every moment shows the ring as
the game would, but the primed ending: its state is the arm's ending at
a full charge, so the moment takes 0.67 of the charge left (six
of nine lit) to show the wipe-back.

**The runtime.** `crates/extensions/game/src/superbounce_readout.rs`
registers it after the superbounce HUD, so that HUD's navpoint is the
kit's first and the readout's three follow, and feeds it each tick
through `superbounce_readout::view`:

| State (`superbounce_mode::State`) | View |
|---|---|
| `velocity`, `yaw`, `airborne` | the speed line (`speed`) |
| `landing` (the assist's prediction), `phase` (the bounce rule's verdict, drop and edge), while airborne | the drop bar and the steer line (`fall`); off the seam (`miss()` over 0) and its `verdict` `Course::Miss`, out of reach; off it and fixable, the way to `aim`, the seam's landing point; caught, the way to drift when `wants_drift()`, else on the seam. `phase` also gives the info row's drop and edge (`air`) |
| `last_landing` (result, tick, origin), `airborne`, `vz`, `position` | the miss line (`miss`, `Info`), below |
| `marks` (`superbounce_targets::Marks`) | the three navpoints' points (`Handle::navpoint_at`) and their distances; a target outside 40 degrees of the aim is cleared (`ahead`), and while the mode is on the `hudg`'s `waypoint_bottom_inset` (`+0xf0`) is raised to 80 so the one arrow the game clamps to the edge clears the tracker, the speed row and the score plate (`keep_waypoint_inset`, from the kit's tick) |
| `superbounce_targets::mode` (else the variant's `targets`), `superbounce_targets::current().len()`, the layout (`superbounce_ghost::current_button`), `scan` and the spot counts | the targets row (`kinds_row`, `flashlight_glyph`): shown once the scan is ready and the map has a spot, whatever the variant's options, as it answers the player's own press |
| `switched` (`superbounce_targets::Switch`, for 60 ticks after a double tap of the flashlight) | the targets row's flash, from the tick the switch first shows (`SwitchFlash`) |
| `second` (`superbounce_second::Ranked`, in the flight after a bounce) | the ranked line (`next`), and the fall row as the best seam's line-up (`line_up`) in place of the assist's prediction; the mode has put the ranked seams on the two target navpoints |
| `guide` (`superbounce_guide::Guide`) | the approach guide's values (`Guide::view`): on the ground its jump cue on the box; in a fall its call on the box (no percentage, and none on the steer line); and its marks' poses (`guide_poses`, `Handle::world_mark`) |
| `charge`, `scan`, the spot counts | the charge segments (`superbounce_readout::segments`): the charge's stage, once the scan is ready and the map has a spot |

The miss line's numbers are worked out beside the tracker's verdict:
the seam is the one the landing was by
(`superbounce_charge::landing_spot`), the distance is the origin's to
its edge, the need is the seam's (`need_of`), and the speed in is the
fall speed on the tick before touchdown (a 48-tick history of the
state's `vz`, `History`) carried down to the seam's height under
gravity (`speed_in`), as the tracker's own. A line whose seam or speed
is not known is not shown, and such a landing leaves a line already up.

The miss line's time on screen (`Info::step`):

| Event | Inside the hold (90 ticks, `MISS_HOLD_TICKS`) | After it |
|---|---|---|
| a bounce filed | takes the line down | takes it down |
| a miss filed | replaces it if it is a fall through, or the same kind of line with a higher speed in (`replaces`) | replaces it, unless the line shown is a fall through |
| a landing with no line (no seam, no speed) | nothing | nothing |
| a real fall toward a seam: airborne, falling at `TOUCHDOWN_VZ` (1 unit/s) or faster, a landing predicted (`falling_to_seam`) | nothing: the rebound hop, a jump, a step off a kerb | takes it down |
| 150 ticks up (`INFO_TICKS`) | | takes it down |

A fall through (`Fell through at`) ranks worst: it replaces any line and
only a bounce replaces it. The clock counts only ticks with a body
(`position`): between lives it stops, so the line stays through the
respawn countdown, when the player reads it. A tick with no state
(`Info::absent`) keeps the line up to 30 ticks in a row (`GAP_TICKS`);
longer is the mode off and clears it and the speed history. A state with
no landing after one is a new game (the mode resets its tracker) and
starts the line again.

A piece the variant's script options turn off is left out of the view
([superbounce.md](superbounce.md#the-huds-options)): `velocity` the
speed line, `alignment` the drop bar, steer line and airborne drop/edge
row, `miss_info` the miss
line, `approach_guide` the guide's marks, `charge_segments` the
segments. With the mode off every element is hidden.

### The charge segments

The priming indicator
(`crates/rewrites/hud-kit-core/src/huds/superbounce_segments.rs`): nine
arcs round the motion tracker's left, bottom and right sides, three a
side, concentric with its rim (`rects::TRACKER`, (47, 360) to (130, 443),
83 across) and 4 pixels (`GAP`) clear of it, so they are apart from the
ring and off the tracker's blips. They fill down the left side, round
the bottom and up the right as the recipe's steps are done, and wipe
back the other way as the charge runs out.

```text
        .---.
  1 (  /     \  ) 9
  2 ( | track | ) 8
  3  \ \     / /  7
      \ '---' /
       ' ___ '
       4  5  6
```

**Geometry** (`crates/formats/font/src/ring.rs`). The arcs are parts of
one band about the rim's centre, (88.5, 401.5): its inner radius 45.5,
the rim's 41.5 and the gap, and its outer 48.5, 3 pixels wide. An angle
is in degrees from straight down, positive to the right. The band runs
from 110 up the left side to 110 up the right, so its ends are at y 384:
13 under the shield meter's band (y 371) and 16 under the arming bar.
Each arc spans 18 degrees, 14.8 pixels along its middle, its ends cut
square on a radius. Two arcs of a side are 6 degrees apart, 4.8 pixels at
the inner edge, and two sides 11 degrees, 8.7 pixels.

| Segment | Angles | Picture's box on screen, top left, and size | From the frame's bottom left (48, 444) |
|---|---|---|---|
| `sbr.seg.1` | −110 to −92 | (40, 384), 6 by 16 | (−8, −60) |
| `sbr.seg.2` | −86 to −68 | (40, 404), 7 by 16 | (−8, −40) |
| `sbr.seg.3` | −62 to −44 | (45, 422), 12 by 15 | (−3, −22) |
| `sbr.seg.4` | −33 to −15 | (62, 439), 15 by 10 | (14, −5) |
| `sbr.seg.5` | −9 to 9 | (80, 446), 17 by 4 | (32, 2) |
| `sbr.seg.6` | 15 to 33 | (100, 439), 15 by 10 | (52, −5) |
| `sbr.seg.7` | 44 to 62 | (120, 422), 12 by 15 | (72, −22) |
| `sbr.seg.8` | 68 to 86 | (130, 404), 7 by 16 | (82, −40) |
| `sbr.seg.9` | 92 to 110 | (131, 384), 6 by 16 | (83, −60) |

The boxes are `font::ring::BOXES`, in the rim's own pixels, which a test
holds to the angles and radii; no two meet, and the right side's are the
left side's mirrored.

**Art.** Each is a `Gauge` ("Gauges"): `font::ring::draw` rasterises an
arc's seven pictures, white, with a one-pixel soft edge, at the eight
alpha levels a font's run bytes carry. Step 0 is the empty track at
alpha 4 of 15 (`TRACK`); step `k` has the first `k` sixths of the arc's
length lit, from the end the fill comes from, and the rest the track.
They are at U+E2C0 to U+E2FE, arc `s`'s step `k` at `0xE2C0 + 7s + k`,
in every font file the game opens (`bot_mark::GearRewrite`), and the
text draw gives them the HUD's text colour. No bitmap of ours is in a
map and nothing is copied from a disc. A fill is drawn as its nearest
step, so the arc at the edge of the fill is part lit and the wipe moves
along it, 2.5 pixels a step.

**Outside the frame.** The tracker's rim is on the HUD frame's left
edge and its foot (x 47 against the frame's 48, y 443 against 444), so
the left side and the bottom are in the screen's margin: 8 pixels left
of the frame and 6 under it, inside the 640 by 480 screen. The elements
say so (`Element::outside(8)`, `superbounce_segments::OUTSIDE`) and the
`frame` check holds them to it.

**Stages.** `superbounce_segments::segments` maps the mode's charge
(`superbounce_charge::ChargePhase`, as `superbounce::Phase`) onto a
stage and a count of lit segments:

| Stage | Charge (`ChargePhase`, `still_ticks`) | Lit | Look |
|---|---|---|---|
| `Idle` | none: not primed, and not by a crouch spot | 0 | the empty ring |
| `Near` | `Near`: crouched within 1.5 of a crouch spot, the standing capsule fits | 0 | the empty ring; the arming bar is up at 0 |
| `Under` | `Under`: under the lip, crouch held | 3 | the left side |
| `Held` | `Held`: crouch let go, the forced crouch | 6 | the left side and the bottom; the right side, the next to light, `Flash(Help)` |
| `Primed` | `Primed`, fewer than 19 still ticks: over half the charge left | 9 × (1 − `still_ticks` / 38) | all nine moving; standing still wipes them from the right side's top, 4.2 ticks a segment, a step of an arc every 0.7 |
| `Warning` | `Primed`, 19 still ticks or more: `LOW_CHARGE`, the tick the prompt says "Keep moving or lose the charge" | as `Primed`, 4.5 or under | the lit ones `Flash(TimeWarning)`, the timer's warning record |
| `Lost` | `Lost`: the word left −3, for one second | 0 | all nine empty in `Flash(TimeExpired)`, `Motion::Blink`: three blinks in 500 ms, then gone until the charge's `Lost` ends |

Every stage but `Lost` eases to its fill (`Motion::Ease`, 200 ms), so a
step lights its side as one sweep and moving again refills the ring.
The segments show once the scan is ready and the map has a spot, on the
ground and in the air: primed is worth seeing on the way down.

**Split screen.** The segments are the readout's, fullscreen only: in a
half or quarter frame the tracker is smaller and the grenade row, the
prompt line's box and the frame's edge leave no ring round it. There
priming is the arming bar, the prompt line's verbs and the
notifications.

```sh
h2 hud preview superbounce-readout --state segments-held --out DIR
h2 hud preview superbounce-all --state 24-charge-warning --out DIR
```

### The approach guide

Five world marks of the readout
(`crates/rewrites/hud-kit-core/src/huds/superbounce_guide.rs`) on the
seam the player is going for and the ground before it, so the line-up
is read off the geometry. What they mean, the prediction, the depth
rule and the jump cue are in
[superbounce.md](superbounce.md#the-approach-guide); this is how they
are built.

```text
            ^          sbr.guide.drift: the way to drift, lit when moving that way
      .-          -.
      |   seam's   |   sbr.guide.box: 0.6 along the edge, the catch (0.2) across,
      '-  point   -'   centred on the seam's landing point
          - - -        sbr.guide.ticks: from the pipper toward the box
           (+)         sbr.guide.pipper: where the path touches down

        ---------      sbr.guide.takeoff: where to press jump, across the run,
      -------------    the band's ends either side
        ---------
```

| Element | Shape | Pose | Values |
|---|---|---|---|
| `sbr.guide.box` | `Bracket` | on the seam's collision edge beside its landing point, heading along the edge; half 0.3 along (at most half the edge) and 0.1 across, the catch, so a pipper inside the box is a caught landing | `Dim` off the line; `Normal` lined up or with `Too shallow`, `Too high`, `Off edge`, `No drift`, `Don't crouch`; `Go` with `Will bounce` or `Jump now` (`box_value`, `go`, `Call`, `Cue`). Before the jump the label is the jump cue: `Walk to line`, `Jump in 0.7 s` (tenths of a second, `tenths`, at most `9.9`) or `Jump now` (`cue_text`). `Cue::Label` is any other cue as the game words it (`Climb up`, `Walk off`), shown as it is. In a fall it is the phase call alone, with no percentage, and none with no call (`box_text`); the widest is `Jump in 9.9 s` |
| `sbr.guide.pipper` | `Pipper` | at the predicted landing on the seam's floor, radius 0.06 | shown while a landing is predicted by the seam; `Go` with the box |
| `sbr.guide.ticks` | `Ticks` | from the pipper's ring toward the box's centre, over the distance between them, 0.6 at most (`ticks_run`) | shown while the landing is not caught; under a run of 0.02 it has no pose |
| `sbr.guide.drift` | `Arrow` | 0.06 outside the box on the drift's side, pointing along the drift | `Normal` at 0.15 units/s or more the drift's way, the full-launch minimum; `Dim` when slower or against it; hidden on a seam with no drift |
| `sbr.guide.takeoff` | `Bar` | at the takeoff line's centre on the ground, heading along the run; half the band along it, 0.25 across (`TAKEOFF_ACROSS`) | shown on the ground while there is a line (`Guide::takeoff`); `Go` with `Jump now`, else `Normal` |

`Guide` is what the marks show and `Layout` where they lie;
`poses(&Layout)` gives each mark's pose, for the runtime and for the
preview's nine samples alike (`samples`: the seam's point at the scene's
origin, its edge along +X, its drift away from the viewer, the takeoff
line 1.1 back down the drift). Every pose carries the guide's fade as
its strength. The marks are fullscreen only, as the readout is.

`Guide::call` is the phase verdict and `Guide::cue` the jump cue on the
ground ([superbounce.md](superbounce.md#the-jump-cue)), settled by the
game. The kit writes one of them on the box's label. It carries no
chance field and shows no percentage beside the call.

## Interfaces

- A feature calls `hud_kit::register(describe) -> Handle`, then
  `Handle::publish(values)`, `Handle::publish_for(user, Option<values>)`
  for a local user's own, `Handle::navpoint(Option<point>)` or
  `Handle::navpoint_at(id, Option<point>)` for its navpoints, and
  `Handle::world_mark(id, Option<pose>)` for its world marks. A
  `Prompt` or `Training` element's shown text is the channel's text.
- A feature whose HUD is never shown with another's names it in
  `Describe::never_with`; the runtime hides the one whose user is not in
  its mode.
- Guest points hooked: `HUD_WIDGET_GATE`, `HUD_WIDGET_INPUTS`,
  `HUD_WIDGET_STRING`, `HUD_WIDGET_SPRITE`, `HUD_WIDGET_EFFECTS`,
  `HUD_INTERACTION_VISIBLE`,
  `GAME_ACTIVATE`, `GAME_TICK`, and the string lookup `0x001a0180`
  through `community_variants`' override.
- Guest memory written: the two block fields of each player `nhdt` root,
  the scenario's point block field, the kit's two allocations, the first
  local player's prompt fields and navpoint list, each local user's timed
  line and the order byte it shares with the notifications, the training
  id and show byte.
- Agent API: none yet (`hud.kit` is planned with slice 6's run checks).

## Rules and limits

- No pixels, strings or colour values from the disc are in the
  repository. An `Art` is a reference; a colour is an address or a tag
  field.
- A HUD may use only the styles above. A new colour is a new retail
  source, named in "Colours", not a constant of ours. The one constant
  is `world::GO_GREEN`, a world mark's `Go`: the window draws world
  marks, not the game's HUD, and it means "this fall will bounce".
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
- That `[0x004b9ed8]` is the user whose HUD the widget loop draws, for
  every user's pass of a split frame.
- That writing the prompt fields at `HUD_INTERACTION_VISIBLE`'s
  interaction call shows the verb and hides it again when the kit stops,
  and that U+E110 is redrawn as the bound crouch key.
- The navpoint: that a point appended to the `scnr +0x1e0` block harms
  nothing else that walks the block, that the list is not rebuilt over our slot each tick, and the
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
- The charge segments: that the game's text draw puts a glyph's
  top-left corner at the widget's point as the preview does (an arc a
  pixel or two off is nearer the rim or further from it), draws the whole of a
  glyph on a line with no letter, and adds no shadow or scale to a text
  widget's glyph; that the retail tracker's art has
  nothing opaque in the 4 pixels round its rim, on the disc's `retail`
  check; how the faint track (alpha 4 of 15) reads over a bright level;
  that 63 more glyphs in every font file cost the font cache nothing;
  that the host's HUD fit keeps widgets in the screen's margin, left of
  and under the frame, on a pinned wide window; that the half-size
  tracker of a split frame is where the segments are not; and the nine
  more text widgets against the `nhdt`'s widget count. The `Held` and
  `Warning` flashes draw as `Normal` until the colour hook is in.
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
- The banner: that the timed line's top, not its baseline, is 40 pixels
  above the frame's bottom (the preview's place), that it draws in a
  multiplayer game with the HUD up, that writing the fields directly at
  the tick's end draws as a call to `0x0024cc73` does, and that
  `player_globals +8` counts the users whose state the line is read
  from.
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
- Team colours: that `0x0007f720` is read as `pad_lightbar::team_rgb`
  reads it on a retail map (the layout is the lightbar's, tested on a
  synthetic guest), and that a world mark in a team's colour reads well
  on the level.
- World marks: that the world-line pass draws the kit's source beside
  the mode's own, the marks' width, glow and label size in a game, and
  how a mark reads on a slope or behind geometry (the pass has no depth
  test, and a mark is flat). The preview's sample view is not the
  game's camera, so the `world` check's frame rule says nothing of the
  game.
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
   `HUD_SHADER_QUAD`) and the flash (`hud_kit_core::flash`), so `Dim`,
   `Inactive` and `Flash` draw; the corner turn. Before coding, decodings
   4 and 7. Tests: a `Dim` text's alpha; a flash at the record's phases;
   four corners turned by a quarter turn.
3. **Preview fidelity.** Decodings 3 and 5, so the preview draws the
   training box's frame and a retail widget's own flash; `h2 hud
   compare` against a `render.frame` capture, as the UI builder's
   fidelity loop.

## Related

- [superbounce.md](superbounce.md), "The superbounce game type": the first user.
- [hud-editor.md](../editor/hud-editor.md): the `nhdt` format, placement rules and the renderer the preview uses.
- [health.md](../halo1/health.md), [hud.md](../halo1/hud.md), [text.md](../halo1/text.md): the Halo 1 HUD, which adds widgets and message text the same way.
- [ui-screens.md](ui-screens.md), [ui-builder.md](ui-builder.md): the UI kit.
- [fonts.md](../formats/fonts.md), [button-icons.md](../input/button-icons.md), [high-resolution.md](../graphics/high-resolution.md).
