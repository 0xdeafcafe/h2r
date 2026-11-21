# Creature replication in a spare entity slot

Static evidence for the [AI networking design](../ai-networking.md#creature-entity).

## 2026-11-21 — Retail registration, replication and admission

**Verdict.** Use entity type **17** for AN12. Both ends of the retail
creation codec can represent it. The host scheduler has tuning for
types 0 through 16. AN12 needs creature tuning and a way to enroll
a host-created creature. **Hold AN13 until P7 demonstrates replication
and AN12's mixed-version admission tests pass.**

**Reproduction.** Read at `f563722220f6` on the owner's retail Xbox
image (title `4D530064`, certificate version 3). Set `H2_DISC` to
the local retail `default.xbe` and use the repository's `h2 disasm`.
Start with the commands below. For each path, follow the named guest
addresses and the indicated branch or virtual call. Counts on the
command line are instructions. All observations here are static.

```sh
h2 disasm 0x67e30 36
h2 disasm 0x82240 48
h2 disasm 0x8a460 90
h2 disasm 0x8a6f0 190
h2 disasm 0x97d80 130
h2 disasm 0x6f2b0 60
h2 disasm 0x94100 28
```

### Registration and table walks

`0x67e30` clears two arrays of **32 pointers each**. It calls
`0x82240`, which fills 17 entity and 26 event entries, then stores
the returned counts. The entity count is at registry offset `+0` and
its pointers occupy `+4` through `+0x80`. The 32-bit field at `+0x84` holds
the event count. Event pointers start at `+0x88`. The event reader at
`0x8b110` checks its index against that second count before dispatch.
This accounts for the apparent extra pointer in the earlier table read.
**15 entity slots are free, indices 17 through 31.**

Store a pointer to a guest-callable definition object at `+0x48` when
the types are populated. Return an entity count of 18 and an event
count of 26. This extends the occupied prefix without a hole. The entity
allocation helpers (`0x8af70`, `0x8b010`) index the pointer directly.
The creation decoder checks the addressed pointer for null. None of
these paths searches for the last definition, walks to a sentinel or
uses the returned count as the wire limit.

`0x67ee0` passes the registry to each new simulation world.
`0x68c00` installs the pointer at `+0x10` in both distributed databases.
The reset and dirty-state loops at `0x8a030` and `0x8a090` walk
**1,024 entity instances** at a stride of `0x20`. An empty instance has
identity −1. Live records carry their own type at `+4`. The identity
refresh at `0x8ad70` uses that type for its callback. Object lookups
also dispatch directly. `0xa58d0` checks the full identity, asks the
definition whether it has an object, then returns the local handle.
Object callbacks at `0xa9894` and `0xa9a5e` reach beyond the base
entity interface. A creature needs the object methods its callers use as
well as a matching type getter, sizes, masks, codecs and lifecycle methods.

### Host enrollment and the scheduler limit

`0xa7870` enrolls objects on the distributed host. Its helper
`0xa7970` skips already-bound objects and calls the selector at
`0xb5990`. The selector accepts object kinds up to 11. A creature
(kind 12) returns −1. For one eligible object the host allocates the
entity at `0xb57d0`, binds its identity to object `+0xd4` at `0xbb780`,
then builds its initial state. P7's `sim.spawn` calls `object_new`
directly in `crates/extensions/agent/src/sim/calls.rs`. A retail spawn
at `0xbf179` explicitly follows allocation with enrollment at `0xbf19f`.
AN12 must cover that distinction so a spawned creature gets one entity.

**The definition registry and scheduler have different capacities.**
Creation relevance at `0x8a460` locates its tuning record by multiplying
the stored type by `0x4c` and adding `0x4cef68`. Update relevance at
`0x8a9c0` calls `0xabac0`, which reads the update fields of the same
record. For type 17 the computed record starts at `0x4cf474`.
`0x66b7f` initializes this address and `0x971ff` reads it as an
elapsed-time threshold. AN12 must route creation and update relevance
to valid creature tuning while preserving those settings. The generic
object's callback at `0xa0890` computes the same address independently,
so reusing it also requires a creature tuning lookup.

### Creation, baseline, update and deletion

The single-entity writers for creation (`0x98620`), update (`0x989f0`)
and deletion (`0x986d0`) all use the same identity writer at `0xb5650`.
Creation and update then dispatch to the entity database. Deletion has
no type-specific payload. `0x98aa0` writes the operation terminator.

- **Creation.** The host at `0x8a400` selects the definition from the
  stored type, captures creation data (`+0x48`), builds baseline state
  and marks all defined fields dirty. The writer at `0x8a5c0` emits an
  unsigned **five-bit type**, calls the creation encoder (virtual offset
  `+0x30`), then can add an initial
  delta from the definition's initial-update mask (`+0x14`). All indices 0–31
  are representable. The field has no reserved end marker. The reader
  at `0x8a6f0` obtains the state size (`+8`) and creation size (`+0xc`)
  from the selected definition. Those are in-memory sizes (for the generic
  object, `0x90` and `0x14` bytes). The staged record stores each size in
  a signed 16-bit field. The reader decodes creation (`+0x34`) and builds
  the baseline (`+0x4c`).
  If an initial delta is present it decodes it (`+0x3c`) and checks the
  returned bits against the initial mask. On commit, `0x8a920` stores
  the identity and type and calls the replica creation method. The
  retail object adapter at `0xa5a60` binds the local object and applies
  its initial state. The creature definition must resolve a local
  `crea` tag before creating that replica.
- **Movement and other changes.** `0xa7a30` forwards dirty flags for
  a bound object. `0x8a090` asks its definition to collect changed
  state. `0x8aa50` calls the update encoder (`+0x38`) and requires a
  nonempty emitted mask. Updates carry an instance identity (10-bit
  slot and four-bit generation). The client at `0x8aac0` checks the
  full identity, recovers the stored type, decodes into a temporary
  copy of current state (or a rebuilt baseline in bitstream mode 4),
  and commits and applies it at `0x8ac40`. The baseline comparison at
  `0x8add0` has `0x400` bytes of state scratch. Its all-fields mask
  uses a 32-bit shift by the flag count. Keep state within that scratch
  and the count below 32. Validate ordinary masks in the creature codec.
- **Shared field codecs.** The generic object writer at `0xa3cd0`
  passes ten object flags to `0xa69a0`. The paired reader at
  `0xa0960` uses `0xa6d50` and checks stream capacity. A position-only
  probe can use the paired position helpers `0x86d20` and `0x86e40`.
  Reusing the complete object wrapper also requires its ten presence
  bits and compatible state layout. `0xa7180` compares object fields
  and `0xa6430` applies them. The creature's own tick and post-physics
  callbacks (`0x118700`, `0x118b80`) need a replica policy to preserve
  host authority.
- **Deletion and reset.** Deletion carries the identity. `0x98fb0`
  checks the peer view. For a live replica `0x8ace0` calls the stored
  definition's removal method. `0x8ad20` releases the record through
  `0x8aed0`. On the object side `0xa7a60` removes the association and
  clears the back-link. Old-map teardown at `0x67f60` disposes of the
  world. The distributed transition at `0x68045` also bulk-unlinks
  objects. The definition survives a map change and its pointer must
  stay valid until simulation shutdown at `0x67eb0`. Creature handles
  and per-entity counters belong to the instance lifetime.

### What an unextended receiver does

A stock client **rejects the replication batch** containing a type-17
creation. At `0x8a6f0` it reads the five bits and finds a null pointer.
The branch to `0x8a910` returns status 3 before allocating creature
state, calling a codec or installing the identity. The operation reader
at `0x98cf0` propagates the error. `0x97d80` skips the apply pass for
the whole batch and releases temporary records, including ones staged
before the unknown creation. The channel reader at `0x88750` stops.
The enclosing datagram handler at `0x93560` may process other work.

A later movement update has no type field. With the creation missing,
the peer-view identity/state check at `0x98e74` returns status 2 before
calling the update decoder. A stale database identity also fails at
`0x8aac0`. A *different definition in an occupied slot* is a separate
compatibility risk. Each codec advances a shared bit cursor, and the
next operation starts where it leaves off. The operation terminator
ends the stream. There is no per-entity byte length or schema identifier
to detect two implementations that disagree about an occupied slot.

### What session admission establishes

- **Joining console.** `0x64d70` builds the description used by System
  Link discovery (`0x9406c`) and Live QoS (`0x73901`, decoded through
  `0x7c530`). `0x6f2b0` checks discriminator 4 and that the advertised
  build interval includes the local build, `0x2651`. Retail advertises
  that build at both ends of the interval. A mismatch sets join reason
  9. LAN discovery at `0x93fa0` and `0x940b0` also checks its
  discovery version and matches the reply nonce.
- **Host.** Join message 8 carries a 16-bit message version. The codecs
  are `0xacc20` and `0xacfa0`. `0x94100` requires version 2 before it
  looks up the session ID. A mismatch returns silently. `0x63080`
  passes the inner request to `0x62fe0` for session state, privacy,
  reservations and capacity. This inner hook has already lost the
  version header. Accepted requests can also reach membership through
  the deferred join path at `0x6d4c0`.

These checks can admit peers with different entity definitions. None
compares a replication version, count or codec. h2recomp also publishes
build and capability fields in Live presence and permits older builds to
join under that policy. [Game options](../game-options.md)

AN12 needs matching replication versions before admission on both
transports, including direct, invited and queued joins. Give the probe
and full codecs distinct versions. A missing or unequal version must
give a named incompatibility error before entity traffic.

**P7 remains open.** Compare a matched control pair with a matched
slot-17 pair on System Link. Verify one host enrollment, matching entity
identities with a local creature on each peer, at least one applied
movement update, and continued stock player movement. Record aggregate
traffic against the control. Mixed-version refusal belongs to AN12's
lobby tests. The full-definition bandwidth target belongs to AN13.
