# Halo 2 engine films

An `.h2film` records the game that the host simulated. Theater boots the
matching recompiled image, supplies the recorded external inputs and lets the
guest advance its own objects, physics and rules. On a distributed game the
recording machine matters. Only an authoritative host observes the actions it
accepted for all players. A film from a client describes that client's game.

## Inspired by Halo 3 theater

The recording has one clock. On the console, a tick supplies the commands
the host accepted, then the engine advances the match. In the browser,
that tick supplies a pose change or an event. In Live, the same event can
credit a kill or close the scoreboard. Each consumer reads the same compressed time chunk and skips unneeded
records by their declared length after inflation. Compression restarts at
each time chunk. Large machine snapshots are stored separately so that a clip or
match page can fetch only the public byte ranges it needs.

A clip holds the canonical film/game ID and the two ticks delimiting the
excerpt. Saving, sharing or changing its camera does not copy the recording.
To rewind in the engine, restore the nearest available machine keyframe at
or before the target and run the intervening inputs. The archived host's
original command order also applies when watching another player.

## Making and opening a film

A leased dedicated host records by default. For local play, pass
`--film FILE` to `h2 play`, `h2 run` or `h2 serve`, or set `H2_FILM_OUT`.
The destination must be a new file. Put it in `<data>/films/` or the
configured `H2_FILM_DIR` for library discovery. Recording starts at boot
so that a player can reproduce the lobby, menus and the match's initial
conditions. A multiplayer game-over seals the recording. An orderly exit
also seals it.

```sh
h2 play --film match.h2film --level ascension
h2 theater play match.h2film --disc halo2.iso --out image --data original-data
```

These examples assume `H2_DISC` is set for recording and that the second
command's disc, image and installed content match the recording.
`--data` supplies initial files referenced by digest. Theater materialises a
private guest disk for each play or seek. Any larger initial file kept by
reference must be the version the recorder saw at boot.

## Unified container, version 3

All fixed-width numbers are little-endian. Offsets start at byte zero. The
first 32 bytes and the final 40 bytes have fixed locations. A web client
can read the front metadata in two short requests, then read the footer
and chronological directory when the viewer opens.

| Absolute byte | Field |
| --- | --- |
| 0 | Eight ASCII bytes `H2BLAM3!` |
| 8 | u32 version 3 |
| 12 | u32 front capacity, 131072 |
| 16 | u32 front JSON byte length (1 through capacity in a sealed file) |
| 20 | u32 flags = 0, u64 reserved = 0 |
| 32 | UTF-8 JSON, followed by zero padding up to byte 131104 |
| 131104 | First private `HEAD` envelope, followed by time chunks and private machine material |
| At `cidx_offset` | `CIDX` envelope containing the public time directory |
| At `indx_offset` | `INDX` envelope containing the private machine directory |
| EOF − 40 | Eight ASCII bytes `H2RANGE3` |
| EOF − 32 | u64 `cidx_offset`, u64 `indx_offset`, u64 complete file length |
| EOF − 8 | u32 inflated `CIDX` byte length, u32 flags = 0 |

The front object contains `id`, `image_stamp`, `runtime_stamp`,
`started_ms` (Unix milliseconds), `map_sha256`, `map_origin`, optional
`match_start_tick` and `ticks_per_second`, initial `roster` (seat, name, team and kind), and
`summary` (level, game type, mode, variant name, duration and player
results). `final_head` locates the private final machine header for the
local selector. The menu reads the front, footer and local header
regardless of match length. Web reads just the used prefix and JSON.
Zero padding reserves room for the final scoreboard and may be sparse
on disk. The recorder writes the footer last. Truncation or an incomplete
front makes a v3 recording unavailable for playback.

Each envelope uses a four-byte ASCII tag and a u32 compressed size,
followed by exactly that many bytes of one checksummed zstd frame.
The writer pledges the inflated size in the frame header. A public time
chunk uses tag `TICK`, with at most 8 MiB inflated and 8 MiB + 64 KiB
compressed. Fetch the chronological directory with the inclusive HTTP
range `bytes=cidx_offset-(indx_offset−1)`. Its inflated JSON is bounded
at 8 MiB (16 MiB compressed) and has this shape:

```json
{"version":1,"chunks":[{"first":1,"last":128,"scene":1,"offset":132000,"size":470,"raw_size":1280}],"keyframes":[{"tick":97,"offset":140000}]}
```

Each row uses inclusive **film ticks since boot**. `offset` points at
the four-byte tag, `size` excludes the eight-byte envelope prefix and
`raw_size` is the inflated byte length. Fetch one chunk with
`bytes=offset-(offset+size+7)`. Rows start at tick 1 and cover consecutive ticks without gaps. Each row
covers at most 128 ticks. Seek by binary search. The first ULEB in the
inflated payload gives `first`, and the second gives `last − first + 1`.
Each following record has a ULEB tick delta, one-byte type, ULEB payload
length and payload bytes. The first delta is relative to `first`, then
deltas are relative to the preceding record. Unknown types can be
skipped by length. An event's payload may identify an earlier guest tick
than its chronological observation.

The optional row member `clock` is an array of pairs
`[offset_from_first_tick, guest_game_tick_or_null]`. At a known anchor the
guest clock advances once per subsequent film tick through the next change.
Before the first anchor, or following a null anchor, no guest match clock was
observed. The writer mirrors clock records (type 6, one ULEB or an empty
payload) from the same time chunk into its index row. This maps a legacy clip's
guest tick to a recorded boot tick without decoding any game state. After a
pause or campaign revert, the earliest recorded occurrence wins.

`scene` gives the first tick of the most recent public scene checkpoint
chunk. To draw a requested tick, load the chunks from `scene` through
the chunk containing it and apply the sparse pose changes in order.
The replay window is at most 16 chunks and 2048 ticks inclusive. Stats
consumers skip pose records and can decode each event payload directly.
`keyframes` contains ordered (completed tick, private `KEYF` envelope
offset) pairs for in-engine seeks. The public directory discloses those
positions. Only the private replay reader consumes the machine bytes.
Live validates public ranges against physical envelopes before authorising
access. The private index includes all envelopes through `CIDX`, then
the footer identifies both directories. Version 1 and 2 files retain
their original layouts.

### Private machine-state section

The in-game reader opens one private section through
`film::Reader::machine_section()`. Its first run starts with the boot
`HEAD`. The section includes the initial guest disk, startup configuration,
ordered HLE results (including controller reads and virtual keyboard input),
scheduler boundaries, snapshots and the continuation data needed after a
restore. It also covers cold replay when the first snapshot has yet to be
taken. Absolute offsets preserve which calls preceded each snapshot.

The private `INDX` is a JSON array in physical envelope order. Each row
stores `tag` (four byte values), `offset` (u64) and `size` (u32).
The first row of each contiguous machine run also stores `machine_span`
(u32), the number of rows in that run, including itself. All other rows
omit the field. For example, `HEAD DATA NETR TICK KEYF NETR TICK` has a
span of three at `HEAD` and two at `KEYF`. The writer can append a
public time chunk as soon as it fills, so the machine section can occupy
several disjoint byte ranges. All runs together form one section, and
each contained subchunk keeps its own zstd frame. An earlier unannotated
v3 private index is read by inferring the same runs from its tags.

Native playback and Live ingest validate the run lengths against the
envelope types. HTTP access uses the public `CIDX` time spans checked
against the actual uploaded `TICK` envelopes. Guest memory, service
results and start files stay outside those spans, including when an
adjacent public chunk is selected for a clip.

### Public and private identity fields

A public join contains the seat, name, kind and team. A public host migration
contains only its event kind. The public stats roster retains appearance and
player slots. Host identifiers, XUIDs, console names, session identifiers,
private simulation settings and bot parameters remain in the machine section.
The public summary omits player XUIDs. When those exist, a private `PSUM`
envelope stores the full summary JSON.

When a time chunk contains private fields, the writer immediately precedes
its public `TICK` with a private `IPRV` envelope. Both frames have the same
first tick, count and record framing. `IPRV` holds the original lifecycle
and closing batches and all six original stats-import event kinds in order.
It can also preserve records with unrecognised extensions. Public records
use the recognised display fields. The matching private `TIDX` block stores
`identity_sha256`, the SHA-256 of the uncompressed `IPRV` bytes. Engine
playback checks this digest and reads the original lifecycle batches, including
when starting from a keyframe. Live imports the original stats from the pair
once. A chunk with entirely public content can omit `IPRV`.

Live records a positive public-range validation decision on the upload receipt.
A legacy receipt without that decision, or a film carrying an unrecognised
or identity-bearing public record, remains available only through private
storage. User-supplied display names, map names and chapter labels are public
display text. The first supplemental v8 sample in each time chunk which
changes appearance starts a fresh predictor. This lets Live check the
bounded binary pose grammar and consume each packed sample fully before
admitting its raw bytes as a public range. Object transforms still use the
sparse scene checkpoints in `CIDX`.

### Tagged records

| Tag | Name | Payload |
| ---: | --- | --- |
| 1 | Input | ULEB changed-seat mask, then one state per set bit in ascending absolute player-slot order. |
| 2 | Event | One-byte subtype and its payload. Subtype 0 is an ordered JSON array of engine lifecycle events. Subtype 1 is a single UTF-8 JSON object with `tick`, `kind` and `data`. Subtype 2 is a lifecycle array observed after the final completed tick and its hash. |
| 3 | Pose | A presentation operation, including sparse object changes or a seek baseline. See the pose layout below. |
| 4 | Hash | Eight little-endian bytes for the post-tick gameplay digest. The reader checks it after the matching tick completes. |
| 5 | Marker | UTF-8 chapter name. The recording marks its start and the first active map. |
| 6 | Clock | One ULEB for the current guest match tick, or empty while unavailable. The index mirrors clock discontinuities for clip lookup. |

For tag 1, start each chunk with 16 empty seats. A state begins with a flags
byte. Bit 0 selects the 18-byte Xbox gamepad, bit 1 the 92-byte retail
action. Zero clears the slot. For each selected field, read its byte-change
mask (3 or 12 bytes respectively, low bit first), then one replacement
byte per set bit. An absent previous field has a zero baseline. Retain every
seat until its next change, across all intervening ticks of this chunk.
This implicit run also covers idle players when other seats change. A
network player can have a complete consumed action without a local pad.

The event envelope carries both the film observation time and, in subtype 1,
the originating match clock. A late hit, medal or final scoreboard retains
its original time. The game statistics projection consumes GameStarted,
PlayerJoined, PlayerLeft, PlayerKilled, ScoreChanged and GameEnded.
Highlights include additional weapon fire, damage, assists, medals, item
and vehicle transitions and objective changes. Recomputing stats from a
highlight as well as its canonical event would count it twice.

## Private machine data and earlier formats

Version 3 keeps the machine bootstrap, call tape and snapshot envelopes described
below. Version 2 used the following sequence from byte zero. Its `INPT`
blocks are supported by the engine reader.

Integers in fixed-width binary fields are little-endian. A ULEB is an
unsigned, minimally encoded base-128 integer with seven low-order bits
per byte, least significant group first, and bit 7 set on every byte
except the last. Tick and executor counters are unsigned 64-bit values.
Offsets are absolute byte positions from the start of the file.

```text
HEAD chunk
other chunks, including final match metadata, summary and end mark
INDX chunk
8 bytes   ASCII "H2FILM2!"
u64       byte offset of the I in INDX
```

Each chunk starts with a four-byte ASCII tag and a u32 compressed byte
count. Exactly that many bytes follow as one independent zstd frame.
Writers enable the zstd content checksum. Stored and inflated chunk sizes
are bounded at 80 MiB each. The enclosing directory allows at most one
million entries. JSON payloads are UTF-8.

| Tag | Contents after decompression |
| --- | --- |
| `HEAD` | JSON machine header. The first has `version: 2`, film ID, source, generated image stamp, runtime source fingerprint, XBE SHA-256, title ID, epoch, UTC start in Unix milliseconds, feature switches, executor budget, network settings and lease and canonical game IDs when hosted. Later headers update the display metadata and boot-relative duration. |
| `BOOT` | Effective boot options, settings, direct-level request and resolved simulation configuration. The frame pipeline records its frame prediction, RNG seed replay, deferred I/O and list of guest functions to hand back to the game thread. Older recordings use the pipeline defaults. |
| `MOVI` | Effective movie selection when Bink is enabled. Maps engine movie names to SHA-256 of the local transcodes. An empty map selects the skip path. |
| `DATA` | Initial guest file manifest. Entries identify a directory, an embedded file or a file to obtain locally by path and SHA-256. Installed content and converted campaign files are identified by digest. |
| `DESC` | Match header, including first match tick, mounted map identity and SHA-256, package/cache source if present, retail variant bytes, game type, mode, difficulty, skulls, seed and host identity. The initial roster uses absolute seats, names, optional XUIDs, teams, local/remote/phantom/bot kinds and available bot parameters. The bot kind remains readable in older descriptors. Current hosts use phantom seats for bots. Late arrivals have a join event. |
| `INPT` | Binary block of consecutive completed ticks, including actions, events and post-tick hashes. |
| `PADR` | Indexed local XInput calls. Records the actual guest result and returned state, including packet number. Covers menu input and reads between ticks. Phantom actions use `BTAP`. |
| `TEXT` | Virtual keyboard edit at the focused guest keyboard, including profile-name entry. Uses the common call cursor and the UI thread ID. |
| `BTAP` | Call-indexed results of the phantom bot action builder, including guest cold-animation requests. |
| `BOTR` | Sparse bot roster changes at management boundaries. Current hosts record phantom seats. |
| `PDEV` | Sparse controller connection changes by input-pump ordinal, including port, generation and rumble capability. |
| `NETR`, `RAND` | Batches of host boundary calls interleaved by a common call index with `PADR`, `TEXT` and `BTAP`. Records operation, arguments, thread, return/error and output slots. Socket delivery, send results and random draws retain their original ordering. |
| `MARK` | Accepted lobby operations by executor boundary, D3D snapshot boundaries, bookmarks or the final mark with slice, tick and call count. |
| `HASH` | Every 30 completed ticks, the arena digest if available and the HLE call cursor. Additional per-tick gameplay digests live in `INPT`. |
| `KEYF` | Scheduler-boundary snapshot and cursors, described below. |
| `SUMM` | Menu JSON, up to 128 KiB. Contains level, game type, mode, variant name, match duration in ticks and player results including score and medal counts. Player turnover can produce multiple results for the same seat. |
| `TIDX` | Tick directory for independently decodable input blocks and keyframes. |
| `INDX` | JSON array of `{tag, offset, size}` for every preceding chunk. A tag is four byte values and size excludes the eight-byte chunk prefix. |

A `TEXT` call uses operation `VirtualKeyboard`. Its six u32 arguments give
the screen address, buffer address, caret address, capacity in UTF-16
units, retail keyboard context and controller port, in that order. It has no output
buffers. Its successful result encodes the end of the edit batch (0),
backspace (1), accept (2), cancel (3), or insertion (0x10000 plus one
filtered BMP code unit). A focused frame with no submitted edits records
the batch end. Recording follows the native edit handler, so a rejected
paste or a modal stealing focus can cut a batch short. Playback supplies
these edit commands using the recorded trampoline configuration in `BOOT`.

A full map's digest covers every mounted byte, including applied header
rewrites. Sparse dedicated-host map stores can supply only their recorded
ranges. In that case `DESC.map.ranges` lists ordered (offset, length) pairs.
Hash the ASCII bytes `H2FILM sparse map v1` and a NUL byte, then the logical
file length and range count as u64s. For each pair hash its two u64s followed
by those bytes from the mounted map. A player with the full map can verify
the same ranges. Auxiliary shared resource maps must also match the recording.

A v2 reader uses the footer to find the directory and checks contiguous
offsets, lengths and the first/final headers. It checks a selected chunk's
physical prefix when it reads that chunk. An unfinished v2 film cannot be
selected as a sealed film. The v1 reader retains its scan-and-recover
behaviour for interrupted recordings. Legacy host films play from their
boot conditions and HLE tape. V1 local films lack a recoverable controller
stream. Their headers still appear in the library.

## Legacy version 2 input blocks

An `INPT` block starts with the first completed tick (one-based since boot)
and the tick count (1–128), both ULEB. Each block begins with all 16 seat
states empty. The state of a seat consists of an optional 18-byte Xbox
gamepad and an optional 92-byte retail player action. Bits and padding in
the action retain the bytes that reached the guest's simulation-update
consumer. Its mask identifies absolute player slots, including remote and
phantom slots. A remote player need not have a locally available raw pad.

1. Read a ULEB run length and ULEB 16-bit changed-seat mask. In ascending
   seat order, each set bit has a flags byte (bit 0 pad, bit 1 action).
   Zero removes that seat. Each present field carries a byte-change mask
   of `ceil(field_size/8)` bytes, low bit first, followed by the replacement
   bytes in ascending order. Missing previous fields have a zero baseline.
   Emit this complete set of inputs for the indicated number of ticks.
   Repeat until the declared count is reached.
2. Read a flags byte for each tick. Bit 0 carries a u64 gameplay digest.
   Bit 1 carries a ULEB byte count and an ordered JSON array of events
   (maximum 64 KiB per tick). Lifecycle events include joined player,
   quitting, removal, team change, host migration and pause edges.
3. The end of the last record must be the end of the decompressed block.
   Unknown flag bits, overflowing integers, invalid masks and incomplete
   runs are errors.

Playback supplies the recorded action block at the retail simulation-update
boundary. The engine still processes the associated network, lobby and
controller inputs. Lifecycle transitions observed as a result are available
to film consumers; applying those transitions twice would change the game.

## Keyframes and seeking

The recorder attempts a full machine keyframe on a quiescent slice and then
at 30-second intervals in guest tick time. A keyframe follows a flushed HLE
batch. In v3 the final tick stays open for observations that arrive before
the next game step or the closing scoreboard. The binary fields follow in
this order.

```text
ULEB      last completed tick
ULEB      completed scheduler slices
ULEB      next HLE call index
ULEB      lobby executor boundary
ULEB      guest disk delta byte count
bytes     delta JSON, or zero bytes for the initial disk
bytes     H2SNAP machine state through the end of the chunk
```

A `d3d_barrier` mark records the scheduler slice, completed tick and outcome
of each attempted D3D section capture. During linear playback the device
drains the same push buffer, writes back dirty render targets and resolves
visibility queries at that boundary. This also covers captures where a
later machine section prevented publication of the keyframe. A seek skips
barriers already reflected in its restored machine state. Film keyframes
use D3D section v2, which prefixes the v1 payload with the 17 little-endian
Xbox present-parameter words needed to recreate host surfaces. Ordinary
machine saves write D3D section v1. Warm restores accept both versions. The
first displayed frame after a seek can lack the previous frame's pixels.

The disk delta records removed paths and new directory/file records relative
to `DATA`. Changed files carry their contents and SHA-256, with a total
budget of 16 MiB per keyframe. The reader materialises the result directly
into a fresh directory. Machine sections carry their own build, version and
integrity checks, including a digest of the restored guest RAM.

The recorder indexes `disk/` and `metadata/` while it captures `DATA`.
At a checkpoint it drains pending VFS metadata and checks filesystem
identities outside the film session lock. On Unix it reuses directory
listings and file contents while device, inode, length, modification time
and status-change time agree. This also detects host-side save replacement
with a preserved modification time. On Windows, it opens each path for
attributes with read, write and delete sharing. It compares the volume
serial number, 128-bit file ID and NT change time from that handle, along
with the length and last-write time. It rejects reparse points. A stable
file therefore costs metadata queries per checkpoint and zero content
reads. On either platform it re-enumerates changed directories and hashes
changed files. Volumes which omit the Windows identity or change time,
and platforms without an equivalent identity, fall back to reading contents. A failed capture leaves the last good index available
for the next attempt.

`TIDX` is `{blocks, keyframes}`. Each array contains
`{tick, count, offset, sha256}`. The count is zero for a keyframe.
The SHA-256 covers the uncompressed chunk payload. The reader checks that
every input and keyframe chunk has a directory row, that all input ticks
are consecutive, and that the final tick agrees with the header.

`film::Library::list()` returns lightweight entries with `version`,
legacy `meta` and an optional `summary` (versions 2 and 3). For a single file,
`film::blam::summary(&mut reader)` reads only the summary payload once the
directory is open. For simulation, use `film::blam::TrackReader::open`,
then `seek(target)` and `step()`. Seek returns the nearest snapshot at or
before the target and positions the reader at its successor tick. If there
is no eligible keyframe it returns `None` and positions at tick 1. Replay
the intervening ticks to reach the requested state. A blocked snapshot
is omitted; the ordinary input log continues.

### Keyframe RAM budget

A v3 anchor is a complete machine snapshot. Each of the next seven successful
keyframes can encode its 64 MiB RAM against that same anchor. The recorder
starts a new group when it captures the loaded match after a lobby anchor.
Private `TIDX.keyframes[].ram_anchor` gives the preceding anchor tick. The
target H2SNAP RAM section names it `film-{tick}`. A cold seek reads at most
two private keyframes. It decodes the anchor's RAM into scratch space, then
restores the target's RAM, devices, kernel and guest disk. A delta referencing
another delta, another build or a different named anchor fails verification.
The boot reader also checks the CRC of every inherited non-zero page and the
target's whole-RAM hash.

There are 16,384 physical pages of 4,096 bytes. The inner RAM codec uses one
kind byte per page, four CRC bytes per non-zero page and the zstd-compressed
contents of changed non-zero pages in groups of up to 256. A delta with zero
changed pages therefore costs 16–80 KiB of page tables before the outer
compression, plus its name and fixed fields. Device sections and guest disk
edits add to this cost. The film recorder holds one 64 MiB anchor in host RAM.

For a full four-minute group at the default 30-second cadence, the exact
keyframe contribution per player-minute is `(A + D1 + … + D7) / (4 × P)`,
where the sizes are stored KEYF bytes (including each eight-byte prefix)
and `P` is the mean number of players. A sizing target of 16 MiB for an
anchor and 1 MiB per delta gives 376,832 bytes per player-minute at 16
players. These are planning figures. High page churn, GPU state and disk
changes raise the total. The public viewer transfers zero KEYF bytes.

## Determinism and verification

Match the XBE SHA-256, guest execution contract, simulation ABI, effective
features, boot parameters, map content and exact variant bytes. A converted
Halo 1 or mod map with the same display name can have different bytes.
Guest storage, RNG, logical clock, scheduling budget, controller topology,
and the ordering of HLE and lobby calls all affect replay. A film carries
the host inputs; their apparent effects in telemetry are insufficient to
reconstruct the simulation.

Current writers put `h2-sim/1` in `HEAD.runtime_stamp`. This ABI covers
the host scheduler, consumed input and guest-facing HLE semantics. A change
that breaks those contracts must bump the ABI. The sealed header also stores
`simulation_hooks`, a SHA-256 of the installed simulation hook chains in
execution order and the core game, clock, player and object table addresses.
It includes hook owners, guest addresses, calling conventions, hook kinds,
instruction sites, permitted continuation addresses, guest-callable entries
and the allocations placed for those features.
The player checks it after installing its features and before entering the
guest. The guest image has its own stamp. Editing Live, the web viewer,
codecs, diagnostic labels or documentation leaves this contract intact.
Earlier experimental v2/v3 recordings with source-based runtime stamps
require their corresponding simulator for engine playback.

`HEAD.telemetry_hooks` selects the observer chains on the guest dispatch
table. Its `observers` field is `events_and_impacts_v1` for new recordings.
The `weapon_fire`, `damage_report` and `object_delete` booleans identify the
optional hook targets present in the recording image. Both machines install
this plan, including when a full playback image provides more functions than
the recording host. Playback requires all selected targets and discards the
observer output. `events_v1` selects the original telemetry hooks and `off`
installs none. Older headers use their `telemetry` presence bit and format
version to select the historical plan.

`HEAD.render_hooks` identifies the guest render feature groups compiled into
the recording executable. `slim` installs the common hooks, `desktop` adds
the render loop and `gpu` also installs the native renderer features. The
recorded feature switches still control which members of each group run.
A desktop can replay a slim host with exactly its guest hook groups while
the independent spectator window uses the desktop GPU. A player missing a
required group refuses playback. Older headers use the player's compiled
groups and retain their original compatibility checks.

Version 3 retains the original native image stamp and additionally records
`HEAD.guest_image` with `version: 1`, the native target triple and an optional
`contract_sha256`. The digest covers the compact UTF-8 JSON array
`["h2film guest image v1 clang", xbe_sha256, guest_stamp_prefix, flags]`.
The prefix is the native stamp up to (and excluding) ` flags `. It identifies
the XBE, generator sources, IR passes, code-generation settings and guest ABI.
The flags preserve their original order and include optimisation, hot/cold
tiers, native x87/SSE paths and floating-point contraction policy. Two ELF
linkage flags are discarded: `-fPIC` and `-fvisibility=hidden`. The native
target and optional slim coverage suffix are checked separately.

An exact native stamp and target permit a machine-keyframe seek. For macOS
and x86-64 Linux, an installed current Clang image with the same guest digest
can replay across native targets. A full image contains every generated
owner and may substitute for the host's coverage-pruned image. A slim
candidate requires the same coverage fingerprint. Cross-target and
slim-to-full playback initialise the disk and machine at boot and check the
recorded inputs and hashes up to the requested tick. A fast-tier or external
host module needs its native image. Earlier files without a portable stamp
keep their original image check.

Post-tick digests cover gameplay observed by the sim harness, including
RNG and player state. The wider arena check covers hidden state. Playback
reports the first *observed* divergence and its tick. A missing digest is
unknown evidence for that tick. The shared RNG can advance during rendering.
Use the recording's render cadence when checking equivalence, particularly
`--render-every 0` for a light host. Mid-match restore, cold replay and
rendered replay need independent comparisons on the same film.

A kernel continuation, pending device work or host feature without
serialisable state can prevent a keyframe. The reader can only seek using
the snapshots the recording actually contains. [Machine snapshot policies](../runtime/snapshots.md)
and [sim determinism](../runtime/sim.md#8-determinism) describe these constraints.

Live clip endpoints use the guest match clock. The in-game player translates
the half-open interval to film ticks using the clock spans in `CIDX`. During
a span without a recorded guest clock, the player aligns the film’s match
boundary with the ledger’s start tick. Local clip markers already use film
ticks. For version 1 and 2 host films the selector retains the original
assumption that the saved clip endpoints refer directly to film ticks.

## Choosing a player's view

In local playback the guest renders its original camera, first-person weapon
and HUD. A v3 local film can display one split-screen viewport at its
original aspect ratio. The slot is the player's seat in the film roster.

```sh
h2 agent theater.camera '{"mode":"first_person","seat":0}'
```

The selection takes effect at presentation and survives a seek. A local
seat is available while the game draws its first-person director. During
a cinematic, death or vehicle camera the window shows the recorded frame.
`theater.camera {"mode":"default"}` returns to the whole local frame.

A playerless host has no local director, first-person weapon or HUD. For
a rendered host film the desktop player opens a separate 960 × 540 GPU
view of the running simulation. It uses the selected unit’s posed camera
marker, aim and unzoomed field of view. When the marker is unavailable,
it estimates eye height from the biped’s collision capsule and crouch state.
The scene uses the active BSP with its base textures and lightmaps, and
the posed opaque models of the visible objects. First-person mode hides
the selected unit and its held weapons. The independent view omits retail
HUD and weapon overlays, transparent effects, sky and post-processing.

Rendered host playback starts with a free camera at the first available
player. Select `first-person` with a roster name or an absolute seat to
follow a particular player's eye. The spelling `first_person` also works.
The `default` host view follows the first available eye. A targeted
first-person view clears during death or disconnect and resumes when the
seat has a camera again. Follow, orbit and free modes keep their last pose
when a subject disappears. The player carries the rig, target, clip marks
and discovered kill markers through a seek.

```sh
h2 agent theater.camera '{"mode":"orbit","seat":3,"look":[0.25,0],"zoom":-1}'
```

The dedicated guest retains its recorded world-render cadence. Changing
POV or display cadence therefore does not add render calls to its RNG or
scheduler timeline. Compare hashes for a headless replay and a visible
replay of the same film when verifying this path.

## Missing observations

The front summary's `dropped_samples` maps each affected source to its count.
A failed guest read can leave a gap in a pose, pickup or damage event. The
recorder retains each participant's last readable score and team. When the
engine signals game over with an unreadable final scoreboard, the event keeps
the finish time and leaves its winner and final rows unavailable.

Within each completed tick the writer allows up to 512 KiB of canonical Live
events and 512 KiB of highlights, including framing. The same budgets apply
to the final batch after the last tick. Each event also has a 128 KiB limit.
An oversized event increments `stats_encoding` or `highlight_encoding`.
Overflowing either batch increments `stats_budget` or `highlight_budget` for
each omitted record. The machine tape continues to record simulation inputs
and post-tick hashes.

## Weapon fire, damage and projectile endpoints

Each observed retail barrel call produces one `WeaponFire` record. It carries the
specific weapon datum and definition, barrel index, holder and player identity
when present. The shooter's origin and aim precede spread and aim assistance.
A held weapon can fire between two pose samples, including the second weapon
of a dual-wielding player.

A `Damage` event carries the established Live hit for stats and the target
position. The accompanying `Impact` carries the guest damage report's origin,
direction, distance, victim, source object, packed reporting code and damage
effect definition. Area damage can name the same blast origin for several
victims. A viewer can use the origin to place one explosion.

A `ProjectileEnded` event captures the last transform position and velocity
at object deletion. This includes bullets that spawn and disappear between
sampled frames. Coordinates are relative to the parent when a projectile is
attached. An endpoint alone does not identify a wall hit, its surface normal
or material. The engine also deletes projectiles at maximum range and during
clean-up. These films preserve that distinction by leaving the cause unknown.
Particle, flash, decal and explosion instances have no object-pose records.

## Objective and power-up events

An `ObjectiveState` event carries an `action` and the fields for that action.
`item` gives the registered flag, bomb or ball, its salted object datum,
designator, current carrying unit and player slot. The designator identifies
the owning team (8 for neutral) or the ball number in Oddball. The change
is `registered`, `carrier_changed` or `unregistered`. A carrier change with
an empty unit records a drop. Object salts distinguish respawns that reuse a
slot. At unregistration the event retains the last observed carrier.

`bomb_state` gives the arming, armed and planted flags for the bomb's team.
`hill` gives the active hill index and 16-bit scoring-player mask. Its
geometry lives in the pose record. `territory` gives a site's owner, team
and occupants, and `juggernauts` carries the current role mask. Each bit
corresponds to a roster slot. These are completed-tick observations.
A mod using a retail game engine inherits that engine's observations.
A custom scoring mechanism needs its own observer to supply named actions.
Score changes still come from the retail scoreboard.

`PowerupTaken` records the salted equipment, its definition tag, the
recipient unit and its player slot when present. The host observes the
successful application before the engine deletes the equipment, including
for equipment outside a scenario placement. [Retail objective and pickup
layouts](bots.md#37-objectives-traced)

## Tag 3 presentation records

The sealed summary's optional `dropped_samples` object counts rejected
observations by source. An unreadable object header table holds the whole
scene until the next census. When the table identifies a live object but its
body, transform or animation is invalid, the viewer holds that object's
previous pose. Other objects still move, appear and disappear. An invalid hill
sample holds the last polygon. A failed objective-registry read retains the
last object roles. Supplemental player and actor rows recover independently.
The recorder continues writing simulation inputs and post-tick hashes.

The enclosing `TICK` supplies the time. A client seeks via the `scene` tick in
`CIDX` and applies pose records from that checkpoint in file order. The host
starts a new scene at a chunk boundary after about 1,800 ticks (one minute at
30 Hz), or earlier to keep each seek within 16 chunks. The indexed window is
shorter than 2,048 ticks. A checkpoint carries the last observed
scene before that tick's changes. An unchanged scene takes no further pose
records until the next checkpoint.

The first payload byte selects one of three layouts. Counts, lengths, object
operation headers and lane masks use canonical unsigned LEB128. Signed
integers use ZigZag then unsigned LEB128. A reader rejects trailing bytes.

| Byte | Remaining payload |
| ---: | --- |
| 0 | Complete object list and hill in the edit format below. Then a packed-frame byte length and that many bytes, zero for an absent supplement. A present supplement is a Live v8 key, stream 0, sequence 0. Replace the scene and restart the packed-frame predictor. |
| 1 | Object edits and hill edit. Apply to the current scene. |
| 2 | One Live v8 key or delta for stream 0 occupying the rest of the payload. It updates the browser's supplemental player/actor fields. |

The object list begins with the operation count. Within a list, datums
increase. Each operation begins with `(datum − previous_datum) × 4 + opcode`,
starting with previous datum zero. These are 32-bit **salted** handles. A
reused slot gets a new identity. Opcodes 0 (create) and 1 (update) carry a
changed-lane mask. Opcode 2 (destroy) ends at the operation header. A full
list contains creations only. The engine object pool has 2,048 slots, so a
delta can contain up to 4,096 deletions and creations combined.

For each set mask bit 0–23 in ascending order, add the signed lane delta
to its previous value. A new object starts with zeroes except for lane 11,
which starts at 16,384 (world up). Optional integer handles and indices use
zero for absent and the underlying value plus one for present as indicated
in the table. Mask bits 24 and 25 are reserved. Bit 26 replaces the tag name
with a byte length and UTF-8 bytes, up to 256 bytes. Zero length clears it.
Higher bits are reserved.

| Lane | Value |
| --- | --- |
| 0 | Guest object class, in order from 0: `bipd vehi weap eqip garb proj scen mach ctrl lifi ssce bloc crea`. |
| 1–4 | Definition datum + 1, scenario placement + 1, multiplayer equipment spawn + 1, parent datum + 1. |
| 5–7 | Signed 32-bit X, Y, Z in centimetres. One guest world unit is 304.8 cm. Attached objects retain parent-local transforms. Largest rounding error is 0.5 cm per axis. |
| 8–11 | Forward yaw and pitch, then up yaw and pitch. One turn is 65,536 units. Yaw deltas wrap through signed 16 bits and decode modulo 65,536. Pitch is signed, −16,384 to +16,384. Each angle rounds to within half a unit. |
| 12–15 | Vehicle seat + 1, aim-present flag, aim yaw and pitch in the same units. |
| 16–19 | Animation graph datum + 1, signed inherited graph and animation indices, signed animation frame in sixteenths. An absent graph clears all three following lanes. |
| 20–23 | Normalised device travel rounded to 0–255 then plus one, depleted flag, controlling player datum + 1, objective kind (0 ordinary, 1 flag, 3 ball, 9 bomb). |

After the object operations, a hill byte of 0 leaves the current hill alone,
1 clears it and 2 sets it. A set carries one byte of hill index, a corner
count (at most 256), and three signed absolute centimetre coordinates for
each corner in boundary order. On a baseline the initial hill is empty.

A supplemental frame supplies the BSP, cinematic and checkpoint state,
animation names, weapon and stance, vitality, velocity and observed actions.
The dictionary supplies matched objects' transforms and animation time. A
player whose unit has gone can retain the last known body location. Seeking
starts with persistent supplemental state, with transient actions cleared.
The browser reads weapon fire and damage from the event records to draw hit
and firing effects. A slow projectile (under 100 world units per second)
or attached projectile gets a pose immediately. A faster projectile gets
a pose once its salted handle survives to the next presentation sample.
Particles, decals, muzzle flash and hitscan traces have no pose objects.
[Live packed frames](../live/live-telemetry.md#packed-frames)

## Size targets

These are engineering budgets, in bytes per player-minute, for a 30 Hz match
with 15 Hz presentation sampling. They are estimates pending measurements on
sealed matches. Event rate, active AI, object churn and the chosen keyframe
cadence all affect size. Fixed and match-wide costs are shown separately so
that an eight-player and a one-player game can be compared.

| Contents | Budget per player-minute | Basis |
| --- | ---: | --- |
| Input | 2,000–18,000 | Held commands need no new input payload. The upper target allows frequent stick and aim changes. |
| Event | 1,000–8,000 compressed | Roughly 60–250 fire, hit, scoring and lifecycle records. Event type and field names repeat within a zstd frame. |
| Pose, player | 9,000–21,000 before zstd | Up to 900 moving samples, including animation and aim. Shared objects are accounted for below. |
| Hash | 19,800 ÷ players before zstd | Eight digest bytes and typically three framing bytes on up to 1,800 ticks. Hashes are match-wide. |
| Marker | <100 ÷ players at one short chapter per minute | Cost is proportional to the UTF-8 label and its framing. |
| Clock | About 75–120 ÷ players | One anchor per chunk during continuous 30 Hz play, plus anchors at clock discontinuities. |
| Front and directories | Fixed 131,104 bytes per film for the front, plus index rows per chunk/keyframe | Divide the fixed cost by match minutes and players. A menu client can request just the prefix and occupied JSON bytes. |

For the world pose dictionary, these budgets are **per object**, before zstd,
excluding infrequent full seek baselines. A resting object generates zero
movement updates. Creation carries identity, class, initial transform and
observed state. Destruction costs roughly 1–5 bytes for the identifier plus
shared operation framing. A respawn gets its own salted object identity.

| Object-table class | Typical changed sample | Example marginal budget per object-minute |
| --- | ---: | ---: |
| 0 biped (`bipd`), including aim and animation | 12–24 bytes | 10,800–21,600 at 15 Hz continuously moving |
| 1 vehicle (`vehi`), including orientation and available turret aim | 10–22 bytes | 4,500–9,900 moving half the minute, plus 3–10 per occupant attachment change |
| 2 weapon (`weap`), including registered flag, ball or bomb | 5–10 bytes for translation, 3–10 for an attachment change | 75–150 for one second of free movement plus each pickup/drop |
| 3 equipment (`eqip`), including powerups | 5–10 bytes for translation | 75–150 for one second of free movement, plus creation and destruction |
| 4 garbage (`garb`) | 5–10 bytes for translation | 75–150 for one second of movement |
| 5 projectile (`proj`), including grenades | 5–10 bytes in flight | 75–150 per second of flight, plus creation and destruction |
| 6 scenery (`scen`) | 5–10 bytes for translation, 10–22 with rotation | 375–1,650 for five moving seconds |
| 7 machine (`mach`), including doors, lifts and gates | 5–9 bytes for travel alone | 375–675 for five seconds of travel |
| 8 control (`ctrl`) | 5–9 bytes for a device-state change | 10–18 for two state transitions |
| 9 light fixture (`lifi`) | 5–9 bytes for a device-state change | 10–18 for two state transitions |
| 10 sound scenery (`ssce`) | 5–10 bytes for translation | Zero while stationary, 75–150 for one moving second |
| 11 crate (`bloc`), including movable or destructible physics props | 5–10 bytes for translation, 10–22 with rotation | 375–1,650 for five moving seconds |
| 12 creature (`crea`) | 5–10 bytes for translation plus changed orientation/animation | 4,500–9,900 at 15 Hz moving half the minute with rotation |
| Active hill (scenario state) | Index, count and three signed coordinates per vertex | About 28–40 for a four-corner hill changing once, at two to three bytes per coordinate |
| Decals, particles, muzzle flash and explosions | 0 pose bytes | Viewer effects use the timestamped event records |

The presentation rows share chunk framing, seek baselines and compression.
They do not include machine state, the guest disk or call results. Those
costs belong to the recording machine, independent of player count. See
the snapshot size discussion for the RAM component. [Snapshot format](../runtime/snapshots.md)

## Live storage

Dedicated hosts send a sealed file under the canonical lease game ID through
`POST /v1/games/{game}/film`. Recent and saved film listings expose
`has_host_film` and the summary after publication. Live derives the existing
player and game statistics from canonical event records. If an idle timeout or
another recorded event disagrees with the existing signed stats, Live keeps
the film, publishes its summary and identifies the first disagreement in
`stats_discrepancy`. The existing ledger remains the source of ranking awards.
Older games may
have only a telemetry archive, and older host films may lack a summary.
A legacy telemetry recording remains readable during migration.
After a failed upload of a default recording, the supervisor keeps the
sealed file in its `pending-films` cache or prints the original lease path
if recovery storage is unavailable. Explicit output files remain at the
requested path. [Live film API](../live/live-api.md)
