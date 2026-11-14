# h2recomp

A static recompilation of Halo 2 (original Xbox) to native code, with the
runtime written in Rust. You need your own copy of the disc: no game data
lives in this repo.

Put your own disc image in `disc/` (it's git-ignored), then:

```sh
cargo h2 info
cargo h2 ls
cargo h2 analyze
cargo h2 extract          # to disc/extracted/
```

On macOS, `cp -c` clones the image without using any extra space:
`cp -c ~/Downloads/your.iso disc/halo2.iso`. You can also pass a path to
any command, or set `H2_DISC`.

Both XISO (game partition only) and full Redump images work.

## Ghidra

Ghidra is used as a second opinion and for browsing. The pipeline never
depends on it at build time.

```sh
scripts/setup-ghidra.sh     # Ghidra 12.1.4 + XboxDev XBE loader + ghidra-cli, into .tools/
scripts/ghidra-export.sh    # import + analyse once, write disc/ghidra-functions.tsv
cargo h2 analyze --against disc/ghidra-functions.tsv
scripts/ghidra decompile 0x2d0aee
```

Ghidra is pinned to 12.1.4 because the XBE loader only loads on the version
it was built for.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how this is meant to end
up as structured Rust.

## Docs

- [ARCHITECTURE.md](docs/ARCHITECTURE.md): goals, the two-stage plan, crate table, milestones
- [audio.md](docs/audio.md): DirectSound replacement (`crates/audio`)
- [bitmaps.md](docs/bitmaps.md): bitmap tags to textures and PNG (`crates/assets`)
- [engine-map.md](docs/engine-map.md): survey of the retail engine and where to start
- [graphics.md](docs/graphics.md): D3D8 on wgpu (`crates/gfx`)
- [input.md](docs/input.md): XInput replacement (`crates/input`)
- [kernel-imports.md](docs/kernel-imports.md): kernel imports work list
- [LIFTER.md](docs/LIFTER.md): lifter design proposal
- [localization.md](docs/localization.md): string IDs and localized text (`crates/tags`)
- [map-format.md](docs/map-format.md): cache file (v8) format and the `map` reader
- [oracle.md](docs/oracle.md): independent x86 instruction oracle
- [sound.md](docs/sound.md): sound tags (`snd!`, `ugh!`)
- [storage.md](docs/storage.md): Xbox storage boundary (`crates/vfs`)
- [xdk-libraries.md](docs/xdk-libraries.md): XDK library identification
