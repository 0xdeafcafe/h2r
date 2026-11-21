# The bots' superbounces in sim mode: practice runs and tuning sweeps

Evidence for [bots.md](../bots.md), "Tuning the bounce", and [sim.md](../../runtime/sim.md), "Bounce practice". Nothing here states what the code does; the design pages do.

## 2026-11-21: harness written, no numbers yet

Written at the `sb6-bottrain` branch (off `2c6b9ff41`). No run has been made: the tables are empty until the runs below fill them. Each filled entry names the commit, the machine (the quiet laptop, through `h2 lab`), `H2_OPT`, and the exact command.

### Before a run

```sh
# Once per map: the nav mesh and the spot scan (<map>.spots) in the data's bots/.
h2 bots nav zanzibar
h2 bots nav ascension
h2 bots nav lockout
h2 bots nav midship
# The image, built once (sim never builds); H2_OPT the same on every sim command.
H2_OBJ_CACHE=~/Library/Caches/h2recomp/obj H2_OPT=2 h2 build
```

### Commands

```sh
# One practice: four bots, Ascension, 18000 ticks (10 game minutes), unpaced.
H2_OPT=2 h2 sim run docs/runtime/sim-examples/superbounce-practice.toml \
    --report /tmp/bounce-practice.json

# Twelve practices: four maps by three seeds, one arm.
H2_OPT=2 h2 sim batch docs/runtime/sim-examples/superbounce-batch.toml --parallel 4

# The sweep: a coordinate search over six fields, two passes, each candidate the twelve runs.
H2_OPT=2 h2 sim sweep docs/runtime/sim-examples/superbounce-sweep.toml --dry-run
H2_OPT=2 h2 sim sweep docs/runtime/sim-examples/superbounce-sweep.toml --parallel 4

# What the runs proved and the ghosts they kept, into the shared bots/.
h2 sim harvest docs/runtime/sim-examples/results/bounce-practice --dry-run
h2 sim harvest docs/runtime/sim-examples/results/bounce-practice
```

The expected cost of a run is a guess until the first run times it. It is the boot (about 20 s of setup in other sim runs) plus 18000 ticks at the sim's unpaced speed with four bots. The sweep's dry run lists the candidates. With these axes the first pass is c000 plus at most 12 candidates (two new values on each of six axes), each candidate twelve runs.

### What to look at

- `report.json`'s `bounce` block per run: `totals.attempts` (the practice tried at all), `primes` against `attempts` (the prime works), `bounced_tries`/`bounce_rate`, `launch` (height from the touchdown), `why` (what fails most), `first_bounce_ticks` per bot.
- `compare.txt`'s `bounce practice` table: each map's tries, rate, launch and the top three reasons.
- `sweep.md`: the ranked candidates and the winner's `[bounce]` block. If it beats c000 on every map with enough tries, copy its numbers into `bots::tuning::BOUNCE_DEFAULT` (`crates/live/bots/src/tuning.rs`). Then confirm in a game that the bots play the new numbers.
- `attempts` of 0 on a map: no scan (`<map>.spots` missing from the data's `bots/`), or no crouch spot near a site. The run's `log.txt` names the spots loaded (`bots: <map>: N superbounce spots from <map>.spots`).

### One practice (superbounce-practice.toml)

| Commit | Machine | Wall s | Ticks | Tries | Primes | Bounced | Rate | Launch mean/best | Bounced deaths | Top reasons |
|---|---|---:|---:|---:|---:|---:|---:|---|---:|---|
| | | | | | | | | | | |

### The batch, per map (superbounce-batch.toml)

| Map | Runs | Tries | Primes | Bounced | Rate | Chain bounces | Launch mean/best | Bounced deaths | Top reasons |
|---|---:|---:|---:|---:|---:|---:|---|---:|---|
| zanzibar | | | | | | | | | |
| ascension | | | | | | | | | |
| lockout | | | | | | | | | |
| midship | | | | | | | | | |

### The sweep (superbounce-sweep.toml)

| Rank | Id | Tuning | Tries | Rate | Launch mean | Launch best | Chain bounces | Bounced |
|---:|---|---|---:|---:|---:|---:|---:|---:|
| | c000 | built-in | | | | | | |

The winner per map against c000:

| Map | c000 rate | Winner rate | c000 launch | Winner launch |
|---|---:|---:|---:|---:|
| zanzibar | | | | |
| ascension | | | | |
| lockout | | | | |
| midship | | | | |
