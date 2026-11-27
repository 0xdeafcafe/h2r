# Campaign co-op over Live (design; phase 0 slice, the phase 2 probe and Co-op over Live built)

Retail Halo 2 plays campaign co-op only in split screen on one console.
This page answers "can we add co-op over our Live service?" and lays out
the options. The owner has since decided
([section 6](#6-decisions)), and the first slice of phase 0
is built on tests ([section 8](live-coop-crate.md#8-the-coop-crate)), as is
the probe of the retail path ([section 9](live-coop-crate.md#9-probe-phase-2)) and, on the
assumption that the probe's path works, Co-op over Live itself
([section 10](live-coop-live.md#10-co-op-over-live-built)).
The owner has Co-op over Live working in the game (2026-11-17).
More than two players (up to 16, behind `coop.max_players`) is built on
tests and traced in [section 13](live-coop-join.md#13-more-than-two-players).
Join in progress (a friend joins a running campaign, nobody reverts) is
traced and its protocol built on tests in
[section 14](live-coop-join.md#14-join-in-progress), and wired through the retail
synchronous join (14.9); it needs its first game run.
Latency on the synchronous path is budgeted, measured by a
`coop-live: latency` line, and prediction assessed in
[section 15](live-coop-live.md#15-latency-and-prediction).

In short:

- **The retail game carries a networked lockstep path, and it was built
  for campaign.** The simulation layer has a synchronous client and a
  synchronous server mode, four "synchronous" network messages, a client
  catch-up loop that calls the tick, and an out-of-sync latch. A network
  session picks the synchronous protocol by itself when it has no
  multiplayer variant, and its game-start routine builds *campaign*
  options (difficulty, a co-op flag) from session parameters. Two checks
  stop it: a campaign session will not start with more than one machine
  in it, and a campaign session reports itself as not joinable. There is
  no lobby screen for it. Whether the path works once those are lifted is
  unknown; it never shipped.
- **We can also do lockstep below the game.** Both machines run the same
  image in ordinary two-player split-screen co-op, completely offline as
  far as the guest knows. The hosts exchange only pad input, each frame,
  and each shows only its own player's half of the screen. This rests on
  the runtime's machine-level determinism rather than on the engine's. It
  needs no dormant retail code, but it has presentation compromises
  (split-screen view and HUD, shared audio and pause) and it puts a hard
  requirement on everything that reaches the guest from the host,
  including GPU readbacks.
- **Recommendation:** build the shared base first (a two-pad tape, a
  simulation hash, a pad-exchange transport, all provable by tests), then
  spend one or two weeks probing the retail path with two instances. If
  two peers hold a campaign level in sync, finish that path: it gives each
  player a normal full-screen game. If not, ship the host-level lockstep.
  The probe is cheap and the base is needed either way.
- **It builds on the settings and hook layer** of the `settings-slice`
  branch: the co-op options are `Def`s, the settings peers must share
  are the ones whose scope is `View` or `Sim`, the lobby is `ui` screens,
  and the hooks chain through `Dispatch::override_handler`. Section 5
  lists what that layer still lacks for this.

Evidence labels, as in [theater.md](../game/theater.md):

- **[local]**: read from this repository's docs or code, or from a static
  reading of the owner's retail XBE with `h2 disasm` and a string and
  cross-reference scan. Static only.
- **[community]**: public knowledge. Where I could not re-check a source
  for this page it says "from memory".
- **[guess]**: my inference. Check it before building on it.

## 1. What the retail engine has

Moved to [Live co-op: the retail engine, lockstep and settings](live-coop-lockstep.md#1-what-the-retail-engine-has).

### 1.1 The simulation modes

### 1.2 Campaign through a session

### 1.3 The synchronous machinery

### 1.4 What blocks it

### 1.5 What is unknown

## 2. Lockstep below the game

Moved to [Live co-op: the retail engine, lockstep and settings](live-coop-lockstep.md#2-lockstep-below-the-game).

### 2.1 The idea

### 2.2 Input exchange and latency

### 2.3 Start-state agreement

### 2.4 Join and start flow

### 2.5 Showing one player's view

### 2.6 Pause, menus, checkpoints, reverts

### 2.7 Disconnect and rejoin

### 2.8 Audio

### 2.9 Determinism risks

## 3. Other options

- **Stream player two's view.** The host runs split screen alone; the
  friend receives video of their half and sends pad input. No determinism
  needed and it works with any build on the far side, even a browser.
  Against: an encoder and a low-latency transport we do not have, 60 to
  120 ms of latency for the friend only, upstream bandwidth, compressed
  half-resolution video, and the host renders everything. It is a remote
  desktop. Tools that do this already exist, so there is little reason to
  build it into the project. Worth remembering as the fallback that
  cannot desync.
- **Play co-op as a multiplayer game**: load a campaign scenario with the
  distributed protocol. [guess] Campaign AI, scripts and cutscenes have
  no replication in the distributed model, which is why the campaign
  protocol is synchronous. Not viable.
- **Host-authoritative state copy**: the host sends the changed parts of
  the 4 MiB game-state block every tick and the guest only renders.
  [guess] Tens of KB per tick even after XOR and compression, and the
  guest's own input still arrives a round trip late. Lockstep is strictly
  better while determinism holds.

## 4. Recommendation and plan

### Which path

| | retail synchronous path (section 1) | lockstep below the game (section 2) |
|---|---|---|
| each player's screen | full screen, normal HUD and audio | half of a split screen, composed by us |
| cost per machine | one view | two views |
| depends on | dormant retail code working, and the engine being deterministic across different local players | the *machine* being deterministic across two computers, GPU answers included |
| if it breaks | a retail bug in code nobody finished; we can see it (per-pool hashes) but may have to fix guest logic | a boundary of ours that leaks; ours to fix |
| menus and lobby | needs a lobby flow built on the session calls | free: the retail split-screen flow |
| Live features in game | the guest is online | the guest is offline |
| join in progress | maybe, through the game-state message | restart from checkpoint |
| known to work | no | no, but every part is ours |

Section 2 is the one we can promise. Section 1 is the better result if it
works, and finding out is cheap because our tools make a desync visible:
two hashes of the game-state block, per named allocation, name the pool
that parted. So: build the base, probe section 1, then commit.

### Phases

| # | what | needs the owner's game runs | effort |
|---|---|---|---|
| 0 | **Base, tests only.** A two-port schedule tap (`InputTap` answering from agreed per-frame packets, deterministic packet numbers); a pad-exchange protocol with input delay, redundancy and stall; a simulated link (latency, jitter, loss, reorder) on a fake clock; a simulation hash over the game-state block beside the whole-RAM hash; on the settings layer (section 5): the co-op `Def`s, a store fingerprint by scope, releasing forced values, the tick observer chaining instead of refusing | no | 1.5 to 2 weeks |
| 1 | **Controls on the real game.** Split-screen co-op boots and plays; a two-pad tape replays to an identical trace twice in two processes, then on a second machine; list every chunk that differs and close the boundaries of 2.9 (visibility policy, readbacks, storage, audio pacing) | yes: record one co-op session, run the replays, send the traces | 1 to 2 weeks, mostly waiting on what the traces show |
| 2 | **Probe the retail path.** Two instances over system link in a pregame lobby; a Rust entry for the session start (`0x0006ec80` is the options builder [feature-engine.md](../engine/feature-engine.md) already wants as a port) without the peer-count refusal; set the difficulty and a campaign map from the host through `0x0019a76d`; read the mode on both peers; per-pool simulation hashes on both; see how far a mission gets | yes: two instances, a few short sessions | 1 to 2 weeks, time-boxed |
| 3a | **If the probe holds: finish the retail path.** A campaign lobby (invite, difficulty, level) on the session calls; the joinable predicate; fixes for whatever desyncs; disconnect handling | yes | 4 to 8 weeks, wide error bars |
| 3b | **Otherwise: lockstep below the game.** Session data directory and start-state pack; handshake; UDP transport and server relay; stall and resume in the runner; desync report; the letterboxed own-view presenter | yes: two machines | 3 to 4 weeks |
| 4b | Anamorphic view and HUD placement; direct level boot with two players; carry-on-alone; copy progress to the guest | yes | 2 to 4 weeks |

Phases 0 and 1 serve Theater too: its phase 1 needs the same tape,
manifest and hash ([theater.md](../game/theater.md)).

### The first slice (phase 0), provable without the game

All on `lockstep::synthetic::Toy`, extended to read two ports:

1. Two toy guests in one process, fed the same two-port tape, produce
   identical traces; a one-bit change in one port's packet on one peer
   diverges at exactly that tick and `compare` names it.
2. The exchange over a simulated link: with latency under the delay
   budget neither guest stalls; above it they stall and still produce the
   trace of a zero-latency run; with loss and reordering the trace is the
   same again.
3. A read can never be answered from a local device: the tap fails the
   test if a frame's packet is missing rather than falling back.
4. Packet numbers come from the agreed stream: two peers with different
   local packet counters give the guest the same bytes.
5. A hash mismatch stops both sides within one exchange and reports the
   tick.
6. The delay can change at an agreed frame without a divergence.

Crates: `xapi` (the tap), `lockstep` (toy, hashes), a new small crate for
the protocol and the simulated link. No guest addresses, no game data.

### What needs the game, and only the owner can run

- That split-screen co-op runs at all here, and at what frame rate.
- The two-run and two-machine controls (phase 1).
- Everything in phase 2: it is an experiment on retail behaviour.
- Feel: whether 67 ms of input delay is acceptable, and how the
  half-screen view looks.

## 5. How it plugs into the settings and hook layer

Moved to [Live co-op: the retail engine, lockstep and settings](live-coop-lockstep.md#5-how-it-plugs-into-the-settings-and-hook-layer).

### 5.1 Settings, and which must match

### 5.2 Screens and rows

### 5.3 Hooks

### 5.4 Gaps in the layer that co-op would need filled

## 6. Decisions

The owner's answers to the questions this page ended with:

1. **Order.** Probe the retail synchronous path first, after phase 0.
   Lockstep below the game is the fallback.
2. **Relay.** Yes: the Live server may relay co-op traffic (pad input and
   tick hashes, or the retail path's packets) when a direct connection
   fails. Built: section 8.
3. **Saves.** The guest keeps the checkpoint and campaign progress too,
   not the host only (2.3).

Defaults taken for the rest, until someone says otherwise:

4. **Two players.** Both paths stop at two.
5. **Input delay** of 2 frames by default, per machine.
6. **Same-architecture peers first.** Cross-architecture play waits for
   traces that prove it.
7. **Telemetry off in co-op** until the tick observer chains (5.3).
8. **View compromises** for the fallback path stay as proposed in 2.5:
   letterboxed strip first, anamorphic later; shared audio and pause.

Still open: whether `settings-slice` lands before the co-op options are
built. Section 5 assumes it does; the slice in section 8 does not depend
on it.

## 7. Research points

- What `S+0x54`, `S+0x40` and `S+0x50` are, exactly (peer count, local
  peer, and a second host comparison at `0x0005b91a`).
- The code that turns the latch at simulation globals `+0x2c` into the
  "out-of-sync" reason, and what a client shows.
- The sync-client's `0x00069880` and `0x000860b0`: where the client's own
  actions are sent, and how many updates the server buffers.
- The five event kinds in the update record, and the `+0x403C` flag.
- Whether the campaign UI's session really is the same object a system
  link lobby uses.
- Readers of the split-screen viewport table and the field of view per
  view, for the anamorphic view.
- Which GPU readbacks happen during a campaign level, and which of them
  the CPU reads.
- The campaign save and profile formats, for copying progress to the
  guest's own profile (2.3).

## 8. The coop crate

Moved to [Live co-op: the coop crate, probe and lobby](live-coop-crate.md#8-the-coop-crate).

## 9. Probe (phase 2)

Moved to [Live co-op: the coop crate, probe and lobby](live-coop-crate.md#9-probe-phase-2).

### 9.1 What it changes

### 9.2 What it writes

### 9.3 Running it

### 9.4 What might go wrong, and how you will see it

### 9.5 What is tested

## 10. Co-op over Live (built)

Moved to [Live co-op: over Live, latency and prediction](live-coop-live.md#10-co-op-over-live-built).

### 10.1 What a player sees

### 10.2 Session plumbing

### 10.3 The hooks (the probe as a feature)

### 10.4 Your test (two instances, or two Macs)

### 10.5 What is tested

### 10.6 Ping

## 11. The retail lobby (co-op over Live, as the game's own co-op)

Moved to [Live co-op: the coop crate, probe and lobby](live-coop-crate.md#11-the-retail-lobby-co-op-over-live-as-the-games-own-co-op).

### 11.1 Static trace

### 11.2 What the hooks do

### 11.3 What needs a game run, and the lines to look for

### 11.4 Previews

## 12. One command on one Mac: `h2 coop test`

Moved to [Live co-op: h2 coop test](live-coop-test.md#12-one-command-on-one-mac-h2-coop-test).

### 12.1 What `--auto` does

#### "UNABLE TO JOIN ... network problems"

### 12.2 Two games on one Mac

### 12.3 The checklist

### 12.4 What is tested

### 12.5 Driving the host's menus (the plan; built as 12.6)

### 12.6 `--drive`: nobody presses

## 13. More than two players

Moved to [Live co-op: more players and join in progress](live-coop-join.md#13-more-than-two-players).

### 13.1 Where the game assumes two

### 13.2 What the code does for N players

### 13.3 What needs a game run

### 13.4 Staged plan

### 13.5 Risks and open items

## 14. Join in progress

Moved to [Live co-op: more players and join in progress](live-coop-join.md#14-join-in-progress).

### 14.1 What the retail engine already has

### 14.2 Two ways to build it

### 14.3 What the snapshot covers, and what the newcomer rebuilds

### 14.4 The flow

### 14.5 Transfer format and size

### 14.6 Failure paths

### 14.7 The join code and its tests

### 14.8 What needs a game run

### 14.9 The retail join, traced and wired (second pass)

## 15. Latency, and prediction

Moved to [Live co-op: over Live, latency and prediction](live-coop-live.md#15-latency-and-prediction).

### 15.1 Which console does what

### 15.2 The budget of one press

### 15.3 What was changed (ours, tested)

### 15.4 Prediction

### 15.5 What needs a game run

