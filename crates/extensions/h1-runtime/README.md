# h1-runtime

Hooks that only converted Halo 1 maps need. It holds two today: the projectile velocity curves and the scope zoom. The rest of Halo 1 in the game (AI control, conversations, HUD text, script builtins, cinematics) is in `game` as `src/h1_*`.

## Where it sits

`extensions`. Uses `hooks`, `runtime`, `tags`, `engine` and `render-bridge`. Used by `boot` and `game`.

## Layout

- `src/lib.rs`: the registration.
- `src/h1_projectiles.rs`: projectile velocity curves.
- `src/h1_scope_zoom.rs`: the scope zoom.

## Tests

`cargo nextest run -p h1-runtime`.

## Docs

- [README.md](../../../docs/halo1/README.md)
- [crate-structure.md](../../../docs/dev/crate-structure.md)
