# Animation channel manager and graph lookup

Status: Implemented. Differential parity and cumulative timing await verification.

The channel manager selects, advances and blends the channels on a guest
object. Its entry points and graph helpers are registered by
`crates/rewrites/engine/src/sim_anim/manager.rs`. Each body declares its
entry, instruction range and return selectors. The animation switch owns
these bodies, including graph resolution. See [switches.md](../runtime/switches.md).

## Calls and fallback

`manager::drive` follows calls, tail jumps and returns between installed
manager bodies while their hooks are enabled and unshared. Each segment
commits its CPU and ordered stores before entering the next. The channel
and graph records stay in guest RAM across callbacks, including the resource
request and RNG paths. A callback can replace a channel or change an alias
before the caller reads it again.

The bridge installs a body only when the image exports its entry and all
return selectors. A declined first segment uses the generated selector.
If a child declines after its caller commits, the bridge hands the pending
transfer and the caller's committed frame back to the dispatcher.
`sim_native::plan` indexes long store logs by physical address so reads
through guest aliases see pending channel copies.

## Graph lookup

`1daea0` resolves a packed clip reference, `1dd9d0` walks its preload
dependencies and `1ddb40` constructs the sampling view. For the named
record search at `1dd560`, `manager/graph/lookup.rs` indexes unique
signed IDs in ascending blocks. The index tracks guest writes through
physical aliases and expires on a guest view or simulation epoch change.
Misses, ineligible blocks and ranges with pending stores use the original
search order.

## Verification

`every_manager_entry_and_return_site` compares the body inventory and
continuations to the original control-flow graphs. It compares transfers,
registers, flags and RAM at each reached boundary. The chained lookup case
also exercises a shared preload hook. The cumulative timing gate is in
[sim-rewrite.md](sim-rewrite.md).
