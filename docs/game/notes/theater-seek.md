# Theater seek measurements

Evidence for [Theater's seek and tape budgets](../theater-design.md).

## Measurement template

- Date, commit, machine, build switches and render path: to be measured.
- Map, player count, tick rate, duration and starting data for each run:
  to be measured.

Use the same installed image, disc, `H2_OPT` and simulation switches for
recording and playback. Keep the captured films, logs and guest storage
in private local scratch. Record only aggregate sizes, timings,
operation names and blocker descriptions here.

1. Record three games with `H2_THEATER_MEASURE=1`: a leased dedicated
   host match, an offline local multiplayer game and an offline campaign
   level. Use the [leased host procedure](../theater.md#tier-b-implementation-and-coordinator-runs)
   or `h2 play --film "$film_run/match.h2film"` for a local recording.
   Give each game its own output file. Play for at least 90 seconds of
   match time, then end normally to seal the film.
2. On each recording's own agent socket, collect `theater.measurements`
   during play and before exit. Record each capture outcome, elapsed
   milliseconds and reason for a skip. Record the 5 s and 30 s
   `page_windows` counts. Keep collecting during longer runs so no
   entries counted by the omitted fields are lost. Use
   `h2 film stat "$film_run/match.h2film"` on the sealed film for the
   actual embedded anchor and delta sizes, barrier ticks and tape
   table. Record all operation counts for either offline recording,
   including any boot-only socket or service calls.
3. Play each film linearly with `h2 theater play "$film_run/match.h2film"
   --render-every 0 --socket "$film_run/play.sock"`. Pass the recording's
   `--data` baseline if the film references external files. Enable the
   measurement switch on playback too. Collect `theater.keyframes`
   and `theater.measurements` over the direct socket after match start.
   The latter gives child entry to first slice and boot to match start.
4. Pause through `theater.pause`. Use `theater.seek` with a nonzero
   target before the first embedded keyframe, or any interior target
   when the inventory is empty. If present, also seek to an interior
   tick after an anchor and a tick after a delta. Choose targets before
   the film's end. Reconnect to the same playback socket after each
   restart and read `theater.state` until `remaining_ticks` reaches zero.
   Record the source and `restore_ms`. Compute unpaced ticks per second
   as `1000 * fast_forward_ticks / fast_forward_ms` from
   `theater.measurements`. Use a longer replay interval if elapsed time
   rounds to zero. Resume through `theater.play` and retain the exit
   status and verified final tick.

The measurement passes when all three films seal and replay to their
recorded end with matching hashes and cursors, every recorded keyframe
and D3D barrier appears in the inventory, and executed seeks reach
their targets through the expected route. The report accounts for
every capture attempt and includes both write windows, launch and boot
timings, and per-stream tape totals for every film. A film with no
successful keyframes still supplies the boot baseline. Record its
blockers and leave cold restore measurements pending for that film.

| Observation | Leased host | Local multiplayer | Campaign |
|---|---|---|---|
| Keyframes published and skipped, blockers | to be measured | to be measured | to be measured |
| Anchor and delta stored bytes | to be measured | to be measured | to be measured |
| Capture duration by outcome | to be measured | to be measured | to be measured |
| Pages written per 5 s and 30 s | to be measured | to be measured | to be measured |
| Child entry to first slice | to be measured | to be measured | to be measured |
| Boot to match start | to be measured | to be measured | to be measured |
| Cold restore time by keyframe type | to be measured | to be measured | to be measured |
| Unrendered replay ticks per second | to be measured | to be measured | to be measured |
| Per-stream calls, inflated bytes, stored bytes and chunks | to be measured | to be measured | to be measured |
| Offline operation counts | host | to be measured | to be measured |
| Last verified tick or first divergence | to be measured | to be measured | to be measured |
