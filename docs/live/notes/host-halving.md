# Dedicated host measurements

Evidence for [halving the dedicated host](../host-halving.md) and the [dedicated host](../dedicated-host.md).

## 2026-11-22 · All-phantom Lockout on a Cloudflare host

One sim lease on production Live, small class, tunnel transport, one match
in its pack. Lockout, Slayer, seed 7, four brain phantoms (2v2), 9,000
ticks (300 game seconds), run as fast as the host can tick. The match
config is `docs/runtime/sim-examples/lockout-2v2.toml` with
`docs/runtime/sim-examples/tuning/base.toml` on both arms.

| Host image (h2 commit) | Bundle | `wall.ms_per_tick` | Phantom build ms/tick | Snapshot | Match wall | Peak RSS | Boot | Cost | Kills |
|---|---|---:|---:|---|---:|---:|---:|---:|---:|
| `c1cda2f4` (`e86ad4aac`) | `3eced913` (nav, module in image) | 74.9 | 67.0 | `copy`, 65.8 ms/tick | 674 s | 509 MiB | 21.9 s | $0.022 | 13 |
| h2-only CI image (`1afa616a6`) | `5e956a03` (nav and module) | 11.1 | 1.35 | none (alias) | 100 s | 299 MiB | 26.3 s | $0.0052 | 13 |

The first image predates the phantom alias (`a7d0010a6`), so every tick
copied the 64 MiB of guest RAM a phantom build reads (Linux has no remap).
The second builds on live RAM through the alias. Both matches ended
`match_over` with the same kills, and both reports have `nav: "loaded"`.

An earlier run on a bundle without nav meshes had no kills.

`wall.ms_per_tick` is the sim's whole tick, not the light host's
`game_cpu_ms_per_tick` in a full pack; the pack targets in the design page
still need a full-pack run.

Reproduce: copy the two files above into a scratch directory with a batch
file holding one versus run (`lockout`, seed 7, arms `base` and `base2`),
then

```sh
h2 sim batch batch.toml --provider cloud --live https://live.forbes.red --max-parallel 1 --budget-usd 0.10
h2 live hosts list --live https://live.forbes.red
```

Leases `hl-zu6OnFJ-IPziqaHSM-XWjw` and `hl-HQpjqK5mL3BqJW2OX7URog`.
