# The game view: cost and latency of the picture

Evidence for [game-view.md](../game-view.md) ("Frames" and "Attached
mode"). Nothing here states what the code does today; the design page does.

## Not yet measured

The Game panel in attached mode (GV2) was written without running a game, so
the readback's cost at 60 Hz and the panel's latency have no numbers yet. A
run fills them in as a new dated entry above this one, with the commit,
the machine and the game's tier.

**Method.** Both ends report what is needed; no new hook is required.

1. Cost on the game. Through `h2 lab` on the game's machine, take the frame
   time twice: with no picture open, and with the
   panel open on the paired game at the default 1280 wide, 60 fps ring.
   The difference is the readback's cost. Repeat with the panel hidden:
   the game stops reading frames back a second after the newest one goes
   unread, so the third run should match the first.
2. Rate and drops. With the panel drawn, `game.status` on the editor's socket API reports `picture.fps` (frames taken a
   second), `picture.dropped` (frames the game published that no poll took)
   and `picture.events` (the `picture` events sent). Over a minute,
   `dropped` over `frames + dropped` is the share the panel missed.
3. Latency. `picture.tick_lag` is the game's tick at the link's last poll
   (every 250 ms) minus the tick in the newest frame, so it has up to one
   poll interval of noise. For a tighter number, pause the game
   (`pair.control`), call `game.snapshot`, and compare the frame's `tick`
   with `session.status`; then step one tick and count the calls until the
   frame's `tick` moves.
4. Upload. The panel's texture upload is one `write_texture` of the frame
   (6.5 MB at 1280 by 1280's capacity, less for the real size); the editor's
   frame time with and without the panel shows its cost.

**Reproduce.** `cargo xtask test -p studio play::picture` runs the reader
over an invented ring (no game).
