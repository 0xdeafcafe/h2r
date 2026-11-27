# Superbounce: the guide, HUD options and fall assist

The approach guide, the HUD options, the hold-X fall assist, the flight model and what is open. Part of [superbounce.md](superbounce.md).

### The approach guide

A pipper and a box on the seam the player is going for, drawn on the
geometry so the line-up is seen and not guessed, and before the jump a
cue on the box that says what to do to get there. They are world marks
of the readout HUD, built in the HUD kit
([hud-kit.md](hud-kit.md#the-approach-guide)); the rules are
`crates/extensions/game/src/superbounce_guide.rs`, pure, run by the mode
each tick (`superbounce_guide::guide_with_phase`, into the state's
`guide`; the state's `phase` also feeds the readout when the guide is
off).
Where to leave from is a bar on the floor, the world's
([The takeoff](#the-takeoff)).

| Mark | Where | Says |
|---|---|---|
| the pipper, a small ring with a cross | where the player's path touches down, on the seam's floor | in the air, the landing itself; on the ground, only while the cue counts to a jump's line, where that jump lands |
| the box, four corner brackets | on the seam's collision edge, at the point of it beside the landing point ([The landing point](#the-landing-point)), turned along the edge, 0.6 long and as wide as the catch (0.2): the edge runs through its middle, under the [seam line](#seam-lines) | dim while the landing is off the seam, lit when it is lined up, and in a fall the phase verdict: green `Will bounce`, or `Too shallow`, `Too high`, `Off edge`, `No drift`, `Don't crouch`; under it, before the jump, the [jump cue](#the-jump-cue) (`Climb up`, `Walk to line`, `Jump in 0.7 s`, green `Jump now`, `Walk off`) |
| the ticks, three dashes | from the pipper toward the box | how far off the landing is: they shorten as the two close and go when the landing is caught |
| the drift arrow | outside the box on the side the seam drifts to | the way to be moving at the landing; lit at 0.15 units/s or more that way, the full-launch minimum |

**Which seam** (`superbounce_guide::target`). The seam a fall is
predicted onto (the state's `landing`); else, while primed, the chosen
strip (the mode's navpoint: the tag, else [the faced seam](#the-faced-seam)); else the nearest of the tagged seam and the
target navpoints' seams within 6 units ([Targets](#targets)).

**When** (`superbounce_guide::strength`). While primed, at any
distance; not primed, never: before the charge there is nothing to land.
The pipper shows only while the
landing is within 3 units of the seam's point: the floor it is predicted
on is the seam's own, which says nothing further out. The marks are
fullscreen only, as the readout is, and the variant's `approach_guide`
[option](#the-huds-options) turns them off.

**The impact** (`superbounce_guide::impact`). The body's free flight is
flown on the [flight model](#the-flight-model) with the stick the player
holds: each tick the fall speed gains 4.171 / 30 units/s and the
sideways velocity steps toward what the stick asks for. The flight ends
on the last tick whose next would be at or under the height the seam
stops the body at (`Seam::contact`): that tick's row is the **last free
row**, its place is the pipper's and its fall speed is the touchdown
speed. A test steps a drop from each of the fall model's heights and
gets the model's row and speed. On the ground the flight is the jump
pressed on the takeoff's line, and only while the cue counts to it
(`Takeoff::leaves_at`): the run carried to the line and 5 ticks on, 3.08
units/s up (`bots::bounce_route::JUMP_VZ`), the stick held that keeps
the run going. Otherwise there is no pipper and no ticks on the ground:
the floor underfoot is not the seam's, so a jump from anywhere else says
nothing. The flight is the body's origin (object `+0x64`), the frame of
the spots' heights; the bounding sphere's centre rides over it by the
lift the mode reads (about 0.4, `CENTRE_LIFT` when it cannot be read).
`Seam::contact` gives the height the centre is stopped at, and the
flight takes the lift off it: off the catch, the floor, with the origin
0.03 over the edge's height (`FALL_CLEARANCE`); in the catch, the
drops' measured rule, which is in the centre's frame: 0.03 over the
edge, or where the far face stops it first (**Depth**, below). No
other geometry is probed, so a landing on something else on the way is
not seen, which is why the pipper stays by the seam.

**Lined up** (`superbounce_guide::caught`). The last free row is within
the seam's catch of its edge (`superbounce::catch_miss`), either side:
the pipper is inside the box.

**Depth** (`superbounce_guide::depth`). The takeoff solver and the legacy
chance estimate a landing in the catch by how low the last free row ends
over the seam: low enough, and the next tick goes deep into it
([notes/2026-11-22-seam-drop-depth.md](notes/2026-11-22-seam-drop-depth.md)).
The rule is in the seam's own frame, so it reads any seam with a drift:
with `past` the row's place past the edge along the drift (negative
before it) and `over` its height over the edge, both for the body's
bounding-sphere centre (the row's origin plus the lift: the drops'
records, which the numbers come from, are the centre's), the row is
**deep** when

```text
over − 0.75 × past < 0.445
```

and it is in the **landing window** when `past` is −0.07 to −0.01. The
three numbers are Ascension's proven seam's, measured there and nowhere
else (`SEAM_SLOPE`, `DEPTH_CUT`, `LANDING_WINDOW`): the note's cut
`z − 0.75·y < −5.755` and window y 3.93 to 3.99, against that seam's
edge at y 4.0 and z −3.2. The fall's tick phase sets the row's height,
and the phase is fixed once the body is in the air, so the depth is
known before the touchdown; about half the phases are deep. The seam
stops the body 0.135 under the cut (`DEPTH_BAND`), which is what makes a
row the last free one by the edge; that number is inferred from the
note's lowest and highest last free rows, not measured on its own.

**The call** (`superbounce_guide::phase_call`), what the box says. Each
airborne tick the mode reads the floor polygons within 4 units
(`Room::floors_near`) and runs `rule::predict` from the body's origin and
velocity with the held air stick. The `phase` keeps the verdict, the drop
from the apex to the origin standing on the deciding floor, and that
point's distance to the floor polygon's nearest edge.

| Predictor's outcome, or no landing by the seam | The box |
|---|---|
| outside the catch, or no landing | dim, off line |
| `bounce` or `bounce_fast` | green, `Will bounce`, the pipper green with it |
| `stopped` or `landing`: the sweep never came on before the floor | `Too shallow` |
| `caught`: the sweep came on too high | `Too high` |
| `pop`: through the floor away from an edge | `Off edge` |
| `level_pinch`: on an edge with too little drift across it | `No drift` |
| crouch held while primed and airborne | `Don't crouch` |
| `no_floor`, no polygons, or on the ground | lined up, no label |

On the ground the call stops at lined up: the landing is the jump's from
the line at the player's own speed, and its phase moves with every step.
There is no touchdown-speed cutoff and no percentage. The readout's bar
is full while the drop is in the 3.35 to 3.60 window and fades over the
next unit outside it; its info row says `Drop 10.5 m  Edge 0.06 m`. **Depth**, above, remains the takeoff solver's
heuristic and the agent API's legacy chance, not the airborne HUD call.

Fullscreen, while the guide is on a seam the [lock-on](#lock-on)'s
brackets are not drawn (`superbounce_world::lock_shown`): the guide's
box is on the seam in their place, and its green says the fall will bounce. In
split screen the lock-on's brackets are drawn.

`superbounce.state` returns the guide as `guide`: the seam's point, the
call, the pipper, the last free row with its ticks and speed, `caught`,
`deep` with the margin under the cut and the row's place past the edge,
the speed along the drift, the fade, in a fall the [chance](#the-chance),
`chance` (0 to 1) and `chance_capped` (the scan-only cap cut it), and on
the ground the [jump cue](#the-jump-cue): `jump_in` (ticks to `Jump
now`, while counting), `cue` (the box's words) and `takeoff` (the
takeoff, `walk` true while the cue is not a count). On the ground `deep`
is the depth of the jump from the line, an estimate that is not on the
box.

Supposed, until a lab run: that the cut and the window hold on seams
other than Ascension's and above 5.39 units/s, the fastest drop
measured; that a jump leaves the ground at 3.08 units/s with the run's
speed kept, 5 ticks after the press; where the seam stops the body
(`DEPTH_BAND`); and the chance's two softnesses and its cap. A
`superbounce.state` poll through a fall beside `sim.drops`' last free
row for the same drop checks the row and the call.

#### The takeoff

The takeoff is a fixed place on a real floor to leave from for the
chosen strip, and how: a jump pressed on a line, or a walk off a lip
(`superbounce_guide::place`, placed each tick by the mode,
`superbounce_mode::takeoff_for`, into the state's `takeoff`).

**The lip.** Where to look comes first from a run: the strip's kept
ghost or verified run (`superbounce_ghost::lip`, [Ghost runs](#ghost-runs)),
the last ground frame before the flight that touched down, the way and
speed of its first tick in the air, and whether that tick was a jump.
Without one, or when its lip is on no floor, the map is searched
(`superbounce_world::lip`): from the strip's `stand` against its drift
first, then in a fan 30 degrees at a time round it to straight behind
(`LIP_FAN`; round the way to the player for a seam with no drift), in
0.05 steps (`TAKEOFF_STEP`) up to 4 units (`TAKEOFF_REACH`), the first
floor at least the strip's `fall` above it (less 0.05) and at most a
unit over that (`TAKEOFF_ABOVE`) is the lip, heading back at the strip.
The player's height does not limit the search, so from the seam's own
floor the ledge to climb to is found. A strip a standing jump reaches
(fall at most 0.05) takes the prompts' take-off, 0.2 back from `stand`
against the drift.

**The two flights.** The lip is put on the floor under it
(`superbounce_capsule::Room::floor_at`, within 0.1, `SAME_FLOOR`; none
there, no takeoff). Both flights are at the run's speed, the run's own,
else 0.8 units/s (`WALK_UP`, a walking approach), along the lip's
heading, flown on the [flight model](#the-flight-model) holding the
stick that keeps that run going (`superbounce_guide::solve`), as the
[impact](#the-approach-guide) is:

- **a walk-off** from the lip, when it is an edge: the floor 0.1 past it
  (`EDGE_PROBE`) is not its own;
- **a jump** from each place back along the lip's floor, 0.005 apart
  (`TAKEOFF_STEP`), up to 4 units (`JUMP_REACH`) or the floor's end: the
  first band of them, from the lip back, whose landing is good. A press
  leaves the floor 5 ticks later (`PRESS_LEAD`, the drill runs' 4 to 6,
  [notes/2026-11-21-bot-bounce-sim.md](notes/2026-11-21-bot-bounce-sim.md)),
  so the line is the band moved back by the ground of 5 ticks: where to
  press. Only near the landing window is every step tried: a stretch of
  0.04 (`TAKEOFF_COARSE`) whose two ends land clear of the window on the
  same side, by 0.12 (`TAKEOFF_CLEAR`), is passed over, and a test
  checks the band is the step-by-step scan's.

The one kept is the first of: a walk-off the box would call `Will
bounce`, a jump band that would, a walk-off that lands in the seam's
catch at its need (`superbounce_charge::need_of`), a jump band that
does. With none, the lip itself is kept unchecked, as the run left it,
else a walk-off from an edge and a jump from anywhere else: still the
place to go, drawn dim. A jump's flight is fixed by where it leaves the
floor, so the landing window, 0.06 wide, is a band of takeoffs just as
wide; the line is for the run's speed, and at another speed the pipper
shows where the jump comes down instead.

**On the map** (`superbounce_world::takeoff_figure`). Primed on the
ground, a bar 0.6 long across the run at the takeoff, an arrow just past
it the way to leave, and `Jump from here` or `Drop from here` on it.
While the player's floor is too low for the strip (feet under its
`fall` above `stand`, less 0.05) the pin and the HUD's navpoint
(`superbounce_mode::navpoint`, the state's `navpoint`) are on the
takeoff, and the strip has no `Land` label ([Floor marks](#floor-marks)).

#### The jump cue

Before the jump the box says what to do, by where the player is against
the fixed takeoff (`superbounce_guide::Takeoff::cue`, `Cue`):

| Cue | When |
|---|---|
| `Climb up` | the feet are more than 0.3 (`LEVEL`) under the takeoff's floor |
| `Walk to line` | more than that over it; or on it, a jump's, and not running at its line |
| `Jump in 0.7 s` | on it, running at a jump's line: at 0.05 units/s or more along its heading (`MOVING`), at most 45 degrees off it, within 0.5 of its centre across the run (`LINE_ACROSS`), and short of the band's far end |
| `Jump now` | the same, within a tick of the band (`NOW_EARLY`) or in it |
| `Walk off` | on a walk-off's floor |

**The count.** `Jump in N`, N the time to the line's near end at the
current speed along its heading, in tenths of a second rounded up
(`Jump in 0.7 s`): at 30 ticks a second a tenth is three ticks, so the
count steps smoothly and its last value before `Jump now` is `0.1`.
`Jump now`, in the go green with the pipper, shows from one tick before
the player reaches the band until they leave it, the tolerance for the
eye and the thumb.

The cue's five words are `superbounce_guide::Cue::text`; the HUD kit's
box shows `Walk to line`, the count and `Jump now`
(`superbounce_guide::kit_cue`), and the bar on the floor says where.

**Steady** (`settle`). The cue is settled against the last tick's on
the same seam (`Guide::settled`, in the mode): a count a tenth back up,
`Jump now` against a count of `0.1`, and a count against no line or
`Walk to line` are each held for at most 3 ticks (`CUE_GRACE`), so
jitter in the player's speed does not flicker the box. A count down, a
new line, `Walk to line`, `Climb up`, `Walk off` and the end of `Jump
now` show at once; `Jump now` never outlasts its band.

#### The chance

How sure the depth heuristic is in a fall while primed on a seam with a
drift (`superbounce_guide::chance`). This is a legacy agent-API estimate,
not the predictor's phase verdict. The HUD shows no percentage: the box
carries the verdict alone and the readout's fall row the steer text
alone. The agent API reports the chance from 0 to 1.

**In the air** the fall is all but decided, so it is a confidence
(`confidence`): how far the predicted last free row is from the two
lines that decide the bounce, against how well those lines are known.
With `margin` the row's depth under the cut over 0.02 (`DEPTH_SOFT`)
and `inside` its distance inside the nearer end of the landing window
over 0.01 (`WINDOW_SOFT`):

```text
confidence = 0.5 + 0.45 × tanh(min(margin, inside))
```

| The last free row | Confidence |
|---|---|
| deep and in the window's middle | 95% (`CONFIDENCE_TOP`) |
| 0.02 under the cut, or 0.01 inside an end of the window | 84% |
| on the cut, or on an end of the window | 50% |
| 0.003 under or over the cut, the width the cut is measured to | 57%, 43% |
| 0.05 over the cut, or 0.03 outside the window | 5 to 6% |
| outside the catch | 5% |

A fall slower than 5.2 units/s has no chance (`DEPTH_SPEED`). This cutoff
belongs only to the legacy estimate; the airborne phase verdict has none.

**Capped by evidence** (`cap`). On a seam with a measured landing
(`Spot::landed`: a verified entry's or a learned line's, [The landing
point](#the-landing-point)) the percentage is as worked out. On a seam
that is the scan's alone it is 60% at most (`SCAN_ONLY_CAP`): the cut
and the window are Ascension's proven seam's and are measured nowhere
else
([notes/2026-11-22-seam-drop-depth.md](notes/2026-11-22-seam-drop-depth.md)),
so there the rule itself is a guess.

The two softnesses are inferred, not measured. The cut is found to
±0.003 and the window's low end to 0.01 (y 3.93 bounced, 3.92 did
not); how far the predicted row is from the row the game takes is not
measured, and `DEPTH_SOFT` stands in for it.

### The HUD's options

Which pieces of the HUD a game shows is the variant's: they are options
of the Superbounce rules script
(`content/modes/rules/superbounce.rules`), carried in the
variant record's option bits after the four scoring options
([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)), each on
unless a variant turns it off:

| Option | Type, default | Off |
|---|---|---|
| `targets` | `common`, `likely` or `all`; `likely` | picks the [targets](#targets)' classes a game starts with (the player can switch them in the game); it has no off |
| `velocity` | bool, on | no speed row |
| `alignment` | bool, on | no fall row: no bar, no steer line |
| `miss_info` | bool, on | no miss row |
| `medals` | bool, on | no medal line in the notification list and no announcer line; the chain, the score and the recorded events are unchanged ([Medals](#medals)) |
| `second_bounce` | bool, on | no ranked seams after a bounce ([The second bounce](#the-second-bounce)): the navpoints, the last row and the fall row are as on any other fall |
| `approach_guide` | bool, on | no pipper, box, ticks or drift arrow, no jump cue, and no percentage on the box or the fall row ([The approach guide](#the-approach-guide)); the lock-on's brackets are drawn on a fall as in split screen |
| `charge_segments` | bool, on | no segments round the motion tracker ([hud-kit.md](hud-kit.md), "The charge segments"); priming is the arming bar, the prompt line and the notifications |
| `pogo` | bool, on | no Pogo Stick: the secondary weapon is the variant's own ([pogo-stick.md](pogo-stick.md)) |
| `pogo_style` | `unsc`, `covenant` or `forerunner`; `unsc` | picks the stick's look; it has no off |

The host reads them by name where it decides the mode is on, at a game's
start and every 30 ticks (`superbounce_options::read`): the active
variant's extension block (`variant_ext::in_game`), its script
reference, the script's IR (`rules::header::superbounce_options`,
`Ir::option_value`). A script that declares none of them takes the defaults.
`superbounce.state` returns the HUD's as `options`, and `pogo.state` the
stick's two. The stick's are also read by the spawn's weapon pick, from
guest memory alone ([pogo-stick.md](pogo-stick.md#the-loadout)).

### Hold X: the fall assist

An opt-in help: with the player's **Fall Assist** setting on
(`superbounce.fall_assist`, off by default, a row at the end of the
Controls list that a press turns on or off; `crates/extensions/game/src/setting_defs.rs`,
`crates/extensions/game/src/video_screen.rs`), holding X in the air
steers the player onto a bounce strip, so the landing is on the strip,
moving slowly along its drift, the way every traced bounce came in. Off,
the tap never reads X, nothing steers, and nothing on the map or the HUD
asks for X. The setting is read live (`follow_setting`); the
`superbounce-assist` feature in `H2_FEATURES` stays the kill switch, which
leaves the tap out whatever the setting says. It works on input, not
physics: the assist replaces the left stick
the guest reads and takes X out of it, and writes nothing to the world. It
is `crates/extensions/game/src/superbounce_assist.rs`; `superbounce.assist`
and `superbounce.assist_test`
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
show it and hold the button without a pad.

**When it applies.** The mode is on, the setting is on, the first local
player's X is held
(pad X, keyboard E by default; the `BUTTON` constant), the player is
airborne and falling (vertical speed at most 0.5 units/s), and a bounce spot
passes the reach cone (`judge`), each part flown on the
[flight model](#the-flight-model):

- it is below the player, and the flight with the stick held as it is
  comes down to its height within 3 s (`MAX_TIME`);
- its point to land on is within 3 units sideways. A strip bounces along
  its whole length, so the point is the stand point moved along the
  strip's edge to where that flight comes down, a capsule's
  radius (0.175) inside its ends (`stand_near`): a fall onto one end of a
  long strip is not pulled to its middle;
- the pursuit of the aim (`superbounce_flight::pursue`) comes down
  within 0.4 units of it (`HOLD_SLACK`): the stick, as measured, can make
  up the rest in the time left. The cone is wider the longer the fall;
- no floor is between the player and the point (`covered`): the highest
  floor under it, up to 0.5 over the player's feet
  (`superbounce_capsule::Room::floor_at`), is no more than 0.3 above it.
  A strip under a slab, a bridge or the ledge the player left is refused;
- nothing solid stands between them: the sight line from 0.3 over the
  player's origin to 0.25 over the point is clear of the scan's collision
  (`superbounce::view::Geometry`).

**Which strip.** Every bounce strip whose edge is within 6 units sideways
is judged, and of those that pass (`choose`):

1. the strip the read before steered onto, while it still passes, so the
   target never changes in the middle of a fall;
2. else the strip the HUD marks, the navpoint's and the world marker's
   ([Floor marks](#floor-marks); `set_marked`, from the mode each tick),
   so the pull is toward the mark the player sees;
3. else the strip the held landing misses by least, one the fall is
   fast enough for (touchdown speed at least its `need`) before one it is
   too slow for.

`superbounce.assist` returns the choice: the reason, each strip judged
with its verdict, and the read the assist last engaged on
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

**The steer.** The aim is the strip's
[landing point](#the-landing-point) (`aim_at`: a measured landing as it
is, a scan's point 0.1 units back along the spot's drift). Each read
the stick is the [flight model](#the-flight-model)'s pursuit of it
(`superbounce_flight::pursue`): the velocity that covers the rest of the
way in the ticks left, as a stick, in world axes. In the air the stick
asks for a velocity, not a push, so `world_to_stick` turns it as it is
into the left stick in the aim's frame (forward is `(cos yaw, sin yaw)`,
right `(sin yaw, -cos yaw)`), cut back to the unit disc. Read again each
tick, the pursuit corrects what the last one left: a simulated fall
against an air stick 0.7 to 1.4 times the measured one still lands
within 0.1 of the aim.

**The button.** While the assist is engaged X reads as released, so the
button does not reload or pick up. Once it has engaged, X stays released
until the player lets go, so a landing or a strip going out of reach does
not hand the game a fresh press. With no strip in reach, X and the stick
pass through unchanged.

**The prompt.** With the setting on, the assist's choice is also what the
world prompts are drawn from, whether or not X is held: the window reads
the player's body and makes the tap's own choice on each frame
(`choose_now`, keeping the strip the tap is steering onto), so `Hold X`
appears on the strip the stick would be steered onto before the button
is pressed, and turns to `Guiding` once the
tap reports it is engaged (`assisting`; [the prompts](#the-superbounce-game-type)).
Off, there is no choice: no prompt in the air. The [lock-on](#lock-on)
does not depend on the assist: it closes in on the predicted landing
either way.

**Where it sits.** An input tap at `XInputGetState`
(`AssistTap`, installed by `register_late` before the agent's tap), so
the agent's `input.pad` merges first and the assist sees that pad. With the
mode off a pad read costs one atomic load. The tap changes the packet before
the recording or replay tap under it sees it: a film or input log records
the pad the guest got, and a replay overwrites the packet, so the assist
never runs in a replay. The `superbounce-assist` feature has no guest hooks,
so a film's hook digest does not include it, and it is snapshot-stateless:
the swallowed-button latch and the test hold are momentary.

The air stick's numbers are the bots' measured ones
([The flight model](#the-flight-model)); the guest's own dead zone on a
replaced stick is not measured, and is the first thing to check in a lab
run.

### The flight model

Every landing the mode predicts is flown one game tick at a time on one
model (`crates/extensions/game/src/superbounce_flight.rs`): the approach
guide's [impact](#the-approach-guide) and [takeoff](#the-takeoff), the
[fall assist](#hold-x-the-fall-assist)'s gate and stick, the
[landing](#landings) a fall is headed for and the
[second bounce](#the-second-bounce)'s seams. Each tick:

1. the fall speed gains 4.171 / 30 units/s (`FALL_GRAVITY`, the sim's
   fall model, `sim_bench::drops`);
2. the sideways velocity steps toward the stick times 1.2 units/s
   (`AIR_TOP`) by at most 0.035 units/s (`AIR_ACCEL`, 1.05 units/s²);
   a stick under 0.2 (`MIN_STICK`) is no stick, so a let-go stick
   brakes the body to a stop at that rate;
3. the body moves its new velocity over 30.

The air stick's numbers are the bots' drill runs'
(`bots::bounce_route`, [notes/2026-11-21-bot-bounce-sim.md](notes/2026-11-21-bot-bounce-sim.md)):
0.035 units/s a tick in every run, braking and pushing, and flights held
at 0.9 to 1.3 under a full stick. So the air does not keep a run: a
run-jump at 2.0 units/s and 3.08 up, which would coast 2.87 on in the 43
ticks back to its floor, brakes to 1.2 with the stick held on (2.01 on)
and toward a stop with it let go (1.76), and a test holds those numbers.
The flight
ends on the last free row: the last tick whose next would be at or under
the surface.

- **Held** (`hold`): the stick the player holds, read from the unit's
  throttle each tick and turned into world axes by the aim's yaw
  (`held`). Without a read, the stick that keeps the velocity
  (`keep`: the velocity over 1.2, cut to the unit disc).
- **Pursued** (`pursue`): the stick each tick that heads for a point:
  the velocity that covers the rest of the way in the ticks left of a
  plumb fall to it, as a stick. It ends on the point when the stick can
  get there, and as near as it can when it cannot; that miss is what the
  fall assist and the second bounce gate on, and its first stick is the
  assist's and the landing's `steer`.

The flight is the body's origin (object `+0x64`). The bounding sphere's
centre (`+0x30`) rides about 0.4 over it and moves with the pose; the
approach guide's [depth](#the-approach-guide) rule is measured on the
centre, so the guide adds the
lift the mode reads to a row before it reads the depth.

## Open

- **The last 5 units/s.** The upward part of the two impulses on the
  touchdown tick gives 23 units/s over the body's mass; the body gains
  17.85 ([touchdown tick](#the-touchdown-tick-trace-stepped)). What the
  virtual `+0x74` before the −3 write does isn't read either.
- **What drops the pair.** Entering a crouch and the body going to
  sleep both rebuild it at 0, which ends the effect. How long a stand it
  takes to sleep isn't measured.
- **60 frames a second.** The player reports no bounces at 60. That isn't
  measured here. The sweep's threshold is a speed and its length one
  tick ([the bounce rule](#the-bounce-rule-code-trace)), so a 60 Hz tick
  would move every window.
- **The launch rule.** [The bounce rule](#the-bounce-rule-code-trace)
  says when the two contacts pinch. How the convex agent (`0x002eeb40`)
  picks the sideways normal, and how the solver (the step at
  `0x00311100`) turns two impulses of 24,000 to 134,000 into 18.5 to 22
  units/s, are not decoded.
- **Lunges.** Whether a landing on the crease during the 1.5 s after a
  lunge (body offset from the origin) bounces differently is not
  traced.
- **Other bounce spots.** Every edge between two floor polygons bounces
  on Ascension ([the bounce rule](#the-bounce-rule-code-trace)). Other
  maps and other bipeds are not measured.
