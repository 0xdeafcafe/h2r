# Animation channel manager and graph lookup

Status: Six entries run as arms that answer from Rust once proven, always installed. Cumulative timing awaits verification.

The channel manager selects, advances and blends the channels on a guest
object. Its entry points and graph helpers are registered by
`crates/rewrites/engine/src/sim_anim/manager.rs`. Each body declares its
entry, instruction range and return selectors. The animation switch owns
these bodies, including graph resolution. The six phase 1d entries
install with or without the rest of the group, with no switch of their
own. See [switches.md](../runtime/switches.md).

## Calls and fallback

The arms cover channel advance (`1c66a0`), base-pose sampling
(`1c6f60`), channel selection for sampling (`1cd590`) and the three graph
entries below. At each entry and return selector, `manager::arm::turn`
picks who answers the segment. Until the entry has matched 4096 segments
(`PROVEN_AFTER`) with no difference, and on one segment in 64
(`AUDIT_EVERY`) after that, the guest answers: `manager::arm::predict`
evaluates the segment on a CPU copy and a private store log, the bridge
runs the generated selector once with direct calls routed through the
dispatcher, and it compares the transfer before the callee can change the
frame. Otherwise `manager::run` runs the segment and commits it, and the
guest does not run. A declined segment panics, naming the entry. One difference
returns that entry to the guest for the rest of the process. The audit
slots restart at map load, checkpoint revert and restore. The
differential tests call the lifted entries directly as the reference.

`sim_anim::manager_channels` computes the loop-frame interval, playback
rate class and channel mix on transient values gathered from the current
frame. Threshold reads follow the guest's comparison order. The adapter
reads the channel again after a callback. Channel advance can write the
packed clip at channel `+4`, playback flags at `+0x11`, event masks at
`+0x14` and `+0x16`, and speed at `+0x18`. It checks the saved clip
against the live channel before calling `1c6560` to commit the frame at
`+0x1c`. The comparison includes the stack slots holding the saved clip,
frame intervals, callback arguments and return addresses.

`manager::drive` follows calls, tail jumps and returns between the other
installed manager bodies while their hooks are enabled and unshared. Each segment
commits its CPU and ordered stores before entering the next. The channel
and graph records stay in guest RAM across callbacks, including the resource
request and RNG paths. A callback can replace a channel or change an alias
before the caller reads it again.

The bridge installs a body only when the image exports its entry and all
return selectors. A declined first segment panics
(`anim <entry>: <float|fault> at <pc>`). If a child declines after its
caller commits, the bridge hands the pending transfer and the caller's
committed frame back to the dispatcher, whose hook for the child then
panics the same way.
`sim_native::plan` indexes long store logs by physical address so reads
through guest aliases see pending channel copies.

## Graph lookup

`1daea0` resolves a packed clip reference, `1dd9d0` walks its preload
dependencies and `1ddb40` constructs the sampling view. The preload
arm decodes clip links and resource indices into a bounded Rust table.
It checks live residency as it follows links and stops at the first cache
request. The next segment reads the graph and current link again. Writes
through physical aliases and changes of guest view or simulation epoch
invalidate the table. Blocks with staged writes use the ordered walk.

The sampling wrappers consume the 12-byte view from `1ddb40`: the
resource data and sizes pointers, node count, frame-info type and frame
count. The view stores interleave with register pops, so writes through
an alias of the saved stack frame affect the returned registers.

For the named record search at `1dd560`, `manager/graph/lookup.rs`
indexes unique signed IDs in ascending blocks. The index tracks guest writes through
physical aliases and expires on a guest view or simulation epoch change.
Misses, ineligible blocks and ranges with pending stores use the original
search order.

## Verification

The read-only agent method `anim.manager` reports matched, differed,
interrupted and answered segments per entry and retains the
first difference. The live arm checks the transfer, CPU state (including defined
flags, x87 and XMM) and final bytes at predicted physical write addresses.
See the [agent API](../runtime/agent-api-checks.md#anim-the-animation-graph-walk).

The lifted differential `every_manager_entry_and_return_site` checks the
body inventory and return selectors against the original control-flow
graphs. At each reached boundary it compares transfers, registers, flags
and all fixture RAM, including bytes outside the predicted write set.
For the six arm entries it also checks that predicting leaves RAM intact
and compares the prediction to the lifted result. The chained lookup case also exercises a shared preload hook. The cumulative timing gate is in
[sim-rewrite.md](sim-rewrite.md).
