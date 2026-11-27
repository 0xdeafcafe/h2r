# Sound simulation parity, 2026-11-27

Commit: 4e0d0b2d6. Switch: `impl.sim.sound-events`.

`h2 sim diff` on `docs/runtime/sim-examples/firefight-beavercreek.toml`, switch off against switch on, was identical over 10351 ticks, with every component agreeing. The switch is on by default from this result ([sound-sim.md](../sound-sim.md)).

Reproduce: `h2 sim diff docs/runtime/sim-examples/firefight-beavercreek.toml` once with `impl.sim.sound-events=off` and once with it on.
