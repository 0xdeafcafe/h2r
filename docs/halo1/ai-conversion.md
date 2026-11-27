# Halo 1 AI: the conversion

How Halo 1 actors, encounters and command lists convert, split from [ai.md](ai.md).

| Section |
|---|
| [The conversion](#the-conversion) |

Part of [ai.md](ai.md).

## The conversion

`h1campaign::ai::convert::convert(&H1Ai, &Options) -> Plan` makes the Halo 2 data as
the decoder's own types; `Plan::blocks(resolve)` writes them as
`map::build` blocks, resolving each character path to a datum in the map
being built; `ScenarioBlocks::apply(&mut scnr_root)` puts them on the
scenario's root block (character palette `178`, squad groups `158`, squads
`160`, zones `168`, orders `240`, scripting data `1d8`). The test builds
them into a map with `map::build::TagSet` and reads them back through
`h2::ScenarioAi::read`: the result equals the plan.

The [Firefight kit](../game/firefight.md) already has Halo 2 character
datums. It calls `h2::ScenarioAi::blocks()` directly and installs the
character palette, squads and zones from the returned fields.
`h2::ScenarioAi::read_map` follows their addresses throughout the map's
ordinary metadata, including new arrays outside a pinned scenario tag.

| Halo 1 | Halo 2 |
|---|---|
| encounter | a squad group and a zone of the same name; team carried over (S: both games number teams default, player, human, covenant, flood, sentinel) |
| firing positions | the zone's firing positions, grouped by area; one area per group letter used, named `a`..`z`, with the tools' mean, deviation, first and count filled in. Ground points name a generated sector in the encounter's assigned BSP. Airborne points retain their authored altitude and cluster |
| squad | squad `encounter/squad` (Halo 1 scripts' own spelling, cut to 31 characters), in the encounter's group and zone; counts, delay and major upgrade carried over |
| squad's attacking groups | an order of the squad's name over the areas of those letters (every area when none match), set as the squad's initial order |
| squad starting location | starting location: position, facing as yaw; a location's own actor becomes its own character |
| squad with no starting locations | **Built.** A named migration destination has its own squad datum, zero initial occupants and an order. A direct script placement with an actor can use the squad's move positions or its assigned firing groups. |
| actor variant | One palette entry per distinct character. Crew and Sentinel variants are rebuilt from the source tags. Other types use the retail character table. |
| command list | **Built.** Its own point set and Halo 2 command script. The authored list index selects both. A starting location with a command list carries the generated script name and index. |
| conversations | **Built** as Halo 2 scripts with converted `snd!` references (below) |
| move positions, respawn | not converted (noted) |

Values Halo 1 has no counterpart for come from the census (`Options`):
firing position flags `0x42`, zone flags 1, area type 0, order style `-1`.
Grenades by species: covenant plasma for elites and grunts, fragmentation
for marines and crew, none otherwise.

### Conversations

**Built.** `h1campaign::ai::conversations` reads retail participants and all six sound
references on every line. `h1campaign::scripts` builds the worker and state into the
scenario's regular Halo 2 script tables. `h1campaign` installs the referenced
Halo 1 sounds with the other script sounds and resolves their datums.

| Element | Offsets used | Evidence from Halo 1 Xbox |
|---|---|---|
| Participant | Flags `02`, selection `04`, actor type `06`, use and assign object name `08`/`0a`, six voice selectors `18`, encounter token `24` and compiled AI selector `44` | `0x00143c00` selects a role, `0x001440f3` compares voice selectors, and `0x0014476c` installs the chosen unit under the assigned object name |
| Line | Flags `00`, speaker `02`, addressee kind/index `04`/`06`, post-line pause in seconds `0c`, six `snd!` references starting `1c` | `0x001425a0` takes the cast's voice slot, resolves the sound and converts the pause to ticks. `0x00144abd` waits for advance on flag `0x08` |
| Conversation | Delay flag `0x40`, trigger distance `24`, run distance `28` | `0x00144466` tests the distance, and `0x00144b7a` retries at 30-tick intervals |

**Built.** Cast selection ranks distinct living units in the authored
encounter or the converted rosters for their actor type. The original
selection switch (`0x00143f6d`) checks allegiance, shared vehicles, unmounted
actors or preference for a unique voice. The bridge reads Halo 2's live
campaign alliance matrix (`0x001df560`) and vehicle driver (`0x002a4270`).
Candidates closer to players and to an already cast peer get the original
distance and view-direction weights (`0x00143a96`, `0x00144026`).
For the first cast member, the bridge screens prospective players with
Halo 1's directional BSP visibility rows (`0x00143a37`). The converter
copies those rows from the Halo 1 BSP's `+0x140/+0x14c` data reference into
the rebuilt v8 BSP at `+0x54/+0x58`. A clear head-to-head collision ray or
separation under three world units earns the original `0.5` bonus (Halo 1
`0x00143a6e`, Halo 2 `0x000b8d30` and `0x001697c0`). The Halo 2
object location update stores the cluster in the object header (`0x000b866b`).
Voice selection prefers the exact persona, then the wildcard, then an
eligible generic alternative (`0x001440e2`).
Assigned object names still resolve to their captured actors.

**Built.** Generic voices are seeded at unit construction. The converter
pairs converted units and AI starting locations with the generic selectors
authored on the source biped or vehicle (`+0x2b4`). The native unit hook
rotates through those selectors in author order, including repeated zeroes,
with a process-wide counter shared across unit definitions (Halo 1
`0x000466e0`, Halo 2 `0x000c4410`).

**Built.** The map carries the source marine squads' unique-leader rules.
The placement-wave hook applies the forced, random and encounter-size
rules before the first spawn attempt (Halo 1 `0x0013e563`, Halo 2
`0x00201520` and `0x00203780`). The voice and leader flag follow the salted
unit datum through squad migration and cast changes. The script map-init
hook moves early births into scenario globals, which carry the profiles
in checkpoints.

**Built.** Flag `0x20` runs the cast toward the focus player using the
source distance at `+0x28` (Halo 1 `0x0014476c`). Line flags `0x10` and
`0x20` wait for the speaker or the cast to finish moving
(`0x001448db`). Halo 2's native `cs_approach` (429,
`0x002a81b0`) does the movement and pathfinding. The scene chooses
a live focus player and applies the view cone when flag `0x80`
requires attention (`0x001445de`).

**Built.** The speaker, the addressee and other participants turn as their
line directs (`0x00144c30`). Halo 2's `cs_face_object` and
`cs_look_object` (400 and 394) run as temporary commands stacked over the
actors' existing commands (`0x00276480`). Line completion or a scene
stop removes only its own commands by script and actor (`0x00258540`).
Selection 1 supplies an unbound radio voice. Selections 6 and 7 bind
an actor and route the voice over radio (`0x001426a3`, `0x00144992`).

**Built.** Every spoken line owns its native sound instance. Spatial
voices use the unit's head marker and the biped lip-sync path
(`0x001891d0`, `0x00109220`). Radio uses the engine's untracked impulse
entry (`0x00189fe0`). Workers poll the salted sound datum until it ends
before applying the authored pause and advance flag. Stopping calls the
engine by instance (`0x00126360`), so overlapping lines sharing a
`snd!` tag keep separate timing and stop handles.

The converted-map hook claims **builtin slot 845** only. Its retail
descriptor is four longs returning void and its original evaluator
(`0x002ab130`) discards the call. The shared table is
`hs::h1_conversation::BUILTINS`. Approach, facing and queue queries use
stock builtin slots. The converted-map operation validates scenario
global types and uses salted object, actor and sound handles.

### Command scripts

**Built.** The ten Halo 1 campaign scenarios contain 552 command lists.
`h1campaign::scripts::compile::commands` appends a kind-5 definition for each, in
list order after the authored scripts. A list may have the same display
name as another list. The generated name `h1_cmd_<index>` keeps their
references distinct. Halo 1 checks the resolved BSP at list offset
`+0x2e` (`0x16a808`). Each point keeps its position and collision surface.
Halo 2 addresses a point by set index in the high word and point index
in the low word.

The generator emits blocking Halo 2 command-script verbs for movement,
look, animations, jumps and delays. Backward edges repeat their command
sequence. The stimulus can arrive before a wait, and interruption retires
the executing instance. The boot feature handles the H1 commands that
need a runtime calculation. H1 `0x16b74d` uses the direction from the
unit to a point, while `0x16b895` uses the unit's facing for a duration.
H2's begin/update pair is `0x258cf0` / `0x259430`. The two H1 vehicle
atoms search for a driver seat on a nearby live vehicle (`0x16bd0c`).
H2 tests seat admission at `0xc8200` and submits the entry behaviour at
`0x1a80e0`. H1's falling-damage exemption sets and clears a unit flag at
`0x16a603` and `0x16b23c`. H2's landing routine tests its counterpart at
`0xe3727` before applying the distance damage effect.

**Built.** Voice banks keep the unit definition's selector and every
authored alternative (`unit +0x2b4` in Halo 1). Halo 1 `0x468d0` draws
among matching rows. The caller at `0x469ad` tries a positive selector,
then selector zero, then all rows. The H2 speech initializer at
`0x114ec0` accepts a model variant name and writes its dialogue datum
and designator to the unit's speech block at `0x114ff3`. The hook
selects the matching voice variant for that call. A unique leader
assigned an existing unit updates the same two speech fields.

**Built.** Native vocalisations use the indexed vocabulary at `adlg +0`.
Halo 2 reads the chosen unit's indexed sound at `0x1144f4`. The rebuilt
table keeps explicit Halo 1 slots after its native rows. A command
atom names a slot and resolves the authored fallback chain from
`0x1e0720` (read by Halo 1 at `0x46a55`). It plays the exact converted
sound from the speaking unit's head and passes the sound instance to
lip sync, using Halo 2 `0x1891d0` and `0x109220`. None of the retail
campaign lists uses the explicit vocalize atom. In automatic chatter
the native table also routes sniper praise (H2 `adlg` event 0,
pattern 27) and flight after a leader dies (event 63, pattern 110).

**Built.** The Halo 1 kill stimulus at `0x149a72` passes the victim,
killer, relationship and damage category. Its weapon-specific patterns
select the killer for grenade (3), sniper (5), mounted weapon (8),
needler (11) and shotgun (12) kills of enemies. Halo 2 event 0
already has a grenade-category cheer. The other categories keep their
original value at converted `jpt! +0x34`, including the sniper category
that the damage converter maps to bullets for Halo 2 gameplay.

Halo 2's damage path calls `0x1c95d0` at `0xca4ed` with the report
in the caller's EBP. The AI kill wrapper emits event 0 at `0x1c975c`.
The hook at `0x20d0c0` checks that call chain and applies the original
category to the winning hostile-killer candidate. It enqueues the
speaking unit's exact source row through `0x114b60` at the candidate's
priority. Halo 2 still selects the winning candidate, including any
witness or player praise.

**Built.** The hostile-killer candidate also picks the line for a
Covenant victim, a Flood combat or carrier form, or a Sentinel.
Halo 1 reads the victim's actor type at `0x146089`, then reads its
dialogue class from the actor descriptor at `0x146095`. The event 0
patterns match that class at `0x146842`. Halo 2 keeps the actor type
at `actor +4` for the converted character. The hook reads that type
on the victim and resolves the result in the killer's voice.

**Built.** The victim's flying-death line is separate from falling.
Halo 1 tests `jpt! +0x1f4` against 2.0 at `0x4721b`, with falling
and fire categories taking precedence at `0x47275`. Converted damage
keeps that impulse at `jpt! +0x38`. Halo 2 calls its victim's damage
speech at `0xca45d`. The lethal path checks the victim's speech at
`0x114a06`, where the hook selects the exact flying-death row.

**Built.** Search completion uses the authored search report. In Halo 1
the search timer reaches zero before stimulus 18 at `0x16a440`. Halo 2
validates the finished search or pursuit at `0x1b26c1` and emits event
46 or 48 at `0x1b26e4` or `0x1b2702`.

**Built.** Halo 1 friendly-fire stimulus 3 picks the victim's
`hurtfriendplayer` line when the attacker is a friendly player.
Halo 2's event 3 `scld_plr_hrtme` applies the corresponding
speaker and relationship checks. Halo 1 stimulus 25 at `0x15718c`
greets a newly sighted friendly player. Halo 2 event 163 uses
`grt_plr` for that actor and player pairing.

**Built.** The search query is periodic stimulus 17. Halo 1's wait entry
initializes its timer at `0x167f3a`, and `0x16814b` fires and rearms
it after 300–599 ticks. The speech update at `0x47073` can trigger
reply rule 34 (`0x1f00a0`). The reply comes from another allied AI
actor, within nine world units, in the same encounter when one is set.
The predicate at `0x1428e0` accepts search mode or uncover mode with
a target.

The Halo 2 hook at `0x114c60` keeps the query timer in guest memory while
the actor is waiting in presearch or postsearch. It scales the 10–20
second interval to the engine tick rate. A completed query can be
answered by allied actors in search, investigate or targeted uncover,
using their own voice and the native per-unit speech queue.

**Built.** Nonlethal damage can make the attacker or victim speak. Halo 1
resolves a vehicle shot to its gunner and a collision to its driver at
`0x149889`. It sends immediate friendly damage as stimulus 3 and sends
hostile injury as stimulus 2 after the 0.3 threshold at `0x149440`.
The latter has victim lines for needler, plasma, melee and mounted
damage. The host reads the original damage category from the report
when Halo 2 calls `0x1c9c80` at `0xca512`. A pending line follows the
salted speaker until its next native speech update.

**Built.** The killer addresses friendly victims and player victims
using separate source rows. The Halo 1 kill patterns distinguish
friendly and hostile victims at `0x1467e0` and test the victim's
class at `0x146842`. Halo 2's lethal damage notification at
`0x1c95d0` captures friendly kills for the attacker. Hostile kills of
a player use the same candidate as the other victim-class lines.

**Built.** The allied responses to damage and kill remarks require a
completed source utterance. Halo 1 dispatches replies at `0x47073`
through the table at `0x1efbd8`. It first seeks a living AI in the
speaker's encounter, or on its team if there is no encounter, within
nine units. The kill replies also test a five-unit unit-contact
predicate at `0x145140`. The Halo 2 hook keys off the native sound's
last active tick at `0x114d8a`. The converted vocalisation rows retain
their source slot so that native and source-only remarks can both
trigger their own replies. Nearby allied actors are selected from the
same group and BSP visibility region and speak through their own
cached bank after the half-second reply delay.

**Built.** Ally witnesses speak from the victim's side. The Halo 1
kill stimulus uses speaker kind 2 for these remarks. Its branch at
`0x14696b` searches the victim's encounter, or its team when there
is no encounter, within ten units. The patterns at `0x1eeeb8`
through `0x1eef80` match friendly or hostile fire and can specialize
by the killer's class. The Halo 2 lethal damage hook casts a living
witness at the victim's location and selects the matching player,
Covenant, Flood, or Sentinel remark from that actor's voice.

**Built.** Halo 1 `0x4154b` tracks successive hostile kills and resets
the counter when more than 120 ticks have elapsed since the previous
kill. It dispatches stimulus 1 after three kills for an AI or five
for a player, then clears the count. The Halo 2 lethal damage hook
tracks the same count per salted killer, scaling the interval to the
engine tick rate. An AI can speak its own spree line. The player is a
silent speaker whose nearby ally can give the dedicated response,
as in the Halo 1 silent-response dispatch at `0x1472a9`.

**Built.** Friendly injury reply rule 24 at `0x1eff38` selects
the exact attacker from the victim's damage event. Its salted
handle is associated with the accepted speech row until that
utterance finishes or is replaced. Grenade, flame and vehicle
kill responses use the original damage category. The silent
player kill takes the nearby allied reply route directly. An AI
with the generic kill remark can also draw the grenade response.
As in Halo 1, an interrupted remark does not start a reply.

**Built.** Halo 1's accepted seat exit at `0x42a5b` notifies the
actor and selects stimulus 37. Halo 2 calls its AI from the
corresponding accepted dismount action at `0xeac66`. The occupant
uses its original exit remark there. Halo 1's motion check at
`0x4b6fb` selects excitement or fright for vehicle riders. Halo 2
also distinguishes its cheer and dangerous-driving events at
`0x1c831d` and `0x1c8347`, and selects the converted sounds through
its rider vocalisations.

**Built.** Halo 1's perception update at `0x15725e` reports an
alliance change using the new relationship. Halo 2 emits alliance
break event 194 at `0x25accf` for the observer and emits alliance
restoration event 196 at `0x1c9958`. The observer can use its exact
alliance-break row, and the restored alliance uses the native
forgiveness vocalisation.

**Built.** Halo 1 chooses its firing remark at `0x162a82`. It first
checks berserk mode, then an enemy flagged as a traitor, then five
or more recognized enemies, and finally whether the actor is seated
in a vehicle. The Halo 2 burst hook at `0x1fe620` reads those
conditions from the current actor, seat and perceived objects. Its
timer follows the source remark's cooldown. The leap calls use a
separate row. Halo 1 plays it after the leaping attack starts
(`0x15ba89`), and Halo 2's leaping-melee entry callback at `0x1ab380`
queues that row only after the action is accepted.

**Built.** Search abandonment is stimulus 19 at `0x166ae3`, after
a previous search attempt can no longer proceed. The Halo 2
pursuit failure events distinguish no claimant, no position and no
usable points. They use the original abandonment remark in the
actor's current voice.

**Built.** A newly sighted enemy can use the recent-combat remark.
The source condition at `0x1463f1` keeps a 180-tick combat interval.
The native sighting call at `0x25b38c` consults the acting unit's
earlier combat state. Nearby ally alerts follow the source awareness
rules at `0x157365` and `0x1573c0`. An eligible ally who has not
acquired the reported target can answer in its own voice, identified
by the exact salted unit in the alert.

**Built.** Halo 1 dispatches loss of contact at `0x166869` and
`0x16a419`, group uncover at `0x169b7c`, cover at `0x167356`
and completion of hiding at `0x16d748`. The Halo 2 control update
follows the native actor's contact and behaviour transitions.
It uses its guard/search states for loss of contact, entry to uncover
while tracking the enemy, entry to cover, and exit from hiding with
a relevant contact. The focused-prop awareness levels come from
the Halo 1 AI compatibility code.

**Built.** The group responses at `0x1eff5c` onward distinguish
allies within five units from other audible nearby actors.
The lost-contact reply requires an alert guard, the uncover
reply requires the same target, and retreat requires the same
squad. The flee reply is limited to allies at the source's full
combat awareness level who are still able to act. These checks
run after the triggering row has finished in Halo 2's speech cache.

**Built.** A new hostile grenade triggers startle in Halo 1 at
`0x15000b`. Its danger update at `0x166f31` distinguishes enemy,
allied and own grenades, with 20- and 30-tick fuse conditions for
the latter two. Halo 2's projectile danger selection at `0x265119`
records the projectile, its relationship and its remaining fuse
for that actor. The hook follows the selected grenade, schedules
the enemy warning after the initial startle, and uses the original
fuse limits for ally and self warnings. The exposed actor's retreat
can select the involuntary grenade line with the original one-half
chance from `0x46c30`.

**Built.** The blocked remark at `0x16376e` requires 45 consecutive
source ticks of blocked movement during combat. Halo 2's control
update compares requested movement and actual velocity and looks
for the allied unit occupying the requested direction. The reply
goes to that exact blocker after the request has finished.

**Built.** Postcombat remarks are assigned from the combat
outcome in Halo 1 at `0x13d66c`–`0x13d760`. That check uses the
pre-combat and remaining group size and the speaker's original
and current body vitality. It distinguishes serious wounds,
an isolated survivor, heavy losses, a rout, and a healthy survivor.
Halo 2's actor control hook records starting strength and vitality.
Entry to its postcombat behaviour at `0x1b4240` selects the remark
after contact ends for the group. Three original aftermath rules
allow nearby allies to answer after the initial line has finished.

**Built.** The source corpse-action stimulus at `0x16d20e` uses
the body's relationship and class. Halo 2 event 98 at `0x1b3c3e`
checks an allied body and uses the original allied check remark.
The actual corpse-firing callback at `0x1b39ce` identifies the
target. The speaker uses its separate authored line when that
target is a player unit.

Automatic stimuli without a verified route remain conversion errors
(see [role-audit.md](role-audit.md#retail-gaps-that-stop-conversion)).


#### Cinematic bindings (x50 and c40)

**Built.** Use the command list index from the containing H1 scenario.
`h1campaign::ai::h1::command_script_name(i)` returns `h1_cmd_<i>` with the
zero-based index in decimal. For example, source list 7 in a level with
40 authored scripts becomes `h1_cmd_7`, H2 script index 47. Resolve the
index independently for each level. Duplicate display names are legal.

| Caller | Binding |
|---|---|
| Converted H1 script | `ai_command_list(ai, list)` runs the generated script on the group. `ai_command_list_by_unit(unit, list)` resolves the unit with `object_get_ai`. Both emit `cs_run_command_script`. |
| Generated H2 cinematic code | Pass the actor and a type-21 `ai_command_script` literal to `cs_run_command_script`. Its data is the generated definition's index. Its token is the generated name. The none value is `0xffffffff`. |
| Scenario starting location | Set `h1campaign::ai::convert::Options::first_command_script` to the number of authored scripts. Conversion binds `placement_script` to the generated name and `placement_script_index` to the first script plus the source list index. A location without a list keeps index `-1`. |
| Advance and poll | The H1 advance calls deliver one stimulus per addressed actor. Status of an object list is the highest member status: 0 inactive, 1 recently finished, 2 waiting for advance, 3 running. |

#### Command execution

**Built.** H1 selects gaze strengths 5, 2, 8 and 7 for modifiers 1 through
4 (`0x16b68f`). The default changes from 7 to 2 at combat status 4
(`0x15c244`). The head-only, aiming and forced-facing controls on H2
are set on the executing command. Strength 7 requests a body turn when
the target leaves the source actor's horizontal gaze cone. Perceived
units stay live. Unperceived unit targets contribute their head position
once (`0x16b6c9`). The gaze expires in whole H1 ticks, truncated at 30 Hz
before conversion to the simulation clock. Random looks with no authored
duration read the actor's idle interval at `actr+0xec/+0xf0`.

Go-to-and-face also waits for alignment after arrival with an authored
radius. H1 `0x16ae14` requires a dot product of at least 0.984 with the
direction to the face point. Ground actors use XY and flying actors use
XYZ. The source flight flag is bit 21 of `actr`. A driver can also inherit
flight from the vehicle (`0x14c4f5`).

**Built.** Shoot requests fire on entry (`0x16bc4c`). On update, a point
target must be strictly less than half a world unit from the authored
point. Each miss restarts the timer at the variant's `actv+0x84` interval
with a two-second floor. An already accepted point can finish on the
first update (`0x16af4a`). H2 copies its accepted target at `0x1fcb4b`.
A positive authored duration overrides the burst length for this actor
(`0x1623ba`). The normal firing controller handles subsequent shots.
Placement string IDs identify source tuning after an actor migrates.

**Built.** All six animation modifiers start one custom animation. Modes
0–2 blend for six source ticks and modes 3–5 start directly (`0x3f319`).
Modes 1 and 4 take XYZ velocity from the playing channel and use normal
collision. Modes 2 and 5 also advance through obstructions. H1 sets the
two biped flags at `0x16bf18`, rotates the authored XY by the biped
heading at `0x4977f`, and clears the flags at `0x16abee`. The H2 hook
reads the converted root-motion stream at the live frame and playback
rate (`0x1c6d00`). Completion follows the unit leaving its custom
animation state, including interruption (`0x16b0b6` in H1, `0x11b930`
in H2).

### Character selection

The nearest retail Halo 2 `char` by type, refined by words in the variant's
path. Every target is on the Halo 2 disc with the same type (V: the census
test checks the table against the disc).

| Halo 1 type (variant words) | Halo 2 `char` | On |
|---|---|---|
| elite (stealth major / stealth / zealot / specops / major / other) | `elite_stealth_major` / `elite_stealth` / `elite_zealot` / `elite_specops` / `elite_major` / `elite` | 01b, 03a, 03b, ... |
| jackal (major / other) | `jackal_major` / `jackal` | 03a, 03b, 05a, ... |
| grunt (specops / major / other) | `grunt_specops` / `grunt_major` / `grunt` | 04a, 04b, 07a / 01b, 03a, ... |
| hunter | `hunter` | 03a, 05b, 07a, 08a, 08b |
| marine, crew (johnson / sergeant) | `marine_johnson` / `marine_sgt` | 01a, 01b, 03a, ... |
| marine | `marine` | 01a, 01b, 03a, ... |
| crew | Halo 1 actor variant, unit, colours and weapon | source level |
| combat form (human / other) | `flood_combat_human` / `floodcombat_elite` | 06a, 06b, 07b / 04b, ... |
| infection form | `flood_infection` | 04b, 06a, 06b, 07a, 07b |
| carrier form | `flood_carrier` | 04b, 06a, 06b, 07b |
| monitor | `monitor\ai\monitor` | 04b, 08b |
| sentinel | Halo 1 actor variant, unit, weapon and animation graph | source level |
| mounted weapon | `ai\generic_turret` | most levels |
| engineer, assassin, player, none | `ai\generic` | every level with AI |

A converted level needs each chosen tag in its map; no single retail map
holds them all. `h1campaign::donor` copies the ones the base (05a) lacks
from a donor, whole tree and sounds (`h1_assets::sounds::transplant`): hunter
from 03a, flood combat elite, infection, carrier and the monitor from
04b, flood combat human from 06a, and the special-ops elite and grunt
from 04b.

**Built - Sentinel graph.** The Halo 1 variant points to its own rebuilt
biped. The model converter reads the biped's `antr` and emits a `jmad`
with local animation resources and owner fixups. On c40, the fixed weapon
class's slot 0 selects each stance's idle. Slot 25 selects the nine-frame
aim with a three-by-three yaw/pitch screen. The builder emits the idle
and lookups for combat, patrol and crouch under class/type `any/any`, as
on 04b. Combat and patrol share the four directional aim overlay names.
c40's ordinary and
defensive variants both name the Sentinel beam and upgrade to the major.
The eleven flight parameters at Halo 1 `bipd+0x324` correspond to the
sequence at `+0x2cc` on 04b's Halo 1 aggressor. Both retail aggressors
have biped flags `0xe00` and character-physics flags `0x13`. The style
comes from 04b when donor maps are supplied.

**Built - crew.** Each distinct `actv` gets a definition-built `char`
with its unit, perception, vitality and weapon properties. a10's tech,
engineer, security, science and command variants have different RGB
bounds in the block at `actv+0x22c`. Their rebuilt bipeds share a model
and graph. Security carries the source pistol (the retail magnum in the
Halo 2-assets variant). The other five variants (including the pilot)
have null weapon references.

**Built - firing positions.** Halo 1 encounter flag `0x20` marks all
three a30 Banshee encounters (584 points). They keep their altitude and
use a null sector, as in 06a's flying Sentinel zones. Ground points use
their assigned BSP at encounter `+0x7e`. The surface reference and the
generated sector polygons give the floor point and normal, including on
slopes and polygon edges. Halo 2 uses a sector index at firing position
`+0x14`, confirmed against 05a's sector polygons. Ground clusters are
queried just above the floor and area centres and deviations recomputed.

The placement counts in this census cover squads with starting locations. a30's
three banshee encounters fight in the air, over the sea: an encounter whose
firing positions are mostly off any floor counts as flying and is left out
of the floor check. The rest must be on a sector for at least 98% of firing
positions and 90% of starting locations; the misses (a30's river and cave
floor, a10's flanks) stand on scenery, which isn't in the structure BSP, on
slopes past 45°, or in water.
**Built - script-created and migration squads.** Halo 1 squad literals
encode `0x80000000 | local_squad << 16 | encounter`. The converter uses
these indices to retain named targets, including on a50 and b40. The
native migration evaluator requires a squad datum (`0x002a54cc`).
Direct placements of empty squads with a defined actor use authored
positions from that squad's movement or firing groups. Examples include
a50's `marines_initial/marines_a`.
b40's `a2_top_cov/grunt_sniper` names only firing group Y, and its
encounter has no position in that group, so no site of any kind can be
authored for it: it converts to a squad datum with zero starting
locations, stays named and addressable, and a placement of it selects
no start and spawns nothing. Halo 1 reads the squad's authored start count
at `0x13b2bb` and indexes the selected 0x1c-byte record at `0x13e397`.
This squad has zero records. The engine the converted map
runs proves the outcome: the native selector (Halo 2 `0x002039d0`)
reads the squad's starting-location count and, with none selectable,
both passes fall through to `orl $-1, %eax` at `0x00203bb3`; it has no
move-position or firing-group fallback of its own. b40's
`a2_bottom_cov/grunts_tub` has no actor or start to instantiate. Both
remain addressable for migration.

A level converted without its scripts keeps none of these: nothing names
or places a squad, and the automatic migration orders are written with
the scripts. Squads of encounters only the scripts create are left out
while the pool is over (a `dropped` line counts them), so a50 fits its
386 slots either way.

The campaign squad-pool hook runs before `0x00200b30` initializes the
arrays. Retail allocates consecutive `0xc760` and `0x163c` buffers
at `0x00200957` and `0x0020098a`. The hook reserves space for 386 squad
records at startup and sets the active capacity from the scenario on each
map load. Smaller maps restore the retail squad capacity. The converter uses the same capacity calculation.

The ignored disc tests cover the a30/a10 sector readback, every campaign's
scripted squad references, and the a10/c40 character-to-animation resource
chain. Gameplay and map size are not verified by them.

### a30 ground encounters

The manual BSP field is zero throughout a30. Its compiled BSP selects the
second structure for the later plateau encounters. The two structures have
separate sector tables. In retail 05a, 469 of 499 assigned firing positions
in the landing zone and 186 of 188 in the tower zone lie inside the named
sector in XY. Treating those indices as collision surfaces produced zero
polygon matches in both samples.

Applying the generator's walkability and area cuts to a30's source
collision assigns 1,635 of 1,656 ground positions in BSP 0 and 2,373 of
2,405 in BSP 1. The other 53 have no walkable source surface. The converter
keeps their coordinates and leaves the sector unassigned. In the first
wave 510 of 512 positions share one component through two-sided links.
All 54 in `mid_cliff_1` share a component in BSP 1. All 17 first-marine
starts and all five mid-cliff starts stand over the main firing component
of their respective encounters. None of the first wave's 73 starts stand
over its main firing component. That wave stages elsewhere and its scripts
place platoons and load their actors onto dropships. Checking a path from
the authored start to the landing zone would miss that transfer.

The ignored `a30_converts_onto_its_base_and_reads_back` test checks the
emitted map's sector references, cluster masks, connectivity and starting
weapons against the characters' inherited firing profiles.
