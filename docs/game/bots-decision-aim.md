# Bots: decisions, aim and emotes

How a bot picks a goal, aims and emotes (sections 3.3 to 3.5). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 3.3 Decision

The design calls for a utility layer running about five times a second.
These are the inputs to that design.

| Action | Driven by |
|---|---|
| fight | a visible enemy; my health; my weapon against theirs at that range |
| chase | an enemy seen recently, its last known position |
| hold angle | a patient bot keeps aiming at its last sighting for a moment |
| pick up weapon or powerup | improvement over the held gun, distance, estimated availability and position strength |
| seek health | shields down, no enemy in view (cover, then a wait for recharge) |
| play the objective | gametype; my role (carrier, escort, defend, attack); team state |
| flee | outgunned, low health, a grenade nearby |
| emote | §3.5 |
| roam | the default: move toward the likely fight (spawns, weapon spots) |

**In play today.** `Mind::step` chooses an activity per tick. A flag or
bomb carrier moves toward its scoring point. A ball carrier looks for cover
and melees anyone in reach. Both suppress weapon switches.

An armed objective player stays on assignment. A dry player can still detour
to get a usable weapon. In King, it only collects guns inside the active
hill.

A bot can fire at a visible opponent while walking toward the hill, guarding
its flag or escorting a carrier. Inside a held zone in Oddball, King of the
Hill and Territories (one of at least `FOOTWORK_ROOM`, 1 world unit) it
keeps its fight footwork; past half the zone's radius the step loses the
part that leads out (`objective::within`), so it strafes in the hill rather
than standing still and never fights its way out. Target choice combines the
existing range, turn and stickiness costs with the objective, teammates'
calls, recent kills and visible openings.

A bot that loses sight plays its objective first. In Slayer
(`Mind::between_fights`, `crates/live/bots/src/mind/strategy.rs`) it then
takes, in order: recover shields; regroup with the nearest teammate after
withdrawing or while afraid; its own lead (hold the angle it lost the target
round, then chase the last sighting); a teammate's call or a trade; a due
supply or loose grenades; a heard shot or a guessed respawn; move up with
the lower-slot teammate when that one is more than 10 to 19 units away; a
firing position; roam.

A teammate's call never pulls a bot off a fresh lead of its own, and
trailing a teammate comes only once nothing else is left. A team call alone
never authorizes a shot.

### Finishing and breaking off

A target whose shield is down is run down, not strafed at while its shield
comes back. The bot reads the shield from the visible contact (at or under
0.25), or counts it as down after twice the melee finish's pulls on target
(`weapons::Combo::softened`: 6 at Legendary, about 1.6 s of automatic fire).
With an automatic, a shotgun or nothing loaded it runs straight in from
within 6 units; with a rifle only from within 2.5, since its headshot
finishes further out; never with a sniper, splash gun or sword
(`tactics::finish_push`).

Running in uses the rush footwork and the plasma reach for the melee (1.17
units), so the melee lands. It does not run in while carrying or
withdrawing, and it keeps firing on the way.

The bot withdraws when it sees more enemies within 8 plus 12 times its
awareness units than it has teammates there with a clear line, plus a
margin of one or two from its caution, aggression and drive. A softened
target overrides that when the bot's own shield is at least 0.5 and it is
outnumbered by no more than one beyond the margin. The Juggernaut never
withdraws (posture Press).

Recovering shields, it backs away from the last sighting for 1.5 s after
losing sight (`mind::RECOVER_BACK_TICKS`), then stands and watches that
way until the shield is back. Fear sends a bot off on its own only while
its shield is under the recovery line (0.9); with a full shield a fearful
bot regroups with a teammate if there is one, else follows its own lead.

**Full shield.** Every shield line in the brain (the recovery lines,
the fight's back-off and the decision layer's 0.4 to 0.86) reads the shield
as a fraction of the bot's full shield. The perception reads that full
shield as `0x1588b0` computes it for a spawn (`guest::full_shield`,
`Me::shield_full`): the variant's shields (1, 0, or `[0x45dbd4]` for
overshields) times the handicap's factor (1, 0.75, 0.5, 0.25).
`mind::relative_shield` divides by it, capped at 1, so an overshield still
reads above 1. In a variant without shields the body's vitality stands in
for the shield, so the bot still backs off when hurt and comes back when
healed.

With absolute lines, a bot whose full shield was under 0.25 was always
"shields down". In game `09af571f9fec7f1a33ed37edb397a140` the bots carried
a handicap of 3 and spent the match recovering. The median angle between
their walk and their aim while firing was 178–180°, so they backpedalled
through every fight.

In game `c9a3aa1a3f867c298a6ae32b56e50399`, with full shields, it was 90°, a
strafe. `bots.stats` `combat.standing_decisions.recover` shows the time
spent recovering. The tests are
`mind::tests::shields_down_is_a_fraction_of_the_full_shield`,
`without_shields_it_backs_off_on_health_alone` and
`guest::tests::the_full_shield_follows_the_variant_and_the_handicap`.

Reload (X) and switch weapons (Y) take 5–13 ticks of stable intent,
depending on skill, then hold 2–4 ticks. Retries are at least 2 s apart
for reload and 1.3 s for switch. Melee also needs a target in reach and
near the crosshair for a short time before the press.

Vehicles (§3.9) sit across this order: in a vehicle the vehicle layer
makes the whole pad; on foot, after the order above, boarding a close and
slow enemy vehicle takes over in a fight or not, and taking a free one
takes over only when calm and not carrying or rearming.

#### Warm-up

For the first 2 to 4 s of its first life in a match (`mind::warmup`:
`WARMUP_MIN_TICKS` 60 to `WARMUP_MAX_TICKS` 120) a bot picks no target, so
it does not shoot, throw or melee; it looks round, moves off its spawn and
plays the objective as usual. The length is the bot's own, from its name's
hash (the Clanker personality's `NameProfile::seed`; the brain's seed
without a name), so it is the same every match and differs between bots.
Taking damage ends it at once: a bot that is shot fights back.

A brain makes a fresh mind each match (`Brain::reset_mind`), which is when
the warm-up is due again; a respawn does not start it. A bare `Mind::new`
(the tests') has none.

A bot with a known grenade in stock can throw at 3–12 world units. It looks
for a cluster, an enemy within 6 units walking at it (faster than 0.8 a
second, heading within about 37° of it: `grenades::Mark::closing_on`), or
the last position where an enemy disappeared within two seconds. Both
chasing and holding the corner present that opportunity.

In these short windows Normal has about one chance per second before
temperament, collision, aiming and the eight-second cooldown. Otherwise a
single visible target gets the skill's ordinary per-minute rate, tripled on
Normal and up when pushing with a short gun. The thrower checks the
predicted blast and checks the actual aim and friendly positions again
before pressing LT. A dual-wielding bot drops the left gun to open its
throwing hand only against a group, or when the left gun is dry with nothing
to load.

### Personality and temperament

The full gamertag fixes a Clanker's combat profile. System Link draws
distinct gamertags from the Live name pool and keeps them as seats are
added and removed. Live games use their rostered names, and simulations
use their configured names. Each host computes the profile in memory.

`crates/live/bots/src/name_profile.rs` hashes the UTF-8 gamertag with 64-bit
FNV-1a, folding ASCII letters to lowercase as Live does for account
identity. The hash supplies one try-hard score and one independent draw per
trait. Each row in the trait table blends that score (or its inverse) with
its own draw, then scales the result into the difficulty band.

The custom-game style shifts tendencies within the band. Changing hosts,
seat, match seed or account ID preserves the profile. The match seed
controls individual choices during play.

A keen player tends to check radar more often, attempt the plasma pistol
overcharge and headshot follow-up, strafe and jump more, and react with less
delay. A casual player is more likely to take a compatible second gun. A
weight of 0.3 on the combo means roughly three attempts in ten eligible
openings.

Holding the same opening for ten ticks gives it one chance. The card
summarises the most distinctive tendencies. `h2 agent bots.stats` shows
their numeric weights for the current difficulty and style.

| Trait | What changes in play | Easy | Normal | Heroic | Legendary |
|---|---|---|---|---|---|
| Aggression | Takes openings and closes distance | .05–.95 | .12–.92 | .24–.90 | .35–.88 |
| Caution | Respects being outnumbered, routes around recent danger and plays a hill's edge | .05–.90 | .16–.88 | .30–.84 | .46–.82 |
| Teamwork | Responds to radio calls and helps teammates | .08–.85 | .18–.88 | .32–.88 | .46–.87 |
| Objective focus | Stays closer to the assignment during a fight, and holds a hill, a territory or the ball carrier's fight one enemy longer | .05–.90 | .18–.88 | .35–.90 | .48–.89 |
| Composure | Has smaller swings in mood and settles faster | .08–.50 | .28–.70 | .50–.86 | .73–.96 |
| Patience | Holds a strafe or a corner longer before chasing, and waits for teammates before pushing the ball carrier | .05–.48 | .22–.72 | .42–.85 | .65–.91 |
| Showboating | Attempts riskier melee and sword lunges and taunts more often | .10–.95 | .10–.87 | .12–.75 | .15–.67 |
| Weapon loyalty | Favors a weapon family and may take a short detour for it during a lull | .05–.95 | .13–.88 | .20–.80 | .28–.72 |
| Chattiness | Refreshes radio sightings more often. New targets and urgent help go out immediately | .05–.95 | .12–.90 | .25–.83 | .36–.80 |

The narrower Legendary bands give bots more consistent discipline while
leaving room for different fighting styles. Favourites are drawn
from automatic guns, precision rifles and pistols, shotguns, snipers, splash
weapons, and swords. Loyalty biases existing pickup choices and can justify
a short detour for a visible favourite when the bot is calm and free to rearm.

Confidence, tilt, and fear each run from 0 to 1. Enemy kills build
confidence, with a larger gain as the streak grows. A death breaks the
streak, drops confidence, and raises tilt and fear.

Betrayals and losing a recent favored duel add extra tilt. A duel is favored
when the bot sees one opponent with no nearby teammate in the fight, holds a
weapon ready at that range, and has a shield lead of at least 0.30. While
dead, the bot can read two crouch requests by its killer within 8 m of the
body in the first 3 s as a taunt.

The three mood values decay with game time, with half-lives of about 8 to 17
s depending on composure. Confident bots commit to more fights. Tilt can
make a bot press and throw grenades more often.

It remembers the killer for up to 10 s and may favor that opponent in view
or pursue its last sighting. A frightened bot is quicker to withdraw and
regroup with a teammate. These choices respect combat range, objective and
input gates.

The human-input layer caps extra tracking noise from tilt and fear at 10%.
Each match pad still passes through the controller's stick slew, button
timing and thumb coordination limits before reaching the phantom's action
gather.

The map decision layer uses `Personality::wait_weight()` for patience
near a predicted spawn and `Personality::weapon_interest(role)` to value a
favourite weapon there. A patient sniper with a favourite rifle will also travel
farther for a firing position that overlooks a stronger lane. See [the agent bot view](../runtime/agent-api-sim.md#sim-simulated-players-and-direct-state)
for the live traits and mood.

## Where the sections live

| Page | Covers |
|---|---|
| [bots-aim.md](bots-aim.md) | aim, the aim net and the emote layer (3.4, 3.5) |

Each heading below is cited from code and moved to the page named after it.

## 3.4 Aim

Moved to [bots-aim.md](bots-aim.md).

### Phantom aim diagnosis and next steps

Moved to [bots-aim.md](bots-aim.md).

### 3.4.1 The aim net (compared: the code aim stays)

Moved to [bots-aim.md](bots-aim.md).

## 3.5 Emotes (the teabag layer)

Moved to [bots-aim.md](bots-aim.md).
