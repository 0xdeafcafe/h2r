# Performance

The target is the retail game running correctly at 30 presented frames
per wall second (33.3 ms a frame) in menus and in levels, with margin
under that line. The docs here are measurement logs: every number has a
date and a commit next to it, and later work has moved most of them.
Re-measure before acting on any row.

## Which doc answers what

| Question | Doc |
|---|---|
| How do I profile, trace or bench something? | [profiling.md](profiling.md): `h2 profile`, `h2 bench`, the timeline, test and bench knobs |
| Where does an in-level frame's time go, and what are the worst frames? | [gameplay-budget.md](gameplay-budget.md) |
| Where does a main-menu frame's time go? | [menu-budget.md](menu-budget.md) |
| What structural changes would close the gameplay gap, and which landed? | [architecture-review.md](architecture-review.md): ranked changes, each with its status |
| How do generated code and the runtime touch guest memory, call and exit? | [guest-memory.md](guest-memory.md): the inline RAM path (`H2_FASTMEM`), direct calls, the exit call, register homes, runner transfers |

Read profiling.md first, then the budget for the scene you care about,
then the architecture review for what is planned.

## Experiments not adopted

- [fastmem-4k.md](fastmem-4k.md): 4 KiB guest translation (branch only;
  slower than the inline path)
- [fastmem-proto.md](fastmem-proto.md): Mach exception write watch
  (`crates/fastmem-proto`)

## Elsewhere

Performance work that lives with its area:

- [build-perf.md](../lifter/build-perf.md): build times and the caches
- [x87-native.md](../lifter/x87-native.md), [sse-native.md](../lifter/sse-native.md),
  [x87-transcendentals.md](../lifter/x87-transcendentals.md): floating point
- [render-thread.md](../graphics/render-thread.md),
  [pipelined-render.md](../graphics/pipelined-render.md),
  [occlusion-culling.md](../graphics/occlusion-culling.md),
  [layer-draw.md](../graphics/layer-draw.md): rendering
- [high-framerate.md](../game/high-framerate.md): 60 and 120 fps
