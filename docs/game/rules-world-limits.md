# Rules world API: security, limits and performance

Status: Design. Nothing here is built; it is the contract every slice of [rules-world-api.md](rules-world-api.md#implementation-plan) is held to.
Code: none yet. The plan names `crates/formats/rules/src/verify.rs` (the refusals and the write estimate), `crates/formats/rules/src/caps.rs` (`Entry::cost`), `crates/formats/rules/tests/world_props.rs` (new) and `crates/extensions/game/src/rules/guest.rs` (handle checks).
Switch: none of its own; the `rules` feature of `H2_FEATURES` ([switches.md](../runtime/switches.md)).

The world API lets a script from any Live account move, push, order,
play and draw on a machine that hosts for other players. This page says
what each party can and cannot make another do, the cap on every write,
what each entry costs, and the measurement each slice passes before it
lands. The user's rule for all of it: **secure and hardened, but
performant and clean**. The rule that must hold: a check that can run in
the verifier or at load runs there, and nothing is clamped quietly at
run time.

Everything here is **Design** unless labelled
([docs-style.md](../meta/docs-style.md)). The API page defines the
roles (`Authority`, `Replica`, `Viewer`), the show channel and the
entries this page limits.

## Who controls what

| Direction | The attacker | Controls | Does not control | What holds |
|---|---|---|---|---|
| script to engine | whoever wrote the script: any Live account | the IR's bytes, the option values, and through them the value and timing of every argument of every call | an address, a datum, a size, an index into engine memory, the order of calls outside its own program order | the verifier, typed handles checked live, the caps below, refused calls |
| host to client | whoever hosts: any player's modified build | every byte and the timing of every show packet, and all replicated game state as in retail | the blob (fetched by hash from Live), the client's decoder, the client's own caps | a valid tag, then full validation; a bad record is dropped and counted |
| client to host | a guest's modified build | its inputs, as in retail, and the hello's public key | any show record: the channel is one way | the host reads no record from a peer |
| Live to host | Live, or anything that answers for it | the blob served for a hash, the session's fields, a hopper's entries, a map's census | the caps and the capability table, which are compiled in | the blob must hash to its full hash and pass `verify::load` again; the start gate runs on the host from the loaded map |
| Live to client | the same | the host's public key on the session | more than the host itself could send | the same client validation as for a host |

## The verifier refuses; the runtime does not clamp

A value that can be checked before the game is checked there, so the
tick pays nothing for it and no call is quietly changed:

- A constant argument out of its range is a verifier refusal by name: a
  duration outside 1 tick to 20 minutes, a velocity over the bound, a
  squad count outside 1 to 32, an axis other than 0 to 2.
- A `hud`, `waypoint`, table, string, tag or label index outside the
  blob's own tables; a `sound` whose tag group is not a sound's; a
  `path` argument whose label kind is not `path`; a text whose holes do
  not match its call.
- **A worst-tick write estimate.** Beside the worst-tick op estimate,
  the verifier counts each handler's worst case of each kind of write
  and refuses a blob whose worst tick passes a per-tick cap below, as
  `rules::fits` refuses one over the op budget.

What only the run knows is never clamped either. A computed value out of
range (a `vec` over the velocity bound, a position outside the world, a
handle that is no longer live) is a **refused call**: it changes
nothing, is kept with its tick and entry in `rules.state`'s `errors`,
and happens the same way on every run. A per-tick or per-second cap
passed at run time is a **script error**, which ends the round.

## Caps on every write

| Write | A tick | A second | Other |
|---|---:|---:|---|
| `teleport` of an object | 16 | 64 | one per object a tick |
| `teleport` of a player | 1 per player | 4 per player | |
| `move_to`, `move_along` started | 8 | 32 | 16 movers alive, 4 of them vehicles |
| `set_open` | 8 | 32 | 16 barriers open; a call for the state a barrier is already in puts nothing |
| `offer_pick`, `pick_row` | 1 and 6 per picker | | 2 pickers of 6 rows; the round's end takes them; the host and a client each open one picker at a time and hold the next local player's back 30 s at most; a client's picks queue 8 deep and the host keeps 32 between ticks |
| `set_velocity`, `push` | 16 | 120 | 50 world units a second; one per object a tick |
| `launch` of a player | 1 per player | 10 per player | 50 world units a second |
| squad orders | 16 | 64 | one per squad a tick |
| bot orders | 1 per phantom | 4 per phantom | |
| `seat`, `eject`, `unload`, `place_squad_in` | 8 | 16 | |
| cues of every kind | 8 | 60 | 10 a second to any one player |
| sounds | 4 | 8 per player | 4 playing per player; one music slot |
| HUD and waypoint writes | 32 | 240 | 16 elements, 8 waypoints, 4 countdowns; a text 32 bytes after its holes |
| show channel bytes to one client | 1,000 (one packet) | 600 | the state 255 bytes; the tag is inside the cap |
| retained `sees` pairs | 32 traces | | 32 ordered pairs in the table |
| objects alive from `create` | | | 64, as rules-engine.md "Budgets" |

A `Viewer` enforces the cue, sound and byte caps itself, on what it
receives, so a host that ignores them gains nothing.

## No script value reaches engine memory unchecked

- No entry takes a length, a count that sizes a buffer, an offset or an
  address. `tag_int` names a field of our schema, resolved at load.
- A handle is resolved in `rules/guest.rs` only: an object's salt must
  match the live datum and its type must be the entry's (a `vehicle`
  entry refuses a crate); a player slot is under 16 and seated; a squad
  slot is under 16 and one of the kit's; a tag index is one the blob's
  tag table resolved at load.
- A `vec` is three 16.16 values, so it is finite by construction; the
  conversion to float is exact, and the engine's own bound test
  (`OBJECT_SET_TRANSFORM`'s answer) decides inside or outside the world.
- Every table of ours is a fixed array with its length checked at the
  write: movers, the sight table, the show state, the cue ring.

## The show channel on a client

A record is used only after all of this, in order: the packet's tag
verifies; its tick is newer than the last; its length fields fit the
packet and the record's fixed bounds; every text index is in the blob's
string table and its hole values match that text's hole types; every
sound index is a sound in the blob's tag table; every element and
waypoint index is one the blob declares; every entity index names a live
entity of the client's world; the client's own caps hold. A record that
fails is dropped whole and counted by reason (`rules.state`'s `show`
counters). Thirty dropped records in a second close the channel for the
session: the script's HUD is hidden, a line names the reason, and the
game goes on. No field of a record is an offset, a length the client
allocates by, or text.

## On Live

A published script **may** do whatever its declared capabilities allow,
inside the caps, in a custom game whose host chose it. Each capability
has its line of plain words in `caps::CAPABILITIES`, shown in the lobby
and on the variant's page before the game.

A published script **may not**: run on a guest; read a gamertag, an
XUID, an input, a camera or another machine's state; write Havok memory,
a tag or a datum; play audio or show text the blob and the map do not
hold; add or remove a player seat; reach the campaign's HaloScript; or
play in a ranked game unless its hash is on Live's allow-list
(rules-engine.md "Security and usability").

**The hopper gate** (`admin::check_scripts` and `check_pairs`,
[rules-start-gate.md](rules-start-gate.md#hoppers-system-link-offline-and-theater))
checks, for each entry whose variant names a script, and refuses the
edit by name on any failure:

1. the blob verifies, and its worst-tick ops and worst-tick writes fit;
2. no declared capability is denied for that kind of game
   (`rules::policy`): `player.move` and `player.push` are their own
   lines and are denied in a matchmade hopper unless the entry allows
   them;
3. the map satisfies the script's labels, `path` labels included, and
   holds every sound the script names (the map's census, `live_proto::map_census`, gains its sound tags);
4. the script's HUD passes the kit's layout check in every screen mode.

A script can still be unfair to its own players inside all of this. It
is the game a host or a hopper's editor chose; the gate keeps it out of
curated lists, and nothing here loosens the ranked allow-list.

## A property test per entry

`crates/formats/rules/tests/world_props.rs` holds one row per world
entry, and a test fails when an entry of `caps::ENTRIES` in a `world/`
module has no row. Each row runs its entry with arguments drawn from
the whole range of each type (every `i32` for a handle, an `int`, a
`fixed`), over the fake and over the guest runtime on invented memory
whose bounds trap: the call is made or refused, nothing panics, nothing
is written outside the entry's own fields, and the same seed gives the
same result twice. The show codec has a fuzz target (any bytes: no
panic, no allocation by a length field, and what decodes encodes to the
same bytes), and the IR fuzz seeds gain the new header table.

## Performance

**Cost per entry**, in VM ops charged against the tick's 100,000
(`caps::Entry` gains `cost`; the worst-tick estimate uses the same
numbers). An op is about 18 ns on an M-series host (rules-engine.md
"Budget"), so the budget's worst case stays about 1.8 ms.

| Entry | Ops | What the host pays outside the VM |
|---|---:|---|
| a datum read (`o.speed`, `p.airborne`, `p.primed`, `o.health`) | 1 | a bounds-checked guest read |
| a `vec` read; `distance` | 3; 4 | three reads; six and an integer square root |
| `sees`, a pair in the table; a new pair | 2; 64 | a table lookup; one trace at the next boundary |
| `tag_int`, `tag_fixed` | 2 | one read at an offset resolved at load |
| `teleport`, `face`, `set_velocity`, `push`, `launch`, `set_open` | 16 | one guest call at the boundary |
| `offer_pick`, `pick_row`; `picked` | 4; 2 | a state write; a table lookup |
| `move_to`, `move_along` | 16, then 4 a tick while it runs | two guest calls a tick per mover |
| a squad or bot order; `seat`, `eject`, `unload` | 32 | one guest call, or one message to a brain |
| `place_squad_in` | 64 | the squad entry's placement |
| a cue (`message`, `sound`, `say`, `medal`, `announce`) | 16 | a ring write; the encode is per packet |
| a HUD or waypoint write | 8 | a write into the show state and a dirty bit |

**Each tick the host pays**: the VM's ops; two guest calls per live
mover (at most 32); at most 32 traces; one pass over the dirty bits; and
for each peer with something due, one encode into that peer's buffer and
one HMAC. With nothing due, a peer costs a comparison.

**Nothing allocates in a tick.** Movers, the sight table, the show state
and the cue ring are fixed arrays in `RulesExt`. The runtime's scratch,
each peer's packet buffer and each peer's filtered state are allocated
at the game's start and reused. `view` writes into values the HUD kit
already holds. A test in each slice steps 1,000 ticks of that slice's
busiest scenario under a counting allocator and requires zero
allocations after the first tick.

**Bandwidth.**

| Item | Per client | Host with 15 clients | Evidence |
|---|---:|---:|---|
| Show channel cap: at most 5 `Show` records a second and a state of 255 bytes, tags included; a target of 600 bytes in any 30 ticks | 0.6 KB/s | 9 KB/s | **Design**; the rate and the bound are built in the channel's `actor_net::Budget` |
| One moved object: about 20 bytes at 10 updates a second | 200 B/s | 3 KB/s | **S**, ai-networking.md "Bandwidth" |
| 16 movers and one drop ship with 8 riders | 5 KB/s | 75 KB/s | **S** |
| Actors, as budgeted | 8 KB/s | 120 KB/s | **Design**, ai-networking.md |

Retail's sender shares one per-client budget between entities by
relevance (**E**, notes), so more moving objects lower every object's
update rate before they raise the byte rate; probe P4 of
ai-networking.md measures it.

**Bench gates.** Each slice is measured before it lands, on the quiet
laptop through `h2 lab bench` (600 ticks, the same level and settings
for both runs; [lab.md](../runtime/lab-runs.md#--serve-and-bench)): run A is
the revision before the slice, run B the slice with its stress script,
each cap of the slice held at its limit every tick. Two profile
counters, `rules.vm_ns` and `rules.world_ns`, are added with slice W0
and read by `rules.state`. Three targets hold for every slice, and the
[plan](rules-world-api.md#implementation-plan) gives each slice's own:

- the rules feature's whole work at a tick boundary: at most 1.0 ms at
  the 99th percentile for a built-in script, and at most 2.5 ms with the
  op budget spent;
- `rules.world_ns`: at most 0.3 ms at the 99th percentile;
- the tick's 99th percentile with the feature on and an idle script: within
  2% of the feature off.

The numbers are targets of ours, set before any run. A slice's result is
a dated entry in `docs/game/notes/`, and a target that a run shows to be
wrong is changed here with a link to that entry.

## Not verified

- Every cap and every bench target: each is a first value, set before
  any run.
- The cost of one HMAC per packet and of 32 traces a tick on a dedicated
  host's core.
- That a property test over invented guest memory finds what a run
  would: the points' signatures are read from the disassembly, and a
  wrong one is found only in a game.
- Where a hopper entry keeps its allowance for `player.move` and
  `player.push`; the hopper file holds no deny list today
  (rules-engine.md "Roles, powers and capabilities").

## Related

- [rules-world-api.md](rules-world-api.md): the roles, the show channel, the entries and the slices
- [rules-world-modes.md](rules-world-modes.md): the two scripts these limits are sized against
- [rules-engine.md](rules-engine.md#security-and-usability): the VM as the boundary, the op budget, the policy and the allow-list
- [rules-start-gate.md](rules-start-gate.md): the hopper gate's pair check
- [ai-networking.md](ai-networking.md#budgets): the actor channel's budget and probe P4
- [lab.md](../runtime/lab-runs.md#--serve-and-bench): the bench the gates run on
