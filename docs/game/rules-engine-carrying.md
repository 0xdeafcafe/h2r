# Rules engine: carrying a script

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: compiler, verifier, the variant record, engine index, blob transport.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## Compiler and verifier

**Design.** The formats crate `rules` holds the parser, type checker,
lowering, verifier, IR codec and VM. It has no I/O and no guest types,
like `crates/formats/firefight`, so Studio, the cli (`h2 rules
check|build|run`), Live's upload route and the host runtime link the
same code. Compiling is deterministic, so the IR's SHA-256 names a
script everywhere.

The verifier is separate from the compiler and reads IR, not source. It
runs at upload, at load on any machine about to host, and in Studio as
you type. It checks the whole blob, dead branches included: every op,
slot index, jump target, label, tag path and API call, and that each
call's capability is one the header declares
("Roles, powers and capabilities"). A blob that fails is refused whole.

## Carrying a script

Three things travel: which script, its option values, and the blob.

**Which script and its options: the variant record.** The `hx` block
([game-options.md](game-options.md#where-the-extension-lives),
`variants::ext`) has 13 reserved bytes at `+0x11f..+0x12b`. The plan
uses them all:

| Offset | Size | Content |
|---|---|---|
| `+0x11f` | 1 | mode: 0 none, 1 Firefight, 2 rules script |
| `+0x120` | 8 | the first 8 bytes of the definition's SHA-256 (the rules IR, or Firefight's definition) |
| `+0x128` | 4 | 32 bits of packed script option values, in the order the script declares them |

The block's CRC covers these bytes, and the table is the one layout for
Firefight ([firefight.md](firefight.md#live)) and scripts. A carrier
that copies the record keeps the reference with no new field: saved
variants, `.h2variant` files of either version (`variants::portable`), cloud saves, the
`0x130`-byte variant in a `0x37c`-byte hopper slot
(`crates/live/live-proto/src/hopper`) and the web hopper builder
(`web/src/admin/HopperBuilder.tsx`).

An editor does not copy: it decodes the block to `variants::ext::Ext`
and encodes it again, and `Ext::encode` writes the reserved bytes as
zero and an all-zero block for default extras (**Code**,
`crates/formats/variants/src/ext.rs`; the in-game page in
`crates/extensions/game/src/variant_page.rs` and `VariantEditor::set_ext`
in `crates/formats/variants/src/panel.rs` both write through it). So
`Ext` carries the reference: a `script` field (mode, hash prefix, option
bits) that `decode` reads and `encode` writes, with `is_default` false
while the mode is not 0. A gravity edit keeps the script, and a script
variant with no other extras still has a block. The version stays 1: a
version 2 would make an older build ignore the extras too
(`Why::Version`) and protect nothing, since an older editor encodes
version 1 from its own struct.

**An older build that edits a script variant drops the reference**, and
no newer build can stop it: its `Ext` has no `script` field, so its
`encode` zeroes the bytes. What is left is a valid record of the carrier
engine with the edited extras. It plays as the carrier's game ("A record
with no mode"), its script name and option rows are gone, and that build
shows no warning. The copy cannot be repaired; the variant is made again
from the script, which stays on Live under its hash.

A script's own options are limited to the 32 bits; the compiler refuses
more. Retail's settings (score and time limits, lives, the per-engine
type area) stay where they are and a script reads them by name.

**A game variant file carries the script whole.** A version-2
`.h2variant` ([variants.md](../editor/variants.md#portable-files)) is a
whole game variant: its record carries the prefix that names the script,
and the file carries the script that prefix names, as its IR blob
(base64) and, when it was made from source, the source. Reading one
checks that the IR hashes to the file's hash, that the hash starts with
the record's prefix, that `rules::verify::load` takes the IR and that
the record uses the script index or the script's retail settings layout
(`Header::engine`). A record with mode 2 and no script
is refused. So a variant file plays with no server, and Live keeps game
variants as these files ([live-api.md](../live/live-api-content.md#game-variants)).

**To other machines: the Live session.** The wire codec drops the spare
bytes (`0x7cc50`/`0x7d520`, **E**, game-options.md "How a variant reaches
other machines"), so the block already travels as
`NewSession.variant_ext` (**Code**,
`crates/live/live-proto/src/session.rs`). The session has one field
beside it, `rules`, holding the full 64-digit hash: lowercase hex, checked
by `NewSession::validate` (`valid_rules_hash`), kept by an update and
listed wherever `variant_ext` is. The host publishes it with
`Hle::set_session_rules` and a guest reads it with `Hle::host_rules`
(**Code**, `crates/live/live-proto/src/session.rs`,
`crates/live/live-hle/src/sessions.rs`).

**The blob: by hash.** Built-in scripts are compiled into the binary and
found by hash with no network. Any other blob is stored once on Live
under its SHA-256, in both homes, and fetched by a machine that lacks it
when the variant is chosen in the lobby. Only the host executes it;
guests fetch it for its display part (name, option labels, message
strings). A film stores the blob, so a replay needs no server.

**Limits.** IR at most 256 KiB, source 256 KiB, VM state 64 KiB in
`SimExt`. A whole Megalo variant is about 20 KB ("Prior art"); the
larger cap pays for names and strings.

## Which engine index a script rides

A new script variant uses index 6 when its retail settings layout is
Slayer and none of its settings write the type area. This includes a
variant that `uses slayer`. Other variants retain their layout's retail
index. These four retail paths need a native answer for index 6:

| Path | What it does with an unknown index | Evidence |
|---|---|---|
| Engine table `0x55e4d0[index]` | no engine object; `0x1574c0` writes 0 to `E+0xc14` | **E**, [bots.md](bots.md) section 3.7 |
| Record clean-up `0x19d650` | resets the record to default Slayer unless the index is 1..4 or 7..9 | **E**; **Code**, `variants::record::check` ([variants.md](../editor/variants.md)) |
| Wire codec decode `0x7d520` | decodes the type area per engine through a jump table, then runs the clean-up; a changed record is a failed decode | **E**, game-options.md |
| Lobby protocol map `0x161c90` | synchronous protocol for the two deprecated indexes and anything out of range | **E**, [live-coop.md](../live/live-coop.md) |

Index 6 (`rules::SCRIPT_INDEX`, `variants::GameType::Script`) uses a
host-owned engine object, native clean-up and wire codec, and the
distributed protocol. It has common settings and an empty type area.
The variant store accepts it with a named script. Saves and editor pages
use the Slayer template, and the script header supplies the menu row and
round-start line.
[Engine paths](game-types-without-carriers.md#no-retail-engine-object).

**Design.** The script names its game type (`type`), and the type's row carries the index
([game-types-without-carriers.md](game-types-without-carriers.md#the-header)):

- **A new mode uses the Slayer settings layout.** The script index
  carries the common settings and the reference to its rules.
- **A port rides its own engine's index.** A saved CTF variant stays a
  CTF record, its type-area settings keep their meaning and their place
  in the editor, and the client keeps that engine's replicated globals
  entity (types 0 to 5, **E**, [notes/ai-networking.md](notes/ai-networking.md))
  for whatever its HUD draws from it.

On the host, a script game on the script index runs on an engine object
whose rule slots are empty, so the script is the game's one scorer ("One
scorer", under "The host runtime"); on another retail index the carrier's
own routines score beside it. The script's handlers run beside the
engine's other work and make their calls at the next tick boundary. No
guest memory is patched but the record the game starts.

The engine table `0x55e4d0[index]` holds one static object per engine,
and the object's `+0` is its vtable: CTF and Assault `0x459d18`, Slayer
`0x45c6d8`, Oddball `0x45c7a8`, KotH `0x45c878`, Juggernaut `0x453d38`,
Territories `0x45c948`, 51 slots each. Every slot is thiscall (ECX the
object, stack arguments the callee pops). The slots that make rules
(**E**, [notes/2026-11-21-engine-vtables.md](../engine/notes/2026-11-21-engine-vtables.md)):

| Slot | Called from | What it does | Named points |
|---|---|---|---|
| `+0x78` kill callback | last in `PLAYER_KILLED` (`0x15d6a4`): attacker, victim, flag, kind; `ret 0x10` | Slayer's and Juggernaut's kill scoring; Oddball, KotH and CTF post events only; Territories does nothing | `SLAYER_KILL` `0x2bc000`, `JUGGERNAUT_KILL` `0x1922a0` |
| `+0x48` per tick | `ENGINE_UPDATE` `0x15c150` (`0x15c2dc`), no arguments | Oddball, KotH and Territories score from it; Slayer's is the shared empty `ret` | `ODDBALL_TICK`, `KOTH_TICK`, `TERRITORIES_TICK`, `JUGGERNAUT_TICK`, `CTF_TICK` |
| `+0x3c` per player | `ENGINE_UPDATE` (`0x15c247`), the player | CTF and Assault capture from it (`0x242e00`); Slayer's and Territories' are empty | none |
| `+0x54`, `+0x58`, `+0x5c`, `+0x60` | objective registered, unregistered, picked up, dropped (`0x15e050`, `0x15e130`, `0x15e250`, `0x15e460`) | the objective engines' own handling | none |
| `+0x6c` | `PLAYER_KILLED` and the spawn-location code: two teams | true when they are enemies; Juggernaut has its own | none |
| `+0x8c` trait query | per player in `ENGINE_UPDATE` (`0x15b28e`): player, trait | true when the variant gives the player the trait | `ENGINE_TRAIT_QUERY` `0x24dda0` (Slayer's, and every other engine's fallback) |

The round resolution `0x15b3a0`, the lives checks and `PLAYER_SPAWN`
make no slot call. A Slayer carrier's tick has no body of its own to
answer, since its `+0x48` and `+0x3c` are bodies every engine shares; its
per-tick handlers answer at `ENGINE_UPDATE`. How much each engine's
client-side display depends on its own vtable rather than on replicated
state is not traced ("Not verified"); slot `+0x9c` gives the replicated
engine-globals entity's type (Slayer 0 to Juggernaut 5).

**A record with no mode.** On a host with the `rules` feature on, a
record whose block has mode 0 (all zero, or extras only), on an engine
in `runtime::PORTED`, runs that engine's built-in script, chosen by
engine index from a table compiled in. An engine is in `PORTED` when this
host delivers every event its built-in answers and makes every entry it
calls: Juggernaut (7). No port has had a parity run. Saved variants, hopper slots
and copies an older build edited are script games with no stored byte
changed: the game starts one of a type with a built-in (CTF, Oddball, King,
Juggernaut, Territories, Assault) on the script index with the built-in's
reference in its block ("One scorer"), so the start below reads it as a
record of mode 2.
The host puts the built-in's hash in the session's `rules` field as for
any script, and a film records which ran: its `simulation_hooks` SHA
differs with the feature and it stores the blob. The rule covers Live,
local, dedicated-host and System Link games: the built-in scripts are
compiled in, so a System Link game needs no blob and no session field.
A System Link game runs any other script only when its blob is on the
host's disk, in the game-variant store ("Live game variants in the
game"); it never fetches one.
