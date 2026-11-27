# Superbounce: ghost runs, medals and the bounce view

Ghost runs, the medals, recording and the bounce view. Part of [superbounce.md](superbounce.md).

## Ghost runs

A ghost run is a recorded run that bounced off a spot, replayed in the
world when the player asks for it: one faint outline figure in the HUD's
colour walks the run, with no trail, ring or button glyph. Showing one
is the `superbounce-ghost` feature, off by default
([switches](../runtime/switches.md)); runs are recorded and kept either
way. It is
`crates/extensions/game/src/superbounce_ghost.rs`; the file format is
`superbounce::ghost` (`crates/live/superbounce/src/ghost.rs`).

**Recording.** The mode's tick hands every player's reads and the medal
chain's awards to the recorder. It records the bots (phantoms; they
practise bounces while the mode is on, [bots.md](bots.md#practising-bounces))
and, while `superbounce.verify` runs, the local player's trials. Each
such player has a track of its last minute of ticks: the origin, the
aim's yaw, the vertical speed, the airborne bit, the level pair's word
and the crouch request (unit flags bit 23). A teleport (more than 3
units in a tick), a new unit or a gap in the ticks starts it again.
When the chain files a bounce ([Medals](#medals)), 3 ticks after its
touchdown as its launch window closes, the track counts the flight from
the touchdown (its launch the best vertical speed since), follows the
climb to its top (the first tick not rising, or 90 ticks) and makes the
run:

- it starts at the crouch that primed the word: the run of crouched
  ticks that ends at most 300 ticks before the first primed tick (or on
  it), else that first primed tick, with 30 ticks before it for the
  approach;
- it ends at the top of the bounce;
- each tick is a frame: the origin, the facing, and flags for the
  crouch request, the jump's take-off (off the ground at 1 unit/s up or
  more, before the bounce), airborne and primed;
- the spot is the strip within 0.5 of the touchdown origin, its `stand`;
  a bounce with no strip that near makes no run.

The inputs come from the unit, not the pad, so every seat is recorded
the same way: the crouch request is the crouch the game acted on (a
forced crouch keeps it), and the jump is the take-off it caused.

**The best run of a spot** is kept in the map's ghosts file,
`<map>.ghosts` in the bots directory beside the `.spots` and `.verified`
files. A run that primed itself (a crouch before the touchdown) beats
one whose word was set for it (a verifier's drop); then the shorter run
wins; then the higher launch. A kept run also marks its spot proven in
the verified file ([Verifying spots](superbounce-landings.md#verifying-spots)), landing offset
and launch with it, unless the spot is proven already. A ghosts file
that cannot be read (another version, a damaged line) is named in the
log and is not written over.

The file is text, versioned by its first line:

```text
# superbounce ghosts v1
ghost -10.620 3.880 -3.190 source=bot launch=12.31 touchdown=212 frames=230
f -10.910 0.720 -1.160 -121.0 c
f -10.905 0.718 -1.160 -120.5 ca
```

A `ghost` line gives the spot's `stand` (matched to a spot as a verified
entry is, within 0.3 across and 0.5 up), `source` (`bot`, `verify` or
`player`), `launch`, `touchdown` (the frame of the bounce tick) and how
many `f` lines follow. A frame is x, y, z, the facing in degrees and its
flags: `c` crouch, `j` jump, `a` airborne, `p` primed, `-` none. A run
is at most 1800 frames.

**Showing one.** With the feature on, only the chosen strip's run is
offered (`showable`: the navpoint's strip while primed, its run's spot
within 0.3 of the strip's `stand`). The player looks at it: its `stand`
within 15 units of the eye (0.62 over the origin), and within 6 degrees
of the aim, or the angle a 0.4-unit mark spans when that is wider. The
prompt line says "Press (X) to show ghost". The prompt line's
recipe steps come first; the ghost's prompt replaces only "Hold (crouch)
and walk under". A press plays the run once; a press while it plays
stops it ("Press (X) to stop ghost"); a press after it ends plays
it again. Leaving the map or the mode stops it.

The button is X (`superbounce_ghost::BUTTON`, read by the fall assist's
input tap), except on the Bumper Jumper layout, where it is B
(`BUTTON_BUMPER_JUMPER`): there X is the flashlight, which drives the
[bounce view](#the-bounce-view). The tap reads the first local player's
button-layout byte from the profile (`layout_glue`'s settings record,
`+4`) every read (`publish_layout`, `button_for`); a layout it cannot
read, or any other layout, is X. The prompt shows the chosen button's
glyph: "Press (B) to show ghost" on Bumper Jumper. The fall assist reads X
only in the air, so a press counts only while the player is on the ground
(`press_counts`); a press in the air is spent, not kept for the landing,
and the two never clash. On the keyboard it is X's key.

**Playback runs on the clock.** The frames are 1/30 s apart (the guest
tick is 30 Hz at every display rate, [high-framerate.md](high-framerate.md)).
The world-line source captures the run, its start time and the viewer
with each image; each present asks for the pose at its own present time
and gets the origin and facing interpolated between the two frames
around it. At 60 or 120 frames a second each presented frame shows its
own pose, at the recorded speed. The figure holds on its last frame for
a second, then goes.

**What is drawn**: the figure alone, an outline flat to the viewer at
35% of the HUD's colour, as tall as the standing capsule (as the
crouched one while the run crouched), with a tick on the floor the way
it faces (`marks`).

`superbounce.ghosts` lists the map's runs, the one looked at, the prompt
and the playback; `superbounce.ghost_show` plays one by its index or
stops it ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type));
with the feature off it refuses to play one.

No game has recorded or shown a run yet. To check one on the lab: turn
the mode on with bots practising, wait for `superbounce.ghosts`'s
`recorded` to count one, then `superbounce.ghost_show {"spot": 0}` and
read `playing` until `done`; the figure itself needs a windowed run.

## Medals

Eight medals: **Superbounce** for every bounce; **Double Bounce**,
**Triple Bounce**, **Quad Bounce**, **Chain Bounce** and
**Bouncetacular** for the second to the sixth bounce with no ground
between them; **Monster Bounce**, for a bounce that launches well above
a normal one; and **Bounced**, for a fall out of the map, or through a
strip's floor, during a bounce. They are awarded to every player in the game, bots included,
while the mode is on. The player sees a medal as Halo 2 shows its own, a
line of the notification list with a medal glyph ([hud-kit.md](hud-kit.md),
"The medals"); the film and Live count them. The announcer speaks each
line the HUD shows, as Halo 2's speaks a multi-kill: a chain of three
says "Superbounce", "Double bounce", "Triple bounce", one after another,
and a Monster says "Monster bounce". Bounced is silent. The lines are
AI-generated placeholders (`art/announcer`); how they queue with the
game's own announcer is in [audio.md](../audio/audio.md#our-medals-announcer-lines),
and `H2_FEATURES=-medal-announcer` turns them off.

Code: the game's `crates/extensions/game/src/superbounce_chain.rs` (the chain, the
table, the reads), stepped from `superbounce_mode`'s tick; tests in
`superbounce_chain_tests.rs`.
The record and the counts: `live_proto::film::events::medals`, the
`Superbounce` and `Bounced` events in `live_proto::game`, `live-server`,
and the site (`web/src/state/derive.ts`), in [Recording](#recording).

### The rule

The mode watches every player that has a biped, each tick: the unit, its
position and velocity, the airborne bit and the level pair's word, the
fields `superbounce_charge::Tick` reads for the first local player
([the state](superbounce.md#the-superbounce-game-type)); `superbounce_chain::read_players`.
Sixteen pair reads a tick at most, only while the mode is on.

- **A touchdown** is the one [Landings](superbounce-landings.md#landings) uses
  (`superbounce_charge::touchdown`): the tick before was airborne and
  falling at 1 unit/s or more, and this tick has stopped falling (over
  −0.5), launched, or landed. Here it counts anywhere on the map, by a
  strip or not.
- **A bounce** is a touchdown the landing classifier files as `bounce`
  once its launch window closes ([Landings](superbounce-landings.md#landings): the best
  vertical speed of the touchdown tick and the 3 after reaches 3.5
  units/s, `superbounce_charge::judge`), with the word at −3 on the
  tick before. The chain holds each touchdown that could be one
  (primed, or in a live chain) for that window, so the medal comes 3
  ticks after the contact. Every measured bounce had the word ([the
  touchdown tick](#the-touchdown-tick-trace-stepped): 0 of 4 without
  it), and the test keeps an explosion or a lift from counting. It
  counts wherever it happened: off a marked strip or not. A bounce by a
  strip names it, the nearest within 0.5
  (`superbounce_charge::landing_spot`), by `superbounce_chain::spot_id`,
  and a running rules script hears it as `on bounce`
  ([the game type](superbounce.md#the-superbounce-game-type)). A bounce off every
  strip earns its medals and is recorded with no strip (`Award::spot`
  `None`); `on bounce` does not hear it, as it has no strip id to give,
  and `on launch` hears it with the strip `none`.
- **A Monster Bounce** is a bounce whose launch comes out over
  16 units/s (`superbounce_chain::MONSTER_VZ`), the launch the landing
  classifier reads (`Pending::launch`). Every measured launch was
  under it: 7.97 to 13.22 over the six traced bounces
  ([Traces](superbounce.md#traces-trace)) and 12.31 to 12.45 with the word set
  ([the pair word](superbounce.md#the-pair-word-trace-lab-playback)); 16 is about a
  fifth over the top of them. The number is reasoned from those, not
  measured: no Monster has been traced (**Open**). The community's
  definition (Halo Fandom wiki and Halopedia, "Super bouncing") is a
  superbounce that launches higher than a normal one, off a seam thin
  enough to fall through onto a second set of gridlines, or between
  gridlines that fan out from one point in a V; neither page gives a
  number. Geometry alone does not tell a V from seams that cross, which
  launch as a normal bounce, so the test is the launch alone. Each
  bounce's award keeps how many bounce strips were within 0.5 of the
  touchdown (`Award::strips`, `superbounce.state`'s `recent`), for
  comparing Monsters against their geometry later; nothing reads it.
- **The chain** is a count per player. A bounce while the chain is live
  adds one; a bounce while it is not makes it 1. Each bounce makes the
  chain live. It stops being live at the first of:
  - a touchdown that is not a bounce (a plain landing, or a caught
    one, which leaves the airborne bit set);
  - a tick with the airborne bit clear, from the third tick after the
    bounce's touchdown on, the tick its window closes (`CLEAR_FROM`; a
    soft landing at the top of the flight is no touchdown);
  - a fall through a strip's floor (`through`, [Landings](superbounce-landings.md#landings)),
    which arms a Bounced (below);
  - the player's unit changing or going (a death, a respawn, a
    vehicle), or its origin moving more than 3 units in a tick
    (`TELEPORT`: a teleporter, `sim.state.set`);
  - the mode going off, or the game ending.

So "right into" the next bounce means the next thing the player's feet
do is bounce. There is no clock. Halo 2's multi-kill count has one: the
kill adds to the count when the last kill's tick is inside a window the
caller passes, and restarts it at 1 otherwise (**Code** `0x001ea733` to
`0x001ea751`; the count is the stat row's multiple-kill counter,
[live-gamestate.md](../live/live-gamestate.md)). A flight off a +12.4
launch is 6 s back to the same floor and 18 units high (gravity 4.15),
longer than a multi-kill's 4 s, and a flight cannot be stretched, so
the ground is the better test.

Medals are given as the count is reached, as the game gives its
multi-kills:

| Chain count | Medals | Id |
|---|---|---|
| 1 | Superbounce | `superbounce` |
| 2 | Superbounce, Double Bounce | `double-bounce` |
| 3 | Superbounce, Triple Bounce | `triple-bounce` |
| 4 | Superbounce, Quad Bounce | `quad-bounce` |
| 5 | Superbounce, Chain Bounce | `chain-bounce` |
| 6 | Superbounce, Bouncetacular | `bouncetacular` |
| 7 and over | Superbounce | |
| any, launched over 16 units/s | the above, and Monster Bounce | `monster-bounce` |

Superbounce is every bounce, so its count is the player's bounces. A
chain gives each of the other five once, as the count passes it: a
chain of eight is eight Superbounces and one each of the five tiers.
The table is one function, `bounce_medals(chain)`. Its canonical home is
`crates/live/live-proto/src/film/events/medals.rs`, used by the film and
Live; the site has a copy in `web/src/state/derive.ts`, as it has of the
kill medals. The game, which does not depend on Live, has its own copy
in `superbounce_chain::bounce_medals`, kept to the same ids
(`medals_by_id`), for the HUD and the agent API. Monster Bounce is not
in the count: `bounce_medals_with(chain, monster)` adds it last, so it is
the line a Monster shows, and the bounce's event carries the flag. The
live and site lists of the eight ids are `BOUNCE_MEDALS` (`medals.rs`,
`derive.ts`).

**Bounced** (`bounced`) is not on the chain's table: it is its own
event, `Event::Bounced`. It is earned when, for a player who was
attempting a bounce, the unit goes (the player's unit reads none) or is
replaced by another (a death and a respawn on one tick), and either of
two things led to it. The first is a fall off the map: on the tick
before the unit went,

- the unit was airborne, and
- falling at 1 unit/s or more (`TOUCHDOWN_VZ`, the touchdown's own
  test), and
- the chain was live, or the word was −3 (primed: the player was going
  for a bounce).

The second is a fall through a strip's floor: the `through` of
[Landings](superbounce-landings.md#landings) (`superbounce_charge::through`: the origin crosses
0.3 under the edge within 0.3 of it, moving down, primed or in a live
chain) arms a Bounced, and the unit going within 300 game ticks
(`superbounce_chain::BOUNCED_TICKS`, 10 s) earns it, whatever the
airborne bit, the word or the vertical speed read by then. A body that
goes through a seam ends up under the world: the level pair is gone or
rebuilt, the body slides or rests on something there with no fall
speed, and the Guardians kill it a second or more later, so the
first rule never holds on its last tick. The 10 s covers the Guardians,
a kill barrier and a fatal landing below. A bounce disarms it, as does
a teleport (`TELEPORT`), and so does living past the 300 ticks: the
player dropped through to a floor below and survived. A touchdown under
that floor within 0.5 of the edge is the same fall and is not judged.
The touchdown the fall through follows, if one was open, was the seam
hit: it is dropped, not judged. The fall through ends the chain, and the
award's `chain` is the count of the chain it ended, 0 if none was live.

So the first rule's window is one tick: the last tick the unit was read,
which must be a primed or chained fall. A fall out of the map is a death
in the air, with no touchdown before it; a death on the ground, a death
climbing off a bounce, or a death of an unprimed player falling earns
none, unless a fall through armed it. Its award's `chain` is the live
chain's count, 0 if there was none, and the chain ends. One death earns
one Bounced: where both rules hold, the armed one gives it, with the
chain it ended. What killed the player is not read: a primed
player shot dead in a fall earns it too, as does one who drops off a
ledge while primed (supposed: that falling out of the map kills the
unit and clears the player's unit before the next tick, rather than
leaving a body in the air for some ticks). A vehicle entered in a
primed fall reads as a new unit and earns it as well; the mode's
strips are floors, where a vehicle mid-fall is rare.

The chain is pure and has no guest access of its own: a `Chain` per
player slot stepped with that player's tick (`Medals::step`), so its
tests are lists of ticks. It is not in a snapshot: after a state load
or a Theater seek a chain in flight starts again at 1.

### What is known about a second bounce

A chain needs a bounce with no crouch between it and the last one.

- The word is still −3 after the bounce tick (**Trace**, [the touchdown
  tick](#the-touchdown-tick-trace-stepped)).
- The word stayed −3 through 175 ticks of walking, a run-up and a jump
  (**Trace**, [the pair word](superbounce.md#the-pair-word-trace-lab-playback)), and
  nothing in the pair pass takes −3 back; only a crouch or a sleep
  rebuilds the pair at 0 (**Code**).
- So a player who neither crouches nor stands still is primed for the
  next landing, and a chain needs no new crouch. Supposed: no trace
  follows the word through a flight after a bounce. The first trace's
  bounces at ticks 11274, 11770 and 12008 were each within 28 s of the
  last crouch, which fits one crouch for several bounces; which crouch
  came before which was not recorded.
- Every measured bounce came in at −5.3 to −5.4 units/s. A landing off a
  bounce comes in at 8 to 13 and more. Whether the crease launches at
  that speed, and how hard, is **Open**: it decides whether a chain
  happens at all on the traced patch.

### On the HUD

One notification per award for a local player, to that player's view,
unless the variant's `medals` [option](superbounce-guide.md#the-huds-options) is off:
the line of the highest medal the award earned (`Superbounce!`, `Double
bounce!`, `Triple bounce!`, `Quad bounce!`, `Chain bounce!`,
`Bouncetacular!`, `Bounced!`, `Monster bounce!`) behind its medal glyph
(U+E2A0 to U+E2A6, and U+E2A8 for Monster Bounce, new art in the two HUD
message fonts), queued through the game's own
insert (`Handle::notify`). Halo 2 shows a multi-kill the same way,
to the earner only and through the same routine, so the line's font,
colour, place and fade are the game's. No sound. A bot's medals and
another console's players' are recorded and not shown. The glyphs, the
other choices for the picture and the preview states are in
[hud-kit.md](hud-kit.md), "The medals" and "Game events and medals".

### Recording

The game says that a bounce happened, what the chain's count was and
whether it was a Monster; the medals are worked out from those wherever
they are shown, as kill medals are worked out from kills.

- **The events.** `Superbounce`, with `tick`, `player` (the slot),
  `chain` (1 and up) and `monster` (true for a Monster Bounce; absent,
  false, otherwise), and `Bounced`, with `tick`, `player` and the `chain`
  it ended (0 if none): game events
  beside `PlayerKilled` (`live_proto::game`, with their validation: a
  player of the game, a chain of at least 1). The mode sends them for
  every player it watches through the telemetry session that sends the
  kills (`Telemetry::superbounce` and `Telemetry::bounced` in
  `crates/live/live-telemetry/src/hooks.rs`, which read the game tick and
  queue `Session::superbounce` and `Session::bounced`), on the console
  that records the game: the host, or the one console of a local game.
  `superbounce_chain::record` makes the calls for each tick's awards,
  from `superbounce_mode`'s tick with the `Telemetry` the boot passes to
  `superbounce_mode::install`; it skips a console whose simulation
  world is a synchronous or distributed client (`superbounce_chain::records`),
  where the game's own damage code, and so its kills, do not run.
  `superbounce.medal_test`'s awards are not recorded.
  Like a kill, neither is sent in a cinematic, after the end, or for a
  slot that is not in the match. A full outbox may drop them, as it drops
  kills. A guest's own HUD line comes from its own simulation of its
  players; where the two disagree the record is the host's.
- **The film.** `film::events::Tracker` takes the events as it takes a
  kill: it writes a `Superbounce` (or `Bounced`) record at the match tick,
  then a `Medal` record for each medal of that count and flag, or `bounced` (the
  `Medal` kind as it is: `player`, `earned_at`, and the id in `medal`),
  and adds them to the counts the summary's `medals` map carries
  (`medals::Tally`). Both are stats records: `Record::decode_telemetry`
  gives them to Live, and `film::privacy` keeps them in the private import
  rows. The film's version stays 3: `Medal`'s `medal` and the summary's
  map are strings, and a reader that does not know the kinds passes over
  them (**Code**: an older `Record::decode_telemetry` returns nothing for
  an unknown kind, and an older `film::privacy`, which cannot decode the
  record, keeps it private). An older film has no bounces.
  [film-format.md](film-format.md) lists the kinds.
- **Live.** `Superbounce` and `Bounced` are in `game::CLIENT_GAME_EVENTS`,
  so the host may append them and the server keeps them in the game's
  events, in both homes (`live-server`; the Worker runs the same `Live`
  over its SQL store, `live-worker/src/sqlstore.rs`). Only the host
  reports them (`games.rs`, 403 for anyone else). A film's import
  carries them like kills and checks them against the ones sent live. A
  server refuses a whole batch holding a kind it does not know, and the
  console drops a refused batch with the kills beside it, so the server
  is released before the consoles that send them. A game's snapshot
  serves its last 100 `bounces` (each with `monster` when true) and `bounced` beside its `kills`; the
  stats summary counts `bounces` and `bounced` in the totals and in an
  account's row ([live-api.md](../live/live-api.md#stats-summary)). Live
  projects the medals from them at ingest with the Tracker's projection
  (`film::events::medals::Projection`) and serves them in the game's and
  the account's `medals` ([live-api.md](../live/live-api.md#medals)). The
  site reads those for the carnage report, a game's preview, the medal
  chest and the service record; it does not count medals itself. A bot's
  bounces are in the game's record and the summary's totals, and in no
  player's row.
- **The site's pictures.** A medal's badge is the art pack's file named
  after its id (`web/public/art/medals/<id>.webp` and `@2x`, original
  art, `web/public/art/ART.md`), or the game's own medal from the
  owner's disc where the server has game art
  ([web-viewer.md](../live/web-viewer.md), `medalKey`). The disc has no
  bounce medal, so `medalKey` gives none for the eight ids; the pack's
  eight are drawn from the HUD glyphs' pixels (`scripts/superbounce-medals.py`
  makes the glyphs; the site's files are their nearest-neighbour upscale)
  and listed in `PACK_MEDALS` (`web/src/lib/art.ts`).
- **Theater.** A film played in the game runs the simulation again,
  and the mode with it when the film's variant turns it on, so the
  lines show as they did (supposed: a mode forced off by
  `superbounce.set` is not in the film). The film's
  summary carries the counts to the film list and the post-game panels
  (`theater_ui::adapter`), and the web Theater reads the `Superbounce` and
  `Bounced` records as it reads the kills, and derives their medals.

### The agent API

`superbounce.state` ([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type))
has `medals`:

| Field | Meaning |
|---|---|
| `players` | a row for each player slot with a chain or a medal this game: `player` (the slot), `chain` (the count of its last chain), `live` (whether the next bounce adds to it) and `counts` (each medal id's total this game) |
| `recent` | the last 8 awards, oldest first: `player`, `tick`, `chain`, `medals` (the ids the award earned, lowest first) and, for a bounce, `strips` (the bounce strips within 0.5 of its touchdown) |

`superbounce.medal_test {"chain": N}` (with `"monster": true` for a
Monster Bounce) or `{"bounced": true}` (access:
act) files a bounce at that count, or a Bounced, for the first local
player with a unit, after the chain's rule: the HUD line and `medals`
as a real one gives them, and nothing sent to the film or Live. It is how a run checks the line without five bounces in a
row, as `superbounce.assist_test` holds X without a pad.

### Checking the medals on the lab

Lab runs through the agent API at 30 frames a second, as
[above](superbounce-landings.md#checking-it-on-the-lab):

- **The word in flight.** The recorded bounce, then `havok.agents`
  every tick to the next touchdown: the word stays −3 and the pair is
  kept at the top of the flight.
- **A second bounce.** From a bounce, steer back onto the strip
  (`superbounce.assist_test`) and read `last_landing` and `medals`:
  `bounce` and a chain of 2, or what the landing was filed as and the
  speed it came in at.
- **The airborne bit** on the bounce tick and the two after
  (`world.object`), which the chain's three-tick rule supposes.
- **What ends a chain.** A bounce, then a landing off the strip, a
  caught landing, a death and a teleport: `live` goes false on that
  tick each time.
- **Host and guest.** A guest's bounce read on both consoles: the
  host's `medals` has it at the same tick, or the record and the
  guest's HUD differ and by how much.
- **Bounced.** A primed jump off the map's edge: `recent` gets a
  `bounced` award at the tick the unit went, and `world.object` on the
  player's unit over the fall says whether the unit clears on the death
  tick or the body stays read for some ticks.
- **The line.** `superbounce.medal_test` at 1 to 7 and with `bounced`,
  with `render.frame`, against `h2 hud preview superbounce`'s `bounced`
  and `medal-` states: the glyph in its own colours, the line's place
  and fade.

No game was run for this section.

## The bounce view

The spots across the whole level, on the flashlight, and the nearby ones
on the motion tracker. Built and not run in a game. Code:
`crates/extensions/game/src/superbounce_view.rs` (the button, the pulse,
the tracker, the drawing, the agent methods),
`crates/live/superbounce/src/view.rs` (the line-of-sight test, the floors
for the map, the pairing). Tests: `superbounce_view_tests.rs` beside it,
and the tests in `view.rs`. Design mockups and the choice among them:
the map-view set of 2026-11-21 (concepts 1, 3, 4 and 6).

| Input | What shows |
|---|---|
| Nothing | Two blips on the tracker, drawn as the game's own are, filled dots: the next spot (the navpoint's) is a dot with a ring round it, and the spot after it, the landing a next crouch spot pairs with, a smaller, fainter dot. One out of the tracker's range sits on its rim, toward the spot. No other spot is on the tracker (`tracker_blips`, `TRACKER_MARKS`). Hidden while the variant turns the tracker off. |
| Tap the flashlight | A scan pulse. A ring leaves the player at 40 units a second and crosses the tracker; each spot in the world lights as it passes, settles and is gone 4 s after it lit. |
| Hold the flashlight | Every spot on the level is marked in the world: a small upright ring or diamond with its distance in metres (none past 40 m), dashed and dimmer when a wall is between it and the player, a chevron over or under it for a floor above or below, and a dashed line from each crouch spot to its landing. The tracker grows to twice its size into a map: the floors from the collision within 50 m and from 2.5 units below to 1.5 above the player's feet, their outline, the spots and pairs, every proven spot out of range on its rim, turned so ahead is up. Let go and it shrinks back. |
| Tap the flashlight twice | Each tap pulses, and the pair switches the kinds of seam the seam lines show ([Targets](superbounce.md#targets)). |

Proven spots are drawn at full strength, likely ones at half, as the
mode's other marks. Everything is in the HUD's colour (`0x00468c84`,
`superbounce_font::hud_colour`), labels in the disc's HUD font.

**The button** is the flashlight action, not the White button: the
unit control flags at unit `+0x148`, bit `0x4`, which Halo 2 tests at
`0x000c5115` and `0x000c52be` and the converted Halo 1 scope reads
(`crates/extensions/h1-runtime/src/h1_scope_zoom.rs`). A layout that
moves the flashlight (Bumper Jumper puts it on X) moves the view with
it. The press is read, never taken from the game: on a converted Halo 1
map the light and the view come on together. A press let go within 8
ticks (0.27 s) is a tap; held 8 ticks it is a hold until let go
(`TAP_TICKS`). The ghost runs take X on the ground, and B on Bumper
Jumper, where the flashlight is X ([Ghost runs](#ghost-runs)), so a press
never does both.

**Behind a wall.** One sight line a spot, from the eye (0.62 over the
feet) to 0.25 over the spot's stand point, against every collision
polygon of the scanned structure BSP, ignoring a hit within 0.05 of
either end. The polygons are gridded in 4-unit cells once a map, on the
scan's thread (`superbounce::view::Geometry`, built with the capsule
test's `Room`). Each tick tests a few spots round the list: 6 while
marks show, 2 otherwise (`SightCache`). A mark is wholly solid or wholly
dashed; nothing cuts it at a wall's edge.

**Another floor.** A spot more than 0.9 units over or under the
player's feet is on a floor above or below (`superbounce::view::level`).

**The pairs.** A crouch spot pairs with the nearest landing within 8
units, stand to stand (`superbounce::view::pairs`).

**The map's floors** are the collision polygons whose normal looks up
more than 0.7, their outline the floor edges no other floor shares (the
foot of a wall, the lip of a drop), matched at 1 cm. Higher floors are
drawn over lower ones and brighter. Stacked floors within the band are
drawn on top of each other.

**The tracker's place** is the retail tracker's, `rects::TRACKER` of the
HUD kit (x 47..130, y 360..443 at 640 by 480 fullscreen, centre 40.5
right and 42.5 up from the HUD frame's bottom-left corner, which is read
at `0x004b9dd8`; [hud-kit.md](hud-kit.md#anchors-the-safe-area-and-scaling)).
A blip at the range's edge sits 35 HUD pixels from the centre (S: the
blip disc's own radius is not decoded). The range is the `hudg`'s
`radar_range` (`+0x290`), as the bots' radar reads it, or 25 m without
one. The host moves the HUD's bottom-left group for the window's aspect
and the HUD Position setting; the view reads that fit
(`render_aspect::hud_bottom_left_fit`, the first view's) and applies it,
so its marks stay on the tracker in Centred and Pinned. In a split
screen it follows the view at the frame's top left (S: the first local
player's). The map's growth keeps the bottom-left corner of its box on
the tracker's; it covers the shield meter while held, which stays where
it is.

The marks over the tracker are host marks, drawn by the window's mark
pass in the image's HUD space (`gfx::world_lines::HudMark`), not HUD kit
elements: the kit only places the game's own widgets, and it has none for
a polygon or a map.

**Any frame rate.** The button, the sight tests, the blips (at the
tick's yaw, so they turn with the game's own blips) and the map are made
once a 30 Hz tick, with the instants the pulse and the growth started.
What is drawn is a function of that and the `now` the window gives each
present: the same `now` gives the same picture however many presents
there are, and the pulse, the fades and the growth run on the wall clock.

**The agent API.** `superbounce.view` (read) says the mode (`none`,
`pulse`, `hold`), the button, the pulse's age and front, the growth, the
spots marked and how many are behind a wall, the tracker's blips and the
map's counts. `superbounce.view_test {"mode": "pulse"|"hold"|"none"}`
(act) drives it with no pad
([agent-api.md](../runtime/agent-api.md#superbounce-the-superbounce-game-type)).

### Checking the bounce view on the lab

- **The button.** On Default, White taps a pulse and holds the map; on
  Bumper Jumper the same on X; on a converted Halo 1 map the light comes
  on too. `superbounce.view`'s `button` follows the press.
- **The tracker's place.** `render.frame` with blips and the map shown,
  4:3, 16:9 Centred and 16:9 Pinned: the blips sit on the tracker and
  turn with the game's own on a fast turn.
- **The blip disc's radius.** A bot at the edge of the tracker's range
  against a spot blip at the same distance.
- **Behind a wall.** `superbounce.view_test {"mode":"hold"}` on
  Zanzibar from a spot inside the base: `hidden` against what a look
  round says.
- **Frame time.** `profile` while held on Zanzibar.

No game was run for this section.
