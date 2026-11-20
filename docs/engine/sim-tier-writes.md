# Writes a tick can make outside the arena

The sim tier's rollback ([sim-review.md](sim-review.md) section 4.2)
restores the arena when a tick hits a rare event. That is only sound if
every byte the tick can write is in the copy. This page lists every store
the tick's code can make, by where it lands, so the state outside the
arena can be named and added to the copy.

## Method

`emit::sim::census` (`crates/emit/src/sim/census.rs`) runs over the lifted
IR of a closure: `game_tick` (`0x00137fe0`) and everything it reaches
through direct calls and tail transfers. Each function is interpreted
abstractly, joined at blocks, and every store's address is classified:

| Class | Meaning |
|---|---|
| absolute | a constant address: a global, with its XBE section |
| indexed global | a constant base plus a computed index: a global array |
| field of a root | a known offset from a pointer read once from a global, a stack argument or an entry register |
| reachable from a root | deeper in an object graph that starts at one of those |
| stack | the thread's stack |
| fs | thread-local storage |
| unknown | nothing the pass can name |

After a direct call, the registers the callee's recovered signature may
write become unknown, and ESP moves by the return address and the
callee's `ret n`. Indirect calls and jumps are listed, not followed; their
callees are outside this census, and the list is the coverage gap.

The arena is allocated at run time, so whether a "field" or "reachable"
store lands inside it is a run-time question. This page names the roots;
checking each root's target against the arena range is the next step.

## Running it

```sh
H2_XBE=/path/to/default.xbe cargo test -p emit --release --test sim_tier_writes -- --ignored --nocapture
```

It prints the census as markdown and writes it to
`target/tmp/sim-tier-writes.md`, with how much of the tick the sim tier
compiles and why the rest is refused, and the whole image's store classes
for scale.

## Output

The census of a run goes here, with the commit it came from.
