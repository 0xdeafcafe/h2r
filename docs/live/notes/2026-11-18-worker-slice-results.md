# Live Worker: slice measurements

Evidence for [live-worker-slices.md](../live-worker-slices.md). Every number is
from local `wrangler dev` (workerd on the development Mac), not Cloudflare's
edge. Nothing here states what the code does today; the design pages do.

Reproduce with `crates/live/live-worker/smoke/run.sh` and the env vars named
under each slice.

## Replay archive smoke (f99b31357, 2026-11-19)

The local workerd smoke used Worker build `3719470d` and harness `b4b49436`.
It ingested frames at ticks 30 and 960 to force two independent segments.
A WebSocket reader observed publication. The harness fetched each segment
over HTTP with Brotli content encoding and checked every decoded field against
the original H2CK response. Admin verification checked the retained originals
and both index hashes. Repeating the final verification cursor returned the
same receipt.

| Operation on the two-frame recording | Local elapsed time |
|---|---:|
| Signed game close, HTTP round trip | 16.5 ms |
| Start of close to publication observed on the stream | 622.7 ms |
| HTTP fetch and decode of each archived segment | 13.8 ms each |
| Verify both segments and retry the final cursor | 106.8 ms |

These are wall times from a single local run with Durable Object storage and
emulated R2. The full-match size and browser CPU measurements use the seeded
multiplayer and campaign recordings. [Telemetry measurements](notes/2026-11-19-telemetry-precision.md#finished-replay-archives)

## Slice 2 (commit 05805c760, 2026-11-18)

These numbers come from `smoke/run.sh 1000` under local `wrangler dev`
(workerd on this Mac), so they are not timings at Cloudflare's edge.

| What | Result |
|---|---|
| Cold start, empty log | 40 ms to the first answer; the hub's start took 4 ms |
| Cold start, 2,007 events | 84 ms to the first answer; replaying the log took 49 ms, about 24 µs an event |
| Anonymous `/v1/snapshot` | 7 ms cold, 5 ms warm (p50) on an empty log; 14 ms warm with 2,007 events (283 KB) |
| Sign-up, two accounts | 32 ms |
| Sign-in (a token-signed read) | 6 ms, `private, no-store` |
| Event append | 15 ms round trip; in the hub 5 ms p50, 7 ms p90 over 2,001 appends |

The cache headers are slice 1's:
- anonymous reads: `public, s-maxage=5` with an ETag;
- signed reads: `private, no-store`.

**Replay at today's log size.** `live.forbes.red`'s log is at position 6,196.
At this rate a full replay takes about 150 ms. It costs more if the log is
heavy with GameFrames, which are larger to decode. Read-model snapshots
(slice 6) can wait until the log is far larger.

The hub reports its own start on the request that caused it, in a
`server-timing: start;dur=<ms>, events;desc=<head>` header.

## Slice 3 (commit bcd0ff8ff, 2026-11-18)

These come from `smoke/run.sh 200` under local `wrangler dev`, as in slice 2.

| What | Result |
|---|---|
| Open `/v1/stream` and get `caught_up` | 24 ms |
| Append to the event reaching the subscribed socket | 23 ms, the same as the append's own round trip |
| Replay from a position (one event, then `caught_up`) | 8 ms |
| `?auth=1` with a fresh read token | let in; a bad token closes with 4401 |

Read tokens are single-use, so a reader needs a fresh one for each socket.

## Slice 4 (commit 1baa9b57a, 2026-11-18)

These come from `WAKE=15 smoke/run.sh 20` under local `wrangler dev`, with
R2 simulated locally by miniflare.

| What | Result |
|---|---|
| Hibernation wake | With a stream socket open and 15 s idle, the next request found a new hub (`start;dur=7`), and the socket got the next event, its session rebuilt from the attachment |
| Ingest | A game started, a frame sent on `/v1/ingest`, the frame reached a JSON reader and a packed reader (the packed tick on demand): 79 ms in all |
| Content upload, 9 MiB | 636 ms streamed into a two-part upload; read back whole in 541 ms; a body that does not hash to its path is refused and not stored |
| Cloud save, 300 KiB | 62 ms to write and read back |
| Title file, 2 MiB | 243 ms to publish and read back (too big for a SQLite row) |

`run.sh` makes a fresh admin gamertag each run (`H2_LIVE_ADMINS`); the
smoke test uses the local dev operator key to register it and publish the title file as that account.

## Slice 5 (commit 725ba630d, 2026-11-18)

Under local `wrangler dev`, as before.

**Attachments on a busy stream** (`BUSY=100`: 100 sockets follow the log
while 60 events are appended; one run each, so the tails are noisy):

| Attachment written | Append p50 | Append p90 | Append max |
|---|---|---|---|
| Per event (slice 4) | 21.3 ms | 35.2 ms | 210 ms |
| On a change of state only | 18.4 ms | 23.4 ms | 32.1 ms |
| The same, in the full run below | 13.2 ms | 34.0 ms | 121 ms |

Every socket had each event within 0.3 ms of the append's answer.

**The co-op relay** (`WAKE=15 BUSY=100 smoke/run.sh 20`: two members of
a real co-op session, through the `CoopRelay` object):

| What | Result |
|---|---|
| Both members in and told of each other | 296 ms (two tickets from the hub) |
| A stranger | closed with 4403 |
| Round trip, 300 datagrams | p50 6.0 ms, p99 11.3 ms |
| 30 a second each way for 17 s | 1,020 of 1,020 delivered; each member's ticket renewed once (`relay: renewed` in the log) |
| 200 a second each way for 5 s | 2,000 of 2,000 delivered |

The relay's own ceiling was not reached: 200 a second each way is the
sender's pacing, under `relay_rate` (240 a second a socket).
