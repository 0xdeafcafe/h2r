# Pipeline compile cost

Evidence for [graphics.md](../graphics.md), "Pipeline compiles
(`compile.rs`) and the pipeline cache". Nothing here states what the code
does today; the design page does.

## 2026-11-15: where a first-time pipeline spends its time

Method: `crates/platform/d3d/tests/pipeline_cost.rs` (ignored) over 24 retail
vertex programs from a local `h2 extract-shaders` corpus, each with a
pass-through combiner, entry points renamed so Metal's own cache cannot hold
them; release build, load average about 75.

| Step | ms per program |
|---|---|
| `vsh`/`psh` translation, link, naga validation (ours) | 2.3 |
| `create_shader_module` (wgpu parses and validates the WGSL) | 1.4 |
| naga's MSL back end, both stages (wgpu-hal runs it per pipeline) | 1.9 |
| `create_render_pipeline` in total | 70.7 |
| of which the Metal compiler (`newLibraryWithSource`, `newRenderPipelineState`) | 68.7 |

About 97% is Metal's. The 220 ms per pipeline in
[gameplay-budget.md](../../perf/notes/gameplay-budget.md)'s worst frame is
the same compile on a busier machine with real combiners. On 4 threads the
same 24 take 0.70 s of wall time against 1.99 s on one, so the compiler
scales across threads. wgpu 30's pipeline cache is a no-op on Metal
(`Features::PIPELINE_CACHE` is Vulkan-only).
