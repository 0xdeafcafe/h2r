# Cloud saves

Signed in to Live, a player's saves follow their account: sign in on
another machine and the profiles, campaign progress, saved game variants
and our per-profile settings are there before the game reads them.

- Server: `crates/live-server/src/cloud_saves.rs`, the endpoints in
  [live-api.md](live-api.md) "Cloud saves".
- Client: `crates/cloud-saves` (merge, state, worker), wired into
  `h2 play` by `crates/cli/src/play_cloud.rs`.
- The VFS hook: `vfs::Vfs::set_change_observer`
  ([storage.md](../runtime/storage.md)).

## What is synced

Halo 2 keeps everything a player makes as XAPI save containers on U:
([storage.md](../runtime/storage.md) "XAPI save containers"). On the host,
U: is

```text
<data>/disk/Partition1/UDATA/4D530064/
  TitleMeta.xbx, TitleImage.xbx, SaveImage.xbx   (the title's own, optional)
  <12 hex digits>/                               (one save container)
    SaveMeta.xbx                                 (Name=<save name>, UTF-16LE)
    <the game's payload files>
```

`<data>` is `h2 play --data` (default `$TMPDIR/h2boot-data`). The VFS
keeps its logical metadata (attributes, NT times) in a separate tree,
`<data>/metadata`, so U: holds only the game's bytes. A container's
directory is named by the hash of its save name, so the same save has the
same directory on every machine.

| What | Where | Item kind |
|---|---|---|
| Profiles (settings, controls, emblem, campaign progress) | a container per profile on U: (content type 0, [variants.md](../editor/variants.md)) | `save` |
| Saved game variants | a container per variant on U:, one 0x144-byte payload (0x130 record + 0x14 signature) | `save` |
| Campaign saves (Save and Quit) | containers on U: | `save` |
| U:'s own files | `TitleMeta.xbx` and the images at the top of U: | `save` |
| Our per-profile settings (collected skulls, per-profile choices) | `[profiles."<name>"]` in `<data>/settings.toml` | `settings` |

Not synced: the cache partitions (Z:, N: and X/Y; disposable by design,
including the running checkpoint snapshots, [checkpoints.md](../runtime/checkpoints.md)),
T: (title data and downloaded content, which come from the disc, map
packs and the content store), and our host-wide settings.

A `save` item is one file, named by its path below U: with `/` between
segments: `0123456789AB/SaveMeta.xbx`. A `settings` item is one profile's
values as canonical JSON (`{"skulls.collected":5}`), named by the profile
name's UTF-8 in hex, since profile names hold spaces and quotes. Host files
whose names start with `.` (`.DS_Store`, our temporary files) are skipped,
and so are names an item cannot have (see the API's name rule).

Signatures: saves are signed by the game with `XboxSignatureKey`, which our
kernel leaves as zeros ([variants.md](../editor/variants.md) "The signature"), so a
save made on one machine verifies on any other running this runtime. Saves
from a real console are signed with that console's key and are not made
portable by syncing them.

### Sizes

What is known without running the game: a saved variant's payload is 0x144
bytes and `SaveMeta.xbx` well under a kilobyte; the campaign state the
checkpoint writer handles is 0x3fe000 bytes (about 4 MiB, reserved as
0x40b000, [checkpoints.md](../runtime/checkpoints.md)), the largest a campaign save
can reasonably be. Profiles have not been measured under our runtime yet;
`du -ah <data>/disk/Partition1/UDATA/4D530064` after playing shows them.

The defaults (`StorageLimits`) leave room for all of that: 8 MiB per item,
64 MiB and 2048 items per account. A write over a cap is refused (413) and
named on stderr; it stays on this machine.

## The server

Per account, private and versioned ([live-api.md](live-api.md) "Cloud saves"):

- **Owner only.** Every request needs the owner's own token
  (`storage.read`, `storage.write`) for the xuid in the path: any other
  account, the server's key and anonymous callers are refused, on public
  servers too. Nothing is in the event log, which anyone can read.
- **Versions.** Each write or delete raises the item's version by one. A
  write names the version it replaces in `If-Match` (`0` for a new item);
  any other version is a 409 with the item as it is now. A delete leaves a
  tombstone, so other machines learn of it.
- **Content addressed.** The writer sends the SHA-256 of the body
  (`X-H2-SHA256`) and the server refuses a body that hashes otherwise. Bytes
  are kept by hash as the content store keeps blobs, but under
  `<data>/storage/<xuid>/blobs/`, never the public content directory: knowing
  a hash must not fetch someone's save. An item's index is
  `<data>/storage/<xuid>/index.json`.
- **Limits.** The caps above, plus the per-author write rate every write
  has (10 a second, a burst of 30; 429).

Not encrypted at rest: the server's operator can read the files. Signing
and owner-only access are what keep them the player's.

## The client

### At sign-in

`h2 play` (with a Live flag) signs in before the window opens and the
game starts; the tvOS and iOS apps, on Live by default, do the same with
the account in their data root's `live/accounts.json`. Then
`cloud_saves::session::start` (shared by both) lists the account's items, scans U: and the profile
settings, and merges them with the last agreed state (below). Everything
the merge pulls is written before the game reads U:. Settings values go
through the settings store, the one writer of `settings.toml`.

The last agreed state (each item's version and hash when both sides last
matched) and the upload queue are in
`<data>/cloud-saves/<server id>/<xuid>/state.json`. A damaged state file is
set aside (`state.json.damaged`); the merge then treats every difference as
a conflict and keeps both copies.

### While the game runs

Nothing is written into U: under a running game: a file replaced behind the
VFS would surprise it. The worker only uploads:

- The VFS reports each finished change on U: (`Vfs::set_change_observer`):
  the last close of a file object that created, overwrote, wrote or
  truncated a file (after its cleanup, so the bytes are on the host), and
  each deletion carried out. Cache partitions and T: are ignored.
- Changes are grouped by container and debounced: a container goes up once
  nothing in it changed for 2 s, or after 15 s of changes.
- Only files whose hash differs from the agreed state are uploaded, each
  against its agreed version. A file the game deleted is deleted on the
  server.
- Profile settings are polled in memory twice a second and pushed the same
  way.
- A 409 (another machine changed it meanwhile) is left alone and sorted
  out by the merge at the next sign-in.
- Quitting pushes whatever is still waiting.

### Offline

If the server cannot be reached at sign-in, the game starts with this
machine's saves, and every container that changed since the last sync is
queued. In play, uploads that fail for want of the server are queued too;
the worker tries again after 30 s, doubling to 5 minutes. The queue is kept
in `state.json`, so it survives quitting; the next sign-in's merge covers
it anyway, since it compares every item with the agreed state.

## Conflicts

The rules, as `crates/cloud-saves/src/merge.rs` applies them per item
(base: the last agreed state):

1. **Same bytes on both sides**: nothing moves; that is the new base.
2. **Changed on one side only** since the base: that change wins: a
   download, an upload, or the deletion carried over.
3. **Deleted on one side, changed on the other**: the change wins. A
   deletion never removes a change made elsewhere.
4. **Changed on both, to different bytes**: the newer change wins, by the
   writer's own clock (the file's modification time here, which a download
   preserves, and the `modified` the other machine uploaded with). A tie goes
   to the server. The losing bytes are kept as a conflict copy.
5. **A save container is decided as a whole**: when any file of a
   container conflicts, the winner of that container (its newest change on
   either side) takes every file of it that differs, and every losing file
   that had changed is kept as a conflict copy. A profile's files stay a
   set the game wrote together.

Two machines that never synced compare as if every item changed on both
(rule 4 for any difference).

Nothing is dropped silently. Conflict copies, and copies of files deleted
here because another machine deleted them, go to
`<data>/cloud-saves/<server id>/<xuid>/conflicts/<UTC time>-<side>/<kind>/<name>`,
with `<side>` `this-machine`, `other-machine` or `deleted-elsewhere`, and
each gets a line on stderr. A conflict copy is never written back into U:
by itself: to bring one back, quit the game and copy it into place.

Settings values cannot be unset, so a profile's values deleted elsewhere
stay here.

## What it prints

Every line starts `cloud saves:`. At sign-in, one line per change, then a
summary:

```text
live: signing in to https://live.forbes.red (the default service; --live-server URL names another)
live: playing as Chief (0009…) on https://live.forbes.red
cloud saves: save/0123456789AB/SaveMeta.xbx: downloaded (changed on another machine)
cloud saves: save/0123456789AB/profile: CONFLICT: changed on both, the other machine's is newer; this machine's copy kept in …/conflicts/20261116T120005Z-this-machine/save/0123456789AB/profile
cloud saves: 2 downloaded, 1 conflict copies kept for Chief on https://live.forbes.red (state in …)
```

or `cloud saves: up to date for …`, or, with the server down, `cloud
saves: not synced: offline (…)` and a summary ending `the server cannot be
reached, N waiting to upload`. In play: `cloud saves: save/…: uploaded
(changed here)`, `… deleted in the cloud (deleted here)`, or `the server
cannot be reached; N waiting, trying again in 30s`. A server without cloud
saves: `cloud saves: <url> keeps none; saves stay on this machine`.

`H2_CLOUD_SAVES=0` turns it off (`cloud saves: off (H2_CLOUD_SAVES)`).

## Tests

- `crates/live-server/tests/cloud_saves.rs`: owner only (other accounts,
  the server key, no token, the wrong method), versions and 409s,
  tombstones, hash and header checks, the caps, no data directory.
- `crates/cloud-saves/src/merge.rs`: the rules above over invented trees.
- `crates/cloud-saves/src/engine/tests.rs`: two machines through a fake
  server (pulls, pushes, conflicts with copies, deletions, the offline
  queue across a restart, deferred 409s in play, settings), and the worker
  against a real VFS over a temporary data root.
- `crates/cloud-saves/tests/http.rs`: `HttpRemote` and the engine against
  the real server on loopback.
- `crates/vfs/tests/storage.rs` `changes_are_observed_at_close_and_removal`.
- `crates/cli/src/play_cloud.rs`: three data roots on one embedded service.
