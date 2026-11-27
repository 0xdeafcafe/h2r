# Bots: phantom peers, audio and the pad

Status: part of [bots-phantom-peers.md](bots-phantom-peers.md); see there for status, labels and the index. This page: phantom audio, the action from a pad and ping.

**Phantom audio.** `crates/extensions/game/src/bot_glue/audio.rs` uses the
Phantom machine ID on the player datum to exclude any Phantom entry from the
local-user map while `SOUND_OWNER_USER`, `SOUND_LISTENERS_UPDATE` and
`SOUND_UPDATE` run. It restores the map on return. Human slots keep their
original user indices, including in splitscreen.

The listener refresh runs both inside the sound frame and from the effect
update. Both entry points clear the active flag for Phantom and unmapped
slots.

The sound owner check leaves a Clanker's attached weapon and unit sounds
positioned. The retail mixer selects a human listener and handles
distance, panning and obstruction. A human's first-person sound uses the
retail owner check. The [agent's active listener list](../runtime/agent-api-input.md#audio-voice-routing)
shows the slots and camera positions the game can select.

**The user's Halo 1 run (Live custom game on
`h1_bloodgulch`, 3 local Clankers and 4 phantoms, a crash a while into
the game).** Read from the log tail and the disassembly; no run yet.

- **"Every phantom got peer 4."** The raw log has all four `bots:
  phantom Clanker N joins (peer 4, in the pregame lobby)` (N 4 to 7),
  after `waits: not a lobby it can join (the lobby's session is not live
  (state 7, live is 5))`, and the start found 5 peers, 4 of them
  phantoms. The phantoms had joined before; state 7 made them unmade
  (no session this console hosts live) but a session not live is not
  written, so their peers stayed. Back in state 5 each rejoin found its
  own peer, took it out (the peers above it moving down) and added it
  again at the end: index 4 each time, every phantom's peer moved under
  the game. Their machine ids were distinct all along (`machine_id` of
  each handle, host-made bots and rostered alike). **Fix:** a phantom
  whose peer is still in the session with its player takes it back in
  place (`bots: phantom G is still in the session (peer K): it stays`);
  only a peer the game emptied is taken out and made anew. Tests
  `phantom::session::tests::lobby::host_made_phantoms_back_from_a_session_not_live_keep_their_peers`,
  `a_peer_with_no_player_is_made_anew`,
  `four_phantoms_in_one_frame_get_a_peer_each`.
- **The crash is not the peers.** `#GP` at `0x28c589` is `movaps xmm1,
  [eax]` in `0x28c530`, codec 8's (`blend_screen`) interpolated rotation
  (the codec table in docs/engine/engine-map.md), called from the
  evaluation setup `0x2798a0` (return `0x279d2c`). `eax` was
  `0x820f922c`: not 16-byte aligned, so `movaps` faults. The address is
  the stream (`[0x504480]`) plus `0x20` plus channel × stride plus frame
  × 16, so a blend-screen stream or its stride in the animation data that
  map loaded is off 16-byte alignment: the converted map's data, not a
  session structure. Every unit with an aim screen evaluates it each
  frame, bots or not; which animation it was is for the converted map's
  owners (h1convert / h1map), with the stream's address in the dump.
- **The roster refusal** (`no roster from Live (the server refused:
  0x80155100)`). Live HLE gives `0x80155100` for a 404 or 409 on `POST
  /v1/games/bots` (`live_hle::client`, `Worker::roster`). The roster
  handler's only 404 is `no session of yours by that id` (the session
  looked up by id with the caller as its host, `live_server::bots`); it
  has no 409. Not a 7-bot limit (the count is cut to the room, and none
  is a 400), and custom games ask without a playlist, which is allowed.
  Not an ordering race: the driver learns the session id only from the
  server's create reply (`Intent::Put` in `live_hle::sessions`), the
  create is applied before it answers (`Live::wait_applied`, on the
  Workers hub too, which is one `Live`), and the client's one worker
  thread runs the roster request after it, as the account that made the
  session (`Active::matches`). Tests hold that for both homes
  (`live-server/tests/bots.rs`, `live-hle/tests/server.rs`). What stays
  is a 404 the client could not tell apart: the same status comes from a
  route the server does not have (`no such endpoint`: a server older than
  the roster endpoint, or a Worker hub still on an older release, see
  docs/live/live-worker.md), which the client reported as a refused
  session. The roster request now goes out after a read of the session as
  its host confirms the server has it (a bounded wait under `Retry`),
  and the 404's message decides the result: a missing route is
  `CANNOT_ACCESS_SERVICE`, a missing session `0x80155100`. Either way the
  game log carries a `live: roster:` line with the status, the session,
  the account and the server's message. If the refusal persists, that
  line says which it is; for a missing route, probe `POST /v1/games/bots`
  on the deployed Live and redeploy the hub.
- **The local bots in the game.** `join gate port 1..3: screen 0xe7 ...
  signed_in true: waits` is the gate saying the bots are signed in
  already, so it has nothing to do: `waits` is every gate line out of a
  lobby. It is no sign they were left out of the game.

#### The action from a pad

The offsets below are relative to one `0x5c`-byte action, in little-endian
order. The local and remote paths can be followed from the generated
functions for `0x185be0`, `0x186ab0` and `0x84990`. The Default layout
initializer (`0x218420`) assigns each abstract action slot `k` a physical
button.

The input pass at `0x217e40` copies each button's down count and elapsed
milliseconds into its slot. The control builder copies those arrays at
`0x186570` and tests the relevant `k`. Finally `0x187029..0x187083` writes
the record that the host consumes at `0x14d6d4..0x14d7e7`.

The host translates that record to unit controls at `0x1505f0`. This is the
same Default layout local Clanker profiles use. [Controller layout
trace](../input/controller-layouts.md)

| Default pad input | Record field and timing | Local evidence |
|---|---|---|
| A / South, jump (k0) | Flags dword `+0`, mask `0x2` while down. A also drives k14, which sets word `+0x1c` bit `0x100` on the first tick. | `0x1868cb..0x186918` (jump inhibit latch, `[0x510c50]+5`), `0x186996` (k14). |
| Left stick click, crouch (k10) | Flags `0x1` while latched. First click admits crouch if movement magnitude squared is below 0.25 or the biped check admits it. Release clears the latch. | `0x1865bf..0x18662b`, including the biped predicate `0xe4050` (object header kind zero at `+3`). |
| B / East, melee (k4) | Flags `0x30` while down. | `0x18691b..0x186942`. The host filters repeats at `0x14f70d`. |
| White / LB, flashlight (k5) | Flags `0x02100004` while down. The mask is shared across contexts. | `0x186898..0x1868cb`. A visible flashlight effect depends on game mode and unit. |
| RT, fire (k7) | Flags `0x00010040` while down, plus pressure (0–1) at `+0x14`. | `0x1867bc`, `0x186945`, `0x187058`. |
| LT, grenade or left gun (k6) | Pressure at `+0x18`. Flags `0x00224800` while active plus `0x04000000` for a grenade. Dual wielding suppresses the grenade bit. Letting go rearms a trigger still down when the left weapon was dropped. | Default LT slot selected at `0x217ed3..0x217f01`. Dual check and four LT flags at `0x186751..0x18684a`, grenade bit at `0x18684d`. The host dispatches a grenade action at `0x14fe58..0x14fe99`. |
| X / West, reload or use (k2) | Word `+0x1c`, bit `0x01` while down, also `0x02` after the hold threshold. | `0x1869de..0x186a2c`. On press the host tries the contextual action and then reload (`0x14f86d`, `0x14f9d0..0x14fa38`). A held X takes a seat or board target through `0x151c90`, or a weapon through `0x151940` (`0x14f89d..0x14f8fb`). The exit path tests pressed or held at `0x14fae1`. |
| Y / North, switch weapon (k3) | Word `+0x1c`, bit `0x10` down, also `0x20` after the same threshold. An unused release selects the next eligible primary at `+0x20`. | `0x186a2c..0x186a7d`, `0x186bcb..0x186c0e`. The local builder calls the four-slot inventory search `0xcdeb0`. A handled Y interaction sets player flag `0x20` at `0x14f922`. |
| Black / RB, swap grenades (k1) | Signed word `+0x22` = requested type (0 frag, 1 plasma, −1 none). Cycle once on press, using whichever types have stock. | `0x18695f` tests down count equal to one. `0x186c46..0x186ce9` checks unit `+0x23d` and the two signed counts at `+0x23e`. `0xc6e41` applies a valid selection. |
| Right stick click, zoom (k11) | Signed word `+0x24` = zoom level (−1 off, 0 first zoom). A tap cycles once. On a long hold, release restores the level from before the click. | `0x18662b..0x18670c` forms the click and down signals. `0x186d45..0x186de0` checks eligibility, calls `0xc8960` to cycle and restores after more than 15 subsequent held updates. `0xc6e51` writes the requested level to unit `+0x241`. |
| D-pad left / right (k12/k13) | Flags `0x80` / `0x100` while down. | `0x186867..0x186895`. |

**Tap and hold timing.** The Default pad conversion preserves face-button
and trigger pressure. The XInput reader at `0x124c53..0x124cc9` tests the
last pressure byte against a per-button threshold and uses the new byte to
update that threshold. A phantom uses the same pressure history and
publishes the new sample for the current tick.

While down, the threshold can rise to pressure minus 32. While up, it can
fall to pressure plus 64. Stick clicks and the D-pad are digital.

X and Y send their levels on every tick. The host compares them to player
`+0x3c` and derives the press/release bits in `0x14f722..0x14f81a` (X
`0x04/0x08`, Y `0x40/0x80`). The threshold comes from the active globals'
player-control block, `[[0x4e034c]+0xf4]` at `+0x7c`, in seconds.

The local builder multiplies by 1000, truncates, and compares to the pad's
accumulated milliseconds. A phantom measures the hold in simulation time.

Exact agreement at the threshold with an asynchronously polled physical
controller is **[guess]**. The named button fields above are traced.

**Selectors, target and motion.** At `0x187036` the local builder packs a
weapon-set generation into `+0x1e` (u16) and the requested primary and
secondary indices into `+0x20/+0x21` (i8, −1 or 0–3). Unit `+0x212/213` are
the actual hands and `+0x216/217` are the requests. At `0xc6e2b` the unit
accepts the two indices only if the generation matches unit `+0x214`.

An objective weapon returned by `0xcfe40` can override the requested primary
(`0x186c0e`). The weapon handles at unit `+0x218` also feed the zoom check
for a deflected aim stick (`0x100f70`). Player flags at `+2` tell Y release
whether an interaction consumed the press.

Weapon, grenade and zoom ranges are checked by `0x14d0a0`. The local
interaction finder `0x151110(player, result)` returns the 12 bytes copied at
`+0x28`.

These include the interaction kind (i16), detail at `+0x2a` (seat for a seat
interaction), object datum at `+0x2c` and melee target at `+0x30`
(`0x151d41`, `0x15166c`, `0x14fd37`). An empty target has kind zero and both
datums −1.

The separate shot-assist block starts at `+0x34`. The local action builder
copies nine dwords there at `0x187074..0x187083`. Previously the phantom
always supplied the empty block from `0x14d040`. Both strengths were
zero, so the barrel's target check at `0x1a6a8b..0x1a6aa5` skipped
shot magnetism even when the reticle was close enough for a local player.

Both seats use the retail XInput reader `0x124c00`, profile/layout routines
`0x190c34` and `0x217c40`, and the full player action builder `0x186ab0`.
The builder indexes the user's 0x94-byte control history by player +0x28. It
calls `0x185be0` for look response (including acceleration, slowdown and
follow) and `0x1a4900` for weapon parameters and target search (`0x1a50a0`,
`0x1a5370`).

It writes yaw/pitch at +4/+8, trigger flags at +0 and the selected target
and strengths at +0x34..+0x57. The host validates the returned record at
`0x14d0a0`.

The retail tables have four entries. At remote gather, a second executor
uses a copy-on-write snapshot of the host world. In that view local user and
controller zero, the two player maps at `[0x4e8c20]+0xc/+0x1c`, and the
player's +0x24/+0x28 all refer to the phantom.

Its virtual XInput handle supplies the arrived pad to `0x409b6c`. The seat
brings its own raw input, button and control history between ticks. A fresh
binding clears the previous user's target and strengths before retail binds
the unit at `0x1872d0`.

The private profile slot has the profile bit and local-user index zero even
if the host has nobody signed in. `0x1557c0` supplies the unit or vehicle
camera origin for the observer at `0x4e9c8c`. Only the validated action
crosses back to the host.

A repeated gather in one tick reuses that action. A phantom samples once per
game tick and advances the button clock by elapsed simulation milliseconds.

**The facing release.** One write of the builder's must reach the live
world. When something other than the control turns a biped, such as a melee
lunge, the biped's facing update `0xe01d0` sets bit 5 (`0x20`) of biped
flags `+0x348` (`0xe0c47`). For a biped on foot with that bit set, the
control update in `0x186ab0` reseeds the control yaw and pitch from the
unit's aim `+0x168` with `fpatan` and then clears the bit
(`0x186f8b..0x18700b`).

The reseed overwrites the look delta that `0x187510` added from the right
stick. For a local player the clear lands in the live unit, so the reseed
happens once. The phantom's private build discards it.

With the bit left set, every later build reseeds the look from the aim and
drops the right stick, so the bot's aim freezes until it dies, while its
movement, fire and other buttons still work. After a built action,
`slot_action` in `crates/live/bots/src/phantom/action.rs` therefore clears
the bit on the phantom's live biped and counts it in `bots.stats`
`maintenance.facing_releases`. Film replay takes the same path.

A missed build leaves the bit set for the next build. The test is
`a_built_action_releases_the_bipeds_facing_flag`.

In the Clanker Team Slayer game `09af571f9fec7f1a33ed37edb397a140`,
before the release, every bot's aim stayed within 0.001 of one vector
from its first melee until death or the end of the match. That covers
31% of all bot frames. In those frames the bots scored 2.6 hits a minute,
against 19.3 in the rest, and their median angle between walking and aim
was 90–115°. Live telemetry shows the release working when no bot's aim
stays unchanged for more than 60 frames after an action that includes
`melee`.

The first 12 sims with a phantom seat stopped between ticks 91 and 1945. In
the Lockout log, `0x2d6307` is the return from kernel ordinal 234 inside
XAPI's `WaitForSingleObjectEx`. Thread 3’s `0x120c4f` is the return from
waiting on the async I/O work semaphore at `[0x4e0360]`.

The two stacks identify the same wait wrapper. The log does not identify the
object the private builder waited on.

The builder’s inventory check (`0xcdeb0`) can look up a weapon animation
at `0x1cc3f0` and predict its resources at `0x1236f0`. A cold read
allocates a job under the pool mutex (`0x1208b0`) and queues it
(`0x1209c0`). A pose lookup can also demand animation bytes through
`0x1dd82b` → `0x1237e0` and wait for an outstanding read at `0x1237a0`.
Which branch triggered the original refusal is **inferred, needs a run**.

The private builder skips animation prefetch. For a demanded resource it
uses the retail getter if the bytes are resident. On a cache miss or pending
I/O it checks the animation and resource against their live graph, calls the
retail getter on the scheduled game thread, then retries the same pad once
against fresh RAM. The live worker can complete that job.

A further miss or refused call discards the private build. The hook logs the
first failure of each kind and revalidates the same unit’s previous action
for one tick. With no eligible action it returns zero, which leaves the
player out of the gather mask.

Repeated gathers reuse that tick’s decision. A later successful build
resumes normal input.

The image-backed test `retail_pad_sequence_matches_local_angles_target_and_fire`
uses `H2_DISC`, its matching `H2_RETAIL_MODULE`, and a multiplayer checkpoint
in `H2_PAD_CHECKPOINT`. Capture the checkpoint with the agent API's
`state.save` while port zero is on foot, the menu is closed and an opponent
is inside the weapon’s assist cone. The test compares the full actions and
requires movement of the aim angles, an object with a controlling player, a
positive assist strength and a fire bit.

**Open, for a run**:

- **Clients and the phantom's address.** A real client opens a channel to
  every peer's XNADDR (`0x5cb80` → `0x76aa0`, `0x5d237`) and its drop loop
  (`0x5a090` → `0x5fda0`) runs in state 5, which is the running game too.
  So on a client a phantom may be dropped locally, in the lobby (where
  phantoms now are from the start) or in the game. If a run shows it, our network layer has to answer for phantom
  addresses on clients. The host is safe: its loop drops only connected
  peers.
- The veto at `[S+0x78a8]` vtable `+0xc` (the object at `0x527fe8`):
  its vtable is set at run time, so what it refuses is not read; phantoms
  never go through it ("Phantoms in the pregame lobby"). `0x5f970` is a
  channel's teardown, not a ready check, and `0x62e30` is the free-slot
  count, now honoured.
- The profile appearance starts at configuration `+0x40` (traced at
  `0x18fd94`). Phantoms set its colours and emblem from their full tag.

**Phantom ping.** The host gathers its phantom seats' controller reports
on the simulation tick that requests them. Perception and motor delays
come from the brain (§3.4). Real network clients receive the host's
normal distributed player updates.

A sim can set `phantom_ping_ms` to model a remote controller. With a
positive round-trip time, the host samples a one-way delay of half the ping
plus uniform jitter bounded by `phantom_jitter_ms` (default 4 ms), rounded
up to simulation ticks. A zero or omitted ping delivers the current report
immediately.

In delayed mode the latest arrival supplies the stick position. The host
delivers each trigger's pressure changes in send order, at most one per tick
after arrival.

A new unit or a break in the stream clears buffered reports. The host always
builds and stamps the retail action at gather with the current weapon and
facing.

**Midship replay.** In game
`531b5da3ceab388417c044afcf18e01a`, the 2,672 recorded frames attribute
2,231 shots to the four phantoms. Fourteen bullet hits and nine melee hits
from them dealt positive damage to opposing players. The three local bots
recorded 1,372 shots and 236 opposing bullet hits.

A team with no kills has no assisted kills either. The damage reports
establish that phantom fire reached the host.

The old phantom adapter treated a half-stick on either axis as about 54°/s.
The local Midship table gives about 9°/s yaw or 5°/s pitch for that input
before aim assist. One phantom's pitch swept from +34° at tick 1284 to −26°
at 1300 while the human's bearing stayed within 6° of level.

Using the local response removes this excess gain from the delayed control
loop. Post-change hit rates require another hosted match.

**Training coverage.** A sim match hosts a private System Link game.
Each phantom's brain, uplink, remote gather and retail action builder
run as they do in a hosted custom game. Training waits for every
configured seat to bind to a player and unit before counting combat.
[Headless simulation](../runtime/sim.md#the-playerless-system-link-host)
