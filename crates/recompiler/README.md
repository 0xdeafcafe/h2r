# crates/recompiler

The recompiler domain turns the retail XBE into host code: function discovery, lifting to a semantic IR, IR passes, the C and Cranelift back ends, the soft-float libraries the generated code calls, and the oracles that check them. It is offline work on a disc-supplied executable and holds no game data. A crate here may depend on its own domain and on `formats` (for `xbe`); `runtime`, `rewrites`, `tools` and `apps` may depend on it. The rules are checked by `python3 scripts/crate-graph.py layers --check` and set out in [crate-structure](../../docs/dev/crate-structure.md).

| Crate | What it is |
|---|---|
| [analyze](analyze/README.md) | Function discovery, control-flow graphs and coverage over an XBE. |
| [symbols](symbols/README.md) | Identifies statically linked XDK library functions by OOVPA signature. |
| [lift](lift/README.md) | Lowers decoded x86 into the semantic IR. |
| [ir](ir/README.md) | The semantic IR, its validator and the reference interpreter. |
| [opt](opt/README.md) | IR-to-IR optimisation passes between lifting and emission. |
| [emit](emit/README.md) | The C11 back end: planned IR to sharded, disposable C. |
| [clif](clif/README.md) | The Cranelift fast tier: planned IR straight to host objects. |
| [hostfault](hostfault/README.md) | The process-wide host fault handler: regions and the fixup tables `emit` generates. |
| [x87](x87/README.md) | Exact software x87 (80-bit extended) arithmetic. |
| [simd](simd/README.md) | Bit-exact, MXCSR-exact SSE1 floating-point semantics, in Rust and C. |
| [oracle](oracle/README.md) | Independent x86-32 instruction oracle for synthetic differential fixtures. |
| [sse-oracle](sse-oracle/README.md) | SSE1/MMX queries to an isolated x86-64 hardware runner. |
