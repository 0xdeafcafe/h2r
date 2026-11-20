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

## From `h2 play` to the main menu

- **Before guest entry.** With a current module, play opens the window and devices.
  Boot maps the XBE, loads the native library, checks its ABI and registers the
  entry and resume tables. One recorded run spent 2.5 s in this interval for
  18,854 entries and 64,140 resumes. That timer includes unread-target setup.
  Fallback analysis overlaps boot when a target list exists. Boot then mounts
  the VFS, prepares map offers and the campaign catalogue, and installs HLE
  and feature hooks. The bot nav scan already runs on a worker.
  [Recorded startup log](../../crates/cli/src/coop_verdict/host-at-menu.log)
- **First VFS access to mainmenu.** Boot reads installed campaign PNGs. The
  first stat, directory listing or read that needs the rewritten map builds
  its picture tags. The reported ten-card rebuild took about 1 s. Subsequent
  boots hash the actual source map and ordered cards, then restore the verified
  map and tag IDs from `<data>/cache/campaign-cards`.
- **Guest loading screen.** The title formats N: (including mainmenu) at boot.
  Shared slots on Z: survive subject to the title’s checks. On a miss, the
  guest copies and checksums the payload, flushes it and publishes the header.
  A fresh-data **level** boot at `26f45a90` drew 1,744 loading frames for
  838 MiB before batching. Foreground payload copies for slots 0–2 can now run
  128 steps between presents, previously 16. The title still loads tags and
  uploads resources. Its movie wrapper controls Bink frame waits, audio and
  skipping. [Cache copy measurements](../runtime/cache-slots.md#copy-speed-on-the-loading-screen)

For a menu A/B, start from equivalent data roots with the same module, disc,
content and movie settings. Capture the startup log and use `h2 agent loads`
with frame stats enabled. Compare the boot stall's duration, presents,
`read_bytes` and `file_io_us` with and without `H2_FEATURES=-map-copy.boot`.
The agent's interval starts when the agent starts. Time launch separately to
include the earlier host setup. [Load timeline](../runtime/agent-api.md#load-timeline)

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
