# Live Worker: migration rehearsal

Evidence for [live-worker.md](../live-worker.md). Nothing here states what
the code does today; the design page does.

## 2026-11-18: copy, resume, back out and cold start


The source was a read-only copy of the real log: Postgres to a SQLite file
in scratch, 6264 events (92 MB, the largest JWS 18 KB, well under a
Durable Object row's 2 MB), copied in 22.5 s. The destination was the
Worker under `wrangler dev`, with id `live.forbes.red` and a dev key.

| Step | Result |
|---|---|
| Copy, killed after 1500 events | the hub at 1500, nothing of its own written (held) |
| Run again | carried on at 1501: 4764 events in 4 s; verified at 6264 |
| Public reads against live.forbes.red (also at 6264) | `/v1/accounts`, `/v1/games?state=ended`, `/v1/teams`, `/v1/maps`, `/v1/sessions`: identical. `/v1/stats/summary`: identical except the order of tied map variants. That order was not deterministic; the map id now breaks the tie (`stats.rs`). |
| Back out to a fresh SQLite file | 6264 events in 16 s; every position and hash the same as the source |
| `--release` | the hub writes again |
| Blobs, the Mac's data dir to a directory | 10 files, 4.2 MB; a second run copied nothing |
| Cold start on that log, hub restarted | 1.38 s to replay 6264 events (`server-timing: start;dur=1376`) |

The first attempt, without the hold, failed exactly as described above.
That is why the hold exists.

