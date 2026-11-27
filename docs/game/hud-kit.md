# The HUD kit: HUD elements of ours, drawn by Halo 2's own HUD

Status: In progress. Built: the model, the compiler, the preview and the checks, the widget runtime (install, gate, inputs, string, crop), the prompt, message (notification), training, banner and navpoint channels, the superbounce HUD on them with its medals ("The medals"), the Firefight HUD ([firefight.md](firefight.md), "The HUD") and the display HUDs of the scripted game types ([game-types-without-carriers.md](game-types-without-carriers.md), "The display"; `views/objective.rs`, their feed from the script's replicated state not built). Not built: the colour hook and the flash, the pointer's turn, and the `hud.kit` agent method (build plan). Nothing of the runtime has been run in a game ("Not verified").
Code: `crates/rewrites/hud-kit-core` (the model, the HUDs' states, `data` which reads a HUD's YAML, the templates, the record compiler), `content/{superbounce,forge,modes}/huds/` (the HUDs' elements as YAML), `crates/tools/hud-kit` (the preview, the checks), `crates/extensions/game/src/hud_kit.rs` (the runtime), `crates/extensions/game/src/superbounce_hud.rs` (the superbounce HUD's feed), `crates/apps/h2-tools/src/hud_cmd.rs` (`h2 hud`). Tests: `crates/rewrites/hud-kit-core/src/tests.rs`, `crates/tools/hud-kit/src/tests.rs`, `crates/tools/hud-kit/tests/disc.rs`, `crates/extensions/game/src/hud_kit_tests.rs`, `crates/extensions/game/src/superbounce_hud_tests.rs`.
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
  each HUD draw scope to the window: Centred keeps the corners in a
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

## Pages

| Page | What |
|---|---|
| [HUD kit: how it works](hud-kit-how-it-works.md) | The mechanism: how the kit builds and draws the HUD. |
| [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md) | The preview and the checks, and the superbounce HUD built on the kit. |
| [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md) | The superbounce readout, the scripted objective HUD, interfaces, rules, evidence and the build plan. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## How it works

See [HUD kit: how it works](hud-kit-how-it-works.md#how-it-works).

### HUDs as data

See [HUD kit: how it works](hud-kit-how-it-works.md#huds-as-data).

### World marks

See [HUD kit: how it works](hud-kit-how-it-works.md#world-marks).

### Direction arrows

See [HUD kit: how it works](hud-kit-how-it-works.md#direction-arrows).

### The Firefight HUD

See [HUD kit: how it works](hud-kit-how-it-works.md#the-firefight-hud).

## The preview and the checks

See [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md#the-preview-and-the-checks).

## The superbounce HUD on the kit

See [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md#the-superbounce-hud-on-the-kit).

### The pill

See [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md#the-pill).

### The arming bar

See [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md#the-arming-bar).

### The medals

See [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md#the-medals).

### States

See [HUD kit: preview, checks and the superbounce HUD](hud-kit-preview-superbounce.md#states).

## The superbounce readout

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#the-superbounce-readout).

### The charge segments

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#the-charge-segments).

## The scripted objective HUD

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#the-scripted-objective-hud).

## Interfaces

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#interfaces).

## Rules and limits

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#rules-and-limits).

## Evidence

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#evidence).

## Not verified

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#not-verified).

## Build plan

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#build-plan).

## Related

See [HUD kit: readout, objective HUD, interfaces and limits](hud-kit-readout-interfaces.md#related).
