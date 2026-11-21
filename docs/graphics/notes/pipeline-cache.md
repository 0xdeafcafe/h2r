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
