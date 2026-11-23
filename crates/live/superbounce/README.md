# superbounce

Superbounce and forced-crouch spots found in a map's collision geometry. The scan is pure: it takes collision surfaces and returns spots; `scan_map` reads them out of a `.map`. Also the bounce and crouch models and the verified and learned spot sets.

## Where it sits

`live` domain. Depends on `tags` and `map`.

## Layout

- `src/scan.rs`, `src/read.rs`: the scan and reading surfaces from a map.
- `src/bounce.rs`, `src/crouch.rs`, `src/geom.rs`: the two spot kinds and their geometry.
- `src/verified.rs`, `src/learned.rs`, `src/ghost.rs`, `src/view.rs`: verified and learned spots and how they are shown.

## Tests

`cargo nextest run -p superbounce`.

## Docs

- [game/superbounce.md](../../../docs/game/superbounce.md)
