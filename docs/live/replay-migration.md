# Migrating finished game recordings

`h2 live frames migrate` scans the admin recording inventory in pages of 32. Each step reads a bounded segment from a finished game, decodes its original records, and verifies that the archive reproduces every decoded frame. Sources include v1–v8 packed streams, H2CK chunks, and signed GameFrame rows containing JSON fields or base64 keyframes. A game can have several source types. Historical coordinates and directions retain the exact values their original decoder produced. The published index records machine travel seeds per segment for accurate seeks on sparse frames. [Archive API](live-api.md#finished-replay-archives)

The local server and Cloudflare Worker expose the same admin operations. Use an installed CLI and server/Worker release with replay archives. The local commands below sign with the local server operator key. For a different server directory, pass `--data DIR`. For production set `H2_LIVE_ADMIN` to the gamertag of a locally held admin account registered on that server.

## Measure the inventory

Run the local dry run first.

```sh
h2 live frames migrate --dry-run --server http://127.0.0.1:8074
```

Then measure the production store. The expected object counts and sizes for that deployment come from this command. It prints each game's decoded frame count and source, compressed segment and immutable index byte totals, followed by counts of original chunk objects, legacy frame rows, and output segments. Source bytes count the stored H2CK blobs, decoded base64 record bytes, and serialized JSON frame payloads. Backup copies and the surrounding event-log rows have additional storage overhead. Both dry runs construct and verify in memory. They write a local progress file.

```sh
h2 live frames migrate --dry-run --server https://live.forbes.red --as "${H2_LIVE_ADMIN:?set the local admin gamertag}"
```

## Apply and verify

Apply locally and verify the published archive from storage.

```sh
h2 live frames migrate --apply --server http://127.0.0.1:8074
```

```sh
h2 live frames migrate --verify --server http://127.0.0.1:8074
```

After reviewing the production dry run, the operator runs the same two operations on production.

```sh
h2 live frames migrate --apply --server https://live.forbes.red --as "${H2_LIVE_ADMIN:?set the local admin gamertag}"
```

```sh
h2 live frames migrate --verify --server https://live.forbes.red --as "${H2_LIVE_ADMIN:?set the local admin gamertag}"
```

Apply skips live, empty and already-published games. Every step pins the original log revision. A late chunk or other change to the source invalidates that revision. The source and stored output must pass verification before the signed index can be published. Viewers continue to read the source until publication. Verify reads the published segments, index nodes and backups, checks their hashes and compares against the retained source. It also counts finished recordings that still need an archive. Check both the failure count and this unpublished count before retiring runtime support for older clients.

## Resume and recover

The CLI issues one request at a time, at least 250 ms apart. Increase the spacing with `--delay-ms` if the service rate-limits requests. It prints the checkpoint path at startup. Each server and mode has its own file in the live data directory under `migration/`. Use `--checkpoint FILE` for an explicit location. After an interruption, repeat the same command. It resumes from the last durable inventory page and per-game continuation. The server checks apply retries against durable progress. Counts are cumulative per game. Repeating a completed scan starts a fresh inventory.

A failure prints the game ID and reason. Per-game failures remain in the checkpoint for retry while other games can proceed. An unavailable service or rejected credential stops the run with its checkpoint intact. If a failed game's source revision changed, a new checkpoint starts an inventory at the current revisions. Review a fresh dry run before applying the changed recording.

## Retention and rollback

Keep original frame objects, signed recording events, archive indexes and backups throughout this release. Private blob storage keeps copies of original chunk bytes and signed index/frame rows under `backup/frames-pre-v7/`. Each backup index names its predecessor. The native inline blob store addresses equivalent objects by SHA-256. The original event rows and chunks remain available to serve rollback and to repeat verification.

Publication records both the archive index SHA-256 and backup index SHA-256. The CLI prints both in the apply and verify receipts. To recover the digests for an automatic job or a missing receipt, read the signed-in admin status.

```sh
h2 live frames status "${GAME_ID:?set the game ID}" --server https://live.forbes.red --as "${H2_LIVE_ADMIN:?set the local admin gamertag}"
```

Rollback checks the two digests against the current game pointer and records the switch back to the retained source. Set the variables below from the receipt or the status reply for that game.

```sh
h2 live frames rollback "${GAME_ID:?set the game ID}" --archive-sha256 "${ARCHIVE_SHA256:?set the archive digest}" --backup-sha256 "${BACKUP_SHA256:?set the backup digest}" --server https://live.forbes.red --as "${H2_LIVE_ADMIN:?set the local admin gamertag}"
```

For local rollback use the local server URL and the operator's `--data` directory. Keep the backup prefix available after rollback. Runtime decoder retirement and backup lifecycle changes are separate rollout steps gated on the operator's production verification. At that gate, the planned ingest policy is to reject v1–v6 with an upgrade-required message and continue accepting v7 and negotiated v8.
