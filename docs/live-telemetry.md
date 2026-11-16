# Live telemetry

`crates/live-telemetry` observes the retail game state described in
[live-gamestate.md](live-gamestate.md) and produces the events and frames in
[live-api.md](live-api.md). It is opt-in and defaults to off. The crate does
not change guest state or return telemetry failures to the game.

## Sampling and hooks

Four overrides are installed after the generated module is registered:

| Guest entry | Observation |
|---|---|
| `0x0018f000` | After the original map activation returns success, read map/mode/variant, team play, score/time limits, tick, and the initial roster. |
| `0x00137fe0` | After the original tick, sample only when the game-time counter changed. Frames are emitted every second changed tick (up to 15 Hz). |
| `0x0015ce70` | Read attacker, victim, betrayal and victim position at entry, then invoke the original kill handler unchanged. Scores are reconciled at a later tick. |
| `0x001389c0` | After the original end-latch function, publish the final scoreboard and a unique high-score winner (or none for a tie). |
| `0x00104150` | Weapon barrel fire: read the weapon object datum at `[esp+4]`, count one shot for it, then invoke the original unchanged. Optional: if the entry is missing or already overridden, telemetry installs without it and frames carry no `shots`. |

The start, tick and end wrappers use shadow entries and runtime continuations to
observe after the original returns. If continuation setup fails, the wrapper
restores the guest stack and invokes the original directly. The kill wrapper
invokes the original immediately after its read.

Per-tick work follows only live player records and their current unit/vehicle
objects. It does not scan all 2,048 objects or the one-MiB object pool. A full
16-player selective snapshot is approximately 2–4 KiB of guest reads, including
592 bytes of player/team stat rows. Scores are compared every tick but emitted
only on change and at most once per 30 ticks. Names and match options are sent
on match/roster changes rather than in every frame. Vehicle passengers use the
parent vehicle's world transform rather than the biped's seat-local transform.
A replacement activation or a main-menu transition emits `GameEnded` with
reason `quit` before clearing the old session; post-end kills are ignored.

Fields rated **likely** by the research document are listed only in
`crates/live-telemetry/src/layout.rs::LIKELY_FIELDS`; they are not required by
the initial event contract. Confirmed offsets used by the readers share that
file so runtime validation has one correction point.

## Frame species, aim, weapon and shots

Each frame player carries what the web needs to draw a model
([live-api.md](live-api.md), frames; offsets and evidence in
[live-gamestate.md](live-gamestate.md), "Unit aim, held weapon, species and
shots"):

- `aim`: the unit's aiming vector (`+0x168`, rated likely), sent only when
  finite and within 10% of unit length, then normalised. A bad offset sends
  nothing.
- `weapon`: the current weapon index (`+0x212`) picks one of the four
  weapon datums (`+0x218`); the weapon must be a live weapon object
  (salt-checked, type 2) whose parent is this unit. Its definition datum is
  named by the map's tag table and sent as the file stem.
- `species`: the unit's own definition datum (the biped's, also while in a
  vehicle) named the same way: `masterchief` paths are `spartan`, `elite`
  and `dervish` paths `elite`, any other biped its directory name.
- `shots`: the weapon-fire hook counts calls by weapon datum; each tick's
  counts go to the players holding those weapons and add up until their
  next frame. A weapon change or death drops the pending count. Shots of a
  frame the sink coalesces (it keeps the newest queued frame) are added to
  the frame it keeps.

Tag names are (likely) not in guest memory: nothing in the XBE refers to
the cache header's tag-name fields, and the loader reads the tag index and
metadata. On map activation telemetry asks the boot for the
map's file (`\Device\CdRom0\maps\<short name>.map` through the VFS),
reads its tag names with `crates/map`, and keeps only `bipd` and `weap`
rows. The table is used only if the file's scenario datum equals the
guest's (tag index header `[0x00547f00]+0x0c`), since both indexes hold the
same salted datums. Without a match, or without a locator (tests, a raw
XBE), `species` and `weapon` are left out.

## Player appearance

Each player's emblem and colours are read from the signed-in profile of
their local player slot ([live-gamestate.md](live-gamestate.md), "Local
player slots and profile appearance"): palette indices for the four profile
colours and the two emblem layers. Those offsets are rated likely, so the
reader range-checks every value (colours 0..17, foreground 0..63,
background 0..31) and omits what fails; an unvalidated offset cannot put
garbage in events. Only the four local slots hold profile copies, so remote
players' appearance is absent until the in-record location is validated.
The game's rank is an Xbox Live attribute, not guest memory, and is never
sent.

Appearance rides the roster events only — never frames or scores — and is
never re-sent unchanged:

- `GameStarted` `players[i]` and `PlayerJoined` `player` gain an
  `appearance` object, omitted when nothing is known:

```json
{ "index": 0, "name": "Sneaky Pete", "team": 1, "local": true,
  "appearance": { "primary": 5, "secondary": 9, "tertiary": 2,
                  "quaternary": 11, "emblem_foreground": 30,
                  "emblem_background": 7 } }
```

- `primary`..`quaternary` are indices into the art manifest's 18-colour
  `palette`; `emblem_foreground`/`emblem_background` index
  `emblem/fg/<0..63>` and `emblem/bg/<0..31>`. Any field may be missing on
  its own.
- A mid-game appearance change to a player already in the roster cannot be
  announced with the current event types (the server rejects a
  `PlayerJoined` for a known player); a `PlayerUpdated`-style type would be
  needed if that ever matters.

The server's live-proto validation ignores the extra `appearance` keys
until its `Player` type gains the field.

## Failure behaviour

Observer errors and panics are caught, logged once, and disable telemetry. A
sink error disables later observations without affecting the guest. Override
installation refuses to replace an existing hook; boot logs the error and
continues without telemetry.

The sink worker has bounded event and frame queues. Full frame queues drop new
frames and count the drops. Events are never intentionally dropped to make room
for frames: if the bounded event queue is full, the observer reports an error
and disables itself rather than blocking the guest thread. The worker drains
accepted events during shutdown. TCP connect, TLS/WebSocket handshake, HTTP,
and socket operations are time-bounded, and the worker finishes its event drain
before shutdown completes.

## Offline JSONL capture

Set `H2_TELEMETRY_FILE` before `h2 play` or another boot entry point:

```sh
H2_TELEMETRY_FILE="$TMPDIR/h2-telemetry.jsonl" h2 play
```

The file contains one JSON object per line. Events use
`{"type":"PlayerKilled","game_id":"...","data":{...}}`; live frames use
`{"type":"GameFrame","data":{...}}`. These are local capture records, not
server `WireEvent` objects: they deliberately have no server positions, hash
chain, or signatures.

## Server ingest

Starting `h2 play --live` enables telemetry automatically: boot reads the
first account in the Live `accounts.json`, uses its private JWK, and uses the
Live `server_url`. No telemetry-specific account flags or key environment
variables are needed. Set `H2_TELEMETRY=0` to disable both Live and file
telemetry.

Plaintext credentials are accepted only for loopback URLs; remote servers must
use HTTPS/WSS. The worker performs discovery lazily, creates each game through
`POST /v1/games`, opens `/v1/ingest`, authenticates with a fresh
`games.write` token, refreshes it before expiry, sends frames unsigned as the
contract allows, and batches up to four game events into one JWS signed with
the account key through `live-proto`. Empty batches and messages over 16 KiB
are rejected. The account key is never included in debug output.

The player on local slot 0 is the signed-in account: its roster entries
(`GameStarted`, `PlayerJoined`) carry that account's `xuid`, while `name`
stays the in-game profile name. Other local slots and remote players have no
`xuid` until their accounts are known. Offline capture links no one.

## Runtime validation needed first

Unit tests use synthetic guest memory and do not execute the game. Before
relying on live output, check in this order:

1. Successful start/end and tick continuation behavior in a local Slayer game,
   including original return values and no lost guest calls.
2. Player name decoding, roster changes, and statborg values against the
   in-game scoreboard; verify kill attribution and score coalescing.
3. Vehicle-parent position/facing and the `vehicle` identifier.
4. Server discovery, game creation, token refresh, signed-batch acceptance,
   and reconnect behavior against the live ingest implementation.
5. Whether end-latch reasons other than `finished` can be distinguished; the
   current hook cannot reliably identify `quit` or `lost`.
6. The likely appearance path: local player slots at `0x0054e8e0` (profile
   copy at `+0x18`, appearance at `+0x118` inside it) joined by player
   `+0x28`, checked against the profile editor's colours and emblem; and
   where remote players' appearance lives in the `0x21c` player record.
7. Frame models: `species` for both bipeds, `weapon` names for each
   multiplayer weapon (the map-file lookup by short name), `aim` following
   the reticle (and not lagging it: `+0x15c` is the other candidate),
   `shots` matching trigger pulls (battle rifle 3 per burst, shotgun 1 per
   pull, plasma weapons and the sentinel beam counted at all), and the fire
   hook installing (stderr says when it does not).
