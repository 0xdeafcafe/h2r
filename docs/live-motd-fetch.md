# How Halo 2 fetches message_of_the_day.dat and multiplayer_hopper.dat — settled

Settles the dispute between docs/live-motd.md §2 and docs/live-boundary.md by
tracing from the completion handlers `0x0008c790` (MOTD) and `0x00194260`
(hopper) back to the XONLINE entry that produced their buffers, then forward
through the call sites. Retail `default.xbe` (XDK 5849), static analysis
only; XDK prototypes checked against the period `xonline.h` (the copy
shipped in the Area 51 source drop, `xCore/3rdParty/Xbox/include/xonline.h`,
which contains the whole 0x80155xxx error block and the exact prototypes
cited below). Every arity claim below was verified by stack-push counts and
callee `ret n` cleanup.

## 1. The verdict in one paragraph

**live-boundary.md is right; live-motd.md §2 misidentified the mechanism.**
Both files arrive as **message attachments**: Halo 2 enumerates its Xbox
Live *messages* after sign-in, opens *message details* for one
(`XOnlineMessageDetails`, thunk `0x3ab2a6`), reads its *properties*
(`XOnlineMessageDetailsGetResultsProperty`, thunk `0x3a9d65`), and downloads
the property with tag `0x9c1` — `XONLINE_MSG_PROP_TAG(ATTACHMENT=9, id=0xc1)`
— with `XOnlineMessageDownloadAttachmentToMemory` (thunk `0x3a9d74`), which
internally resolves the attachment's wide *server path* from the details
property directory and fetches it from title storage
(`POST /stfd/readfile.ashx`, string `0x0056c540`). The "service codes
(service_type<<8)|service_id" of live-motd are actually **message property
tags** (`type<<8 | id`, `XONLINE_MSG_PROP_TAG`); the "logon service
directory" is the **message-details property directory**; and `0x3a07a3`,
which live-motd read as "the transfer task creator", is
`XOnlineFriendsStartup` — its handle is not on this path at all.

## 2. The verified chain (MOTD and hopper both)

Driver: the per-frame update of a UI/manager class at `0x002c7410`
(vtable-dispatched; no direct callers — the class is constructed in the
sign-in flow). When signed in it calls `0x002c74f9` (message + download
state machine) and `0x002c77fe` (download poll) every frame.

Backwards from the handlers to the entry:

| address | role | evidence |
|---|---|---|
| `0x0008c790` / `0x00194260` | download-complete handlers (MOTD / hopper): validate a game-built result record (`+0x24` data ptr, `+0x28` size), inflate, write the `n:\` cache | descriptors `0x004672b8+0x1c` / `0x0046dd5c+0x20`; **no direct callers** — invoked as function pointers |
| `0x002c7a6a` | hands the finished buffer to the game's async-file manager: `0x538e0(class-id, buffer=this+0x3514, size=[this+0x7eb0])` → `0x554c0` (engine) → per-file descriptor (`0x004672b8` group / `0x0046dd5c` group) → callback | reads exactly the two fields GetResults produced |
| `0x002c77fe` | polls the download task `[this+0x2d0c]`; on completion `0x0008eb50` → thunk `0x3a9d83` = `XOnlineMessageDownloadAttachmentToMemoryGetResults(hTask, &ptr, &received, &total)` (4 args, XDK-verified); when received == total, calls `0x2c7a6a` | the XDK GetResults prototype supplies the (pointer, size) pair |
| `0x002c7683` | starts the download: `0x0008eab0(user, enum 0, &this[0x3514], 0x499a)` → thunk `0x3a9d74` = `XOnlineMessageDownloadAttachmentToMemory(hDetailsTask, wPropTag=0x9c1, pbBuffer, dwBufferSize=0x499a, hEvent=0, phTask)`; task handle → `[this+0x2d0c]` | six stack args pushed, internal `ret 0x18`; only call site of `0x8eab0` in the game |
| `0x003a9d74` → `0x003a87b3` | the API: validates `hDetailsTask` (`[h+0x10]==2`), parses the details blob at `h+0x1c` (`0x3b9d63`), walks the property directory (count at `+0x58`, entries `{byte id, byte type, word offset}` at `+0x5c`), requires id `0xc1` **and type byte 9 (ATTACHMENT)** — errors `0x80155A03` (property not found) / `0x8015000c` (not an attachment / DIRECTORY flag set, `XONLINE_MSG_ATTACHMENT_FLAG_DIRECTORY`) — checks advertised size vs buffer (`0x80150001`), converts the property's **wide server path** (`value+0xc`) to a 0x100 ASCII path, and builds the storage request via `0x3c6f4d` → `/stfd/readfile.ashx` (string `0x0056c540`) | the literal type-9 byte compare is the smoking gun: it is `XONLINE_MSG_PROP_TYPE_ATTACHMENT` |
| `0x002c7a46` | creates the details task: `0x0008e950(summary)` → thunk `0x3ab2a6` = `XOnlineMessageDetails(dwUserIndex, dwMessageID=[summary+0x20], dwFlagsToSet=0x10, 0, 0, &phTask)`; handle stored at user-record `+0xc`; `0x8e950` returns the user-object handle to `[this+0x2d08]` | six pushes + `ret 4` wrapper; only call site of `0x8e950` |
| `0x002c7361`, `0x002c788a` | message enumeration: `0x0008e750` → thunk `0x3ab04a` = `XOnlineMessageEnumerate(dwUserIndex, pSummaries, &count)` (XDK: `XONLINE_MSG_SUMMARY` array; `dwMessageID` at summary `+0x20` is what `0x8e950` reads) | `0x8e750` has 8 game call sites (friends/invites UI too); this class enumerates in its constructor/refresh |
| `0x0006c0fd`, `0x0006c11b` | arm the MOTD/hopper channel descriptors (`0x0047d8f4`, `0x00479780`) at sign-in; gates `0x0008ca50` / `0x00193f50` behave as live-motd §2.3/§5.3 described | dword refs to the descriptors |

The one hop still unpinned (and already an open question in live-motd.md):
the game-internal dispatcher that takes `0x554c0`'s completed transfer and
calls the descriptor's `+0x1c`/`+0x20` callback with the result record. It is
entirely past the XONLINE boundary and does not change the verdict.

## 3. What the game asks for, property by property

`0x002c74f9` reads the details task's properties through
`0x0008e9e0(user, enum, buf, size, &done, &result)` → `0x3a9d65`
(`XOnlineMessageDetailsGetResultsProperty(hTask, wPropTag,
dwPropValueBufferSize, pValue, pdwValueSize, pdwAttachmentFlags)`, 6 args).
The enum → tag map is the 24-entry switch at `0x0008eda0` (table
`0x0008ee44`); entries observed on this path:

| enum | tag | decoded (XDK `XONLINE_MSG_PROP_TAG(type,id)`) | what the game does with it |
|---|---|---|---|
| 1 | `0x3c2` | `TAG(I2, 0xc2)` | 2 bytes at `this+0x7eb6`; **must equal 1** — `XONLINE_MSG_TYPE_TITLE_CUSTOM` filter |
| 2 | `0x4c3` | `TAG(I4, 0xc3)` | 4 bytes at `this+0x7eb8` |
| 3 | `0x6c4` | `TAG(STRING, 0xc4)` | 0x800-byte wide string at `this+0x2d10` |
| 4 | `0x4c5` | `TAG(I4, 0xc5)` | 4 bytes at `this+0x3510` — matches the descriptors' `+0x10` class id (**1 = MOTD, 4 = hopper**); this is the file selector |
| 5 | `0x581` | `TAG(I8, 0x81)` | 8 bytes at `this+0x7ec0` |
| 0 | `0x9c1` | `TAG(ATTACHMENT, 0xc1)` | probed into `this+0x3514`; `XONLINE_E_MESSAGE_PROPERTY_DOWNLOAD_REQUIRED` (`0x80155A02`) is the expected "fetch it separately" answer, which then triggers the `0x8eab0` download (cap `0x499a` = 18,842 bytes) |

Other table entries (0x481…0x791: I4/I8/STRING tags `0x81..0x91`) serve the
friends/invite UI, which shares `0x8e9e0`.

## 4. Which earlier claims were right and wrong

**live-boundary.md — right:** `0x3a9d74`
`XOnlineMessageDownloadAttachmentToMemory` (6/6 args, `ret 0x18` verified),
`0x3a9d65` `XOnlineMessageDetailsGetResultsProperty`, `0x3a9d83`
`…GetResults` (`0x0008eb50`, 4 args), `0x3ab2a6`
`XOnlineMessageDetails` (`0x0008e950`), `0x3a07a3`
`XOnlineFriendsStartup` — the `(NULL, &h)` call from `0x0008ccb0` matches
the XDK prototype exactly, and the internal `0x3b87be` get-or-creates the
persistent sub-type-2 task at context `+0x14d0`, which the whole friends
family (`0x3a07ae`…) shares. Its `0x3ab6ea`
`XOnlineStorageDownloadToMemory` row is also correct as an identification —
but that API belongs to the **DLC engine**, not this path: its wrappers
`0x000b4450`/`0x000b45f0` are called only from `0x0008e3f5`/`0x0008e656`,
inside the `0x0008e0xx` cluster that walks the DLC pump table `0x004d8c28`.

**live-motd.md — right:** the game wrapper addresses (`0x0008eab0`,
`0x0008e9e0`, `0x0008eda0`, `0x002c74f9`), the sign-in arming of the channel
descriptors, the `/stfd/readfile.ashx` wire fetch, the 0x100-byte ASCII path
conversion, the completion-handler layouts and everything in §3/§5
(game-side formats). **Wrong:** calling `0x9c1` a "service code" (it is a
property tag; the high byte is the property *type*, and the internal's
literal `9` compare is `XONLINE_MSG_PROP_TYPE_ATTACHMENT`); the "per-user
logon service directory at ctx+0x13e0" (the walk is over the
message-details property directory at `hDetailsTask+0x1c`); "`0x80155a02`
means still pending" (it is `XONLINE_E_MESSAGE_PROPERTY_DOWNLOAD_REQUIRED`;
it *functions* as "download it separately", which is why the polling still
made sense); and "task created at `0x3a07a3` from `0x0008cc40` starts the
transfer" — that task is the friends task and never feeds `0x3a9d74`.

## 5. What our server must do

1. **Deliver two messages, not two files-by-name.** After sign-in the
   messaging service must present, via the message list
   (`XOnlineMessageEnumerate`), one message per downloadable file with
   properties: type filter value 1 (`TITLE_CUSTOM`, tag `0x3c2`), a string
   (`0x6c4`, e.g. the display name), the **file-class I4 (`0x4c5`): 1 =
   MOTD, 4 = hopper** (this is what routes the blob to the right game
   descriptor; inferred from the exact match with descriptor `+0x10`), and
   an **attachment property `0x9c1`** whose details value carries the total
   size and a wide **server path** of our choosing — the client never sends
   the file name; the path in the property is the only name on the wire.
2. **Serve the attachment bytes over the storage endpoint**
   (`/stfd/readfile.ashx` semantics in the original protocol; under our HLE
   this is whatever backs `XOnlineMessageDownloadAttachmentToMemory`) as the
   exact envelope the handlers expect: 16-byte header + big-endian u32
   inflated size + zlib stream; MOTD inflates to exactly `0x25224`, hopper
   to exactly `0x162cc0` (docs/live-motd.md §3.1/§5.2).
3. **Respect the buffer cap:** the single download call site passes
   `dwBufferSize = 0x499a` (18,842 bytes) for both files. The compressed
   hopper envelope must fit; a larger attachment fails the size check
   (`0x80150001`) and the game silently keeps its cached/compiled-in copy.
4. No client-side signature/hash check beyond the inflated-size equality and
   zlib integrity; failure paths are silent (defaults survive), as
   live-motd §6 already recorded.

## 6. Residual uncertainties

- The I4 `0x4c5` → descriptor-`+0x10` mapping is inferred from the exact
  1/4 correspondence plus position in the sequence; I did not find the
  explicit compare in the game's file manager.
- Which enumerated message the orchestrator picks (the selection filter in
  `0x002c788a`'s enumeration walk) was not fully traced; the `0x3c2 == 1`
  gate shows it wants `TITLE_CUSTOM` messages.
- The game-internal hop from `0x554c0` to the descriptor callbacks (section
  2's noted gap).
- `0x0008cd60` (`0x3a07c4`) matches `XOnlineFriendsGetLatest(userIndex,
  count=100 = XONLINE_MAX_FRIENDS, buffer)`, which explains live-motd's
  "budget 100 per call" as the friend-buffer count, not a task budget; I
  verified the wrapper's user-record resolution and gates but not its final
  thunk pushes.

*Tools: capstone 5.0.7 under `python3 -I` (user site), helper scripts
`xref.py`/`addrref.py`/`callers.py` in this directory plus `xb.py`; XDK
prototypes from `xonline.h` in the Area 51 source drop (the only complete
public copy found; RXDK distributes no headers).*
