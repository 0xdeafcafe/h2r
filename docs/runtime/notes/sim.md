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

## 2026-11-21: same-seed repeatability on Lockout

Two runs of `sim-examples/results/aggressive-vs-base/runs/lockout-s1/match.toml`
(Lockout, seed 1, four phantoms, 9000 ticks, `H2_FEATURES=-impl.sim.native`,
`H2_OPT=2`) on one image. Before the spawn words were written, `h2 sim diff`
found the first difference at tick 1: every player spawned elsewhere, with
the shared seed (`0x8116017e`), its second word and the logical clock equal
in both. A trace at `PLAYER_SPAWN` showed the game options' words
`[[0x4e6948]+0x10]`, `+0x14` differing between runs (`0x6ca0017e
0xd66f5226` against `0x1ca36eaa 0x044b844f`); `0x14ef00` mixes them into the
spot choice. With `sim_glue::spawn_words` written ahead of the first spawn
(branch `sim-seed-early`), the two runs were identical: 9000 ticks, every
component equal.

Reproduce: run the match twice with `--report a.json` and `--report b.json`
(separate `--data` directories), then `h2 sim diff a.json b.json`.
