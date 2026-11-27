# Superbounce: the game type

The superbounce game type: targets and the second bounce. Part of [superbounce.md](superbounce.md).

## The superbounce game type

A built-in variant, Superbounce, for practising: Bounce Practice's
rules (no score or time limit, 3 s respawn,
overshields, damage resistance, a magnum, nothing on the map) plus a HUD
and lines over the map. It is listed under **Party Games**, one of our rows at
the bottom of the game type list (after Territories), and not under Slayer; underneath
it is a Slayer variant ([community-gametypes.md](community-gametypes.md#tweakers)).
Its entry is built from its script, one of `rules::builtin::LISTED`
(`crates/extensions/game/src/community_variants.rs`,
[rules-variants.md](rules-variants.md#built-in-list-entries)): the
script's record names it (mode 2, the script's hash prefix and default
options), and `community_variants::write_blocks` writes that block into
its built-in record when the list loads, so a game on it and a saved
copy of it name the script. The script is
`content/modes/rules/superbounce.rules`, compiled into the
game: `type party` files the variant under Party Games and
`requires hud.superbounce` turns the mode on
([rules-engine.md](rules-engine.md#firefight-ai-and-bounces)). It scores
bounces alone, and keeps their chain itself on what the medal chain
([Medals](superbounce-ghosts-medals.md#medals)) saw: each landing judged a bounce (`on launch`, with
its strip and its launch) and each end of a run (`on grounded`). A
launch while the run is live adds one to the count, any other starts it
at 1, and a launch over 16 world units a second (`MONSTER_VZ`) is a
Monster, so the script's counts are the medals' (checked tick for tick
on every chain scenario, `superbounce_chain_twin.rs`). Every bounce
scores, a bounce off a seam no marked strip accounts for too. A kill
scores nothing:

| Event | Points, by default | Option |
|---|---|---|
| a bounce | `bounce_points` (1) times its place in its chain, at most 6: a single 1, a double 2 (3 for the chain of two), the sixth and every bounce after it 6 | `bounce_points`, 0 to 5; 0 scores no bounce |
| a Monster Bounce | twice the bounce's, after the cap: a Monster single 2, a Monster eighth of a chain 12 | `monster_double`, on |
| a Bounced | −1, once: the suicide point while `suicide_costs` is on, else the Bounced penalty | `bounced_costs`, on |
| a suicide (a death with no killer, or by the player's own hand) | −1 | `suicide_costs`, on |
| a kill | nothing | none |

The cap is Bouncetacular's count, so a long chain cannot run away. A
kill scores nothing, so the board counts bounces; a betrayal costs
nothing either. The suicide point stays, so dying on purpose to reset a
bounce attempt is not free. A
Bounced is a death with no killer too, so while `suicide_costs` is on,
`on kill` takes its point and `on bounced` takes none: a Bounced costs one
point either way (supposed: that the game reports a fall out of the map as
a death with no killer; a kill feed line on a lab run would show it).

The mode is on only when the variant's extension block names a rules
script that requires `hud.superbounce`
(`rules::header::script_asks_superbounce`, in
`variant_ext::Decision::superbounce`, with or without the `rules`
feature, on a guest that applies its host's block too). The built-in
Superbounce and Bounce Practice scripts do. The block's flag byte has no
superbounce bit (bit 2 is reserved and reads as off), the variant's name
turns nothing on, and no switch does. An agent can force the mode off in
any game with `superbounce.set`; it cannot force it on
([agent-api.md](../runtime/agent-api-world.md#superbounce-the-superbounce-game-type)). It is
`crates/extensions/game/src/superbounce_mode.rs`.

Each tick, what the medal chain saw of each player's physics goes to a
running rules script first: `on touchdown(p, down, up)` for a landing,
`on launch(p, spot, speed)` for a landing judged a bounce, `on
grounded(p)` when a run ends ([rules-world-api.md](rules-world-api-reference.md#physics)).
Then each bounce the chain files off a marked strip goes as
`on bounce(p, spot, chain, monster)`, `spot` the strip's id
(`superbounce_chain::spot_id`), `chain` its count and `monster` whether
it is a Monster Bounce; each Bounced goes as `on bounced(p)`.

**State.** On or off is decided when a game starts and again every 30
ticks (so a guest's late host block counts); off, a tick costs a counter
and a flag. On, each tick reads the first local player's unit, its
origin (object `+0x64`, `superbounce_mode::OBJECT_POSITION`: the point a
teleport places and the frame of every spot's heights), how far its
bounding sphere's centre (`+0x30`, `OBJECT_CENTRE`) sits over that
origin, whether its biped is airborne (biped flags bit 0, as
`world.object`'s `physics.airborne` reads it), its velocity, the stick
it holds (the unit's throttle, turned into world axes by the aim's yaw,
for the [flight model](superbounce-guide.md#the-flight-model)), its
aim (the vector, and as a yaw: the first-person camera looks along it),
its flashlight action and the word of its
one pair against the level (pair `+0x0c`,
`live_telemetry::havok_agents::fixed_pair_word`, which checks handles and
islands and decodes no collision-agent cache). A word of −3 is primed. It
publishes that with the nearest bounce spot; the HUD, the lines and
`superbounce.state` ([agent-api.md](../runtime/agent-api-world.md#superbounce-the-superbounce-game-type))
all read it.

**Spots.** The first tick on a map and structure BSP starts a scan of the
BSP's collision geometry on a background thread (`superbounce::scan_in`
over the map's context, `superbounce::map_context`: its starting
locations and proven places, "Spots in the geometry"); the result is kept
per map name, BSP and the map file's header checksum, so an override with
the same name is scanned again, and a scan still running when the map is
left is not started twice. The map's verified file ("Verifying spots") is
applied to the result, and again whenever the file changes: a `proven`
spot scores 1, a `failed` one is kept for `superbounce.verify` but hidden
from the HUD, the lines and the overlay. The feature is presentation-only (a recorded film's hook digest
ignores it) and snapshot-stateless: nothing it holds needs saving.

**The bots.** While the mode is on the bots fight no one and practise
bounces instead, spread over the map's sites, proven ones first
([bots.md](bots.md#practising-bounces)). The nearest
bounce spot is the one with the nearest edge.

**The HUD.** Drawn by Halo 2's own HUD through the HUD kit
([hud-kit.md](hud-kit.md), "The superbounce HUD on the kit"), for the
first local player, in one block at the bottom left beside the motion
tracker: nine charge segments round the tracker's left, bottom and
right sides, cut from the energy sword's battery strip, which fill a
side for each of the recipe's steps and wipe back as the charge runs
out ("The charge segments" in hud-kit.md); Assault's red arming bar
over the readout's rows, filled by the recipe's steps, with a short
ending when primed
("The arming bar" in hud-kit.md); "Primed!" in the
notification list, as Assault's "Bomb armed!";
the step's verb on the game's prompt line ("Hold (crouch) and walk under", "Let go
of (crouch)", "Walk out to stand up"); and the game's navpoint over a
crouch spot while not primed, with the distance in metres, and over the
chosen strip while primed, with the drop in metres
([Floor marks](superbounce-landings.md#floor-marks)).
`crates/extensions/game/src/superbounce_hud.rs` registers it with the
kit and feeds it each tick from the mode's state (`view`); with the mode
off every element is hidden. Launch and drop are on the strips' labels
in the world and in the agent API, not on the HUD.

**The readout.** Three rows right of the motion tracker and a fourth
over the arming bar, a second HUD on
the kit ([hud-kit.md](hud-kit.md#the-superbounce-readout);
`crates/extensions/game/src/superbounce_readout.rs`), fullscreen only:

| Row | Shows when | Says |
|---|---|---|
| targets | the scan is ready and the map has a spot | the kinds of seam the [targets](#targets) are on, how many of them, and the flashlight's glyph twice, the double tap that switches them: `Targets: Likely (4)  (W)(W)`; it flashes for two seconds after a switch |
| speed | moving | an arrow for the way of travel against the aim and the horizontal speed in metres a second, `(arrow) 7.2 m/s`; while falling, the fall speed after it |
| fall | airborne and falling toward a strip (the state's `landing`) | a bar, the touchdown speed the fall gives over the speed the strip needs, full when fast enough; beside it `On seam` when the landing with the stick held as it is ([the flight model](superbounce-guide.md#the-flight-model)) is caught and drifting the seam's way, `(arrow) Drift` when it is caught and is not, `Out of reach` (no arrow, the bar kept) when it is off the seam and the stick cannot bring it in (the prediction's `Course::Miss`), else an arrow for the way to push and how far the landing is from the seam's [landing point](superbounce-spots.md#the-landing-point), `(arrow) 0.40 m`; no percentage follows |
| miss | after a landing by a strip that did not bounce: held three seconds, then until the next real fall toward a strip (falling at 1 unit/s or faster with a landing predicted), five seconds at most; a bounce takes it down at once. A hop, a jump or a step off a kerb in those three seconds leaves it, as does a landing with no line to give. Between lives its clock stops, so it stays through the respawn countdown | `Not primed`, `Missed seam by 0.31 m` (the origin's distance to the edge), `Too slow: 12.4 of 16.5 m/s` (the speed in and the strip's need), `No bounce: on seam at 17.2 m/s`, or `Fell through at 17.2 m/s`, which outranks the others: it replaces any line, and only a bounce replaces it. In the hold another miss replaces the line only with a higher speed in |

Nothing of the mode's HUD is in the middle of the screen but the arming
bar, at Assault's place, and the game's navpoints, which are where their
spots are. The readout's three navpoints are the seam
[targets](#targets). Each row but the targets row is an
[option](superbounce-guide.md#the-huds-options) of the variant: the targets row answers the
player's own press.

```sh
h2 hud preview superbounce --state all --mode all --out DIR
```

`h2 hud preview` draws it offline over the retail HUD, from the disc or
the synthetic map, with the kit's checks (hud-kit.md, "The preview and
the checks").

**The tutorial.** Four steps in the game's training box, two centred
lines each. The box is in the middle of the screen, so they show only
when `H2_SUPERBOUNCE_TUTORIAL=1` ([switches](../runtime/switches.md))
asks for them:

1. Crouch, walk in under a lip, let go, then walk out.
2. You are primed. Do not crouch again.
3. Follow the navpoint. Jump from the Jump mark onto the Land mark.
4. Jump or fall onto the strip. Fall Assist (Controls) can steer.

No step asks for a button: the landing is the player's own, and the
last step names the opt-in [fall assist](superbounce-guide.md#hold-x-the-fall-assist) once,
by its row.

Priming moves step 1 to 2, a bounce spot within 8 m moves 2 to 3, a
bounce landing or 20 s moves 3 to 4, and a bounce or 15 s closes 4
(`hud_kit_core::huds::superbounce::Tutorial`), in game seconds: 600 and
450 ticks at any frame rate. A training text of the
game's own takes the box while it shows. Closing the last step sets the
`superbounce.tutorial_seen` setting.

**The lines.** One marker, on the one spot the player is going for,
drawn by the window's world-line pass with no depth test (the styles are
described in
[the agent API](../runtime/agent-api-edit.md#drawing-world-space-lines-over-the-frame)),
from `crates/extensions/game/src/superbounce_world.rs` (`frame`,
`targets`, `figures`, and the lock-on's `LockMarks`): a bold ring on the
floor with a pin standing over it and a short label at the pin's top, as
[Floor marks](superbounce-landings.md#floor-marks) and
[Crouch spots in the world](superbounce-landings.md#crouch-spots-in-the-world) describe, and
while falling the [lock-on](superbounce-landings.md#lock-on), the slot's outline.

- The spot is the navpoint's: not primed, the nearest crouch spot;
  primed, the chosen strip, which the
  [fall assist](superbounce-guide.md#hold-x-the-fall-assist) steers onto.
- No other spot has a marker: no field of rings, no lines between
  spots. While primed, each seam of the targets' set has a line on its
  collision edge ([Seam lines](#seam-lines)). The whole level's spots
  are on the flashlight
  ([The bounce view](superbounce-ghosts-medals.md#the-bounce-view)), when the player asks.
- Not primed and further than 6 world units from the next crouch spot
  (`NEAR_RANGE`), nothing is drawn in the world; the HUD's navpoint
  leads there.
- The marker does not fade with distance: it shows from across the map,
  under the HUD's navpoint.
- Lines are in the user's HUD colour and labels in the HUD's font
  (`crates/extensions/game/src/superbounce_font.rs`, below). The lock is
  the one exception: red, as the game's reticle is over an enemy in
  range.
- The marks are rebuilt each guest tick (30 Hz); what moves between
  ticks (the lock-on closing in, the pulses) moves by the wall clock at
  each present, so it is as smooth at 60 or 120 frames a second as at 30
  ([high-framerate.md](high-framerate.md)).

**Which spots are marked.** The mode marks, points at and steers onto a
subset of the scan (`superbounce_mode::marked`, applied where the map's
verified file is, so the marker, the navpoint, the tracker, the bounce
view and the fall assist agree):

- a spot a game run failed is never marked;
- every other crouch spot is;
- a strip is marked by its `verified` flag (`superbounce_mode::verified`:
  the verified file made it `proven`, [Verifying spots](superbounce-landings.md#verifying-spots))
  once any strip of the map has one: then only proved strips are marked;
- a strip the map's learned file has a `learned` or `partial` line for
  is marked at the line's aim ([Bounce](superbounce-spots.md#bounce)), not proved by it;
- on a map with no proved strip, the scan's two-joint slots are marked
  and its [single creases and Vs](superbounce-spots.md#single-creases) are not: they are
  candidates for the sim's training (`Spot::candidate`);
- a proven V's arm is marked only for a player on the side it is
  approached from, the far side of the V's bisector from the arm
  (`superbounce_mode::for_player`, each tick at the player's position):
  the arm across from the player, the mirror of their approach
  ([Vs](superbounce-spots.md#single-creases)). The medals still judge every player's
  landings on every marked spot.

`superbounce.verify` and the bots still see every spot.

#### In the air

While the player is airborne the screen is the
readout's and the motion tracker's. The
HUD kit's superbounce elements keep the notification and the
medal line and drop the arming bar, the prompt line,
the navpoint and the tutorial step (`superbounce_hud::in_air`); the
readout's target navpoints and charge segments stay. The
world keeps the chosen strip's rings and line and the lock-on, and no
label. The mode draws no prompt over a strip:
`crates/extensions/game/src/superbounce_prompts.rs` keeps `prompt_for`'s
table (`Hold X`, `Guiding`, `Jump`, `Drop 3 m`) and its label rules,
which nothing draws.

### Targets

Seams get the game's own navpoint, the arrow pointing down at the seam's
[landing point](superbounce-spots.md#the-landing-point) (the spot's `stand`: a proven seam's
measured landing, a learned line's aim, else the scan's point on the
collision edge) with the waypoint marker above it. A navpoint off screen is
kept at the edge of the waypoint area by the game, so a target is found
from any height or facing; nothing fades with distance.
`crates/extensions/game/src/superbounce_targets.rs` says where they go;
the readout's three navpoints draw them
([hud-kit.md](hud-kit.md#the-superbounce-readout)).

**Classes.** Every bounce spot of the scan, the map's verified and
learned files applied (`superbounce_mode::scanned_spots`), has one class
(`superbounce_targets::class`), the best its data gives:

| Class | Data |
|---|---|
| known | proven: a game run bounced on it (the map's verified file, [Verifying spots](superbounce-landings.md#verifying-spots)), or it is a measured place (`Status::Proven`) |
| learned | the sim's training closed on it: a `learned` line with a hit in the map's learned file ([bounce-search.md](bounce-search.md)) |
| partial | a `partial` line with a hit: it bounced, and the budget ran out |
| slot | the scan's two-joint slot, the shape measured on Ascension ([Bounce](superbounce-spots.md#bounce)) |
| candidate | scan only: a [single crease](superbounce-spots.md#single-creases), a V's arm or tip, a [busy point](superbounce-spots.md#busy-points) |

A spot a game run failed has no class and is never a target; a control
line of the learned file says nothing.

**Modes.** The variant's `targets` option picks the classes shown
(`superbounce_targets::in_mode`), each mode holding the one before it:

| `targets` | Classes | Reads as |
|---|---|---|
| `common` | known | the seams people use |
| `likely`, the default | known, learned, partial, slot | what should work |
| `all` | every class | every seam the scan found that no run failed |

On a map with nothing proven or measured, `common` shows no target. The
list is classed once per scan, file change and mode.

**Switching the kinds in the game.** Two taps of the flashlight within
0.4 s (12 ticks from one let-go to the next, `DOUBLE_TICKS`) move the
mode on: common, likely, all and round again
(`superbounce_targets::Switch`, `TargetMode::next`). The variant's
option is where a game starts; the player's switch lasts that game. The
readout's targets row, over the arming bar, always says which kinds are
shown, how many seams of them the map has, and how to change them, the
flashlight's glyph twice: `Targets: Likely (4)  (W)(W)`, with X's glyph
on Bumper Jumper ([hud-kit.md](hud-kit.md#the-superbounce-readout)). It
shows once the scan is ready and the map has a spot; `common` on a map
with nothing proven reads `Targets: Common (0)`. A switch names the new
kinds on the row at once and flashes it for two seconds. The
flashlight is the mode's one button: the game binds it on every input
set and layout, a tap is the bounce view's pulse and a hold its marks
and the tag, and after two taps a biped's light is as it was.

**Navpoints.** Two seams have one: the chosen strip (the mode's own,
on the ground) and the tag. The other seams of the set are
[seam lines](#seam-lines), not navpoints: `sbr.target.1` and
`sbr.target.2` are the [second bounce](#the-second-bounce)'s ranked
seams, in the flight after a bounce, and are empty otherwise
(`superbounce_mode::on_tick`). `superbounce_targets::choose` still
works out the two nearest seams the player can reach from their height
(`reachable`: the feet at least the seam's `fall` above its `stand`,
less 0.05), which the approach guide falls back on.

#### Seam lines

While primed, every seam of the targets' set has a line on the map,
drawn on its collision edge from vertex to vertex, 0.02 over it: the
edge the bounce happens on, to the float the map stores
([the target](superbounce-spots.md#the-target-the-collision-edge)). The rules are
`crates/extensions/game/src/superbounce_seams.rs` (`lines`, `segments`),
pure; `superbounce_world::frame` draws them (`seam_callouts`) in the
HUD's colour through the world-line pass, with the sight test the other
marks have (a line behind the level is dim).

| Seam | Line |
|---|---|
| the chosen strip | 4 px, full alpha at any distance, with a tick across each end as wide as the catch (0.1 either side) |
| any other seam of the set | 2.5 px at 0.6 alpha, fading from 25 units out to nothing at 45 |
| a seam with no length (a busy point, a V's tip) | a cross on its vertex, 0.1 each way |

A V's arm has its line only from across its bisector, as its marks.
Switching the targets' kinds changes the lines at once: the set is the
one the readout's `Targets` row names.

#### The faced seam

Primed, with no tag and no strip being fallen onto, the chosen strip is
the seam of the targets' set the player faces
(`superbounce_seams::faced`, kept tick to tick by `Facing`;
`superbounce_mode::faced_target`), so the navpoint, the takeoff, the
approach guide, the bold seam line and the fall assist follow where the
player is going. A tag outranks it, and so does the strip a flight is
predicted onto.

- **The heading** is the aim's yaw; moving over 0.5 units/s, the aim
  and the way of travel together.
- **A seam's cost** is the angle between the heading and the nearest
  point of its edge, in radians, plus 0.02 for each unit of distance
  to that point. Seams over 50 degrees off the heading or over 25 units
  away are not faced; one whose edge is within 0.5 is, whatever the
  heading. The lowest cost wins.
- **Kept.** The seam faced the tick before is 0.15 radians ahead, so
  the choice does not flicker as the aim crosses between two. With no
  seam in the cone the last one stays while it is in the set, and in
  the air it never changes: a jump keeps the seam it left for.

With no seam ever faced, the chosen strip is the nearest strip the
player's floor is high enough for (`superbounce_charge::nav_target`).

**The tag.** Holding the flashlight (8 ticks, the hold that opens
[the bounce view](superbounce-ghosts-medals.md#the-bounce-view)) while aiming at one of the mode's
seams tags it (`Tagger`): the seam nearest the aim's line from the eye,
within 6 degrees or the angle 0.4 units spans at its distance (`aimed`).
The tagged seam keeps a navpoint (`sbr.tag`) wherever the player goes,
reachable or not. Holding on the tagged seam clears the tag; a hold
aimed at no seam changes nothing; a new game clears it. While primed and
not falling onto a strip, a tagged seam that is one of the
[marked](#the-superbounce-game-type) strips is the chosen strip
(`superbounce_targets::chosen`), so the mode's navpoint, the take-off,
the world marker and the [fall assist](superbounce-guide.md#hold-x-the-fall-assist) follow
the tag.

`superbounce.state` has the mode in effect, its count, the tag and the
marked points as `targets`
([agent-api.md](../runtime/agent-api-world.md#superbounce-the-superbounce-game-type)).

### The second bounce

In the flight after a bounce the readout shows where that flight can
bounce again, the best seam first
(`crates/extensions/game/src/superbounce_second.rs`).

**When.** The first local player is in the air and their
[chain](superbounce-ghosts-medals.md#medals) is live (`window`): from the launch to the next
touchdown, rising or falling.

**Reached.** A seam of the [targets](#targets)' mode is reached
(`judge`) when:

- the flight, flown on the [flight model](superbounce-guide.md#the-flight-model) with the
  stick held as it is, comes down to the seam's height within 8 s, at
  the speed the seam needs or faster (the touchdown speed of its last
  free row against `need_of`, the [one threshold](superbounce-spots.md#speed) for a scanned
  strip). A seam over the apex, or just under it, is not;
- the pursuit of the seam's [landing point](superbounce-spots.md#the-landing-point) nearest
  that landing (`superbounce_assist::stand_near`, `aim_at`;
  `superbounce_flight::pursue`) comes down in the seam's catch: the
  stick, as measured, can get it there;
- no floor under the flight's apex covers the point
  (`superbounce_assist::covered`);
- a V's arm, only from across its bisector.

**Ranked.** Each reached seam has a cost (`Ranked::cost`), lower first:
the steer's share of the stick's reach (0 to 1) plus half the seam's
need over the touchdown speed (0.5 with no speed to spare, less with
more). So a seam the flight is already coming down on outranks one that
needs steering, and of two as easy to steer onto the faster landing
wins. The two lowest are kept (`rank`), and of seams within 1 unit of
each other only the better.

**Shown**, all by the game's HUD:

- the readout's two target navpoints are on the ranked seams, the best
  on `sbr.target.1` (`over`); the tag keeps its own;
- the readout's last row lists them in order, each a rank, an arrow for
  where the seam is against the aim and its distance:
  `1 (arrow) 12 m  2 (arrow) 30 m`;
- the fall row is the best seam's line-up
  (`superbounce_readout::line_up`): the bar is the touchdown speed there
  over its need, and beside it `On seam` when the flight comes down in
  its catch with the stick held as it is, `(arrow) Drift` when it does so without the seam's
  drift, else an arrow for the way to steer to its
  [landing point](superbounce-spots.md#the-landing-point) and how far.

With no seam reached the readout is as on any other fall. The game draws
no number on a navpoint, so the order is the row's; a navpoint's look
cannot be changed until its draw record is decoded. The variant's
`second_bounce` [option](superbounce-guide.md#the-huds-options) turns all of it off.
`superbounce.state` has the ranked seams as `targets.second`.
