Evidence for [native-objects.md](../native-objects.md) ("Shared program resources"). Nothing here states what the code does today.

## Shared program resources on a multiplayer map

| Date | Commit | Method | Result |
|---|---|---|---|
| not recorded | not recorded | `h2 lab` run 0076, Ascension's load, with one resource set per program (before `object_program_gpu::Shared`) | about 9,000 textures, 12,800 samplers and 3,600 modules and layouts created |

Reproduce: load Ascension on a lab host and read `LitObjects::created` for the set. With the shared resources the counts fall to the distinct shaders, stage layouts, samplers and bitmap images.
