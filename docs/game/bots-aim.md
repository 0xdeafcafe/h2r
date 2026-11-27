# Bots: aim and emotes

Status: part of [bots-decision-aim.md](bots-decision-aim.md); see there for status, labels and the index. This page: aim, the aim net and the emote layer (3.4, 3.5).

## 3.4 Aim

Aim feeds the biped's current view back into a model of a human thumb
on a stick.

| Part | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|
| fresh reaction (ticks from first sight) | 10–18 | 7–13 | 5–9 | 5–8 |
| perception delay (ticks) | 3 | 3 | 2 | 2 |
| motor/controller delay (ticks) | 2 | 2 | 2 | 1 |
| continuous delay (ms at 30 Hz) | 167 | 167 | 133 | 100 |
| button-edge timing σ (ms) | 8 | 6 | 4.5 | 3 |
| tracking error (std. dev. of the angle at 5 units, rad) | 0.10 | 0.06 | 0.035 | 0.02 |
| lead (fraction of the needed lead: flight and own delay) | 0.3 | 0.6 | 0.85 | 1.0 |
| turn asked per radian of error (rad/s) | 4.8 | 7.5 | 10.5 | 13.5 |
| flick overshoot (mean, test plant) | 10% | 7% | 6% | 5% |
| automatic inside the gun's band | held | held | held | held |
| automatic past the band (ticks on, off) | held | 24, 8 | 16, 8 | 12, 8 |
| grenades (arc solve) | rarely, poor | sometimes | often, good | often, good |
| weapon choice by range | clear mismatches | yes | yes | yes |

The continuous delay covers seeing a rendered change and getting a new
controller report into the game. Easy and Normal share a five-tick budget at
30 Hz. Their hesitation and timing precision differ.

Heroic sees the change one tick sooner, and Legendary also returns a report
one tick sooner. These are tuning defaults for playtesting.

The first-sight reaction remains the time to notice and commit to a new
target after its first visible sample reaches the brain. The five-tick floor
(167 ms) applies to that reaction. The first input arrives after the
perception delay, one reaction wait and the motor delay, with up to a tick
of scatter on a button edge.

Later decisions on the same target use the same continuous pipeline. A brief
occlusion keeps the reaction clock and clears the velocity sample.

Both virtual pads use the game's stick response, look acceleration and aim
assist at sensitivity 3 (§5.1). The hand steers by a model of that response
(`mind::look::RESPONSE`): on each axis, stick travel against turn rate,
piecewise linear from the 0.13 dead zone. Its rows are seeded from the one
measured point, about 9°/s in yaw and 5°/s in pitch at half stick before aim
assist (§5.1), and the nominal 3 rad/s at full yaw stick, with pitch at 5/9
of yaw throughout.

The full-stick ends are a guess until a stick sweep on each axis, read from
`sim.player.list`'s `aim`, replaces the rows. The hand asks for the skill's
`gain` × 3 rad/s per radian of error and reads the stick for that off the
table; under 0.012 rad/s the thumb rests. The right thumb takes several
ticks to cross the stick: the skill's travel a tick (0.15 to 0.25 of the
stick), and on a tracked target more than 0.3 rad off, 0.4 a tick at every
skill (`mind::look::FLICK_STEP`), under the pad's 0.45, so a large flick is
at full stick on its third tick.

On a still target the thumb settles instead of correcting every tick
(`mind::look::rest_band`). Once the error the hand sees is under
`clamp(0.2 / d, 0.004, 0.04)` radians it lets the stick go, and takes it
up again only past twice that. A target whose bearing moves faster than
0.05 rad/s gets no rest. A steady hand mostly rests on a still target;
a shaky one's own error keeps it outside the band and correcting, as a
player's thumb stays still on a target it is on.

The view is late. The mind sees an observation `perception_ticks` old, its
own facing included, and a report lands `motor_ticks` later. The hand keeps
the modelled turn of its last 16 reports and adds the turn of the last
`perception_ticks + motor_ticks` of them to the facing the view shows
(`Hand::ahead`) before it measures the error, so it does not swing past a
target it has already turned to.

The same predicted facing turns the walk into the left stick and aims a look
with no target. A small integral term takes off a standing offset that the
table's error leaves: it gathers the error the hand sees at 1/s while that
error is under 0.1 rad and the thumb is not resting, adds at most 0.05 rad,
and clears when the target is lost.

Large flicks carry a little past the target before settling. Small
correlated errors and the occasional larger misread vary by skill.
`error` is their spread at 5 world units. Further out the angle shrinks
to `error × (0.2 + 0.8 × 5 / d)` (`mind::look::spread_scale`), so a hand
misses a far target by a smaller angle and a larger distance.

The aim point leads a moving target by the skill's `lead` fraction of
its velocity times the shot's flight time plus the hand's own delay,
`perception_ticks + motor_ticks` (`mind::look::lead_time`). The hand
adds no lag of its own.

**The fire cone** (`mind::look::fire_cone`). The trigger is pulled while
the aim is within `clamp(0.35 × (1 + looseness) / d, 0.012, fire_cone + 0.3)`
radians of the aim point: half a body's width as an angle, wider for a
forgiving gun (looseness as in §3.4.1's inputs: sniper 0, precise 0.15,
blast and unknown 0.5, spray 0.7, shotgun and sword 1). With a rifle that
is about 1.2° at 20 units and 7.7° at 3. The skill's `fire_cone` only caps
it at point blank.

**Holding the trigger** (`tactics::hold_pattern`, `tactics::Hold`). An
automatic inside its gun's band is held until the target or the magazine
is gone, at every skill. Past the band Easy holds anyway; Normal, Heroic
and Legendary fire the skill's bursts, the on time 0.7 to 1.5 times the
table's as the name's spray (`tactics::spray`: half its aggression, half
its reaction jitter) runs from 0 to 1. Once down, a held trigger stays
down while the aim is within 2.2 fire cones, and through 4 ticks outside
that or with the shot blocked or out of range. A burst let go before its
end is finished by the next press, without waiting out its gap.

The reaction gates only the trigger, melee and grenades. Until it is
over the bot keeps walking where it was going (its route, else last
tick's walk) and the right thumb already turns toward the part of the
enemy it sees, at `REACTION_GAIN` (half) of the angle. An alert bot
reacts two ticks sooner (`Tactics::focus_alert`): one that saw the enemy
within the grace period, has a tracker dot near (§3.1, "The motion
tracker") or was just hit from out of view. Tests:
`it_keeps_moving_and_starts_turning_before_it_may_fire`,
`shot_from_behind_it_turns_to_the_shooter_within_ten_ticks` and
`a_moment_out_of_view_does_not_freeze_it_again`.


At the controller boundary, each stick stays within its unit circle and can
travel at most 0.45 stick radii per tick. A full reversal takes at least
five reports. The model uses a standard two-thumb grip.

The right thumb can press one of A/B/X/Y/Black/White while the right stick
is centred, or use the stick or its click. It can switch between those jobs
once per tick (33 ms) by default. A face-button tap waits for the stick to
return to centre, then occupies at least one controller report.

The left thumb can move its stick and click it. The index fingers can use
the triggers during a face-button press.

Button and trigger transitions draw independent, centred Gaussian timing
errors using a per-bot seed. An error of, for example, 6 ms has a 6/33.3
chance to shift the report by a tick in the direction of the error. Reports
slip by at most one tick.

At zero motor delay an early edge can only land on the current tick. Two
edges on the same boundary can erase a short tap.

Sticks use the nominal motor delay. The same seed, samples and tuning
reproduce the same input.

A tuning file's `burst_on` and `burst_off` set the bursts past the band;
`burst = false` holds the trigger at every range, and `burst = true` on
Easy uses Normal's 24 on, 8 off.

`bots::tuning::Tuning` exposes `perception_ticks`, `motor_ticks`,
`button_sigma_ms`, `stick_step`, `thumb_switch_ticks`, `phantom_ping_ms`
and `phantom_jitter_ms`. The sim accepts these in bot tuning files
([sim.md](../runtime/sim.md#bot-parameters-from-a-file)).

Lead comes from the target's velocity, the weapon's projectile speed,
read from the tag at load, and the hand's delay. A grenade's throw is
flown against the map's collision (bounces, the fuse, a plasma sticking)
and its aim searched until it goes off at the target; the bot throws only
when one does (built: `bots::grenades`, "Built: weapon skill" in §10).
The numbers in the table are starting points; tuning them needs runs
(§11).

### Phantom aim diagnosis and next steps

In the Midship replay
(`531b5da3ceab388417c044afcf18e01a`), four phantoms fired 2,231 shots.
They dealt positive damage to opponents on 14 bullet hits and 9 melee
hits. Three local Clankers fired 1,372 shots and landed 236 bullet hits.
The hits show that phantom actions reached the host.

The old adapter turned a half-stick input at about 54°/s. A local bot at
the same deflection turned about 9°/s in yaw or 5°/s in pitch before assist.
The old adapter also sent an empty shot-assist block. The contribution
of each difference to the observed miss rate is still unknown.

Retail builds local actions at `0x186ab0`. The control update at `0x185be0`
calls `0x1a4900`, which gets the held weapon’s cone from `0x1a50a0` and
searches for a target at `0x1a5370`. The local builder copies nine dwords
from the per-user control record into action `+0x34..+0x57` at
`0x187074..0x187083`.

The remote gather at `0x84990` expects that search to have run on the
sender. An empty record has no target and zero strengths for the barrel's
magnetism check.

The host now runs the retail local builder on each arrived phantom pad. Its
private copy of user zero has the phantom's unit, camera origin, profile
binding and control history, including on a host with no local profile. The
host validates the full action at `0x14d0a0` (§5.1).

A historical ten-seed Midship batch measured four players for 9,000 ticks
per original leg. Red locals averaged 0.165 opposing hits per shot and Blue
phantoms 0.155. The swapped legs never bound all four players, so team and
transport were confounded.

For a phantom in a hosted match, check `sim.player.list` for
`aim.assist_player` and `aim.assist_strength` as a target enters and
leaves the reticle. For aim comparisons, run matched phantom rosters
with the same maps and seeds (§3.4.1).

### 3.4.1 The aim net (compared: the code aim stays)

**Result.** `aim-net-vs-code.toml` ran all 80 matches
(Midship and Lockout, easy to legendary, 5 seeds, both arm-swap legs,
5-minute 2 v 2, using the local seats available at the time). The code aim
won 60% of games to the net's 40%, and the net lost on accuracy (13.1%
against 15.8%), damage (9.8 against 11.8 a game) and grenades (0.3 against
1.1). The net barely overshoots (0.03 rad against 0.17), but it settles 1.3
s slower and its stick jitters more.

It is cautious, and it loses the first shot. The gap grows with skill: at
heroic the code won 78% of games, at easy 50%. Both stay in the tree. `code`
remains the default, and the net is a starting point if its settle time can
be cut without bringing the overshoot back.


The user asked for a small neural net with hand-written weights for aim
and tracking, tested in simulation against the code above, the winner to
stay. `bots::aim_net` is that net. Nothing is trained and there is no ML
dependency: the weights are `const` arrays built from named constants, so
each hidden unit has one job.

**The switch.** `aim_model = "code" | "net"` in a bot's tuning
(`bots::tuning::Tuning::aim_model`, `sim::config::BotParams::aim_model`).
`H2_BOT_AIM_MODEL=net` sets the default for bots whose tuning does not
name one. The default is `code`.

**What it replaces.** Only the step that turns the hand's view of a
tracked target into the stick it wants. With `code` that is the tracking
lag, the flick offset and `gain × error` through the stick curve. With
`net` it is one forward pass. Both models share everything around that
step:

- the view, as late as `perception_ticks` makes it;
- the hand's wandering error and misreads, from the same seeded process,
  scaled by `error` and by mood through `human_input::tracking_noise`;
- the thumb, which moves at most the skill's travel a tick (0.15 easy to
  0.25 legendary) inside the stick's circle;
- the controller: motor delay, the 0.45 stick step, the one-thumb rule;
- target choice, the fire cone, the trigger, movement and the shared
  action builder.

A look with no tracked target (travel, a glance at a sound, a pilot's
line) uses the code's curve under either model.

**Inputs (18).** Yaw then pitch where there are two.

| Input | Unit | From |
|---|---|---|
| error | rad | aim to the led aim point, before the hand's error |
| error rate | rad/s | this view's error less the last one's |
| target rate | rad/s | the target's bearing from the eye, view to view |
| stick | -1..1 | the thumb now, turn-positive |
| pending turn | rad | the nominal turn of the last `perception_ticks + motor_ticks` reports: asked for, not in the view yet |
| since acquired | s | 0..2 |
| size | rad | 0.25 / distance: the target's apparent half width |
| looseness | 0..1 | the weapon: sniper 0, precise 0.15, unknown and blast 0.5, spray 0.7, shotgun and sword 1 |
| gain | 0..1.5 | the skill's `gain`: 0 at easy's 1.6, 1 at legendary's 4.5 |
| spread | 0..1.5 | the skill's `error` / 0.10 |
| lead | 0..1 | the skill's `lead` |
| noise | rad | the hand's wandering error this tick |

The three skill scalars come from the tuned aim table, so `gain`, `error`
and `lead` in a tuning file move the net as they move the code.

**Hidden units (24, ReLU).** They come in mirrored pairs, one unit a
direction. Skill enters only through biases: it widens or narrows a dead
band or a clamp. With `seen = error + noise`,
`predicted = seen + 0.02 × error rate + 0.12 × target rate − 0.9 × pending`
and `tolerance = 0.006 × spread + 0.06 × size + 0.004 × looseness`:

| Units | Name | Computes | Job |
|---|---|---|---|
| 0, 1 | yaw pull | `±predicted − tolerance` | The PD term. The error rate is the D. The target rate carries the error forward over the view's age, and the pending turn takes off what is already on its way. Out at 3.9 stick/rad. Inside the tolerance the thumb rests. |
| 2, 3 | yaw boost cap | `2.0 × pull − 0.43 × gain` | Takes the extra 2.0 stick/rad away again once it has given `0.43 × gain` of stick. Easy gets 1.9 stick/rad everywhere; legendary 3.9 for the first 0.2 rad. |
| 4, 5 | yaw breakout | `±seen − tolerance` | The micro-correction. Out at 8 stick/rad on the error as seen, so a small miss gets a stick past its dead zone at once. |
| 6, 7 | yaw breakout cap | `8 × breakout − 0.17` | Stops the micro-correction at 0.17 of stick, just past the 0.13 dead zone, where the view creeps. |
| 8, 9 | yaw lead | `±target rate` | Velocity feed-forward: 0.35 stick per rad/s, so the thumb holds the turn a moving target needs with no error to drive it. |
| 10, 11 | yaw lead cap | `0.35 × lead unit − 0.6 × lead` | Stops the feed-forward at `0.6 × lead` of stick. Easy keeps up with 0.5 rad/s, legendary with 1.7. |
| 12, 13 | yaw follow-through | `±stick − 0.5 − 1.2 × since + 0.3 × spread` | The flick overshoot. A thumb already hard over early in an engagement stays over (0.8 out), so the aim carries past and comes back. It fades within 0.4 to 0.7 s and is wider for a wider spread. |
| 14–21 | pitch pull, boost cap, breakout, breakout cap | as yaw's | The same four pairs on pitch. |
| 22, 23 | pitch lead | `±target rate` | Pitch feed-forward, 0.25 stick per rad/s, no cap. |

**Outputs (2).** The yaw and pitch turn, each the weighted sum of its
units clamped to −1..1. The hand maps `[yaw, pitch]` to the right stick
`(−yaw, pitch)`, keeps it in the unit circle and moves the thumb there.
No fire gate: the trigger stays with the shared fire cone, so the
comparison measures aim alone.

A view is `perception_ticks` old and a report lands `motor_ticks` later:
five ticks at easy and normal, four at heroic, three at legendary. Both
models count the turn they have asked for that the view does not show yet.
The net takes 0.9 of it off its error, counted on the nominal radial curve
its weights were designed against (3 rad/s at full stick, about 54°/s at
half).

The code takes all of it off, counted on the response table (§3.4). The
comparison ran before the code counted it: that code steered on the old view
as if it were current, and in the test plant a 0.95 rad flick swung 37–54%
past a still target where the code now goes 5–10% past.

**Tests.** `cargo test -p bots aim_net` covers the network alone: the
same bits for the same inputs, no stick for no error or inside the dead
band, sign and axis, saturation, the gain order by skill, each named
unit's effect, and the table's shape. `cargo test -p bots look` runs both
models through one kinematic harness (the view as late as the skill's
delays; the code on the response table, the net on the nominal curve):
determinism, the code aim untouched by the switch, the net landing a
0.95 rad flick and settling sooner and closer at each higher skill,
tracking a strafing and a crossing target, and the mood scaling of its
error. The code aim's own tests run on a plant with the response table
and the late view: a flick that does not swing past
(`a_flick_on_a_late_view_does_not_swing_past`), pitch held as well as yaw
with a height gap, the error's angle shrinking with range, a
strafing target led for the hand's delay, the thumb resting on a still
target and not on a crossing one
(`on_a_still_target_the_thumb_rests_and_on_a_moving_one_it_tracks`), and
a large flick at full stick by its third tick
(`a_large_flick_takes_the_thumb_to_full_stick_at_once`). The code's
thumb, resting on target, is no busier than the net's on a flick.

**The comparison.**

```sh
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/aim-net-vs-code.toml --disc disc/halo2.iso --parallel 3 --pace unpaced
```

Midship and Lockout, four skills, five seeds, two against two, every bot
at the run's skill. Each match is played twice (`mode = "arm_swap"`): the
second leg gives every bot the other arm, so each model plays every seat,
team, spawn and brain seed. 80 matches. Only `aim_model` differs between
the arms. `compare.txt` under
`docs/runtime/sim-examples/results/aim-net-vs-code/` has one table over
all runs and one per skill, with net − code and its 95% interval:

| Row | Meaning | Better |
|---|---|---|
| `accuracy` | hits / shots | higher |
| `kd`, `win`, `kills`, `deaths` | the fights | higher K/D |
| `ttk_s` | seconds from the killer's first hit of a fight to the kill | lower |
| `aim_error` | mean angle off the aim point while tracking, rad | lower |
| `overshoot` | how far past the target a flick of 20° or more goes, as a fraction of the flick | 0.03 to 0.25 reads as a hand |
| `settle_s` | seconds for such a flick to stay within 0.05 rad | lower |
| `jitter` | right-stick travel a tick while tracking | lower |
| `reversals` | changes of the stick's yaw direction a second | lower |

**The decision rule.** Read the per-skill tables.

1. The net wins if, at three skills or more, it is better with the
   interval clear of zero on at least one of `kd`, `accuracy` and
   `ttk_s`, worse with the interval clear of zero on none of them at any
   skill, and no less like a hand: `jitter` and `reversals` not higher
   with the interval clear of zero, and `overshoot` inside 0.03 to 0.25.
2. The code wins if the reverse holds.
3. Anything else is close, and the simpler one stays. That is the code:
   it is the incumbent and carries no extra state.

Also check that each model's own `accuracy` still rises from easy to
legendary. A net that makes easy bots sharp has lost the difficulty
ladder whatever its K/D.

The loser is deleted, not kept behind the switch. If the net wins, the
default becomes `net`, then the code's flick offset and integral term go.
If the code wins, `bots::aim_net`, the switch and the hand's net state
go. The aim-motion rows and `arm_swap` stay either way.

## 3.5 Emotes (the teabag layer)

The user asked: "make them teabag if you do something dumb". The emote is a
short burst of crouch presses over a body, made with the real crouch input.
The kill feed supplies the body position and the event.

Self-kills (including world deaths) and betrayals can prompt a taunt.
Showboats can also taunt over their own ordinary kills, at a lower rate.

The bot may emote only when:

- the body is within 1.5 world units (about 4.5 m: `emote::NEAR`, the
  game's teabag radius, so every emote is a teabag for the Incel medal,
  [teabag.md](teabag.md#the-rule));
- no enemy has been in the bot's view for 1.5 s;
- the bot is not carrying, defending or contesting an objective that is live
  (a flag at risk, a bomb armed, a hill or territory contested);
- it has not emoted in the last 30 s.

When it does, it presses crouch 3 to 6 times at a human rhythm (built:
130 to 200 ms per press), for 2 s at most, and stops at once if an enemy
appears, an objective takes priority, or taunts are turned off.

How often it happens:

    chance = clamp(taunt × severity(event) × showboating_weight, 0, 1)

Severity is 1 for a betrayal, 0.8 for a self-kill, and 0.35 for the bot's own
ordinary kill. Showboating and current confidence set the weight. The
Clankers page's **Taunts** switch sets `taunt` to 0 when off. A persistent bot
has its own base taunt value.
