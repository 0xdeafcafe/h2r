# D3D caches: measurements

Evidence for [d3d-caches.md](../d3d-caches.md) and [d3d-visibility.md](../d3d-visibility.md). Nothing here states what the code does today; the design pages do.

## Dated measurements

| Date | Commit | What | Result | Reproduce |
|---|---|---|---|---|
| 2026-11-16 | `5b8cd780a` | vertex cache, 1000 draws a frame, about 45 MiB of vertex windows, cache off vs on | 3.05 vs 1.33 ms/frame (render thread, guest side: 4.17 vs 1.84); 0 bytes copied a frame instead of 46,165 KiB; no allocation of 4 KiB or more instead of 0.75 a draw | `cargo bench -p d3d --bench streams` |
| 2026-11-16 | `5b8cd780a` | same, one of four vertex buffer sets rewritten every frame | 3.32 and 1.81 ms, 11,231 KiB copied a frame, no snapshot taken after warm-up | `H2_BENCH_DYNAMIC=1 cargo bench -p d3d --bench streams` |
| 2026-11-16 | `b422e14ea` | `GetVisibilityTestResult` waiting for every count, with the render thread, Old Mombasa | 8.3% of the game thread (profile at `183781e`) | profile the game thread in that level; `cargo bench -p d3d --bench visibility_wait` prints the guest's time in the call with a lagging render thread, waiting and polling |
