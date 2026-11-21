# Grenade trajectories in telemetry playback

Evidence for the [3D viewer](../web-viewer.md) and [frame telemetry](../live-telemetry.md).

## 2026-11-20 · Lockout

Source at `4a77318c239e`. The public recording has 1,633 frames in 27
version 7 chunks. Player 0 is afr. Six frames report a grenade action for
that player. In each, a new frag projectile first appears. The event records
what happened in the simulation. The recording does not include the human's
controller input. [Match detail](https://live.forbes.red/v1/games/4478945a39e8d9d6049910abfb074934)

Two throws near the same spot identify the direction error. At tick 1354
the new grenade is attached at afr's position. By 1356 its velocity points
along afr's aim. By 1362 both horizontal components have reversed sign,
with afr still in place. The damage reports at 1398 credit afr's frag with
hitting afr. A second grenade appears at 1412, comes to rest nearby by
1424 and deals self-damage in the frame at 1448. The kill log records afr's
suicide at 1447. The second throw also illustrates why the selected grenade
is unsuitable for colouring a path. That action frame says plasma while
both the projectile and the damage report identify a frag.

To reproduce, read the [chunk index](https://live.forbes.red/v1/games/4478945a39e8d9d6049910abfb074934/chunks)
and request each chunk with `?format=json`. Group projectile observations
by their full `id`. Compare their first tick with each player's
`actions`. Both throws above appear in the
[chunk starting at 1202](https://live.forbes.red/v1/games/4478945a39e8d9d6049910abfb074934/chunks/1202?format=json).
Compare each throw's positions and velocities through the last sample with
the `from`, `to` and `weapon` in the subsequent hits.

At this revision `EventFeed.observe` in `web/src/map/events.ts` schedules
the arc from the player whose grenade count decreased. Its `grenadeAt`
uses sampled player aim, a fixed hand height, speed, loft and gravity, and
stops at the thrower's foot height. It neither reads a projectile nor
tests level collision. `replayFrame` in `web/src/viewer/session.ts` drops
the telemetry projectile list before `web/src/viewer/effects3d.ts` draws
the trail. The inferred path keeps going forwards through the recorded
change of direction. The coarse samples bound the bounce in time and
space but do not identify the surface it struck.
