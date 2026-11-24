# Map-load memory

Evidence for [native-renderer.md](../native-renderer.md) and
[graphics.md](../graphics.md).

## 2026-11-24

### Campaign with capped caches and decals

**Run.** Build `00671a7`, campaign `05b_deltatowers`. The watchdog
reported sustained growth from 4.87 to 5.03 GB around presents 7,500–8,400.
The decal pool was at 208 of 1,024 with about 31 draws per present. The
rate fell from about 30 fps to 12–14 fps at presents 9,200–9,700, with
about 15 decal draws per present. The native world loaded BSP 3 and BSP
1. The transparent binding log advanced by about 1,800 draws per 100
presents, and the light log reported zero host plans in 1,800 decisions.

**Source audit.** The transparent log counts accepted draw calls, including
cache hits. Its increment does not measure retained memory.
`native_transparents_glue::Cache` bounded packed vertex bytes, while each
packed binding also held a stream key and vertex declaration. The BSP
switch invalidated the world key on this build. Some objects owned by
the old BSP still depended on later admission to clear their caches.
The decal measurements direct the memory investigation to the other
paths. They do not establish the cause of the frame-rate drop.

**Reproduction.** Repeat the same campaign route across BSPs 3 and 1.
Compare the world geometry, indexed-window and packed binding probes on
both sides of each BSP switch once the retire worker drains. Correlate
footprint changes and heap stacks with each load, and inspect the frame
timeline over presents 7,500–9,700.

### Menu and Live match

**Run.** Play run 2 used build `332125c` on an M3 Pro at 2560×1920
internal resolution. The user ran `h2 play disc/halo2.iso --live` on the clang
tier, reached the main menu, and joined a custom Slayer game on Cyclotron.
Process footprint rose from 0.14 GB to 2.5 GB at uptime 14–16 s, then from
2.9 GB to 5.9 GB at 122–126 s. It settled around 3.9 GB by 170 s. Peak was
5.96 GB. The report recorded 1,577 MB of Malloc Small, 815 MB of Malloc
Large, 949 MB of graphics footprint, 225 MB of IOSurface and 131 MB of
IOAccelerator. The pipeline cache probe counted 1,753 entries.

**Source audit.** On `332125c`, native world, object, viewmodel and
screen-effect workers could overlap their map reads and decoded images.
Viewmodel workers held those inputs while waiting for requests. Native
BSPs, screen effects and object models could own resources until another
draw asked for those stages, and the sun geometry registry had its own
strong references. The VFS held every rewritten map payload it had served,
including maps visited by directory enumeration. The pipeline cache held
every variant and compiler queue slots owned their inputs. These are code
paths that can contribute to the peak or retained footprint. The report
has no allocation stacks or subsystem byte counts to apportion either total.

**Other owners.** The bot navigation path exchanges the active mesh on
activation and releases the displaced mesh on the map-data worker
(`game/src/bot_glue.rs`, `bots/src/autonav.rs`). An active mesh build can
still overlap rendering. Theater builds keyframes and keeps an anchor
for deltas (`game/src/theater/record.rs`). The report records a tape-size
error at tick 1,448. Its timing alone does not establish which allocations
caused the load peak.

**Reproduction.** Repeat the same menu-to-match sequence and return to the
menu. Sample process footprint and the named cache probes through
[`host.memory`](../../runtime/agent-api.md#explain-memory-growth) across
activation and steady play. Collect heap stacks at the load peak and once
the footprint settles. Compare the return-to-menu counts after the
retirement worker drains and repeat the map switch to measure accumulation.
