# ir

The semantic IR for lifted 32-bit x86 code: what an instruction does to architectural state, with no C syntax and no host signatures. It is the contract between the lifter and every back end, and it ships a reference interpreter.

## Where it sits

`recompiler` domain. Depends on `simd` and `x87` for the float semantics. Used by `opt`, `lift`, `emit`, `clif`, `lifted`, `recomp`, `engine`, `native-render`, `perf` and `cli`.

## Layout

- `src/lib.rs`: `IrFunction`, `IrBlock`, `IrInsn` and the register, flag and memory model.
- `src/interp.rs`: the reference interpreter the passes and the lifter are tested against.
- `src/validate.rs`: structural checks run on lifter and pass output.
- `src/display.rs`: textual dump of IR.
- `src/x87.rs`, `src/x87_exec.rs`: x87 IR ops and their execution over `x87`.

## Tests

`cargo nextest run -p ir`. No disc needed.

## Docs

- [lifter/LIFTER.md](../../../docs/lifter/LIFTER.md)
- [lifter/FLOAT.md](../../../docs/lifter/FLOAT.md)
- [lifter/FLOAT-ABI.md](../../../docs/lifter/FLOAT-ABI.md)
