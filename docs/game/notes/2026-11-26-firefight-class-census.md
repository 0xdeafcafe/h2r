# Firefight classes: which campaign maps carry each `char` tag

Evidence for [firefight.md](../firefight.md), the kit's retail class
table (`firefight_kit::retail::CLASSES`).

## 2026-11-26: the census of the disc's `char` tag names

Read at `271c0ece6`, offline, on the owner's disc. Tag names only; no
tag bytes were read or kept. Nothing was built or run.

Method: a short python XDVDFS reader of `disc/halo2.iso` (the same reader
as [2026-11-23-firefight-kit-headroom.md](2026-11-23-firefight-kit-headroom.md))
opens each campaign map, reads the header's tag name table (`+0x2c0`
count, `+0x2c4` offset, `+0x2c8` size, `+0x2cc` the offsets table) and
the tag index (header `+0x10`; index `+4` group count, `+0x18` tag
count; the entries, 16 bytes each, after the 12-byte group rows), and
lists the entries whose group is `char` with their names.

Names follow `objects\characters\<race>\ai\<class>`. The Covenant
classes, by map:

| `char` tag | Maps |
|---|---|
| `grunt\ai\grunt` | 01b, 03a, 03b, 04a, 04b, 05a, 05b, 06a, 07a, 08a |
| `jackal\ai\jackal` | 03a, 03b, 05a, 05b, 06a, 07a, 08a, 08b |
| `bugger\ai\bugger` (the Drone) | 01b, 03a, 05a, 05b, 07a, 08a, 08b |
| `brute\ai\brute` | 06a, 07a, 08a, 08b |
| `hunter\ai\hunter` | 03a, 05b, 07a, 08a, 08b |
| `elite\ai\elite` | all twelve read |

Also present: rank variants (`grunt_major`, `grunt_ultra`,
`grunt_heavy`, `grunt_specops`, `jackal_major`, `jackal_sniper`,
`brute_major`, `brute_captain`, `brute_honor_guard`, `elite_major`,
`elite_ultra`, `elite_ranger`, `elite_specops`, `elite_stealth`,
`elite_zealot`), `heretic_grunt` (04a, 04b) and `brute_tartarus` (08b).
Map names abbreviated: 01b `01b_spacestation`, 03a `03a_oldmombasa`,
03b `03b_newmombasa`, 04a `04a_gasgiant`, 04b `04b_floodlab`, 05a
`05a_deltaapproach`, 05b `05b_deltatowers`, 06a `06a_sentinelwalls`, 06b
`06b_floodzone` (Elites only of these), 07a `07a_highcharity`, 08a
`08a_deltacliffs`, 08b `08b_deltacontrol`.

What the table takes from it: `03a_oldmombasa` carries the Grunt, the
Jackal, the Drone and the Hunter, so it is their first donor and their
shared tags are copied once; the Brute's first donor is `08a_deltacliffs`.
Each class's closure cost on a given base is not measured here: the kit
prints it (`class NAME: N tags, N sounds, N bytes`) when it builds.

Reproduce: `h2 tags 03a_oldmombasa` with `h2-tools` installed lists the
same names with their groups.
