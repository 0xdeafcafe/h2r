# Teabags and the Incel medal

Status: Built. No HUD line shows the medal in the game.
Code: `crates/extensions/game/src/teabag.rs` (`Watch::step`, the feature
`TeabagWatch`); the medal in `crates/live/live-proto/src/film/events/medals.rs`
(`Incel`). Tests: `crates/extensions/game/src/teabag_tests.rs`, the
`incel` tests in `live-proto`'s `film::events` and `medals`.
Switch: the `teabag` feature (on), listed in [switches.md](../runtime/switches.md).

The game watches every player, in every game type, for a teabag: a living
player crouching over the body of a player who just died. Each one is a
`Teabag` game event for the film and Live. At the game's end, every player
who completed it and was on neither side of a teabag earns the medal
**Incel**, "Completed a game without teabagging or getting teabagged".
Bots count as players on both counts.

## How it works

### The rule

`teabag::Watch` is stepped once a game tick (30 Hz) from the tick
observer, so it counts the same at any frame rate. Its constants:

| Constant | Value | Meaning |
|---|---|---|
| `CROUCHES` | 2 | crouches over one body that make a teabag |
| `WINDOW_TICKS` | 90 (3 s) | from the first crouch's press to the last one's release |
| `BODY_TICKS` | 300 (10 s) | how long after a death the body counts |
| `RADIUS` | 1.5 units (about 4.5 m) | from the crouching player's origin to the body's, across the floor |
| `HEIGHT` | 1.5 units | and up or down |

- **A crouch** is the crouch request (unit flags bit 23, the bit
  `superbounce_charge` reads) going on, then off, in one life of a living
  biped. A crouch held into a new life is no press. A dead player's bit
  counts for nothing.
- **A death** is a player's living unit of the tick before reading as
  depleted (object damage flags bit 2). The body is that unit, read by its
  datum each tick, so it moves as the body slides. A unit that goes
  without dying (a player who leaves) leaves no body; one still alive (a
  vehicle seat) is no death.
- **A crouch over a body**: pressed at or after the death, and within
  `RADIUS` and `HEIGHT` of the body at its press and at its release, the
  body fresh, and not the player's own.
- **A teabag**: `CROUCHES` crouches over one body inside `WINDOW_TICKS`.
  The giver is the crouching player, the receiver the dead one. One per
  giver and body: more crouches over it are the same teabag.

The bots' teabag layer ([bots.md](bots.md) §3.5) emotes only over a body
within `bots::emote::NEAR`, 1.5 units, no wider than `RADIUS` and
`HEIGHT`, with three to six presses in two seconds, so every bot emote is
a teabag (the test `a_bots_emote_over_a_body_is_a_teabag` plays the
emoter into the watch).

The watch is not in a snapshot: after a state load or a Theater seek the
crouches and bodies in flight are forgotten.

### Recording

- **The event.** `Teabag`, with `tick`, `player` (the giver's slot) and
  `victim` (the receiver's): a game event beside `PlayerKilled`
  (`live_proto::game`, with its validation: two players of the game, not
  the same one). `teabag::record` calls `Telemetry::teabag` for each one
  on the console that records the game, the host or the one console of a
  local game (`superbounce_chain::records`: not a synchronous or
  distributed client of the simulation). `Session::teabag` sends it at the
  match clock, as the superbounce events
  ([superbounce.md](superbounce.md#recording)): not in a cinematic, after
  the end, or for a slot that is not in the match. A full outbox may drop
  it, as it drops kills; Live then gives an Incel the film does not.
- **The film.** `film::events::Tracker` writes the `Teabag` record at its
  tick. It is a stats record (`Record::decode_telemetry`,
  `film::privacy`); the film's version stays 3.
- **Live.** `Teabag` is in `game::CLIENT_GAME_EVENTS`; only the host
  reports it (`games.rs`, 403 for anyone else), in both homes (the Worker
  runs the same `Live` over its SQL store). A server refuses a whole batch
  holding a kind it does not know, so the server is released before the
  consoles that send it. A game's snapshot serves its last 100 `teabags`
  and `joined_late`, the slots of players in it who joined after its first
  kill. The stats summary counts `teabags` in the totals and
  `teabags_given` and `teabags_received` in an account's row, never a
  bot's ([live-api.md](../live/live-api.md#stats-summary)).

### The medal

`film::events::medals::Incel` decides, inside the medal projection
(`film::events::medals::Projection`) that the Tracker and Live both run.
At `GameEnded` the Tracker writes a `Medal` record at the end's tick for
each slot, in slot order, and counts it in the summary; Live counts it in
the game's and the account's `medals`
([live-api.md](../live/live-api.md#medals)). A player earns it once when all of these
hold:

1. **In from the start.** On `GameStarted`'s roster, or joined before the
   game's first `PlayerKilled` of any kind. A Live guest joins itself just
   after the start, so the roster alone would leave every guest out; a
   join after a death missed part of the game.
2. **In at the end.** Not left: a player who leaves is gone, and whoever
   takes the seat after is a new participant (the Tracker's generations).
3. **The game finished.** `GameEnded`'s reason is `finished`; `quit` and
   `lost` give none.
4. **No teabag.** Neither the giver nor the receiver of any `Teabag` in
   the game.

The site does not work the rule out. It shows the `incel` count Live
serves in a game's and an account's `medals`.

**The pictures.** A teabag under a red no sign, drawn by
`scripts/live-medals.py` in the bounce medals' manner. The HUD message
fonts carry it at U+E2A7 (`font::medals::INCEL`; the rows are
`art::INCEL`, which the script writes with `--font-art`). The site's
files are `medals/incel.webp` and `@2x` (`web/public/art/medals/`, in
`PACK_MEDALS`), named after the id.

**In the game.** No line shows it. The medal is worked out in the film and
on Live at `GameEnded`, when the game's HUD has gone to the post-game
screen, and the superbounce medal line (`superbounce_hud`) shows only that
mode's awards while it is on. There is an announcer line for it,
`art/announcer/incel.wav` (an AI-generated placeholder), but the announcer
speaks only the medal lines the HUD shows
([audio.md](../audio/audio.md#our-medals-announcer-lines)), so Incel is
silent and the clip is not compiled in.

## Not verified

No game was run. Whether the crouch request goes on and off with each
press for a remote player and a bot on the host, and whether a dead
player's unit reads as depleted on the tick it dies (rather than going at
once), are supposed from the code that reads them elsewhere (the
superbounce charge and `agent_route`'s living-unit check).

## Related

- [superbounce.md](superbounce.md#recording): the medal events this one
  copies.
- [bots.md](bots.md): §3.5, the bots' teabag layer.
- [film-format.md](film-format.md): the stats records.
- [live-api.md](../live/live-api.md): the event and the summary's fields.
