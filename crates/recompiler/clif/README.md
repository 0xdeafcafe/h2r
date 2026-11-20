# clif

The Cranelift fast tier: it turns planned IR (`emit::Owner`) straight into host objects, linked into the same kind of module the clang build makes. It exists to compile the lifted code far faster than the C route.

## Where it sits

`recompiler` domain. Depends on `ir` and `emit` (and dev-depends on `lift`, `analyze`, `xbe`). Used by `recomp`.

## Layout

- `src/lower.rs`: IR to Cranelift lowering, one handler per owner (`compile_shard`).
- `src/abi.rs`: the handler signature and the ABI probe each shard carries.
- `src/x87.rs`: x87 operations lowered to calls into the `x87` crate's ABI.
- `src/lib.rs`: `registry_c`, `HELPERS_C` and the module assembly.

## Tests

`cargo nextest run -p clif`. `tests/differential.rs` and `tests/image.rs` read the retail executable from `H2_XBE`.

## Docs

- [lifter/cranelift-tier.md](../../../docs/lifter/cranelift-tier.md)
- [lifter/LIFTER.md](../../../docs/lifter/LIFTER.md)
