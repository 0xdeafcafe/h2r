# Halo 2 Live: message_of_the_day.dat and multiplayer_hopper.dat

How retail Halo 2 (XDK 5849, `default.xbe`) fetches, stores and parses the two
title-storage files downloaded after Xbox Live sign-in. Addresses are XBE
virtual addresses (image base 0x10000). Everything here was recovered by
static analysis of the retail disc image; nothing was run.

Abbreviations: filo = the game's 0x110-byte file-request struct (signature
dword `0x66696c6f` at +0x00, flags at +0x04, ASCII path at +0x08).


> **Disputed (2026-11-15).** The file formats (§3, §5) are game-side and
> self-consistent. The fetch chain in §2 is not settled: it names the
> download entry `0x3a9d74` and the task entry `0x3a07a3`, which
> [live-boundary.md](live-boundary.md) identifies as
> `XOnlineMessageDownloadAttachmentToMemory` and `XOnlineFriendsStartup`,
> and that inventory has `XOnlineStorageDownloadToMemory` at `0x3ab6ea`
> (called from `0x000b4450`). Treat §2 as unverified until the storage
> path is traced from the completion handlers `0x0008c790` and `0x00194260`
> back to the API that delivered their buffers.

## 1. The .data descriptors

### 1.1 `0x004672b8` — message_of_the_day.dat

Only the first dword is ever read by code (at `0x0008ca39`,
`mov edi, [0x4672b8]`), so treat this as a global `char*` plus adjacent
statics, not a referenced struct:

| offset | value | meaning |
|--------|-------|---------|
| +0x00 | `0x00450f7c` | `char*` = `"message_of_the_day.dat"` (ASCII) |
| +0x04 | 0 | |
| +0x08 | `0x00450f4c` | `wchar_t*` = `"message_of_the_day.dat"` (UTF-16LE) |
| +0x0c | 1 | flag/count |
| +0x14 | `0x000a45d0` | fn: trivial predicate, `return true` |
| +0x1c | `0x0008c790` | fn: download-complete handler (inflate + cache write, §3) |

Immediately before it at `0x004672a0` is a same-shaped group for
`"bungienet_user.dat"` (wide string at `0x00450f20`): `{ name, 1, 0,
0x0008c5b0, 0, 0x0008c5f0 }`. After `0x004672b8` comes an unrelated stats
upload table (`"/upload_server/stats.ashx"` at `0x004672d8`, XONLINE error
codes `0x80152100/0x80152101` paired with codes `0x5e/0x5f`). So the game has
(at least) three downloadable/uploadable blobs: `bungienet_user.dat`,
`message_of_the_day.dat`, `multiplayer_hopper.dat`.

The only code that reads the name global is the filo builder
`0x0008ca00(&filo)`: zeroes 0x110 bytes, writes the signature and
`word +0x06 = 0xffff`, path-appends `"n:\"` (string `0x00450f48`, helper
`0x00137320`) then the filename → the local cache path is
`n:\message_of_the_day.dat` (n: is the save-device mount, also used for
`n:\savegame%d.bin`). Path length cap 0x100 (`0x001374c0`).

### 1.2 `0x0046dd5c` — multiplayer_hopper.dat

| offset | value | meaning |
|--------|-------|---------|
| +0x00 | `0x00453e60` | `char*` = `"multiplayer_hopper.dat"` |
| +0x04 | `0x00453e34` | `char*` = `"multiplayer\matchmaking_hopper_descriptions"` (Halo tag path) |
| +0x08 | 0 | |
| +0x0c | `0x00453e04` | `wchar_t*` = `"multiplayer_hopper.dat"` |
| +0x10 | 4 | file-class/channel id (MOTD has 1); exact meaning unconfirmed |
| +0x14 | 0 | |
| +0x18 | `0x00194160` | fn: load default (compiled-in) hopper descriptions |
| +0x1c | 0 | |
| +0x20 | `0x00194260` | fn: download-complete handler / parser entry |

Immediately after, at `0x0046dd80`, a NUL-terminated table of 15 field-type
names indexed by numeric type id (used by the generic online-file manager,
not by the hopper parser itself — see §5.4):

```
none identifier integer signed-integer point3d boolean raw-data string
wchar-string secure-address quantized-real logarithmic-quantized-real
unit-vector vector axes
```

and after that (`0x0046ddc0`) a table of named records with dword fields,
starting `"games-played" {0, 0x10000, 1}`, `"games-quit"`, ... — the
compiled-in default matchmaking statistics/attribute names that the
downloaded file overrides.

Only the filename field is read by absolute address (`0x00193fd9`); the rest
of the descriptor is consumed indirectly (see §5).

## 2. Fetch: game → XONLINE call chain

XONLINE is entered through thunks at the head of the section that load the
global context `ecx = [0x0048480c]` and jump into the library. The relevant
exports:

| thunk | internal | role |
|-------|----------|------|
| `0x003a9d74` | `0x003a87b3` | storage file download (by service id, to memory) |
| `0x003a9d65` | `0x003a85d4` | service record query / result read |
| `0x003a9d83` | `0x003a88c7` | sibling storage op (forwards to `0x003c70c5`, next to the readfile task builder — likely the writefile side) |
| `0x003a07a3` | `0x003b87be` | task create/attach (sub-type 2) |
| `0x003a07ae` | `0x003b87f3` | task attach (sub-type 3) |
| `0x003a07c4` | `0x003b65f8` | task pump/continue, budget 100 (0x64) per call |
| `0x003a07da` | `0x003b6d0e` | task result/close |
| `0x003a0671` | `0x003a0f6c` | subscription/notification read (4 records of 0x70 bytes) |
| `0x003a067c` | `0x003a38b7` | per-entry setup over the 13-entry table at `0x00467178` |

### 2.1 Sign-in → download kickoff (confirmed)

- Sign-in state machine (~`0x0006c548`.., jump table `0x0006c620`) calls
  `0x0006bff0` on the signed-in transition (from `0x0006c323`) and on XONLINE
  code `0x1513f1` (from `0x0006c402`).
- `0x0006bff0` requires session handle `[0x00467214] != -1` and online-state
  helper `0x0006cd50() == 1`; calls `0x3a0671`; loops the 13-entry table at
  `0x00467178` calling `0x3a067c`; arms four async-transfer descriptors via
  `0x00080f70` — `0x00477058`, `0x0047d8f4`, `0x00479780` (MOTD), plus
  `0x00479750` — then calls `0x0008cc40` (starts a transfer: allocates a
  0x14-byte record from the pool at `[0x004cf78c]+0x44`, stride
  `[+0x24]`, capacity `[+0x38]`; calls thunk `0x3a07a3(0, &rec+0x0c)` →
  stores the XONLINE task handle at rec+0x0c, sets rec `+2 = 1`, state
  `+4 = 2`) and `0x0008d4d0` (thunk `0x3a069d`, state 0xd).
- Retry path: dispatcher case at `0x0006c5a1`; on error ≠ `0x80151000` it
  calls `0x0008cc40` again.
- Storage-change notifications also re-arm downloads: `0x0008c660` (called
  from the per-frame pump at `0x0006c0ba`) reads four 0x70-byte records via
  `0x3a0671` and re-queues fetches through `0x00080f70` /
  `0x0018fe9e` for files whose subscription fired.

### 2.2 The download itself

- Game wrapper `0x0008eab0(handle, code_enum, arg2, arg3)`: requires
  signed-in state (`0x0006b5d0(handle)==2`), maps a small enum through the
  switch table at `0x0008eda0` (24 entries) to a packed service code
  `(service_type << 8) | service_id` — enum 0 → `0x09c1` (type 9 =
  title-storage, service id `0xc1`); then calls export `0x3a9d74`.
- `0x3a87b3` (inside XONLINE): resolves the caller's logon service directory
  (per-user records at `ctx+0x13e0`, 0x48 bytes each), finds the record whose
  byte0 = service id and byte1 = 9 (storage); errors `0x80155a03` (no
  services), `0x8015000c` (wrong type), `0x80150001` (buffer smaller than the
  service record's advertised size). The record carries the **server path as
  a wide string**, converted to ASCII (0x100-byte buffer) — i.e. the filename
  on the wire comes from the service directory delivered at logon, not from
  the client binary.
- It then builds the HTTP task via `0x003c6f4d` → `0x003c7b28`:
  `POST /stfd/readfile.ashx` (string `0x0056c540`), opcode/verb `0x94`,
  timeout `0xea60` = 60000 ms, receive chunk buffer `0x8200` bytes
  (`0x003c6c13` worker, path buffer at task+0x19c9c, callbacks
  `0x003c6b41` / `0x003c791a` / `0x003c77a3`). Max remote path/filename
  length 0x200 (`0x8015c008` if longer). Directory enumeration variant
  appends `"cache.xcd"` (`0x0056b564`).

### 2.3 Completion polling

- Per frame, UI/manager code `0x002c7410` → `0x002c74f9` (requires
  `[this+0x7ecc]==0`, `[this+0xd9c]!=0`, signed in) polls each pending op
  with `0x0008e9e0(op_id, buf, size, &done_flag, &result)`, which maps the op
  id through `0x0008eda0` and reads the result via export `0x3a9d65`;
  `0x80155a02` means "still pending". Ops seen: 1,2 (2 bytes), 3 (0x800
  bytes), 4 (4 bytes), 5 (8 bytes).
- The transfer records are pumped through `0x0008cd60` → thunk `0x3a07c4`
  with a 100-unit budget; handles are closed with `0x0006b640` when done.
- When a transfer completes, the engine (pump table `0x004d8c28`) sets bit 1
  of the descriptor's flags word — for MOTD that is `0x00479784`
  (`0x00479780` descriptor object; `+0x08` = `0x00479788` is the state/count
  word). `0x0008ca50()` is the public gate: `count != 0 && flags & 2`.
- Sign-out/teardown (`0x0006cbf7`) clears bit 1 on all four channel flag
  words — the cached MOTD/hopper are invalidated and re-fetched on the next
  sign-in.

## 3. MOTD: what the game does with the bytes

Download-complete handler `0x0008c790(task_result)`:

- task result struct: `word +0x08` = flags (must be nonzero),
  `byte +0x04 bit 1` = completed-ok, `word +0x0e` = must be 1,
  `dword +0x24` = data ptr, `dword +0x28` = size.
- If data != null and size > 0x10: **skip a 16-byte header** (contents never
  inspected by the game).
- Read `u32` at offset 0x10, **big-endian** (`bswap`, helper `0x00199560`):
  it is the inflated size and must equal exactly `0x25224` (152,100).
- Allocate 0x25224 bytes (retry loop up to 90 tries with a sleep
  (`0x0012c600`) and a D3D keep-alive `0x003fac30` while waiting).
- zlib-inflate (`0x00199740`) into it, then write the local cache file with
  `0x0008c900(buf)`: rebuilds the filo (`0x0008ca00`), opens
  `n:\message_of_the_day.dat` for write, writes one 0x200-byte record from
  `buf + slot*0x200` and one 0x4004-byte block from
  `buf + 0x1200 + slot*0x4004`, where `slot = [0x0047ff38]` (0..8, computed
  by `0x0011ca80` — the console language/locale index).

### 3.1 message_of_the_day.dat — wire format

```
+0x00  16 bytes   header (not interpreted by the game; likely version/
                  signature material — UNKNOWN)
+0x10  u32 BE     inflated size, must be 0x25224 (152100)
+0x14  zlib deflate stream
```

Inflated payload (152100 bytes):

```
+0x00000  9 x 512 bytes   per-language header strings (UTF-16LE, NUL-term)
+0x01200  9 x 0x4004      per-language message blocks, each:
            +0x00 u32 LE  message count (must be <= 32)
            +0x04 32 x 512 bytes  message strings (UTF-16LE, NUL-term)
```

(9 x 0x200 + 9 x 0x4004 = 0x1200 + 0x24024 = 0x25224 exactly.)

### 3.2 Local cache `n:\message_of_the_day.dat`

Written by `0x0008c900`, read by `0x0008ca70`. Layout (single language — the
console's current one):

```
+0x0000  512 bytes  header string (UTF-16LE)
+0x0200  u32 LE     message count (<= 32, else read fails)
+0x0204  32 x 512   message strings (UTF-16LE)
```

total 0x4204 bytes. Reads (`0x0008ca70(mode, out_wchar_buf_0x200)`):
mode 0 → the header/first record; mode 1 → reads count + records, picks one
at random (LCG `0x19660d`/`0x3c6ef35f`), copies ≤ 0xff wchars into the
caller's 0x200-byte buffer.

### 3.3 Failure behavior

- Download fails / file absent / size mismatch / inflate fails: handler
  returns 0, cache write skipped, `0x00479784` bit 1 stays clear →
  `0x0008ca50()` false → screens never call the reader. No visible error.
- Read with no cache file: `0x0008ca70` returns false, caller leaves its
  fallback text (localized string id `0x7f8a0` in the matchmaking screen
  ctor).

## 4. Where the MOTD shows

Two UI screens read it (both gate on `0x0008ca50()`):

- **Live main-menu-family screen** (class factory `0x002310b7`, update
  `0x00231227`; opened by the UI manager at `0x0019b0ad` only when signed in,
  `0x0006c7e0()` true): reads once per screen lifetime with mode 0 (first
  record), pushes the wide string to child widget id (5,6) via the widget's
  vtable (+0x3c → text sub-object, then +0x04 → text setter).
- **Matchmaking screen** (factory `0x002519bb`, name string "matchmaking
  list" at `0x0045a408` beside its vtable; opened at `0x0019b2a1`): state 1
  updater `0x00251f8d` re-reads the cache file **every 15 s** (`0x3a98` ms)
  with mode 1 (random record) and sets child widget (7,6); widgets (2,6) and
  (5,6) show the surrounding caption/status from string ids
  (`0x1400043a`, `0x0e000761`, `0x11000762`).

## 5. multiplayer_hopper.dat (the playlists)

### 5.1 Wire format

Same envelope as the MOTD, validated by the download-complete callback
`0x00194260(download_result)`:

- result struct checks: `[+0x04] & 2` (complete), `[+0x08]` u16 != 0,
  `[+0x0e]` u16 == 1 (version 1); payload at `[+0x24]`, size `[+0x28]`.
- Skip a 16-byte header (never interpreted).
- `u32` BE at offset 0x10 = inflated size, must equal exactly
  **`0x162cc0`** (1,457,344) (`cmp`/`jne` at `0x001942cf`) — the only content
  validation on the downloaded bytes.
- zlib stream from offset 0x14 (inflate wrapper `0x00199740` → `0x002cb5b0`,
  zalloc `0x0024f850` / zfree `0x0024f880`, "zlib pool" string `0x004544f0`),
  inflated into a pooled buffer, then saved to the local cache by
  `0x00193ff0`.

### 5.2 Inflated payload = the on-disk file, byte for byte

The local cache `n:\multiplayer_hopper.dat` is a raw fixed-size binary image
of exactly 0x162cc0 bytes with **no magic, no version, no checksum**:

```
+0x000000  16 x 0x614     hopper descriptor entries (runtime copy at 0x551ae8)
+0x006140  16 x 0x15cb8   per-hopper data blocks
```

- Descriptor entry (0x614 bytes): +0x00 type (1..5), +0x04 status
  (0 = not loaded, 1 = ok, 2 = error), +0x08 record count (must be 1..100,
  validator `0x00193560`), +0x0c 4 x 0x10 per-player data,
  +0x9c 4 x 0x80 per-player strings, +0x51b ASCII name (chars <= 0x7f),
  +0x5a0 dword array, +0x5e4 dword, +0x5ec/+0x5f0/+0x5f4 range fields.
- Data block (0x15cb8 bytes): +0x00..+0x43 header, +0x44 dword record count,
  +0x48 records, stride 0x37c. Record: +0x04 id (must resolve to a record in
  the `matchmaking_hopper_descriptions` tag block — lookup `0x0019c5f0`,
  else the slot is marked status=2 error), +0x08 0x40-byte template blob,
  +0x48 0x200-byte copy, +0x248 dword, +0x24c string, +0x294 flags,
  +0x2a4 dword.
- Defaults (`0x001940d0` and helpers) use packed `(value << 16) | field_id`
  dwords, field ids 0x7a4..0x7ab, with millisecond constants 0x7530 / 0xafc8 /
  0x1d4c0 / 0x249f0 / 0x493e0 (30 s / 45 s / 120 s / 150 s / 300 s) —
  matchmaking timeouts. Per-game-type record templates at
  `0x00444fc0`/`0x00445068`/`0x00445188`/`0x00445218`/`0x00445258`.

So: the tag defines the universe of hopper descriptions; the .dat supplies
live-tunable per-hopper parameters that reference tag records by id.

### 5.3 Load paths and failure behavior

- `0x00194160` (descriptor callback +0x18): reset-to-defaults — allocates the
  0x162cc0 table, fills defaults (`0x001940d0`), saves via `0x00193ff0`.
- `0x00194260` (callback +0x20): download-complete — validate envelope,
  inflate, save via `0x00193ff0`. Any failure → return 0, file not written.
- `0x001943d0(index, &dst)`: read one 0x15cb8 block (open `0x00136970`
  mode 1 = GENERIC_READ/OPEN_EXISTING, seek `0x6140 + index*0x15cb8`, read,
  close).
- Ready gate `0x00193f50`: `[0x0047d8fc] != 0 && [0x0047d8f8] & 2`
  (+version word `0x0047d8fe` == 1) — same descriptor-flags pattern as MOTD,
  on the descriptor at `0x0047d8f4`.
- Consumers: matchmaking start flow (`0x00231066` area, reads the selected
  hopper into a 0x15cb8 frame), session setup (`0x00192eb0` from the session
  state machine, states 6/7/8), playlist UI (`0x00065530`, iterates all 16
  descriptors filtering types {2,4,5}).
- If the download fails or the cache file is missing, the game runs on the
  compiled-in defaults — playlists still work, just not updated.

### 5.4 The type-name table

Descriptor +0x24..+0x5c: 15 type names indexed by numeric type id
(`none`, `identifier`, `integer`, `signed-integer`, `point3d`, `boolean`,
`raw-data`, `string`, `wchar-string`, `secure-address`, `quantized-real`,
`logarithmic-quantized-real`, `unit-vector`, `vector`, `axes`), NUL
terminator at +0x60. Referenced only from the table (no strcmp) — used by
the generic online-file manager (diagnostics/schema), not by the hopper
parser itself. The `"games-played"` / `"games-quit"` records that follow at
`0x0046ddc0` belong to the player-statistics schema (cf.
`/upload_server/stats.ashx`), a different following struct — not the hopper
format.

Descriptor field +0x10 = 4 (MOTD has 1): a file-class/channel id; exact
meaning unconfirmed.

## 6. What our server must do (summary)

1. At logon, hand the client a service directory containing a type-9
   (title storage) service record with id `0xc1` (and the other ids implied
   by the `0x0008eda0` enum table), each carrying the wide server path of
   the file to fetch.
2. Serve `POST /stfd/readfile.ashx` with the raw file bytes (chunked into
   the client's 0x8200-byte receive buffer; 60 s task timeout).
3. `message_of_the_day.dat`: 16-byte header (any bytes pass) + BE u32
   `0x00025224` + zlib stream inflating to the 152,100-byte payload of §3.1.
4. `multiplayer_hopper.dat`: 16-byte header + BE u32 `0x00162cc0` + zlib
   stream inflating to the 1,457,344-byte image of §5.2. Record ids must
   match the `matchmaking_hopper_descriptions` tag records or the slot is
   marked error.
5. No client-side signature/hash check exists beyond the exact inflated-size
   equality and zlib integrity. (XONLINE may additionally enforce content
   signing via `/xsig` for services flagged signed in the service directory
   — not verified here.)
6. On failure the game silently keeps defaults / last cached copy; there is
   no user-visible error path to satisfy.

## Open questions

- Contents of the 16-byte file header (never read by the game).
- Which language/locale mapping `0x0011ca80` implements (9 MOTD slots).
- Descriptor field +0x10 (=1 MOTD, =4 hopper): file-class id, meaning
  unconfirmed; the {1, -1, 1} dwords at 0x0046dd48..0x0046dd50 are
  read/written raw by matchmaking code (`0x0017e67b`, `0x0017e810`,
  `0x0017e8ff`).
- The runtime dispatcher that invokes the descriptor callbacks (+0x18/+0x20)
  — the descriptors are reached through a runtime (ptr, tag) table at
  `0x00472410` whose base has no code refs; the download manager in the
  `0x0008cc40`/`0x0008cce0`/`0x0008cd60` cluster is the likely invoker.
- Whether `/xsig` content signing is enforced for these files on retail.
- The 0x499a-byte download at `this+0x3514` in the per-frame orchestrator
  (`0x002c74f9`) — probably the compressed MOTD buffer; not pinned to a
  filename.
