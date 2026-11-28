# Players before and after: what a Rust body has to cover

2026-11-28, base `2b40b4bbf`. Read from `h2 disasm <disc> 0x14d3a0` and
`0x14d8b0`. Nothing here is measured; the ledger times are the 16-bot
`sim.ledger` figures (players_before 0.16 ms, players_after 0.14 ms), which
include every guest callee.

Why neither phase is a one-lane body: each is a skeleton over the 16-slot
player datum array (`[0x4e8c24]`, stride `0x21c`, used bitmap at `+0x48`)
whose time sits in its callees, not in the loop. A Rust skeleton that calls
the same guest callees (the shape of `effects_phase`) would save the loop
alone. The ledger moves only when the callees go too.

## players_before `0x14d3a0` (stack record, `ret 4`)

| Part | Callees (guest) | Notes |
|---|---|---|
| machine-in-game pass | `0x14d2a0` (not with a client, `[0x4e6948]+0xc` = 4), event `0x2bbfa0` | skipped on a distributed client |
| spawn pick | `0x15dd40` per candidate, `0x14f2e0`, `0x15ed60`, `0x14f0f0` | one random draw (`[0x4e7408]` LCG, `idiv` by the candidate count) per tick with candidates |
| rules by mode (`[0x4e6948]+8` = 1 or 2) | `0x138820`, `0x1388a0`, `0x14cd80` | co-op respawn walk `0x14cdd0` runs every tick |
| respawn timer | `cvtsi2ss`, `mulss`, `fistp` into `player+0x164` | x87 `fistp`, rounding from the control word |
| per-player control walk (from `0x14d5d8`) | `0x14d040`, `0x14f6f0`, `0x1e6b00`, `0x14f820`, `0x1505f0` (a seat), `0x150790` (a seat), `0x162c50`, `0x2259f0`, `0x2259a0`, `0x162d00`, `0x0a9400` | the two seats already run in Rust; the rest are guest |

## players_after `0x14d8b0` (no argument, `EBX` unused)

| Part | Callees (guest) | Notes |
|---|---|---|
| per player, dead or spawning | `0x176780`, `0x0cc460`, `0x0b7360`, `0x0b58c0`, `0x0a7c50`, `0x19eb30`, `0x0d6660`, `0x0d66d0`, `0x0d7b80`, `0x153cd0`, `0x153bd0`, `0x1521d0`, `0x1508e0`, `0x1e6bd0` | the respawn timer `+0x16c` against `[0x510c54]+4`; the death event is built on the stack (`0xa7c50`, `0x19eb30`) |
| BSP switch volumes | `0x11c470`, `0x12b850` | `scnr+0x130` volumes of `0xe` bytes against the unit's position |
| tail | none | the switch cooldown at `players_globals+0xa4..0xa8` |

## First slice that moves the ledger

Take the leaves first, each as a kernel arm of the `seats.leaf` pattern, then
the skeleton: `0x15dd40` and `0x1508e0` (called per player), then the
`0x153cd0`, `0x153bd0` and `0x1521d0` timers, then the BSP switch walk
(`0x11c470`, `0x12b850`). The two phase bodies follow once each phase's
guest calls are the few that are left.
