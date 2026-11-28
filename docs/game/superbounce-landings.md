# Superbounce: charge, landings and verifying spots

Charge, crouch spots, landings, floor marks, lock-on and how spots are verified. Part of [superbounce.md](superbounce.md).

## Charge, crouch spots and landings

What the mode shows about the three steps of a bounce: getting the
charge, keeping it, and landing it. The HUD parts (the arming bar, the
charge segments, the prompt line, the notifications, the navpoint)
are in [hud-kit.md](hud-kit.md), "The charge meter", "The arming
bar" and "The charge segments"; this is the state they
read, the scanner's rules and the world overlay. The numbers are in
[notes/superbounce-spots.md](notes/superbounce-spots.md).

The scanner's part (the stricter crouch rule, the strip's edge and the
one catch) is built in `crates/live/superbounce`. The state is built:
`superbounce_mode::State` carries `charge`, `verb`,
`nav_target`, `landing`, `last_landing`, `off_spot`, `still_ticks` and
`standing_fits` (Code: `crates/extensions/game/src/superbounce_charge.rs`
for the tracker and the landing classifier,
`superbounce_capsule.rs` for the capsule test, `superbounce_assist.rs`'s
`predict` for the predicted landing), and `superbounce.state` returns
them ([agent-api.md](../runtime/agent-api-world.md#superbounce-the-superbounce-game-type)).
The world overlay ("Crouch spots in the world", "Floor marks" and
"Lock-on") is built in `superbounce_world.rs`.

### Charge

The pair word has no amount. It is 0 through a crouch, forced or not,
and −3 from the first tick of the stand-up out of a forced crouch, where
the crouch lets go at the band's way out, 6 cm from where the body
rested in the measured corner (**Trace**, the note). So "how charged" is
two things the engine does have:

- **How far through the recipe** (crouch, walk in, let go, walk out:
  one phase a step). `near` (crouched, the crouch request
  set, within 1.5 of a crouch spot's stand point), `under` (crouched with
  crouch held and the standing capsule does not fit), `held` (the crouch
  button is up and the request, unit flags bit 23, is still set: the
  engine keeps it only while standing does not fit, [forced
  crouch](#forced-crouch-code)), `primed` (the word is −3 and the request
  is clear).
- **How long it will last.** A primed body that stands still sleeps
  after 38 to 40 ticks and wakes with a new pair at 0 (**Trace**). The
  mode counts the ticks the body's speed is under 0.03 units/s, back to
  0 when it moves; the charge left is 1 less that count over 38. A
  crouch ends the charge at once (**Trace**), so a primed player whose
  request is set is `lost`, as is one whose word leaves −3. `lost` shows
  for 30 ticks.

**Why it went.** The tracker gives each loss a reason, read on the tick
the charge goes (`superbounce_charge::LostReason`):

| Reason | When |
|---|---|
| `crouched` | the crouch request is set |
| `stood_still` | the word left −3 after 30 still ticks or more (`STOOD_STILL_TICKS`): the body slept. 30 is under the 38 to 40 measured, so a sleep a few ticks early still reads as one |
| `unknown` | the word left −3 for another reason: a respawn, a vehicle, the pair gone, or a sleep the still count missed |

A landing is not a reason. Nothing measured has a landing use the
charge up: the word stays −3 after a bounce until the body sleeps (the
note), and a landing that does not bounce is filed as a landing, not a
loss.

**The warning and the new charge.** From 19 still ticks while primed
(`WARN_STILL_TICKS`, half of 38, where the fill is 0.5) the verb is
`KeepMoving`. For 150 ticks after a loss (`AGAIN_TICKS`: the `lost`
second and four more), while the phase is `lost` or hidden, the verb is
`ChargeAgain`; a crouch step (`near`, `under`, `held`) takes the line
from it.

`superbounce_mode`'s state carries it as `charge` (the phase, the fill:
0, 1/3, 2/3, then the charge left, 0 while lost; and the loss, its
reason and tick, while lost), `still_ticks`, `verb` (`LetGoOfCrouch`
while `under`, `WalkOutToStand` while `held`, `KeepMoving`, and
`ChargeAgain` with the loss) and `standing_fits`. `superbounce.state`
returns them, the reason as `lost_reason`, and `world.object`'s
`physics` block has `standing_fits` for bipeds.

Two inputs are not the engine's own:

- **The standing capsule.** The engine's answer, output flags bit 1 of
  the character physics update (`0x001e55d0`), is not kept in a field
  found so far, so `under` and `standing_fits` come from the mode's own
  test (`superbounce_capsule.rs`): the scan's standing capsule (bottom
  at the origin, 0.725 tall, radius 0.175, in deeper than 5 mm, upward
  faces at the feet are support) against the structure BSP's collision
  polygons, which the scan keeps in a 2-unit grid. It exists only while
  the mode is on and its scan is ready (**S**: that it agrees with the
  engine's bit at the measured corner). Reading the engine's bit needs a
  hook at `0x000e28f4` or the output pointer of `0x001e55d0`, and is
  **Open**.
- **The crouch button.** `held` needs to know the button is up. The fall
  assist's input tap (`superbounce_assist.rs`) reads it from the pad the
  guest gets on the first controller, through the player's action table
  (`0x51ea18 + 8 + 10`, crouch's slot; the Default layout's left-stick
  click when it cannot be read), with the agent's `input.pad` steps and
  before an `H2_INPUT_REPLAY`'s. Without the tap (the
  `superbounce-assist` feature off) the button is unknown and the phase
  goes from `under` to `primed` with no `held`.

Not a charge, and not shown as one: how long the crouch is held (nothing
counts it), and how deep the standing capsule is in the lip (most bands
are far deeper than the one that is known to prime, the note).

### Crouch spots: the stricter rule

The scan keeps a forced-crouch region by the rules of
[Crouch](superbounce-spots.md#crouch), then cuts the spots by these, in this order
(`crouch.rs`, `select`):

1. **In play.** The stand point is no more than 1.0 below the lowest of
   the scenario's player starting locations (`scnr+0x100`, the position
   at `+0x00`), counting only those over the scanned geometry's bounds,
   since a scenario's starting locations are for all its BSPs. With none
   there the rule is off. Regions under the map are dropped (5 of
   Ascension's 21).
2. **By a strip.** A bounce spot's stand is within 8 units, straight
   line: a rule, where the score only prefers a strip within 15. If
   fewer than 3 spots are kept by the rules above and below, the spots
   nearest a strip are added, nearest first, through the same spacing,
   until 3 are (Coagulation). With no strip on the geometry that is the 3
   best by score.
3. **Spread out.** A spot within 3.0 of a better one is dropped, and the
   12 best are kept.
4. **Proven first.** A measured priming place (`superbounce::PROVEN`,
   looked up by the map's header name, or the list in the
   `superbounce::Context` handed to `scan_in`) is kept whatever
   the other rules say, before the others, so it crowds them out rather
   than the other way round, and it scores 1, so it ranks first. A spot
   with a sample within 0.3 of one is `proven`; the others are `likely`.
   A place no region is near (its lip is an object the scan doesn't
   read) is a proven spot of its own: `stand` where it was measured, its
   `out` from the table, its edge a 0.4 line across that, and its note
   in `--explain`. The places: the Ascension corner (−10.91, 0.72,
   −1.16), which by score alone ranks 13th of 21; Zanzibar's window sill
   (5.22, −26.285, 5.11), under broken glass (break it first; the word
   turns on the jump); and Zanzibar's walk-out place (8.07, −3.01, 4.51)
   ([Zanzibar's window](superbounce.md#zanzibars-window-trace-lab-play)).
5. **Stand at the way out.** `stand` is the region's exit sample nearest
   the nearest strip, not the sample nearest the centre: the word turns
   at the way out. The spot also carries `out` and `lip`
   ([Crouch](superbounce-spots.md#crouch)).

`superbounce::scan_map` and `explain_map` read the starting locations
and the proven places from the map (`superbounce::map_context`);
`superbounce::scan` and `explain` on bare polygons have neither, so the
in-play rule is off and nothing is proven.

There is no depth rule: a cut by the standing capsule's overlap drops
almost nothing and has no measurement behind it (the note).
`--explain` prints, per region, each rule's verdict in this order (`in
play`, `by a strip`, `spread out`, `proven`, `stand at the way out`),
with the lowest starting location, the nearest strip and its distance,
the better spot that crowded it out, and its status. Expected on the
disc's maps: 7 to 12 a map, 3 on a map whose strips are far from its
lips (an estimate from the note's table; `h2 superbounce scan <map>
--json` counts them).

Which `likely` spots prime is a game run's to say: `superbounce.verify`
tries each kept spot in the game and writes the map's verified file
([Verifying spots](#verifying-spots)), which marks the spots that primed
`proven` and the ones that never did `failed`.

### Crouch spots in the world

While not primed, the crouch spot the navpoint is over (the nearest, by
its stand point) carries the marker, drawn by the window's world-line
pass (`crates/extensions/game/src/superbounce_world.rs`, the geometry;
`crates/apps/app/src/overlay/marks.rs`, the drawing):

- **The marker.** One ring of the capsule's radius (0.175) round
  `stand`, 0.02 above the floor: the size of the player's own feet,
  where to be. A pin stands over it from 0.2 to 0.8 above the floor with
  `Crouch` at its top (`marker`); it fades out as the eye comes within
  2.5 units of its top and is gone within 1.5 (`near_fade`), so it never
  runs off the top of the screen. The verb with the button is on the
  HUD's prompt line.
- **Under the lip.** The ring pulses while the phase is `under` or
  `held` and the player is within 1 unit of the spot, sideways.
- **The approach arrow.** One arrowhead on the floor 0.15 outside the
  ring along `out` (`arrowhead`), pointing in toward the ring until the
  phase is `held`, then out: walk in, then walk out.

The marker appears once the player is within 6 world units of the spot
(`NEAR_RANGE`). No other crouch spot is drawn. A `likely` spot that is
next has the same marker as a `proven` one. While primed, and while
airborne, no crouch spot is drawn: a crouch now would lose the charge,
and a crouch is for the ground.

### Landings

The scanner's bounce spot carries its collision edge as the target
([the target](superbounce-spots.md#the-target-the-collision-edge)), judged with the one
catch; its `need` is the [one threshold](superbounce-spots.md#speed). How a landing is shown is in
[Floor marks](#floor-marks) and [Lock-on](#lock-on); this is the state
under them.

- **On course.** The state's `landing` (`superbounce_assist::predict_on`)
  is there whenever the player is airborne, rising or falling, primed or not:
  of the strips below with no floor between
  ([the assist's rule](superbounce-guide.md#hold-x-the-fall-assist)) and a point within
  3 units of the landing with the stick held as it is, the one it lands nearest the slot of,
  with its `verdict`
  (`OnSlot`, `Fixable` by the assist's reach gate, else `Miss`), `speed`
  and `need`, and the strip's `edge`, `stand` and `fall`. `miss` is how
  far outside the edge's catch it lands (0 when caught), and it is
  `locked` when it is `OnSlot` and `speed` is at least `need`
  (`superbounce_charge::Predicted`).
- **What happened.** A touchdown is a tick whose previous tick was
  airborne and falling at 1 unit/s or more and which has stopped falling
  (over −0.5), launched, or landed; the origin tested is the previous
  tick's, before the contact moves it. The launch is the best vertical
  speed of the touchdown tick and the 3 after
  (`superbounce_charge::LAUNCH_TICKS`, `Pending`): the push can show a
  tick or two after the contact, with the touchdown tick itself near 0.
  The landing is judged and filed when that window closes
  (`superbounce_charge::judge`), so the HUD, the steps and the medals
  only ever see the settled verdict. Within 0.5 of a strip it is
  `bounce` when the launch reaches 3.5 units/s
  (`superbounce_charge::BOUNCE_VZ`), else the first of `not primed` (the
  word was not −3 on the tick before), `missed` (the origin was outside
  the edge's catch), `too slow` (the speed in was under `need`), `no
  bounce`. Off every strip a primed launch that reaches 3.5 is a
  `bounce` all the same, and any other touchdown there files nothing.
  3.5 is the sim's seam drops' `bounce_vz`, the one value both use
  (`sim_bench::drops::BOUNCE_VZ`): over a standing jump's launch, 3.08
  (`bots::bounce_route::JUMP_VZ`), so a hop on landing is not a bounce,
  and under every measured bounce, which launch at 4 and over
  ([Covered seams](superbounce-spots.md#covered-seams)). The HUD says it in the notification list, and
  `superbounce.state` returns the last landing as `last_landing`: the
  outcome, the touchdown's game tick and the origin.
- **Through the floor.** A primed fall can go through a seam's floor
  instead of bouncing (Zanzibar's crack, [Zanzibar's
  window](#zanzibars-window-trace-lab-play); a seam on Ascension). It
  never stops falling, or stops for a tick and sinks on, so no window
  closes on it; it is filed at once as `through`
  (`superbounce_charge::through`, `LandingResult::Through`), on the
  tick the origin crosses from at or above the edge's height less 0.3
  (`SINK`) to below it, within 0.3 of the edge horizontally
  (`THROUGH_RADIUS`), moving down (the tick before falling at 1 unit/s
  or more, or a touchdown by the strip open) and primed (the word −3 on
  the tick before or on that touchdown's tick before). The height is
  the edge's at its point nearest the origin. 0.3 is deeper than any
  measured bounce's sink (0.07 to 0.23 under the floor, [Traces](superbounce.md#traces-trace)
  and [the touchdown tick](superbounce.md#the-touchdown-tick-trace-stepped)), so a
  bounce's dip is never one; the radius is the catch (0.1) and the
  capsule's radius (0.175), rounded, and the Zanzibar fall came down
  0.048 off its crease. A touchdown the fall follows was the seam hit:
  `through` replaces its verdict, with its tick and origin; otherwise
  the tick is the crossing's and the origin the tick before's. A
  touchdown under that floor within 0.5 of the edge after it is the
  same fall, not a landing. The speed in, the fall carried down to the
  edge's height, is on `Through::speed_in` (`Tracker::last_through`).
- **Off-spot bounces.** A primed bounce no marked strip accounts for
  (none within 0.5, or the origin outside the catch of the one there
  is) is a seam the marks don't know yet. The mode keeps the first local
  player's last 16 of the game (`superbounce_mode::OFF_SPOT_MAX`) on
  `superbounce.state`'s `off_spot`: the touchdown's tick, the origin, the
  edge of the bounce spot within 0.5 (the marked strip's, else the
  nearest of the whole scan, proven or not; absent with none), the speed
  in and the launch. They are for harvesting: the verified file is
  keyed by a scanned spot and written by runs that try it
  ([Verifying spots](#verifying-spots)), so one bounce in play proves
  nothing there.

### Floor marks

While primed the marker is on the **chosen strip**: the navpoint's
(`superbounce_charge::nav_target`), the nearest strip the player's floor
is high enough for (feet at least `fall`, the [speed model](superbounce-spots.md#speed)'s
2.43 drop, above `stand`, less 0.05), else the nearest strip, and while
falling onto a strip that strip (`nav_to_landing`). The HUD's navpoint
is over the same strip with the drop in metres
([hud-kit.md](hud-kit-preview-superbounce.md#the-navpoint)), and the
[fall assist](superbounce-guide.md#hold-x-the-fall-assist) steers onto it. Not primed, there
is no chosen strip. Only [marked](superbounce.md#the-superbounce-game-type) strips can
be chosen.

- **Land.** The chosen strip has one ring of the capsule's radius
  (0.175) round `stand` and the strip's own line through it, 0.26 each
  way along `edge` (at most half the edge; `strip_bar`), which says
  which way the strip runs. On the ground, from a floor high enough, a
  pin stands over it with `Land, 3 m` (the drop, `fall` in whole metres;
  `Land` alone for a standing jump). From a floor too low, with a
  takeoff found, the pin and the navpoint are on the
  [takeoff](superbounce-guide.md#the-takeoff) and the strip has no label. In the air it has
  no pin and no label: the [lock-on](#lock-on) is the slot's outline,
  and locked, the mark turns red with the lock.
- **Near the eye** the ring, the line and the pin fade out: full from
  2.5 units, gone within 1.5 (`near_fade`), so a ring at the feet does
  not fill the screen.
- **The guide's box.** While the [approach guide](superbounce-guide.md#the-approach-guide)
  is on the strip, its box is the strip's only mark: no ring, line or
  pin.
- **The takeoff.** On the ground the [takeoff](superbounce-guide.md#the-takeoff)'s bar,
  `Jump from here` or `Drop from here`.
- **Behind geometry.** A mark the map hides from the eye (the segment
  from the eye, 0.62 over the feet, to the mark's anchor crosses a
  collision polygon short of its last 0.15,
  `superbounce_capsule::Room::blocked`) is drawn dim, with no label
  (`superbounce_world::occlude`), so it does not read as being on the
  near surface.
- **No other strip** is drawn.

The scanner cannot know steps that need an object (break the window,
crouch under the shard) or a turn before the jump.

### Lock-on

While primed, airborne and falling onto a predicted landing, the crease
is a target (`superbounce_world::lock_box`, `brackets`), in split screen
and with the variant's `approach_guide` option off; fullscreen with the
option on, the [approach guide](superbounce-guide.md#the-approach-guide)'s box is on the
seam in its place (`superbounce_world::lock_shown`). The lock-on is
four corner brackets on the floor round the point of the crease nearest the
landing, 0.3 along it and 0.2 across when closed. They open by 1.5 per
unit the landing misses the edge's catch by (at most 0.9 wider) and move toward
the landing by half the miss (at most 0.5), so they close in as the
landing comes onto the crease. A box that appears starts 0.6 wider and
closes in.

**Locked** is the landing's `locked`: within the edge's catch and at least the
touchdown speed the strip needs. Locked, the brackets and the landing
mark are red (`RED`, 1.0, 0.275, 0.196) with `Locked` over them. Red is
the one colour of ours: the game's red reticle means "this will hit".

**Wall clock, not frames.** The box and the lock are worked out per
guest tick from the state. Between ticks the brackets ease to each
tick's box over 0.12 s by the wall clock (`Ease`, smoothstep from what
was shown when the tick's box came), evaluated at each present's time
(`ImageMarks::visit`'s `now`), so they move the same at 30, 60 or 120
frames a second.

**The reticle.** Nothing is drawn over the reticle: the lock is the
brackets on the floor and the readout's fall row
(`superbounce_hud::reticle_lock_now` says whether it holds).
The game's own reticle is not turned red and no guest hook is involved:
turning it red through its widget inputs needs a hook after the HUD's
widget input call (`0x0013ad48`), which puts a
guest hook on the HUD. The tested
pieces of that route stay (`superbounce_hud::autoaim_entry`,
`autoaim_exit`; input 18 is the unit's "autoaimed" input by Assembly's
`nhdt` list). The player's aim-assist target (`touch_glue::on_target`)
is not written: it is input to the simulation and the network.

The colours are the game's: lines take the user's HUD colour, the
three floats at `0x00468c84` (the human triple `0x0013927e` returns),
read from the guest each frame (`superbounce_font::hud_colour`), but the
the go green. The Elite triple and the `hudg` timer-warning colour are not
used. Labels are drawn in the disc's HUD font (font 6) with its button
glyphs, in sentence case, numbers in whole metres. No letter shape is
ours.

**The font.** `superbounce_font::Capture::keep`, called by
`PackageRewrite::observing_fonts` after the `glyphs` kit
(`crates/apps/boot/src/packages.rs`), keeps `font_table.txt` and the file on
its line 6, counted from 0, as the game reads them: the HUD's font, with
the icon sets' and bot mark's glyphs in it. The first frame that draws
packs its printable ASCII and button glyphs (U+E100 to U+E111) into a
512-wide atlas (`gfx::world_lines::LabelFont`, built in
`superbounce_font::atlas`) and publishes it. The window uploads the
atlas once per font and draws each glyph as a textured quad in the same
draw as the lines, laid out as the engine lays text out (kerning,
`x_offset`, `y_offset` from the ascent). A plain glyph takes the label's
colour; a glyph with coloured pixels (the face buttons) keeps its own. A
font pixel is a HUD pixel (the HUD is 480 lines) at `label_scale` 2, the
size the world labels use. Centred labels have a black drop shadow one
font pixel down and right; the HUD's own shadow colour
(`[0x004686d4]`) is not read.

Without the font (a host that does not mount the disc through the boot
runner, or a font that does not parse) the labels are drawn in the
window's 8x8 debug font, the X button is written `X`, and the log says
`superbounce: world labels use the 8x8 debug font and button letters`
with the reason, once. Without the HUD colour the lines are white and
the log says so, once.

### Checking it on the lab

Each of these is a lab run through the agent API
([agent-api.md](../runtime/agent-api.md), [lab.md](../runtime/lab.md)) at
30 frames a second, read every tick at pinned ticks:

- **The phases.** Play back the recorded forced crouch (above, "The
  recipe") and read `superbounce.state` with `world.object` and
  `havok.agents`: the phase is `under` from the first tick the standing
  capsule does not fit, `held` from the tick the button is up, `primed`
  on the tick the word is −3.
- **The arming bar.** With the same playback at 30, 60 and 120 frames a
  second, take `render.frame` shots through the primed tick: the bar at
  2/3 before it, full by 130 ms, wider and taller at about 210 ms, gone
  at 290 to 360 ms, narrowing from 430 ms and gone at 600 ms, the same
  at each rate. With the swell missing the effects hook did not fire for
  the bar (hud-kit.md, runtime step 4): the plain ending, gone at 430 ms.
- **The time left.** Set −3 with `havok.pair.set`, stand still, and read
  `havok.agents` until the pair goes: the fill reaches 0 on that tick,
  within 2. Then creep at 0.02, 0.05 and 0.1 units/s for the speed
  limit.
- **Each kept crouch spot, and each strip.** `superbounce.verify`
  ([Verifying spots](#verifying-spots)).
- **The landing report.** The recorded jump with the word at −3, at 0,
  from a lower take-off and 0.3 to the side: `bounce`, `not primed`,
  `too slow` and `missed` in turn. `superbounce.verify {"kind":"drop"}`
  with `prime` `set` and `none` gives the same verdicts from a fall.

No game was run for this section.

## Verifying spots

`superbounce.verify` tries the loaded map's scanned spots in the running
game and says which work
([agent-api.md](../runtime/agent-api-world.md#superbounce-the-superbounce-game-type)).
It is `crates/extensions/game/src/superbounce_verify.rs`: the plan in
`plan.rs`, a trial tick by tick in `trial.rs`, the verdicts in
`summary.rs`.

**A batch.** One call plans a batch of one kind (`crouch`, `drop` or
`route`) over the scan's spots of that kind, best score first (the order
`dry_run` lists, which `spots` indexes count in; failed spots are left out
unless named or `max_spots` is given), or over explicit `points`. An
agent ticker then runs the trials one after another on one seat, a tick
at a time, because a batch runs far longer than a deferred answer may
wait. `superbounce.verify_status` reports progress and, at the end, a
verdict and one line a spot; `superbounce.verify_cancel` stops it. A
batch needs the mode on (a game whose script requires
`hud.superbounce`) and its scan ready (`superbounce.state`'s
`scan.status`), except over
explicit points. A new map, or the game ending, fails a running batch.

**The seat.** `player.0` (the default) is driven through the first
controller's pad: the fall assist's input tap replaces the whole pad
while a trial runs, so it needs the `superbounce-assist` feature. A
brain-driven bot (by name, handle or its `player.<n>`) is driven by
holding its pad (`bots::phantom::Phantoms::hold_pad`): its brain's own
pad is ignored until the trial lets go. Trials start by teleport (the
game's own, through the agent's calls queue) and clear or set the level
pair word directly (`live_telemetry::havok_agents::set_pair_word`); they
do not use snapshots, which `state.save` refuses in a game that is not
quiescent. A teleport keeps the pair word and the velocity, so each
trial clears the word after it lands and settles before it moves.

**Crouch trials** follow the recipe: crouch, walk in, let go, walk out.
Each starts outside the lip on the approach side: `distance` out from
`stand` against the way in, which is the way out (`out`) reversed and
turned by `turn` degrees, moved by `offset`, facing in. The default grid
is turns 0, 25 and −25 by distances 0.3 and 0.6. Then:

1. settle `settle_ticks` (20) with the word written 0;
2. press crouch and stand still until the crouch is down (at least 6
   ticks, at most 20);
3. crouch-walk in at 0.6 of full stick toward a point 0.06 past `stand`,
   until within 0.08 of it (at most 60 ticks);
4. hold crouch there `hold_ticks` (15), pushing in gently;
5. let go, still pushing in, for 6 ticks; the crouch request still set
   then is `forced`;
6. walk out along `out` at full stick for `out_ticks` (30).

The word reading −3 from step 5 on primes the trial and ends it. A trial
records whether it crouched first, whether the standing capsule failed to
fit at some tick (`under`, the mode's capsule test), `forced`, how close
it got to `stand`, and the ticks from letting go to the prime and where.
A trial that did not prime fails as `not_under` (never within 0.2 of
`stand`), `not_crouched`, `not_forced` or `forced_no_word`.

**Drop trials** fall onto a strip with the word set. Each is a point in
the slot, `across` the spot's target line toward the drift (default
−0.1 to 0.1 in 0.05 steps, centred on the seam) and `along` it (default 0), and a `height` over the
strip's stand height (default 1.5, 2.5 and 3.5). The seat is teleported
onto the point, settles with the word written (`prime`: `set` −3, `none`
0, `keep` left alone), is lifted straight up, and falls; the word is
written again each tick there is a pair. Contact is the first tick the
vertical speed is back over −0.5 after falling faster than −1; the speed
in is the tick before's, the speed out the best of the contact tick and
the three after. The landing is classified as the HUD's are
(`superbounce_charge::classify`): `bounce` (out at 3.5 units/s or more),
`not_primed`, `missed`, `too_slow` or `no_bounce`; `no_touchdown` when
it never lands within `fall_ticks` (120). `prime` `none` is the control:
a strip that launches with it is not a superbounce.

**Route trials** order a bot's whole bounce route (`bots.bounce_to`) to a
point `ROUTE_RISE` (6) units straight up from each strip's stand, or to
`to`, `repeat` times (3), and watch it to its end (`route_ticks`, 3600).
A route works when it ends `done` with a touchdown that launched; the
trial carries the route's log, its touchdown and where it stopped.
Routes write nothing to the verified file.

**The verdict.** A spot one trial worked for is `proven`; one whose
trials all ran and none worked is `failed`; else `untried` (the batch
ended first, or only the setup failed: a refused teleport, the seat
gone). The best crouch trial is the quickest prime, the best drop the
biggest launch. Each spot's line reads like `crouch #3 (-10.91, 0.72,
-1.16): primed 4/9, best offset (0.06, -0.03) yaw -121 from 0.30 out;
failed: forced_no_word 5`.

**The verified file.** A batch of the scan's spots that ends (not
cancelled) merges its proven and failed spots into
`<data>/bots/<map>.verified` (`superbounce::verified`; written by
`bots::spots::store_verified`):

```text
# superbounce verified v1
crouch proven -10.910 0.720 -1.160 dx=0.060 dy=-0.030 yaw=-121.0 trials=9 ok=4
bounce failed -10.620 3.880 -3.190 dx=0.000 dy=0.000 trials=15 ok=0 best=1.71
```

A line is the kind, the verdict and the spot's `stand` as the scan gave
it, then `key=value` fields: `dx`, `dy` (a crouch's working start, a
drop's landing point, from `stand`), `yaw` (the working way in, degrees),
`height` (the working drop height), `trials`, `ok` and `best` (the
biggest launch seen). An entry is about the spot of its kind within 0.3
of its point sideways and 0.5 up or down. The scan's readers apply it:
the mode's spots (failed ones hidden, "The superbounce game type"),
`superbounce.spots` (`status`), and the bots, which route by proven spots
first, never by failed ones, start a walking prime where the working
trial started and along its way in, and aim a drop at the working
landing point from the working height ([bots.md](bots.md)). A person can
edit the file; a later batch replaces the entries it tried.

No game has run a batch yet: the defaults (the grid, the speeds, the
tick counts) are first guesses to tune on the lab.
