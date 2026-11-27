# Insertion pod donors on the Halo 2 disc

Census of `objects\vehicles\insertion_pod\insertion_pod` (`vehi`) across the
disc's campaign maps, for the pod donor of the
[Firefight kit](../firefight.md). Date 2026-11-27, branch `ff-pods-donor`
(from `ff-f5-pods`). Reproduce: `h2 tags MAP vehi --disc disc/halo2.iso` per
map, then `h2-tools firefight kit MAP --disc disc/halo2.iso --data DIR
--replace [--retail-classes --cut]`.

| Map | Holds the pod | Size |
|---|---|---|
| 03a_oldmombasa | yes | 1692 B tag |
| 06a_sentinelwalls | yes | 1692 B tag |
| 08a_deltacliffs | yes | 1684 B tag |
| every other campaign map, mainmenu | no | |

The pod needs no donor of its own. The kit searches the classes' donor maps
and `03a_oldmombasa` is already the dropship donor (`firefight_cmd.rs`
`SHIP_DONOR`), so the pod is found there with and without
`--retail-classes`; the Phantom and the pod share the donor read. Pelican
sits in 03a, 03b, 05a, 05b, 06b, 07b (not 01b, which has only its guns).

## Kit results

The pod was cut on Coagulation and Zanzibar with "no zone or start has a
clear column to the sky", not for a missing donor: a retail structure BSP
closes its world in a lid at the sky height, and the sky test counted the lid
as a roof. With the lid left out (as the ship's paths already do):

| Map | Build | Pods | Zone falls | Open starts | Pod bytes | Available |
|---|---|---|---|---|---|---|
| coagulation | `--retail-classes --cut` | kept, 23 tags | 5 | 100 of 123 | 39130 | 7155712 |
| coagulation | default (Phantom copied) | kept, 24 tags | 5 | 100 of 123 | 39338 | 7155712 |
| zanzibar | `--retail-classes --cut` | kept, 23 tags | 8 | 36 of 59 | 28882 | 6393856 |

Both keep the Phantom and the pod with all five Covenant classes
(Coagulation, Zanzibar: tag growth 1.20-1.26 MB of 6.4-7.2 MB available).
