# Live: client HLE, player services

Part of [Xbox Live replacement](live.md). Covers: Client HLE: player feedback; Client HLE: mute list; Client HLE: bans; Client HLE: title storage and profile fallback; Client HLE: local stats; Client HLE: outgoing messages; Client HLE: game invitations and joining.

## Client HLE: player feedback

`XOnlineFeedbackSend` (`0x003a0890`, `crates/live/live-hle/src/feedback.rs`)
takes eight stack words: the controller, the target's XUID by value (three
words), the XDK feedback type, an `XONLINE_FEEDBACK_PARAMS` pointer (its
one field is a wide-string pointer; the library sends it as UTF-8 of at
most 255 bytes), the work event and the task output.

The title calls it from one wrapper, `0x0008d3d0`, reached from the
player-details screens at `0x002b89be` and `0x002b8a65`. The wrapper maps
the screen's reason index 0..9 through a table on its stack to the
feedback types 5, 6, 0, 1, 2, 3, 4, 12, 11, 10, and passes a params block
whose string is null, no event, and the task slot in its task list.
Indexes 0 and 1 (types 5 and 6, the public XDK's `POS_ATTITUDE` and
`POS_SESSION`) are sent at once; the rest, complaints, after a
confirmation dialog. Types 0..4 are the XDK's `NEG_NICKNAME`,
`NEG_GAMEPLAY`, `NEG_SCREAMING`, `NEG_HARASSMENT` and `NEG_LEWDNESS`. The
names of 10, 11 and 12 are not confirmed from code, so the server keeps
them by number (`type_10` and so on).

The HLE posts each one as a player report ([live-api.md](live-api.md)
"Player reports") with the game id the console is in, and the task
completes with TASK_SUCCESS when the server answers, or with the mapped
error: INVALID_USER for an unknown player, SERVERS_BUSY when rate limited,
CANNOT_ACCESS_SERVICE otherwise. Like the retail library (`0x003b8cbb`), it
remembers the last 20 players each controller gave feedback on, with a bit
per type below 8, and completes a repeat at once without sending it.
Unlike retail, a send that failed is forgotten, so a retry is sent. Types
the title never sends (7, 8, 9, over 12) are refused with
INVALID_ARGUMENT; with no server the call returns NO_SESSION and makes no
task.

## Client HLE: mute list

The three mute-list calls (`crates/live/live-hle/src/mutes.rs`) keep each
signed-in player's list in the HLE and back it with the server's
([live-api.md](live-api.md) "Mute list"), so a mute follows the player to
other machines and the website.

- `XOnlineMutelistStartup(event, &task)` (`0x003a0841`, from `0x6c1a0`)
  starts loading every signed-in player's list (`GET
  /v1/accounts/by-xuid/{xuid}/mutes`). Its task completes with TASK_SUCCESS
  when all are in, or with the mapped error of one that failed.
- `XOnlineMutelistGet(controller, max, event, &task, buffer, &count)`
  (`0x003a0832`, called with 250 from `0x58b70`) writes up to `max` entries
  of 16 bytes (the XUID, its 4-byte flags, 4 bytes of zero) and the count.
  A loaded list is written during the call and the task completes on its
  first pump; a list still loading is written when it arrives.
- `XOnlineMutelistAdd` and `Remove(controller, xuid lo, xuid hi, flags)`
  (`0x003a084c`, `0x003a086e`) change the cached list during the call and
  return S_OK, then send the change (`PUT` or `DELETE
  /v1/accounts/by-xuid/{xuid}/mutes/{target}`, `mutes.write`). The game
  waits on no task. A change the server did not take is sent again before
  the player's next load, and a list loaded while changes were in flight
  has them applied on top. Adding the 251st player, or the player
  themselves, returns INVALID_ARGUMENT.

The game's matrix builder (`0x541a0`) compares the 12 bytes byte for byte,
so Get returns the flags Add was given for that XUID. A list loaded from the
server has flags 0 for players this machine never muted, which is what
every h2 live account's XUID carries. Signed in without a server, the lists
start empty and stay on the machine until sign-out.

## Client HLE: bans

A banned account's writes are refused with `banned` ([live-admin.md](live-admin.md)
§9). The HLE's HTTP worker reads that error body and maps it to
`0x80151200`, account requires management, wherever it would map a
status: a logon whose `SignedIn` is refused fails with it, both as the
task's result and in every user's per-user HRESULT, which the game turns
into result 2 and its account dialog `0x27`. A ban during a session shows
on the next heartbeat (or presence update): the persistent logon task's
status becomes `0x80151200` instead of CONNECTION_LOST, so
`XOnlineTaskContinue` and `XOnlineLogonTaskGetResults` return it.

## Client HLE: title storage and profile fallback

The signed-in resource manager `0x0008e240`, called each network update by
`0x0008df50`, downloads the MOTD and hopper. The verified chain is
`0x0008e3f5` → `0x000b4450` → `0x000b4120` →
StorageCreateServerPath, then `0x000b4511` → StorageDownloadToMemory.
There is no StorageEnumerate call in the boundary inventory.

At sign-in, `0x0006bff0` arms the MOTD channel `0x00479780` and hopper
channel `0x0047d8f4` in slot table `0x004d8c28`. Requests use facility 4
(per-title), user/team XUID zero, controller index zero, flags zero and no work
event. The filename is the UTF-16 relative path `9809/<filename>`; console
check `0x0006bfa0` selects `test/<filename>` when byte `0x0050944e` is set.

| Formatted guest path | Caller buffer capacity | Server file under `/v1/titles/4D530064/files/` |
|---|---:|---|
| `//title.4d530064/t:4d530064/9809/message_of_the_day.dat` | `0x8000` | `message_of_the_day.dat` |
| `//title.4d530064/t:4d530064/9809/multiplayer_hopper.dat` | `0x20000` | `multiplayer_hopper.dat` |

The same two paths with `test/` in place of `9809/` are accepted. Only
these four complete paths, with facility 4, reach Service::file on the HTTP
worker. The guest call returns a running task; Continue polls the owned reply
and copies a complete file into checked guest memory. Oversize returns
XONLINE_E_OVERFLOW (`0x80150001`) without a partial copy. There is no
`0x499a` attachment limit on these requests: that cap belongs to the
unrelated playback screen.

StorageDownloadToMemoryGetResults has six arguments: task, optional buffer
pointer output, received count, total count, owner PUID and creation FILETIME.
On success it returns S_OK, the original caller buffer, and received == total.
Shared title owner is zero; creation time is zero because the endpoint does not
supply one. While pending it returns TASK_BUSY without publishing outputs.
GetProgress reports 100 percent and the completed byte counts after success.
Close/cleanup invalidate tasks; the worker never accesses guest pointers.
HTTP 404 becomes STORAGE_FILE_NOT_FOUND (`0x8015c004`); transport failures
remain service errors. Every fresh explicit request fetches current bytes.

The game completion chain is `0x0006c4a0` → `0x0008e5e0` → GetResults.
It requires equal received/total counts within the channel cap, then validator
`0x00081230` checks the file header. Completion `0x0008e0f0` →
`0x00081050` sets flags |= 2 and invokes `0x0008c790` (MOTD) or
`0x00194260` (hopper). The header and payload formats are in live-motd.md.

Channels arm on the signed-in transition and user-change completion
(`0x001513f1`), skipping channels that already contain data. Sign-out clears
them. There is no mid-session re-arm for a new publication or inbox event:
publish before sign-in, or sign out and back in to request the new files.
Errors, including FILE_NOT_FOUND and `0x8015c007`, use the existing fallback
without retry. Only received != total re-queues the transfer.

StorageCreateServerPath implements the retail formatter at `0x003c79f2`, without
any network request. It needs XOnlineStartup but not a signed-in user. The three
supported facilities produce these UTF-16 paths, where `title` is lowercase
8-digit hex (`4d530064`) and user/team IDs are lowercase 16-digit hex:

| Facility | Path |
|---|---|
| 3: teams | `//tteam.{title}/u:{team}/{title}/{user}/{filename}` |
| 4: per-title | `//title.{title}/t:{title}/{filename}` |
| 5: per-user/title | `//tuser.{title}/u:{user}/{title}/{filename}` |

The capacity and returned required size count UTF-16 code units including the
terminator, not bytes. NULL output supports a size query. Insufficient capacity
returns `0x8007007a` and updates the required count. If the buffer can hold the
prefix and terminator, retail writes that prefix before checking whether the
filename fits; the HLE preserves this behaviour. Other facilities return
STORAGE_INVALID_FACILITY (`0x8015c009`) without changing outputs. The formatter
does not enforce the public 256-character path limit.

Other paths/facilities and package StorageDownload return tasks completing with
STORAGE_FILE_NOT_FOUND; receive/result buffers are untouched. This includes
profile storage, which remains future work. With no server configured even
the known title files are unavailable. UploadByServerPath and UploadFromMemory
complete with STORAGE_ACCESS_DENIED (`0x8015c002`); no upload is claimed to be
saved. Failed GetProgress reports zero bytes/percent with the task's error.
Guest paths never reach the host filesystem or become arbitrary HTTP paths.

The title classifies these errors: `0x0006c4a0` maps FILE_NOT_FOUND to
resource result 5 and ACCESS_DENIED to result 3. Tests cover both path prefixes,
both channel capacities, publication after logon followed by an explicit
request, missing/unknown paths, exact-capacity and oversized replies, packed
optional results, progress and task cleanup. The path formatter tests also
cover split IDs, UTF-16 size queries, Unicode and prefix-only overflow.

## Client HLE: local stats

The periodic stats manager `0x00074aa0`, called by the network update at
`0x0008df50`, reaches four public entries through wrappers `0x00092220`,
`0x00092330`, `0x000923c0` and `0x00092450`: StatRead, StatReadGetResult,
StatWrite and StatWriteGetResult. No StatReset or leader-enumeration call was
found in this chain or the verified boundary table; no new addresses are guessed.

StatWrite (`0x003a08c0`) validates the guest batch and copies its nested values
before returning a task. A spec is 24 bytes: a 12-byte XUID (64-bit ID plus flags),
leaderboard ID, stat count and guest array pointer. Each stat is 16 bytes: a
16-bit ID, two padding bytes, a 32-bit type and an eight-byte union. NONE, signed
LONG, signed LONGLONG, finite DOUBLE and terminated UTF-16 strings are supported.
The write batch allows at most 20 specs and 64 stats per spec; strings are bounded
to 32 UTF-16 units. Invalid types, malformed strings and oversized counts return
stat errors before accepting anything. All guest buffers are checked, including
nested arrays and strings; an invalid pointer follows the runtime's memory-fault
path without accepting a partial write or publishing a task handle.

TaskContinue returns XONLINETASK_S_SUCCESS (`0x001500f0`). Ordinary S_OK from the
starter alone would not complete a task. StatWriteGetResult (`0x003ab6a6`) returns
S_OK and clears any supplied server-file handle, attachment-reference pointer and
reference count. No attachment upload is needed. In the retail success branch at
`0x000754e8`, the title then calls `0x00080d70` to acknowledge its original stat
arrays and clear uploaded dirty state. Those input arrays remain unchanged by HLE.

Accepted writes remain in the Hle as owned StatWrite records, including sequence,
Unix capture time, user ID/flags, leaderboard, IDs and typed values. Closing tasks
or signing out does not discard them. `Hle::stat_writes()` returns a snapshot;
`Hle::drain_stat_writes()` transfers them to a future server-event forwarder.
`H2_BOOT_VERBOSE=1` emits the parsed records as debug diagnostics; account keys
are never part of the capture. The local journal is bounded to 256 batches and an
8 MiB value budget. When full, a new write fails with OUT_OF_MEMORY without
silently evicting an accepted batch. Draining frees the budget and preserves the
monotonic sequence. Nothing is sent to the server or persisted to disk yet.

StatRead (`0x003a08cb`) owns its query IDs, supports the manager's 32-spec batch
(up to 64 specs), and completes successfully. StatReadGetResult (`0x003a08d6`)
checks the caller-initialised specs/IDs before writing type NONE and a zero union
in each existing stat. Headers, IDs, padding and the unused extra buffer are
preserved. The consumer at `0x000751a9` treats NONE as zero. Reads currently do not
serve the local write journal as canonical leaderboard data.

Synthetic guest-stack tests exercise each entry, all five value types, copied
ownership, optional attachment outputs, successful task pumping/closing,
transactional rejection, pointer faults, the 32-spec read batch and capture
retention/draining. No test runs the title.

## Client HLE: outgoing messages

The observed stop `0x003a9d8e` is reached from `0x0008f070`, through
`0x0008ef90` and the UI send action `0x002b7e69`. This is the by-name social
message composer. Its internal type 1 builds a Live type-2 friend request; its
internal type 2 builds a Live type-4 clan recruitment message, with the team
context flag. Both can carry text/language and voice data/codec/duration. The
backtrace alone does not include the type field, so it does not distinguish
these two actions. This path does not send feedback or the MOTD/hopper files.
The shared composer also builds type-3 game invitations and title-custom mail.

MessageCreate (`0x003a9d8e`) now returns an opaque guest handle. It retains the
message type, 64-bit context, flags and expiry. Property/count sizes are allocation
hints rather than hard limits on later setters. MessageSetProperty (`0x003a9dc3`)
validates and owns typed values: NULL, 8/16/32/64-bit integers, UTF-16 strings,
FILETIME, binary, attachment, BOOL and string ID. Replacing a property's identifier
replaces its stored value. Inline details are limited to 4096 bytes; voice/blob
attachments are copied into host-owned memory, bounded to 1 MiB each and 16 per
message. The non-exportable attachment flag is retained. Directory attachments
are not read from the host filesystem and return INVALID_ARGUMENT. Guest pointer
faults and rejected values leave the existing builder unchanged.

MessageSend (`0x003ab060`) snapshots the message and its array of 12-byte XUID
recipients. The composer uses typed send APIs too: FriendsRequestEx and
FriendsRequestByNameEx (`0x003a835d`/`0x003a838a`), TeamMemberRecruit and
TeamMemberRecruitByName (`0x003a8399`/`0x003a83d4`), and GameInviteSend
(`0x003a8321`). These capture the same owned message with the recipient XUID or
bounded ANSI gamertag, plus team/member properties or session ID as appropriate.
A null optional message on a typed send becomes an empty message of that type.
The sender must be signed in. With a server configured, GameInviteSend posts
signed mailbox invitations and its task waits for acknowledgment. FriendsRequestEx
and FriendsRequestByNameEx post a signed `FriendRequested` event on the player's
account stream (`SocialRequest::FriendRequest` / `FriendRequestByName` in
`crates/live/live-hle/src/client.rs`); the by-name form first reads
`/v1/accounts/{gamertag}` for the xuid. The task completes when the server has
the event; an unknown gamertag or xuid completes it with FRIEND_NOT_FOUND
(`0x80152008`), and a refused request (already friends or pending, a Clanker)
with INVALID_ARGUMENT. A plain text message, the generic MessageSend of a type-1 message, posts a
signed `MessageSent` event to each recipient's mailbox (`SocialRequest::Text`
in `client.rs`, started from `state.rs`). The text is the `0x6c4` property,
cut to `live_proto::mailbox::TEXT_MAX` characters. The task reports the
server's answer: success once the event is stored, FRIEND_NOT_FOUND
(`0x80152008`) when the recipient is not a friend or the account is unknown,
INVALID_ARGUMENT for text the server refuses. A message with voice and text
sends the text only; a message with no text stays a local capture. Voice
attachments are not delivered. Other generic captures remain local; clan
recruitment has its separate server-backed Teams path.

Every accepted local send returns a task whose Continue result is
XONLINETASK_S_SUCCESS (`0x001500f0`), rather than the running value S_OK. The game
pumps these tasks through `0x0006c450` and `0x0006c670`; the completion callback
`0x0008f960` can finish and dispose its message. No SendGetResults or
SendGetProgress entry is used in this verified boundary. MessageDestroy
(`0x003a9db8`) frees the builder; TaskClose frees only the task. Each accepted
send is captured once, independently of later edits, destruction or task pumps.

`Hle::message_sends()` snapshots the accepted sends and
`Hle::drain_message_sends()` transfers them to a future server-event forwarder.
Records retain sequence/time, sender, send kind, recipients and built properties;
`H2_BOOT_VERBOSE=1` prints these as debug diagnostics. They contain no account
signing key or guest pointer. Captures survive cleanup/sign-out, while those
operations invalidate live builders and tasks. The local journal holds up to
256 sends and an 8 MiB value budget; builders have a separate 8 MiB budget and
64-handle limit. Exhaustion returns OUT_OF_MEMORY without discarding earlier
captures. Destroying builders and draining captures free the respective budgets.
Captures are in memory only. Local capture success does not establish delivery
or friendship. A server-backed game-invite task instead reports the real mailbox
write result; invitation acceptance is a separate operation.

Synthetic guest-stack tests cover all nine entries, the by-name composer flow,
all property types, packed XUID/member layouts, successful tasks, copied
ownership, validation/fault atomicity, destruction, cleanup and bounded journals.
They use no XBE and do not run the title.

## Client HLE: game invitations and joining

GameInviteSend now posts `GameInviteSent` to each recipient's server mailbox.
Its task stays running until the HTTP worker finishes, then reports success or
the server/transport error. The original owned message capture remains available;
text and voice attachments in that capture are not delivered in this slice.
FriendsGameInvite is the older no-task API: it copies the supplied 86-byte friend
array, queues the same writes and returns once queued. Backend failures on these
no-task calls are visible through `Hle::server_status()`, without blocking a frame.
Without a server URL, typed MessageSend captures retain their local behaviour;
the new friend-invite actions return NO_SESSION.

FriendsStartup polling and the menu's GetLatest/notification/message reads queue
cached refreshes, at most once per 250 ms per controller after a reply. The worker
fetches actual friends/requests, open sessions and the caller's mailbox. No HTTP
runs under a guest-memory or HLE lock. FriendsGetLatest returns a DWORD count,
not an HRESULT. Its records include online/playing/joinable state and received
invite flags. PresenceGetLatest overlays that cached view on watched users.
These reads do not implement sending or accepting friend requests: establish the
friendship through the existing CLI or web UI first.

MessageEnumerate presents each pending invite as a type-3, 64-byte summary from
the real inviter XUID/gamertag, and each text message from a friend as a type-1
summary from the sender, together newest first and at most 100
(`Invites::inbox`). The summary's message flags word at +36 holds HAS_TEXT
(`0x8`) for a text message and nothing for an invite; neither sets HAS_VOICE.
The message screen's voice line does not read the summary: it shows when
the account voice check `0x001906da` fails for the screen's controller, or
when detail property `0x03c2` reads 1 and attachment `0x09c1` exists
(`0x002c74f9`), and neither property is answered here. GetNotificationEx returns one type-3 notification
per invite; repeated polls return FALSE with cleared output. Details owns a
snapshot task, and DetailsGetResultsProperty returns the required session-ID
property `0x0581` of an invitation, or the text property `0x06c4` (a NUL-ended
UTF-16 string) of a text message. Other properties report PROPERTY_NOT_FOUND and
no attachment is invented. MessageDelete queues a remove (or block-sender)
answer and hides the item; for a text message it posts a signed `MessageDeleted`
and the server drops the message. An already handled known ID is acknowledged. Title resources remain
on the storage path and never become inbox messages or notifications.

FriendsAnswerGameInvite and GameInviteAnswer queue an authenticated recipient
answer, using the actual cached invite ID. YES/accept publishes a pending join
only after server acknowledgment; decline/remove never do. The newer peer
answer enum's NEVER value blocks later game invitations from that sender.
GameInviteAnswer owns a task; the older friend answer queues its work without a
task. FriendsJoinGame resolves an existing friendship and the friend's open
session before publishing the same pending join, even without an invitation.
This keeps a same-title join inside the current process instead of using the
retail cross-title dashboard launch path.

GetAcceptedGameInvite returns S_OK once for an accepted join, then E_FAIL with
a cleared output. Its 156-byte record is statically verified against
`0x003b49ce` and the consumer `0x0022f3b5`: packed InvitingFriend at +0 (86
bytes, padded to 88), accepted XUID/flags at +88, acceptance FILETIME at +100,
and four 12-byte logged-on XUID/flag records at +108. The friend's session ID is
at +40 and title ID at +48. The record expires after 15 minutes. Pointer faults
cannot consume it. Sign-out clears tasks, caches and pending joins. Answers for
a controller whose account changed cannot create a join for the new account.

The game can then use its existing FindFromID, search-result parsing, key
registration, XNADDR conversion, QoS and sockets. The host's exchange key is
never published; the remote key is zero for the current unencrypted HLE
transport. The HLE QoS request carries the joining console's replication
version in its echoed nonce. The responder adds its own version after the
length-delimited title data. Both sides require an explicit match and the
responder withholds the title data and session key hint on a mismatch.
The diagnostic is `replication_version_mismatch`. This check applies to
direct and invited joins and the queue's peer and dedicated host checks,
including the dedicated tunnel. A queue refusal fails the search task with
`REPLICATION_VERSION_MISMATCH` and keeps the diagnostic on its matchmaking
status. The guest join check also gates membership on both transports.
[AI over the network](../game/ai-networking.md#creature-entity)

An embedded-server test follows this path through a successful QoS probe and
a two-way guest UDP exchange, with no broadcast peers configured.
The full retail invite menu/confirmation sequence, cancellation timing and a
complete custom game still need owner play testing. No game was built or run.

A two-instance recipe, using an already built `h2`, is:

1. Run one persistent server, for example `h2 live serve --db file --listen
   127.0.0.1:8074`. Both instances must use its URL.
2. Create accounts Alpha and Bravo with `h2 live account create NAME --server
   http://127.0.0.1:8074`. Establish friendship with `h2 live friend add Bravo
   --as Alpha --server http://127.0.0.1:8074`, then `h2 live friend accept Alpha
   --as Bravo --server http://127.0.0.1:8074` (or use the web UI).
3. Start Alpha with `H2_XNET_ADDR=127.0.0.1:31000 h2 play --live-server
   http://127.0.0.1:8074 --gamertag Alpha`. Wait for sign-in, then start Bravo
   with `H2_XNET_ADDR=127.0.0.1:32000 h2 play --live-server
   http://127.0.0.1:8074 --gamertag Bravo`. Sequential startup also avoids the
   CLI's shared play-accounts file being rewritten before the first HLE reads it.
4. Alpha hosts a custom lobby and invites Bravo from Friends; Bravo accepts.
   `H2_XNET_LOOPBACK_PEERS` is unnecessary: HTTP supplies discovery. Bases on one
   IP must differ by at least seven ports. Across a LAN use reachable LAN IPv4s;
   across the internet forward UDP base through base+6. An HTTP tunnel alone
   does not carry gameplay packets.
