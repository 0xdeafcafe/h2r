# wma

A clean-room decoder for the retail WMAv2 streams. It is checked against
the game's own decoder on 48 streams, within 1 LSB apart from rare 2-LSB
float-rounding differences. No codec tables are compiled in: the owner
supplies the retail XBE and the tables are read from it.

## Where it sits

Domain `platform`; it depends on `asf`, `xbe` and `snapshot`. `dsound` feeds
it xWMA streams, and `cli` runs it.

## Layout

- `src/container.rs`: `decode_asf`, demux through `asf`.
- `src/decoder.rs`: `Decoder` and `Format`.
- `src/transform.rs`, `src/bits.rs`, `src/boundary.rs`: the transform, bit
  reader and block boundary handling.
- `src/tables.rs`: `Tables`, read from the XBE.
- `src/oracle.rs`: the game's decoder under Unicorn, as the reference.

## Features

- `synthetic`: `Tables::synthetic`, for other crates' tests.
- `oracle`: the Unicorn-backed reference decoder; a validation tool, never
  the playback path.

## Tests

`cargo nextest run -p wma`. `tests/oracle_disc.rs` and a test in
`src/decoder.rs` are `#[ignore]`d: they need your own disc and XBE (and
`--features oracle` for the oracle).

## Docs

- [docs/audio/wma.md](../../../docs/audio/wma.md)
