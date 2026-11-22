# Pipeline cache: measurements

Evidence for [graphics.md](../graphics.md), "Pipeline compiles (`compile.rs`)
and the pipeline cache" (the "Persisted keys, prewarmed" entry).
Nothing here states what the code does today; the design page does.

## 2026-11-21: the unbounded prewarm ran out of memory at boot

Lab jobs 0020 (commit `83f9c2f`) and 0022 (commit `105faaf`, `superbounce`,
`h2 play --level ascension --fps 30`), on the lab Mac. The game code had
run there the same morning without trouble; what had changed was the size
of the shared pipeline cache file, which every run on that machine had
appended to.

- 0022: the footprint grew from 23 MB to 8.59 GB in 19.2 s, about
  450 MB/s from the first second of `h2 play`, while the loaded level was
  still `mainmenu`. The watchdog stopped it at its hard limit. The growth
  was in `Malloc Small` (8268 MB of 8928 MB) and in mem-watch's
  `unscoped` tag (3.62 GB at 10 s, 6.95 GB at 20 s); `render.pipeline`
  held 12 MB.
- 0022's GPU census at 10 s: `wgpu.shader_module_count` 7,115,
  `wgpu.render_pipeline_count` 824, `gfx.pipeline_cache_entries` 819. At
  20 s: 16,966 modules, 6,983 pipelines, 6,978 entries.
- 0020: the same growth, about 440 MB/s, from 82 KB to 16.4 GB in 31 s,
  stopped by the 12 GiB memory limit; `Malloc Small` 16 GB.
- The game ran at about 1 fps, with pace waits of 1 to 3 s.

The reading. `PipelineStore::open` loaded every record the file had ever
held, of every build. The prewarm thread translated all of them as fast as
it could and sent each program with its `wgpu::ShaderModule` down an
unbounded channel, so modules ran far ahead of registration (7,115 against
824 pipelines at 10 s). A module made from WGSL keeps its parsed shader
until pipelines are built from it, about 400 KB a program by the numbers
above (6.95 GB over 16,966 modules), all allocated on the prewarm thread
with no mem-watch tag. Each present registered every program that had
arrived, and a draw whose program was still queued waited for the thread
to reach it, so frames stalled. Memory and startup time grew with how many
builds and maps the machine had ever run.

What followed: versioned records, compaction on open, prewarm scoped by
map and capped, a bounded backlog, a per-present registration budget, and
the `render.prewarm` and `render.translate` tags.

Reproduce: a lab run on a machine whose pipeline cache holds records from
many builds and maps, against a fresh cache:

```
h2 lab run --name "Prewarm, shared cache" --level ascension --mem-limit 10G
h2 lab run --name "Prewarm, fresh cache" --level ascension --mem-limit 10G \
  --env H2_PIPELINE_CACHE=fresh
```

Compare the two runs' `mem/` reports (`h2 lab fetch <id>`): the footprint
samples, the `gpu` census and the `render.prewarm` tag.

## 2026-11-21: the main menu after a level at 5 to 12 fps

Reported from a player's run on a 64 GB M3 Mac (a build of master near
`e0a0585`, `h2 play --level ascension`, 4x render scale): Ascension ran
at about 28 fps throughout. After quitting to the main menu the frame-rate
log read `present 14700: last 100 at 4.65 fps`, then 8.79, 9.02, 10.13,
11.57 and 12.03 fps over about 600 presents. The same menu booted fresh
runs at 28.5 to 30 fps (lab 0034). The menu's own load was quick
(`native-world: mainmenu BSP 0 ...: 167 of 180 materials native ... in
0.7 s`).

The reading, from the code at `e0a0585`. A map's scope queued up to
`PREWARM_PROGRAMS` programs' recorded pipelines on gfx's compile workers
at `Priority::Background`, about 70 ms of Metal compile each on an idle
machine and 220 ms under load, two to four at a time. A new scope
replaced what was left on the prewarm thread, but nothing took back what
was already on the workers' queue. Leaving a level early left its
remaining guesses queued ahead of the menu's, all compiling on the
game's cores. A menu draw whose pipeline was one of the menu's queued
prewarms found it in the renderer's cache, did not move it ahead, and
built it on the drawing thread at encode time (`wait_for_pipelines`).
The rate climbs as the queue drains. A fresh boot has no level backlog.

What followed: `Renderer::drop_prewarms` at each scope change
(`Compiler::prewarm` tags each prewarm with its renderer), the scope's
registered programs' pipelines queued again, and a draw that hits a
queued prewarm moving it to draw priority. The menu after a level is a
scope of its own (`next_scope` in `nv2a.rs`).

Not confirmed by a run yet. Reproduce with
[`game.menu`](../../runtime/agent-api.md#returning-to-the-main-menu),
on the fix (branch `menu-return-slow`) and on `e0a0585`, and with the
prewarm off as the control:

```
h2 lab run --host laptop --name "Menu after Ascension" --level ascension --serve --env H2_RENDER_SCALE=4
h2 lab agent --host laptop <id> wait '{"phase":"gameplay","map_ticks":3600}' 300000 --timeout 400
h2 lab agent --host laptop <id> game.menu
h2 lab agent --host laptop <id> wait '{"map":"mainmenu","map_ticks":900}' 300000 --timeout 400
h2 lab stop --host laptop <id>
```

Compare the `present N: last 100 at X fps` lines after the return in each
run's log, and, with `--env H2_FRAME_STATS=1`, the `pipelines` counter
(pipelines the drawing thread built or waited for). Add `--env H2_PREWARM_PROGRAMS=0` on
the old commit for the control: if the menu is fast there, the prewarm
backlog was the cause.
