# MOTD fetch: the first reading of the XONLINE call chain

Evidence for [live-motd.md](../live-motd.md). Nothing here states what the
code does today; the design page does. Static analysis of the retail image,
no game run; written 2026-11-16 and replaced the same day by the title
storage reading in section 2 of the design page.

The addresses stay useful;
the reading of the XONLINE calls is superseded.

## 2026-11-16: the correction and the first reading

**Corrected (2026-11-16).** Both files come from **title storage**,
pulled by the signed-in resource manager: facility 4 (per-title), paths
`//title.4d530064/t:4d530064/9809/message_of_the_day.dat` and
`.../9809/multiplayer_hopper.dat`, download buffers 0x8000 / 0x20000
bytes, and a **validated 16-byte header** (CRC, version, size). §2 below
is rewritten from that trace (§2.0-§2.4); the older subsections are kept
for their addresses but their reading of the XONLINE calls is
superseded. An earlier (2026-11-15) "message attachment" reading was
wrong and its document is gone: the class at `0x002c7410` /
`0x002c74f9` / `0x002c788a` is the game's voice/text message playback
screen, whose property tags (`0x9c1` voice data, `0x3c2`, `0x4c3`,
`0x6c4`, `0x4c5`, `0x581`) are standard XDK message properties. It only
opens messages from the targeted player, and its 0x499a buffer feeds
voice playback, not the MOTD descriptors. `0x003ab6ea`
`XOnlineStorageDownloadToMemory` is the call these two files use
(§2.1').

The body formats in §3 and §5 stand, from offset 0x10.

### 2 (superseded). Fetch: game → XONLINE call chain, first reading

XONLINE is entered through thunks at the head of the section that load the
global context `ecx = [0x0048480c]` and jump into the library. The relevant
exports:

| thunk | internal | role |
|-------|----------|------|
| `0x003a9d74` | `0x003a87b3` | storage file download (by service id, to memory) |
| `0x003a9d65` | `0x003a85d4` | service record query / result read |
| `0x003a9d83` | `0x003a88c7` | sibling storage op (forwards to `0x003c70c5`, next to the readfile task builder - likely the writefile side) |
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
  `0x00080f70` - `0x00477058`, `0x0047d8f4`, `0x00479780` (MOTD), plus
  `0x00479750` - then calls `0x0008cc40` (starts a transfer: allocates a
  0x14-byte record from the pool at `[0x004cf78c]+0x44`, stride
  `[+0x24]`, capacity `[+0x38]`; calls thunk `0x3a07a3(0, &rec+0x0c)` →
  stores the XONLINE task handle at rec+0x0c, sets rec `+2 = 1`, state
  `+4 = 2`) and `0x0008d4d0` (thunk `0x3a069d`, state 0xd).
- Retry path: dispatcher case at `0x0006c5a1`; on error ≠ `0x80151000` it
  calls `0x0008cc40` again.
- Storage-change notifications also re-arm downloads: `0x0008c660` (called
  from the per-frame pump at `0x0006c0ba`) reads four 0x70-byte records via
  `0x3a0671` and re-queues fetches through `0x00080f70` /
  `0x0018fe9e` for files whose subscription fired. *(Corrected: this
  covers only the bungienet_user.dat channel, not MOTD or hopper; §2.0.)*

### 2.2 The download itself

- Game wrapper `0x0008eab0(handle, code_enum, arg2, arg3)`: requires
  signed-in state (`0x0006b5d0(handle)==2`), maps a small enum through the
  switch table at `0x0008eda0` (24 entries) to a packed service code
  `(service_type << 8) | service_id` - enum 0 → `0x09c1` (type 9 =
  title-storage, service id `0xc1`); then calls export `0x3a9d74`.
- `0x3a87b3` (inside XONLINE): resolves the caller's logon service directory
  (per-user records at `ctx+0x13e0`, 0x48 bytes each), finds the record whose
  byte0 = service id and byte1 = 9 (storage); errors `0x80155a03` (no
  services), `0x8015000c` (wrong type), `0x80150001` (buffer smaller than the
  service record's advertised size). The record carries the **server path as
  a wide string**, converted to ASCII (0x100-byte buffer) - i.e. the filename
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
  of the descriptor's flags word - for MOTD that is `0x00479784`
  (`0x00479780` descriptor object; `+0x08` = `0x00479788` is the state/count
  word). `0x0008ca50()` is the public gate: `count != 0 && flags & 2`.
- Sign-out/teardown (`0x0006cbf7`) clears bit 1 on all four channel flag
  words - the cached MOTD/hopper are invalidated and re-fetched on the next
  sign-in.

