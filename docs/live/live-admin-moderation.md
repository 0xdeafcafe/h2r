# Live admin: moderation

Player reports; 9. Bans.

## 7. Custom content: moderation

Players share custom and modded maps through the server's content store
([maps.md](../game/maps.md) §5, [live-api.md](live-api.md) "Custom maps"). Admins
(and the operator) moderate it with tokens for `lxm` `content.moderate`:

| endpoint | does |
|---|---|
| `GET /v1/admin/content` | `{packs, maps, blobs, reports, uploads_off, position}`: every pack (including members, state and count of pack and member reports), every package (its summary plus `state` and `reports`), every stored blob (`sha256, size, uploader, state, reports, stored_at`), the reports newest first (`target, id, by, reason, at`), and the gamertags whose uploads are off |
| `POST /v1/admin/mod-packs/{id}/{action}` | applies one action to a pack and all of its members |
| `POST /v1/admin/maps/{map_id}/{action}` | acts on a package |
| `POST /v1/admin/content/{sha256}/{action}` | acts on a blob, by hash |
| `PUT /v1/admin/uploads/{gamertag}` `{allowed}` | switches an account's uploads and publishes off or on |
| `PUT /v1/admin/model-uploads/{gamertag}` `{approved, note?}` | the account's `model_uploads` flag: a package with a `model` or `animation` item is published only by an approved account whose uploads are on (`ModelUploadsSet` on the account's stream) |
| `PUT /v1/packages/{id}/credit` `{authors?, release?, released?, source_url?, notes?}` | who made a package, over the publisher's and the manifest's credit; `{}` clears it (`PackageCredited`) |
| `PUT /v1/hoppers/{hopper_id}/pins` `{map_ids}` | the custom maps a matchmaking hopper allows (empty: retail only) |

The actions, and the `state` they leave:

- `hide` → `hidden`: the bytes or manifest stay, but reads answer 410 Gone,
  lists leave it out, and sessions on it are refused. `unhide` undoes it.
- `delete` → `deleted`: a blob's file is removed (its uploader's quota is
  freed); a package's manifest is dropped. Reads answer 410. A fresh upload
  or publish of the same hash or `MapId` brings it back.
- `block` → `blocked`: deleted, and refused from now on (uploads and
  publishes answer 410). A hash can be blocked before anyone uploads it.
  `unblock` leaves it deleted.

Taking down a pack prevents downloads of its member manifests, images and
exclusive layers. Sessions cannot advertise any of those members. A layer
shared with a visible mod stays available to that mod.

The admin can still
inspect the pack and its reports. The public event log also omits the
pack, member and exclusive blob streams. Unhiding the pack restores access to
members that have no separate takedown.

Unblocking leaves the pack deleted
until its uploader republishes it.

A package is refused in sessions (410) when it, or any of its layers, is
hidden, deleted or blocked; a deleted layer alone shows up in the package's
`missing` list. Each action is a server-signed `ContentModerated` event on
the target's stream (`{target, id, action, by}`, `by` the admin's xuid),
reports are `ContentReported` (`POST /v1/mod-packs/{id}/report`,
`/v1/maps/{id}/report` or
`/v1/content/{sha256}/report` by any account), the upload switch is
`ContentUploadsSet` on the account's stream, and pins are `HopperPinsSet`
on `hopper-<id>`. All of it replays from the log, so it survives restarts.

### Screenshot review

Open **Screenshot review** at `/dash/admin` to see the held images and why
they entered the queue. **Show image** fetches the private, normalised
preview. **Approve** makes it public across all assigned galleries.
**Remove** hides all copies permanently, frees its quota and attempts to
delete the stored image. The same review page holds approved captures
that a player reports. Existing uploader gallery images enter the queue
on replay if their upload predates proof checks.

Three distinct images flagged by the classifier or removed in review
switch off the uploader's content uploads. Player reports without an
admin removal do not count. Approval clears a classifier strike on that
hash.

An admin can restore uploads through the account's upload switch.

The Worker and the native server share the review queue and rules.
Native Live holds captures for review when a local classifier is absent.
The optional local adapter sends the display WebP in an HTTP POST and
expects a JSON response such as `{"nsfw":0.25}`, with a numeric score
between zero and one.

The operator supplies the model service on loopback.
[Classifier setting](../runtime/switches.md#tools) · [Screenshot API](live-api-content.md#verified-screenshot-uploads)

**Pins.** The game's hopper file has no room for a `MapId`, so pins are kept
beside it, keyed by the `hopper_id` sessions advertise. A session with
`hopper` or `ranked` flags may only advertise a retail map or one pinned on
its hopper. A pin allows a custom map; it does not make a playlist play
it (live-matchmaking.md §6.10).

**The retail list.** `<data>/retail-maps.sha256` (next to the server's key)
holds the hashes of retail maps, in `sha256sum` output form:

```sh
cd /path/to/your/disc/maps && sha256sum *.map > <data>/retail-maps.sha256
```

Blobs with a listed hash are refused, so the store never holds a retail
map. A line that names its map (`<hex>  lockout.map`) also makes
`MapId::retail(name, hash)` a retail id, which ranked sessions may use.
The file is absent, so empty, by default: nothing from a disc ships with
the server.

It is read on each upload, so edits apply at once. Whatever
the list says, a manifest with no layers (only a retail base) is never
published: it would be the retail map itself.

The Worker has no data directory: it reads the R2 object
`retail-maps.sha256` at start (`Live::load_retail`), so a replaced object
applies from the hub's next start. Package locks are checked against the
same list ([live-api.md](live-api-content.md#packages)).

The Halo Admin's **Custom content** tab (`/games/halo2/admin?tab=content`, linked
from the dashboard's "halo admin" pod) lists packs with expandable member
maps, standalone maps and blobs. It shows their states, report counts, available
actions, reports and upload switch; the playlist builder pins published packages per hopper.

## 8. Player reports

Players report each other from the game's player-details screens
(`XOnlineFeedbackSend`, [live-api.md](live-api.md) "Player reports"), and
any account can report another through the API. Only admins and the
operator read the reports: neither the reporter nor the reported player
sees them, in the log or anywhere else. Admins review them with tokens for
`lxm` `reports.moderate`:

| endpoint | does |
|---|---|
| `GET /v1/admin/reports?state=open\|closed\|all` | `{items, players, position}`: the reports newest first, and a record for every reported player in them |
| `POST /v1/admin/reports/{id}/dismiss` `{note?}` | closes a complaint: nothing to act on |
| `POST /v1/admin/reports/{id}/note` `{note}` | closes a complaint with a note kept on the player's record |

`state=open` (the default) lists complaints nobody has reviewed;
`closed` the reviewed ones; `all` everything, the commendations
(`good_attitude`, `good_session`) included, which are never reviewed. An
item is `{id, target, by, kind, negative, reason?, game_id?, at, review?}`:
`id` is the report's log position, `target` and `by` are `{xuid, gamertag}`,
`game_id` the game the reporter was in when the game said, and `review`
`{action, note?, by?, at}` once an admin acted (`by` absent when the operator
did). A player's record is `{complaints, open, commendations, notes}`, over
every report against them, with the admins' notes oldest first.

A note
needs text; either is at most 500 characters. Reviewing a commendation is
400, a report already reviewed 409.

Each report is a server-signed `PlayerReported` event and each review a
`ReportReviewed` event, both on the reported player's `reports-<xuid>`
stream, so the queue replays from the log. Reviewing a report bans no
one; banning is its own action (§9), which can name the report it answers.

Player reports are the service's, not the game's, so they are on the
dashboard's server screens (§5), not the Halo Admin: **player reports** on
the accounts screen opens `/dash/server/reports`, which shows the open
complaints with each player's record, a note field, and Note and Dismiss;
closed and everything list the rest. The queue is one component
(`web/src/dash/Reports.tsx`) over `web/src/admin/reports.ts`, laid out
plainly for now.

## 9. Bans

An admin bans an account for a while or until lifted, often in answer to a
report (§8), with the same `reports.moderate` tokens:

| endpoint | does |
|---|---|
| `POST /v1/admin/bans/{xuid}` `{reason, until?, report?}` | 201 `{ok, position}`: bans the account; a newer ban replaces the one in force |
| `POST /v1/admin/bans/{xuid}/lift` `{note?}` | lifts the ban in force; 409 when there is none |
| `GET /v1/admin/bans?state=active\|all` | `{items, position}`: the bans in force (the default) or every ban, newest first |

`reason` is what the player is told, required, at most 500 characters.
`until` is an RFC 3339 time in the future; without it the ban lasts until
lifted. `report` is the log position of a report against the same player.
Admins can't be banned (403). An item is `{id, target, reason, until?,
report?, by?, at, state, lifted?}`: `state` is `active`, `expired`,
`lifted` or `replaced`, `by` the admin's gamertag (absent for the
operator), and `lifted` `{note?, by?, at}`.

While a ban is in force the server refuses every token the account
presents for a write scope with `banned` (403), saying until when and why:
signing in (`SignedIn`), the presence heartbeat, sessions, games, the co-op
relay, reports, cloud-save writes and the rest all go through the same
check. Reads stay open (`read`, `recovery.read`, `storage.read`,
`log.read`), so the player can read their notice and their saves. An end
date is checked whenever a token is used, so a ban ends by itself with
nothing written.

In the game, the Live HLE turns a `banned` answer into `0x80151200`
(account requires management): at sign-in the game shows its account
dialog (`0x27`, [live-boundary.md](live-boundary.md) §7), and a ban during
a session reaches the connection task on the next heartbeat with the same
code.

Each ban is a server-signed `AccountBanned` event (`{target, reason,
until?, report?, by?}`) and each lift an `AccountUnbanned` event
(`{target, ban, note?, by?}`), on the account's `bans-<xuid>` stream, so
the bans replay from the log. Admins, the operator and the banned player
read them ([live-api.md](live-api.md) "Private events").

On the dashboard, **bans** on the accounts screen opens
`/dash/server/bans` (`web/src/dash/Bans.tsx`): the bans in force with a
note field and Lift on each, and every ban under "every ban". Each open
report in the queue has a Ban button that bans its player for the report,
for a day, a week, 30 days or until lifted, with the reason; an admin
viewing a player's dashboard screen sees their ban with Lift, or Ban. A
banned player sees the notice on their own screen.
