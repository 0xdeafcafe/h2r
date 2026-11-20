# Sim mode: estimates and per-process measurements

Evidence for [../sim.md](../sim.md). Newest first. The measured ms/tick of
real matches is in [../../perf/sim-budget.md](../../perf/sim-budget.md); this
page holds the estimate they replaced and one memory measurement.

## 2026-11-20: memory of one Waterworks 2v2 sim process

Commit: not recorded. Machine: macOS. One Waterworks 2v2 sim process.
Reproduce: run `h2 sim run` on a Waterworks 2v2 match and read the `wall`
block's `peak_rss_mib` and `footprint_mib`.

- Peak RSS about 981 MiB, 791 MiB of it the resident pages of the whole
  mmapped disc image.
- Footprint (dirty) 143 MiB: 103 MiB Malloc Large, of which guest RAM is
  64 MiB, and 35 MiB Malloc Small.
- Shared `__TEXT` 414 MiB.

## 2026-11-20: the speed-up estimate for Clanker Training (a prediction)

Not a measurement; written before the light-profile matches were run, from
the earlier two-bot run at about 11.9 ms/tick.

- That run's whole kernel, XAPI and DirectSound bucket was 1.5 to 2.1
  ms/tick, so removing the mixer alone could save only part of it.
- Starting allowance for a comparable unaudited workload: 10 to 12 ms/tick
  and 1 GiB RSS per process. Guest RAM accounts for 64 MiB.
- For 5,000 games at 1,800 ticks that is about 25 to 30 core-hours of tick
  work before loading and match setup.
- With eight effective cores and 12 GiB available, start from at most eight
  workers before allowing for headroom.

The measured numbers that replace it are in
[sim-budget.md](../../perf/sim-budget.md): 14.12 ms/tick on Waterworks and
8.49 ms/tick on Lockout with four local bots.
