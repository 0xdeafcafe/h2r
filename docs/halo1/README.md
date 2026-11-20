# Halo 1

Halo 1 (Halo: Combat Evolved, Xbox) running in the Halo 2 engine. We don't
port Halo 1 engine code or recompile Halo 1's XBE: Halo 1's data is read
from your own disc image (`disc/halo1.iso`) and converted offline into Halo 2
`.map` files, which the game loads like any other level. Where the two
engines differ, Halo 2's wins, except for health. No game data goes in the
repo.

To play: put `halo1.iso` next to `halo2.iso` and run `cargo h2 play
disc/halo2.iso`. The first start converts the disc
([campaign.md](campaign.md) §10, "Importing").

Each doc's status line is the current word on how far its part got.

## The game side

| Doc | What | Crates |
|---|---|---|
| [campaign.md](campaign.md) | The plan for Halo 1's campaign: decisions, pipeline, order of work, the ten levels, importing | `h1campaign`, `cli::h1_import` |
| [multiplayer.md](multiplayer.md) | The 13 multiplayer maps on Halo 2 base maps, the Halo 1 sandbox, packages with variants (`h2 h1 convert`) | `h1convert` |
| [campaign-picker.md](campaign-picker.md) | How 1.0's menus list campaign levels, the campaign picker and per-campaign progress | `boot` |
| [health.md](health.md) | Halo 1's health model: no body regeneration, health packs, the meter | `boot` |
| [hud.md](hud.md) | Halo 1's HUD on converted maps | `boot` |

## Reading and converting Halo 1 data

Read [map-format.md](map-format.md) first: the others build on its
reader.

| Doc | Tags | Crate |
|---|---|---|
| [map-format.md](map-format.md) | Cache files (v5, zlib) | `h1map` |
| [models.md](models.md) | `mode`, `antr` to Halo 2 `mode`, `jmad` | `h1models` |
| [bitmaps.md](bitmaps.md) | `bitm` and lightmap pages | `h1bitmaps` |
| [sounds.md](sounds.md) | `snd!` to Halo 2 `snd!` and the `ugh!` gestalt | `h1sounds` |
| [objects.md](objects.md) | `weap`, `proj`, `eqip`, `bipd`, `vehi`, `coll`, `phys` | `h1objects` |
| [role-audit.md](role-audit.md) | Halo 2 values left in converted tags (role words, counterpart references), each allowed or a bug | `h1objects` |
| [ai.md](ai.md) | Actors, encounters, command lists to Halo 2's scenario AI | `h1ai` |
| [ai-control.md](ai-control.md) | Encounter script controls bound to Halo 2 evaluators; orders and migration | `h1scripts`, `h1ai` |
| [scripts.md](scripts.md) | Campaign scripts to Halo 2 builtins and nodes | `h1scripts` |
| [script-builtins.md](script-builtins.md) | Unit, object and miscellaneous script builtins: retail equivalents and the hooks on converted campaign maps | `h1scripts`, `boot` |
| [sky.md](sky.md) | `sky ` | `h1sky` |
| [text.md](text.md) | `ustr`, `hmt ` to Halo 2 `unic` | `h1text` |

The Halo 2 formats they convert into are in [../formats/](../formats/),
starting with [map-format.md](../formats/map-format.md). The editor opens
and converts Halo 1 maps too ([editor-guide.md](../editor/editor-guide.md)).
