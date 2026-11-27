# HUD kit: preview, checks and the superbounce HUD

The preview and the checks, and the superbounce HUD built on the kit. Part of [hud-kit.md](hud-kit.md).

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
| world | every sample pose is for a world mark of the HUD and has no fault (a number that is not finite, a negative size, a strength outside 0 to 1); a world mark shown in a state has a sample pose there, draws lines and keeps them inside the HUD frame; and `Go`, `Team` and `Axis` are on world marks alone, `Team` of a team the game has and `Axis` of one of the three. A world mark's lines are not compared with the widgets' (`own`): they are under the HUD, on the geometry |

The synthetic source has no fonts, so it skips the glyph check and says
so, and its retail check is against the flat disc and boxes, not the disc's art.

The agent API gets one read method, `hud.kit`: the registered HUDs, and
per element the id, kind, widget address, placed rectangle, text, fill
and whether the gate last drew it. It is how a run is checked against
the preview's JSON, with `render.frame` for the pixels.

## The superbounce HUD on the kit

`crates/rewrites/hud-kit-core/src/views/superbounce.rs` describes it
(its elements are `content/superbounce/huds/superbounce.hud.yaml`):
`View` (what the HUD shows of `superbounce_mode`'s state),
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
(`crates/rewrites/hud-kit-core/src/views/pill_block.rs`), a HUD of the
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
