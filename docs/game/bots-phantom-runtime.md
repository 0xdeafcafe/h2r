# Bots: phantom runtime and custom games

Why phantoms, CPU per bot, the black screen, brain threads, the game record and Game Setup (sections 5.2 to 7). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

### 5.2 Why phantoms are needed

A phantom joins as its own machine and player. It can carry a bot
account with a gamertag, emblem and rank, with all host controller
ports available for people. A playerless sim host still occupies one of
the 16 machine slots, leaving 15 slots for single-player phantoms.

### 5.3 CPU per bot

Per bot, per tick:

- perception: about 10 to 20 rays, cheap against the BSP;
- combat decisions and controller input: every simulation tick;
- path following: one A\* per re-plan, on a graph of 1 to 10 thousand
  sectors;
- aim: constant cost.

Our estimate is under 0.1 ms per bot per tick on the host's CPU. The game's
own cost for the bot's player is the same as for a human: one more biped
simulated. A phantom adds a remote player's distributed updates to
each client. A real lobby of the same size has the same update count.
Measure in a run (§11).

### 5.4 Waiting for the black screen

Phantoms send a neutral pad while the opening screen fades in and each
time they respawn. Their brains still observe and plan on every gathered
tick. The gather drops queued input while the fade is active and publishes
the current pad on the first clear tick. Configured ping and jitter resume
on the following tick.

At match opening, the host reads the black overlay's opacity for each
mapped human player from `[GAME_ENGINE] + 0xe4 + 4 * user`. Map
activation sets it to 1 (`0x1585a4`), and the renderer draws the overlay
while it is positive (`0x1599b0`). The opening gate clears once all
those views are clear. Later spawns use each phantom's own clock.

Retail keeps opacity for local users. A phantom uses its own spawn age from
the player record at `+0x194`, which starts at the guest tick rate
(`0x14f68e`) and counts down by one per player update (`0x15bb4f`). It
models the black fade retail starts at 1 on a local spawn (`0x1584e6`) and
reduces by `3 / tick_rate` per simulation tick, down to zero
(`0x15c4d6..0x15c4f9`).

At 30 Hz this takes ten updates. A playerless System Link or dedicated host
has no local views.

Its opening gate applies the retail hold through `round(1.5 * tick_rate)`
(`0x15c4c6`). Since action gather precedes the update, the gate opens at
tick 56 at 30 Hz if the phantom's own fade has cleared.

### 5.5 Brains off the game thread

`bots::think`. The simulation's gather (`0x84990`) asks for one phantom's
pad at a time, on the game thread. Read and think used to run whole at each
slot.

In a Clankers sample on Waterworks the brains took about 62% of the game
thread. Three modes, set with `Phantoms::set_think_mode`, or from the
environment for the process's phantoms:

| `H2_BOT_THREADS` | `H2_BOT_PIPELINE` | Mode |
|---|---|---|
| unset, `0`, `off` | unset, off | **Serial** (the default): each phantom sampled whole at its own slot. |
| `N` or `auto` | unset, off | **Batched**: every phantom at the tick's first gather, the thinks across `N` workers. |
| `N`, `auto` or unset | `1`, `on` | **Pipelined**: batched, and the thinks overlap the guest's tick. |

`auto` takes one worker fewer than the cores, at most 8. The game thread
also runs queued thinks while it waits for them.

Serial is the default. `h2 sim run` sets the variables only from the
match file, so a match with no `bot_threads` thinks serially; see
[sim.md](../runtime/sim.md#bot-thinking-across-cores).

A batched sample has three phases (`PadSource::observe`, `think`,
`conclude`; a plain `pad` runs all three in a row):

1. **Read**, on the game thread in slot order. Each phantom reads the
   guest's state, the kill feed, recent shots, the loaded map and the radio
   into its own staged observation. Every guest read happens here, before
   any think starts.
2. **Think**, on the workers. Perception filtering is already done.
   Memory, decisions, routes, aim and the human-input controller run here.
   A think touches its own brain, the shared map data (`Arc`, read-only)
   and thread-local scratch: the A\* workspace, grid visit marks and
   the tuning scope. Each of these is reset per query. No think reads
   another brain or guest memory.
3. **Conclude**, on the game thread in slot order. Each brain publishes
   its radio calls.

Radio order is kept. Each brain prunes calls with its own deaths, so a
serial gather could prune a call published by an earlier slot in the same
tick. A batch records each listener's place and the deaths it pruned with
(`radio::Seat`, `Radio::publish_seated`).

A late publish is then pruned only by later slots, and only while the board
was on the call's tick for that slot. That is what the serial order did.
Batched and serial give the same pads, minds and radio at any thread count.

The batch covers the phantoms the gather would sample this tick. These are
joined phantoms whose own slot is remote mode 4 with a listed player of its
machine ID and a live unit. An interrupted stream is reset first, as at the
slot.

Slots go in address order. The batch is held per hosted game and tick, and
later slots take their pad from it. A phantom outside it (the checks
disagreed) is sampled whole at its slot.

Building the retail action in a private executor writes nothing a perception
read looks at. Between two slots, main memory only gets the slot's action
record and stamp, the `0x14d0a0` check of `out`, and possibly an
animation-cache fill.

Reading every phantom at the first gather therefore reads what each later
slot would have read. The sim's state hashes should confirm this (§11).

**Pipelined**, the thinks staged at tick N run while the guest simulates
N, and their pads are tick N+1's: the bot acts on what it saw a tick ago.
Each completed think carries its pad, transport timing and observed target
range together. Serving later slots can proceed while workers compute the
following tick’s reports.

With immediate transport, the pipelined brain's pad reaches the host one
tick later than the serial brain's. With simulated transport, the uplink
credits that tick against the requested delay (`Uplink::think_ticks`). A
script or recording supplies a current pad and gets its full configured
delay.

A think from before an input reset (generation) or a tick gap is dropped,
and the seat sends a neutral pad. When the hosted game ends, the host
discards queued decisions and unpublished reports. Workers can finish an old
decision while the shell draws.

If the next match already needs input, the host supplies neutral pads until
those workers finish and resets each source before sampling it again. During
the fade (§5.4), brains observe and plan, the uplink sends a neutral pad
while it is up, and the first clear tick sends the newest pad at once.

The boot sources (`clankers::Clanker`, `sim_glue::Metered`) forward the
three phases. `Metered` counts the sim tally at the conclusion, on the
game thread.

Tests: `bots::think::tests` (serial against batched at 0 to 8 threads,
pipelined pads one tick later with identical thinks and radio, reset and
gap drops, the pool) and `phantom::action::tests` (the fade and latency
with the pipeline).

## 6. Phantom players in the game record

Bots run on the host. Clients see them as remote players, with their
state distributed by the host. Recomp clients recognise the gear mark
in the player configuration's name (§8.2). The bots leave when the host
ends the game.

**Connection bars.** The Live match scoreboard draws the connection
badge with `0x1634d0`. Retail's `0x68a90` takes the lowest score across RTT
and three traffic metrics, so a zeroed rate field can give one red bar even
at low latency. `crates/extensions/game/src/lobby_quality.rs` keeps the
game's badge visibility and uses the player's session peer for its bar
count. The local console and phantom machines get six bars.

Remote peers use the last acknowledged round trip from the game's reliable
packet queue (`0x96dc7`, timestamp at `+0x95c` and milliseconds at
`+0x960`). One bar is the fallback while the viewer has no sample for that
peer. The scale uses retail's latency calculation, interpolating from six
bars at 60 ms to one at 800 ms with nearest-even rounding.

The pregame lists show voice activity. The agent reads the same connection
ratings for their members through
[`ui.lobby.players`](../runtime/agent-api.md#ui-what-the-menus-show).

**Scoreboard and stats.** Bots participate in the scoreboard, the
carnage report and post-game stats. Telemetry identifies phantoms by
machine id and records them by game slot with `bot: true`
(`live_telemetry::Bots`, from `clankers::Driver::bots_in_force`).
A phantom on the active server roster carries its rostered XUID (§9).

The web shows a small BOT mark beside a bot's name (the live scoreboard,
the carnage report, the map legend, the map and 3D viewer labels, the
viewer's follow list). A bot with an xuid (a bot account, §9.5) links to
its player page, BOT mark and all; a host's own Clanker with no xuid links
nowhere. A bot keeps the name the game sent.

**Bots are game-only.** The game record keeps every bot with its kills,
deaths and score, and every knockout between a bot and a human. Career stats
do not: the conservative choice. A human's kill of a bot is not a career
kill, and a death to a bot is not a career death (nor do they count toward
sprees, weapons or medals); games, wins and time played still count.

This holds in the server's stats summary (`live_server::stats`, both homes;
the web mock's fold mirrors it) and in the web's derived service record
(`derive.ts` `careerKills`, `serviceRecord`). Bots get no stats-summary row
and are nobody's "played with".

A bot account's own page (§9.5) is the one place its knockouts count: its
service record counts all its kills and deaths, against humans and bots
alike. Ranks are unchanged: a matchmade game ranks accounts only, and a
rostered persistent bot's own rank is sbmm-ranked's (§9.4).

## 7. Clankers in custom games (the Game Setup page)

Game Setup gets a **Clankers** row next to the Halo 1 Options row. It is
added the same way (`crate::h1_options_row` is the model). The row opens a
**Clankers** page.

Live custom game advertisements report whether the host plans Clanker
seats, so the browser's **Clankers** filter can find games with or without
them. [Browser controls](../live/live-custom-games.md#filter-games)

The page is a native ui_kit page: a `ListPage` or a `Page` impl
(`crates/extensions/game/src/ui_kit/page.rs`). That gives it the kit's preview, its
layout checks and the `Page` table tests. These are its rows:

| Row | Values | Setting |
|---|---|---|
| Clankers | Off, On | `clankers.enabled` |
| Count | 1 .. 15 (capped at the variant's max players minus humans) | `clankers.count` |
| Difficulty | Easy, Normal, Heroic, Legendary, Mixed | `clankers.difficulty` |
| Teams | Auto-balance, Red, Blue, … (team games), Free (FFA) | `clankers.team` |
| Aggression | Calm, Normal, Wild | `clankers.aggression` |
| Aim style | Steady, Twitchy | `clankers.aim_style` |
| Taunts | On, Off | `clankers.taunts` |
| Glitches | Off, Some, Lots | `clankers.glitches` (§3.8) |

A host can change Aggression and Aim style on an existing custom-game
roster. The next input sample picks up the edit. Normal aggression uses each
bot's seeded traits and current mood.

Calm subtracts 0.35 from its drive to press, Wild adds 0.35, and the
difficulty's awareness and the fight still decide whether it advances.
Steady uses the difficulty's usual hand movement. Twitchy raises that hand's
stick engagement by 45% and the overshoot on a fresh flick by 35%.

The bot keeps its skill's tracking error and lead. Reaction timing stays
within the skill's window.

Clankermatch uses its roster's difficulty and each gamertag's profile.
Simulation scenarios use their declared tuning.

**Mixed** gives each bot a difficulty drawn around Normal. Per-bot
difficulty can come later, as a sub-page per bot; the first page does not
have it.

The settings are kept in the settings store through `setting_defs`, like
`HALO1_SANDBOX`. They persist and appear in `h2 settings`. The page is a
user of the kit.

If it needs anything the kit lacks, that goes to the ui-builder agent, which
owns `ui_kit`, through the coordinator. As far as read, `OptionRow`'s
two-choice rows need a many-choice sibling for Count and Difficulty; that is
the one kit ask expected.

**Joining the lobby.** When the host's System Link or Xbox Live custom
game lobby opens with `clankers.enabled`, the driver adds phantoms as
peers (§5.1). The count is capped by the available session slots. The
host and clients see each bot's name and team colour in the roster.
Changing the page in the lobby adds or removes phantoms.

**Leaving the lobby.** When the party leader is the only human, Back in
the custom-game lobby follows the solo leave path and skips the party
leader picker. Leave Game keeps its solo confirmation. Accepting Back in
the matchmaking lobby retires the Clankers from the target session and
returns to the party. If another human remains, the retail leave flow
handles the party and only human players qualify for leadership.

`game::bot_glue::party_leave` scopes the squad peer count at `0x199f34`
to the lobby Back and pause Leave Game checks. It uses the live squad and,
in matchmaking, the target membership to identify the sole human. The
leader picker and the automatic successor check both validate squad
player slots at `0x19a951`. A slot owned by a phantom machine fails
those checks.

The confirmed leave paths at `0x199e3c` and `0x252150` remove phantom
players and peers through `bots::phantom::Phantoms::retire_solo_session`.
The pause and lobby Back calls at `0x232ccc` and `0x25125c` target the frame
at `0x199e3c`, which follows the tail jump at `0x199e37` from
`UI_LEAVE_SESSIONS`. The hook verifies the frame prologue at that entry.

Retirement also drops their matchmaking roster entries and holds off new
seating until the departing session closes. The Clankers page settings still
apply in the next lobby.

Use `h2 agent ui` to check for `dialog.name = party_leader_picker` while
driving Back through the agent API. A custom lobby with a solo human
leader and Clankers exits without that screen. A party with two humans
on separate consoles and Clankers keeps the leader picker with the other
human as its only choice.
