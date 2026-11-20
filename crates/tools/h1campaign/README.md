# h1campaign

Halo 1 campaign levels as Halo 2 v8 maps: a level is converted onto a pinned retail Halo 2 campaign map, keeping the base's globals, HUD and shared tags.

## Where it sits

`tools` domain. Depends on `h1convert`, `h1objects`, `h1-assets`, `h1map`, `scenario-edit`, `engine`, `hs`, `pathfind`, `mopp`, `content` and `h1-contract`. Used by `cli`.

## Layout

- `src/structure.rs`: Halo 1 BSPs into the base's slots and new `sbsp`/`ltmp` tags.
- `src/donor.rs`: characters the base lacks, copied whole.
- `src/ai/`, `src/squad_roster.rs`, `src/roster.rs`: AI and squads.
- `src/scripts/`: script conversion.
- `src/scenario/`, `src/cutscenes.rs`, `src/recordings.rs`: scenario placement, cutscenes and recorded input.
- `src/manifest.rs`: where a converted level goes and how the campaign picker lists it.

## Tests

`cargo nextest run -p h1campaign`. The `*_disc` tests are `#[ignore]`d and read `disc/halo1.iso` and `disc/halo2.iso` (`H1_ISO`, `H2_DISC`); run them with `-- --ignored`.

## Docs

- [halo1/campaign.md](../../../docs/halo1/campaign.md)
- [halo1/ai.md](../../../docs/halo1/ai.md)
- [halo1/scripts.md](../../../docs/halo1/scripts.md)
- [formats/pathfinding.md](../../../docs/formats/pathfinding.md)
