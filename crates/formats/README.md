# crates/formats

Disc, tag and file formats, ours and the game's: parsers, writers and
codecs that are plain data in and plain data out. A crate here has no guest
memory and no host window, device or thread. It may not depend on any other
domain (`scripts/crate-domains.toml`, checked by `scripts/crate-graph.py
layers --check`), so every other domain can use it. Where one format
crate needs another it depends on it directly (`tags` on `map`, `asf` on
`xiso`). The rules are in
[docs/dev/crate-structure.md](../../docs/dev/crate-structure.md).

| Crate | What |
|---|---|
| [asf](asf/) | Bounded ASF container demuxer for the game's WMA streams |
| [content](content/) | Map packages: manifest, `MapId`, patch readers and the resolver |
| [font](font/) | Packed bitmap font parser, encoder and CPU text rasteriser |
| [forge-doc](forge-doc/) | Forge documents, portable ops and the host-stamped edit log |
| [h1-contract](h1-contract/) | Names shared by the Halo 1 converters and the in-game Halo 1 hooks |
| [hs](hs/) | Compiled HaloScript: decode, check, cast, and the script VM |
| [map](map/) | Cache v8 `.map` reader and writer |
| [mopp](mopp/) | Havok MOPP code for structure BSPs: query model, validator, compiler |
| [movies](movies/) | Transcoded movie manifest, lookup and resumable installer |
| [pathfind](pathfind/) | AI pathfinding data: decode, generate, compare, emit |
| [settings](settings/) | Host settings as a schema, one store, one writer for `settings.toml` |
| [shaders](shaders/) | NV2A vertex microcode and register combiners to WGSL |
| [tags](tags/) | Tag definitions, codecs, repository and owned domain types |
| [variants](variants/) | Game variants: the 0x130-byte record, schema, saved files, editor state |
| [xbe](xbe/) | Parser for Xbox executables |
| [xiso](xiso/) | Reader for the Xbox DVD filesystem (XDVDFS) |
