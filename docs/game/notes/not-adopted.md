# Game: not adopted

Evidence for the game pages under [docs/game](../). Designs that were
built and removed, one row each, so that the next lane does not build
them again.

## Removed

| What | Why it went | Where it was |
|---|---|---|
| The superbounce HUD as a host overlay: a glass panel at the right of the game image (translucent dark blue, clipped corners, cyan edges, a glow), drawn on the CPU in design units with `font8x8` letters, a launch gauge, an arrow to the nearest spot, and four tutorial cards with diagrams, F4 to move a card on | Not Halo 2's HUD: invented art, fonts and colours over the game image, outside the game's HUD frame, split screen and aspect handling. Replaced by the HUD kit's elements, which the game's own HUD draws ([hud-kit.md](../hud-kit.md)) | `crates/apps/app/src/superbounce_hud.rs`, `crates/apps/app/src/hud_shapes.rs` and `h2 superbounce hud-preview`, removed on branch `sb4-hud` (parent `a7b682cde`) |
