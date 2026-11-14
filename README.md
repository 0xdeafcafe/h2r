# h2recomp

A static recompilation of Halo 2 (original Xbox) to native code, with the
runtime written in Rust. You need your own copy of the disc: no game data
lives in this repo.

```sh
cargo run -p cli -- info  "path/to/Halo 2.iso"
cargo run -p cli -- ls    "path/to/Halo 2.iso"
cargo run -p cli -- analyze "path/to/Halo 2.iso"
cargo run -p cli -- extract "path/to/Halo 2.iso" extracted/
```

Both XISO (game partition only) and full Redump images work.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how this is meant to end
up as structured Rust.
