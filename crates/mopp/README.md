# mopp

Havok MOPP code for Halo 2 structure BSPs (`sbsp+0x1e0`): the engine's
interpreter semantics as a query model, a validator, and an offline
compiler. Format, opcodes and proof: [docs/formats/mopp.md](../../docs/formats/mopp.md).
`h1convert` uses `build_structure` to give converted Halo 1 levels their MOPP code.

| module | what |
|---|---|
| `code` | the code object: 0x30-byte header and bytecode (`Code::parse`, `Code::to_block`) |
| `vm` | `query_sphere` (the engine's `0x00300290`, bit for bit) and `query_aabb` (its semantics on boxes) |
| `check` | `validate`: every branch by the engine's rules; histogram, keys, reach |
| `bounds` | `key_bounds`: the box each key's path guarantees |
| `geom` | `Surface`, structure keys, and an exact polygon/box overlap test |
| `build` | `build` (keyed polygons to a program) and `build_structure` (a collision BSP to the block bytes) |

```sh
cargo test -p mopp                       # opcode semantics, compiler on invented geometry
cargo test -p mopp --release --test disc -- --ignored --nocapture   # retail proof (needs the disc)
cargo run -p mopp --release --example survey                        # retail header fields and opcode use
cargo run -p mopp --release --example compare -- lockout 0           # retail vs ours, opcode by opcode
cargo run -p mopp --release --example replace -- lockout DIR         # lockout with our program, for H2_MAP_DIR
```
