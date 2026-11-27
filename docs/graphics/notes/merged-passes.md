# Merged passes: evidence

Evidence for [graphics.md](../graphics.md), "Merged passes". Nothing here
states what the code does today; the design page does.

| Date | Commit | Result |
|---|---|---|
| 2026-11-15 | 38cd3f7 | The exact merge rule (a render pass of its own for a draw whose program can `discard`, and for a draw after a partly masked one with a different mask) took The Heretic's `queue submit` from 2.1 to 7.1 ms a frame between 9a39ed7 and 1a5aa8d. `d3d/tests/draw_churn.rs` (600 textured draws with level-like churn) goes from 1 render pass a frame to 200. |
| 2026-11-15 | 9a39ed7 | Opt-in merged passes: 1 render pass in the same bench. The later commits in the range (compile workers, render thread, W-buffer) change neither count nor cost. |
| undated | not recorded | Merging is worth about 6 fps at the main menu against about 2.5 with one render pass per draw. |
| undated | not recorded | With both exact rules, a menu run to present 560 with a frame every 20 gives the same bytes merged and per draw on all 29 frames. |
| undated | loose rule (not recorded) | Old Mombasa (presents 4000-7000, 1x): a 1000-draw frame goes from about 193 render passes to 41, a 2000-draw frame from 297 to 43; render-thread encode plus submit at 2000 draws goes from about 28 to 16 ms. 149 of the 193 splits were discarding draws after a partly masked one (`pass_discard` in the frame stats). `H2_GFX_MERGE_CHECK` at frame 5000 (970 draws) found two passes off: 1 pixel by 1 level (draws 836-837) and 11 pixels by up to 3 (draws 897-898). |

Cost of a render pass: wgpu 30 encodes each as two Metal command buffers (a
"Pre Pass" one before every pass) and a render encoder, about 40-75 us of
CPU each on a loaded machine, most of it in Metal
(`renderCommandEncoderWithDescriptor`, command-buffer commit and teardown).

## The two cases the exact rule came from

| Case | Frame | Draws | Difference | Trigger |
|---|---|---|---|---|
| Main menu, additive strip | 480 | 594, with draw 324 in the same render pass | up to 91 levels on 32 edge pixels | one fragment of draw 324 at (256, 63) (not one of the differing pixels); a built-in-shader quad there triggers it only with blending on, a passing depth test and the alpha write masked |
| Two additive draws of one mesh | 460 | 635 and 636 (depth LessEqual, not written; 635 writes RGB only, 636 writes all four and has no `discard`) | 119 pixels, up to 110 levels | starting a new render pass at 636 makes it exact |

Draw 594 sampling at an explicit level 0, or without its `discard`, gives no
difference, though its `discard` is never taken (no kill source). Re-setting
every piece of render pass state before each draw changes nothing, so the
cause is not state left bound in wgpu or wgpu-hal. Both cases have an
earlier blended draw with the alpha write masked, and implicit-LOD sampling
at triangle edges shows it.

Random synthetic scenes (blended triangles, D16
and D24S8 depth-equal tests, MSAA, a discarding textured strip after a
blended triangle) do not reproduce it, so the regression tests pin the
rules, not the GPU behaviour. The rules were found on those runs and are not
proven in general.

Reproduce: `H2_GFX_MERGE_CHECK=<frame>` (headless `h2 run`).
