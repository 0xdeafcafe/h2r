# Render-state list: skip rates

Evidence for [render-loop.md](../render-loop.md), "The render-state list". Run 2026-11 at commit 8b1ccc4bf; reproduce with `cargo xtask test -p engine` (the `states_differential` test needs the XBE and prints the lines below).

| Run | Result |
|---|---|
| Differential, 1,200 states | 10,583 calls equal (exact); skip mode skipped 574 of 5,100 Simple calls, same GPU state, shadow and other calls; 31 targets |
| `skipping_on_a_per_draw_workload`, one shader | 400 draws, 3,200 Simple calls in the original, 1,596 skipped (49.9%) |
| Same, four shaders sorted | 3,200 calls, 1,683 skipped (52.6%) |
| Same, four shaders interleaved | 3,200 calls, 396 skipped (12.4%) |
