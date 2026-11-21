# Halo 3 saved films: evidence

Evidence for [input-film.md](../input-film.md) and [theater-design.md](../theater-design.md). Nothing here states what our code does; the design pages do.

Newest first.

## Layout and sizes from public sources

Read on 2026-11-21 at `4279cebce`. Method: public source code and a public
metadata API only. No Halo 3 film file was obtained or decoded, so no
offset here was checked against a real file. No game binary was read.

Sources: the `blf` library of Blam-Network (the saved-film header and game
options of the last Xbox 360 title update), BlamLib's Halo 3 `BLF.cs` (film
header and data readers), the Blam Network file share's public listing
(`GET https://halo3.blam.network/halo3/fileshare/files?fileType=films`),
Halopedia's Theater page, and the GDC 2011 talk "I Shot You First". Only
these sources are used; what the tick record holds inside is not
described by them and is not recorded here. To reproduce the sizes, page through the
listing and divide `size_in_bytes` by `length_seconds`.

**File.** A film is a chunked file with a fixed order: a file header, a
content header (name, author, kind, length, map and game ids), an author
chunk with the build, a film header, the film data, an end chunk, then
zero padding to 4 KiB. The film header is fixed-size, about 64 KB, so the
tick stream starts at a constant offset.

**Start state.** The film header holds the build string, executable type
and network versions, a signature of the map file, three flags (host
film, contains game state, is a clip), a session id, and the whole game
options block: simulation mode, tick rate, the random seed, a determinism
version, map id and scenario path, campaign difficulty and skulls, the
whole game variant and the whole map variant by value, the machine set,
and 16 player entries with their full configuration. It ends with the
recorded time and the length in ticks. A full film has no game state. A
clip that starts mid-game carries one compressed game state, about 1 MB.

**Tick stream.** A flat run of records with no index. Each has a 32-bit
header, two bits of type and thirty of size, then the payload. One
simulation-update record is written per tick, bit-packed, beginning with
an update number. The stream is not compressed. Its fields beyond the
update number are not covered by these sources.

**Seeking.** The file holds no keyframe or chapter index. Campaign films
cannot be rewound (Halopedia).

**Versioning.** The header carries the executable type, a determinism
version and a signature of the map file.

**Sizes**, from the listing's 89 items (31 films, 58 clips):

| Kind | Sample | Bytes a minute |
|---|---|---:|
| 1v1 (315 s) | 696,670 bytes | 133,000 |
| 4v4 (365 s) | 3,674,175 bytes | 604,000 |
| 16 players (902 s) | 10,825,351 bytes | 720,000 |
| Campaign (1,268 s) | 1,961,477 bytes | 93,000 |
| Campaign (1,704 s and 2,478 s) | 5.1 MB and 7.5 MB | 180,000 |
| Multiplayer films, median of 31 | 3.05 MB, 406 s | 494,000 |
| Clips, 5 to 136 s | 0.35 to 4.4 MB | 1.2 MB fixed, then 700,000 |

At 30 ticks a second that is about 70 bytes a tick for two players and
400 for sixteen: 25 to 35 bytes per player per tick, so 45,000 to 63,000
bytes per player-minute.

**Not established.** What a tick record holds beyond its update number,
and so how much of the multiplayer figure is input.
