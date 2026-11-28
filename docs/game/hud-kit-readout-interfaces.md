# HUD kit: readout, objective HUD, interfaces and limits

The superbounce readout, the scripted objective HUD, interfaces, rules, evidence and the build plan. Part of [hud-kit.md](hud-kit.md).

## The superbounce readout

A second HUD on the kit, `superbounce-readout`
(`crates/rewrites/hud-kit-core/src/views/superbounce_readout.rs`): nine
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
| `sbr.speed` | text, font 6 | (101, −68) | moving: an arrow for the way of travel against the aim ([Direction arrows](hud-kit.md#direction-arrows)) and the horizontal speed, `(arrow) 7.2 m/s`; falling, the fall speed after it, `(arrow) 2.1  (down) 12.4 m/s`. Hidden under 0.2 m/s |
| `sbr.fall` | meter, the battery strip's last 64 columns | (101, −41) | on a fall toward a seam: the drop from the apex against the 3.35 to 3.60 window; full inside it, fading to empty one world unit outside (`window`) |
| `sbr.steer` | text, font 6 | (171, −47) | beside the bar: `On seam` when the predicted landing (the stick held as it is) is caught and drifting the seam's way, `(arrow) Drift` when it is caught and is not, else an arrow for the way from the landing to the seam's landing point against the aim and how far, `(arrow) 0.40 m` ([superbounce.md](superbounce-spots.md#the-landing-point)) (`steer_text`); `Out of reach`, with no arrow and the bar kept, when the landing is off the seam and the stick cannot bring it in. Its `longest` is `Out of reach`. It carries no percentage: the approach guide's box has the one verdict ("The approach guide") |
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
(`crates/rewrites/hud-kit-core/src/views/superbounce_segments.rs`): nine
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
(`crates/rewrites/hud-kit-core/src/views/superbounce_guide.rs`) on the
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

## The scripted objective HUD

`crates/rewrites/hud-kit-core/src/views/script_objective.rs` (its elements
are `content/modes/huds/script-objective.hud.yaml`) shows what a script
sets with the `hud.objective` capability (`objective(to, text)`,
`objective_mark(to, zone, kind)`, `meter(to, value, of)`,
`countdown(to, ticks)`; [rules-engine.md](rules-engine.md#the-engine-api))
of the viewer's team. The VM keeps the state per team
(`rules::vm::Objective`, `RulesExt::objectives`); the game's
`crates/extensions/game/src/objective_hud.rs` makes a `View` of it for each
local user each tick and publishes its values. A script that does not
require `hud.objective` draws nothing: the HUD is registered hidden and
hidden again once a script stops feeding it. It is multiplayer only.

| Element | Kind | Shows |
|---|---|---|
| `so.banner` | banner | the objective line, started again by the kit when the text changes (a phase change); a text the banner cannot take (`banner_fault`) is left out |
| `so.clock` | text, `hud` font, right-justified, top right | `m:ss` of the countdown, rounded up; the help flash from 30 s down |
| `so.line` | text, `hud` font, right-justified, top right under the clock | the objective line as resting text |
| `so.meter` | meter, the retail strip, under the grenade row's next pill | `value` of `of`; not drawn for an `of` of 0 or less |
| `so.nav.1` to `so.nav.3` | navpoint | the first three marked zones, each with its distance in metres from the viewer's unit; the point is the zone's middle |

The banner and the clock have half and quarter places; the line, gauge and
navpoints are fullscreen only (fullscreen first). The HUD's `never_with`
lists the HUDs that use the grenade row's next pill. Its states are `idle`,
`phase`, `capturing`, `defending`, `last-30` and `widest`
(`script_objective::views`), each checked by the layout and `hud.render`.

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
- A monitor's HUD says so (`Describe::monitor`): it is compiled with the
  grenade row's origins at the row's first place
  (`Templates::for_monitor`, `GrenadeRow::first`: the frag pill's and
  count's places less one step, the plasma's), and the preview and the
  `retail` check draw it over the motion tracker alone
  (`preview::monitor_retail`). Its feature turns the retail widgets of
  its user off while the monitor is up, `Handle::retail(user, shown)`.
- The Rust walk's override seam (gate, inputs, string, sprite, scale;
  `render_bridge::native_hud::seam`) and the guest points
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
- That narrowing the sprite's bounds at the seam narrows the quad (the quad's size taken from the
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
  right under Pinned and Centred at wide aspects.
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
   Pinned and Centred, fullscreen and split; the widget cap and the
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
