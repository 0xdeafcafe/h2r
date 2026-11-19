# Telemetry precision and replay measurements

## Match and measurement

The seeded multiplayer benchmark runs a ten-minute match with 16 players,
vehicles, projectiles, dropped items and hits. It emits 9,000 samples at 15 Hz
over 18,000 simulation ticks, with a keyframe every 150 samples. Live byte
counts include record length prefixes. Each of the 60 ten-second replay chunks
starts with its own keyframe. Transfer sizes are response-body bytes.

| Ten-minute match | v4 | v6 baseline | v7 |
|---|---:|---:|---:|
| Live bytes/player/s | 272.09 | 254.71 | **145.25** |
| Live bytes/frame | 290.23 | 271.69 | **154.93** |
| Encode µs/frame | 15.72 | 17.09 | 17.20 |
| Encode µs/simulation tick | 7.86 | 8.54 | 8.60 |
| Replay JSON bytes | 68,037,538 | 88,675,937 | 90,082,165 |
| Replay JSON gzip bytes | 11,773,243 | 13,052,003 | 13,517,251 |
| Replay binary chunk bytes | 2,405,346 | 2,110,374 | **1,322,664** |
| Decode full replay JSON, ms | 219.39 | 372.96 | 257.68 |
| Inflate and decode full binary replay, ms | 236.67 | 200.80 | **165.35** |

The v7 scene uses 43.0% fewer live bytes and 37.3% fewer replay bytes than
v6. Its replay is 1.47% of the uncompressed JSON transfer and 9.78% of the
gzip JSON transfer. The 30-second seeded size regression includes all lists,
record lengths and keyframes, and requires **under 147 B/player/s**.

V4 is included for historical context. Its input predates projectiles,
dropped items and vehicle velocities. The v7 measurement uses wire revision `afe9fa2c`.
The v6 baseline is the merged
`84ebbd6f` with the same input scene and list schema as v7. Its guest-scene
output differs from v4 only in projectile and dropped-item lists, vehicle
velocity and the units’ synthetic circular-motion velocity. All events in
the 12-tick regression scenes match. Encode time
covers packing each sampled frame. The per-tick figure spreads this cost
over all 30 ticks per second. Neither figure includes game-state sampling,
storage compression, HTTP or rendering.

Run the ignored release test `bench_match_wire` in `live-telemetry` with
`TELEMETRY_BENCH_OUT` set to an output directory. From `web/`, run
`bun bench/replay-transfer.ts <directory>` with the corresponding reader.
The benchmark compares every decoded field with the exported Rust JSON,
allowing only floating-point rounding in the final vector normalisation.
It preloads both representations and warms each reader, then takes the median
of five full-match decodes, alternating the order. Binary includes chunk
inflation and reconstruction of viewer frames. JSON includes UTF-8 decoding
and parsing. The separate gzip size uses level 6 per JSON response. Timings
are Bun 1.4.2 on the development machine and vary with concurrent work.

## Which changes saved bytes

At the v7 grid resolution, continuing the previous angular and velocity
trend on equal tick spans brought the match to 179.94 B/player/s. Coding
changes to player and actor edit masks in field-frequency order reduced
that to 157.62. Reusing a player's exact position for a matching hit and
giving in-order edits a one-bit opcode brought it to 145.25. Off-player
impacts still carry their own coordinates. These savings depend on the
workload. For example, the synthetic scene places all 13,388 hit positions
on players.

Fresh raw DEFLATE streams at level 1 made one- and two-frame packets larger
in aggregate. Over the final match's replay records, choosing compression
only when smaller saved 0.19% in either case, even before a codec flag.
The one-frame trial cost about 15 µs per packet on this machine. Live packets
therefore carry the packed bits directly. Replay chunks amortise compression
over up to 150 frames and use DEFLATE only when it reduces the stored length.

## Error at a captured tick

| Field and maximum error | v4/v6 | v7 |
|---|---:|---:|
| Position per axis, including hit locations and object transforms | 2.98 mm | 1.49 mm |
| Velocity per axis, including vehicle and projectile velocity | 0.0078125 world units/s | 0.00390625 world units/s |
| Body yaw / heading | 0.04395° | 0.01099° |
| Aim direction on the sphere, including pitch | 0.05938° | 0.01484° |
| Vehicle, machine, object and item rotation, per forward or up axis | 0.05938° | 0.01484° |
| Scalar speed | 1/32 world unit/s | 1/32 world unit/s |
| Body, shield and hit amount | 1/126, 1/64, 1/512 | same |
| Device travel, animation phase | 1/2046 of travel, 1/32 frame | same |
| IDs, ticks, masks, counters and discrete states | exact | exact |

Bounds apply to inputs in the field's supported range. Fixed signed world
coordinates avoid map-specific bounds in each keyframe. One world unit is
3.048 m. The v7 position step is 1/1024 unit and velocity step is 1/128 unit/s.
The table gives half steps. Aim and orientation use two octahedral
coordinates with maximum M of 4094 or 16382. Rounding perturbs each projected
coordinate by at most 1/M. The unfolded vector error is at most √6/M and the
unfolded vector length is at least 1/√3, giving an angular bound of
asin(√18/M). Pitch inherits that bound. Aim azimuth near a pole has no
uniform error bound. The heading row measures the separately transmitted yaw.

A sweep of 200,000 Fibonacci-sphere directions found maximum angular errors of
0.05846° and 0.01453° respectively. Maximum pitch errors were 0.05847° and
0.01448° (rounded up). Finer quantisation reduces spatial steps at captured
ticks. The encoder sends a correction whenever a prediction differs by a
quantum, so errors cannot accumulate across unchanged runs.

## Between samples

The simulation runs at 30 Hz. At 15 Hz a pose sample normally spans two
simulation ticks. A motion that begins and ends between those samples can
still be absent. Fire masks and tick-stamped hits carry events across that
gap. The viewer places shots at their firing tick, blends by elapsed ticks
between endpoints, and holds the previous pose up to a checkpoint cut.

Machine transforms can reuse the same position, direction and change-mask
fields with their stable device key. The device travel scalar already has a
ten-bit representation. A future schema extension needs corresponding Rust
and web field definitions, keyframe coverage and a versioned fixture.

The wire grammar, storage routes and compatibility rules are in the
[telemetry reference](../live/live-telemetry.md#packed-frames) and
[frame chunk API](../live/live-api.md#frame-chunks).
