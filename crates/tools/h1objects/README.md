# h1objects

Halo 1 sandbox objects as Halo 2 tags: weapons, projectiles, equipment, bipeds and vehicles, with collision, physics, particles, HUDs and effects rebuilt in Halo 2 form.

## Where it sits

`tools` domain. Depends on `h1map`, `h1-assets`, `h1convert`, `map`, `tags` and `mopp`. Used by `h1campaign`, `studio` and `cli`.

## Layout

- `src/h1.rs`, `src/refs.rs`: reading Halo 1 object tags and finding references.
- `src/sandbox.rs`: the objects a converted map's spawns name.
- `src/coll.rs`, `src/physics.rs`, `src/ragdoll.rs`: collision and physics.
- `src/convert*.rs`, `src/rebuild.rs`: tag conversion per object kind.
- `src/h1hud/`, `src/health_hud.rs`: HUDs.
- `src/verify.rs`, `src/provenance.rs`: checks and where each field came from.

## Tests

`cargo nextest run -p h1objects`. The `*_disc` tests are `#[ignore]`d and read the Halo 1 disc (`H1_ISO`) and the Halo 2 disc (`H2_DISC`); run them with `-- --ignored`.

## Docs

- [halo1/objects.md](../../../docs/halo1/objects.md)
- [halo1/rebuild.md](../../../docs/halo1/rebuild.md)
- [halo1/README.md](../../../docs/halo1/README.md)
