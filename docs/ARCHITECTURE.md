# Architecture

Goal: Halo 2 (original Xbox, title `4D530064`) running natively, ending up as
well-structured, mostly safe Rust. The game disc is supplied by the user at
build time and nothing derived from it is committed.

## The path to good Rust

Going straight from x86 to idiomatic Rust is not something a tool can do. The
game is about 3.4 MB of machine code, and a mechanical translation will always
be ugly. So the project runs in two stages that overlap:

1. **Mechanical recompile (correct, ugly).** A lifter turns every function in
   `.text` into generated code (LLVM IR or C, compiled and linked as a library).
   Guest memory is one flat 64 MB region, registers live in a `Cpu` struct, and
   every function has the signature `fn(&mut Cpu, &mut Guest)`. This stage only
   has to be right, not readable. It gets regenerated from the disc and is
   never edited by hand.

2. **Function-by-function rewrite (readable, typed).** Every guest function
   address goes through a dispatch table. A hand-written Rust function can
   register itself as an override for an address, and from then on calls go to
   it instead of the generated version. This is the same approach decomp
   projects use: the game runs at every step, and each replacement is small and
   can be backed out.

What makes stage 2 safe:

- **Differential testing.** Calls to a function can be recorded (input
  registers, the guest memory it reads, the memory it writes, its return
  value) while playing the recompiled build. Each override is replayed
  against those recordings and has to match before it ships. For floating
  point the bar is matching bits, or a tolerance documented per function.
- **Switch at runtime.** Overrides are enabled per function from config, so a
  regression can be bisected down to a single function.
- **Work up from the leaves.** Start with leaf functions such as math, string
  handling and the tag system. A caller is rewritten only after its callees
  are, so typed Rust grows upward through the call graph.
- **Typed memory in stages.** Overrides start out reading guest memory through
  `Guest` accessors. Once a structure is understood (the Halo 2 tag and map
  formats are well documented), it becomes a `#[repr(C)]` view type, and
  later on Rust-owned data with the guest copy kept in sync at the boundary.
  The `unsafe` code ends up confined to `runtime::guest`.

Statically linked SDK libraries (D3D, DSOUND, XONLINE, XNET, BINK, ...) are
not recompiled. Their functions are found by signature and replaced at the
dispatch table with native Rust implementations: D3D on wgpu, DSound on
cpal, Bink on a native decoder, and Xbox Live stubbed out.

## Crates

| crate     | role |
|-----------|------|
| `xiso`    | Reads the XDVDFS game partition (plain XISO or full Redump image) |
| `xbe`     | Parses the XBE: header, certificate, sections, libraries, kernel thunks |
| `analyze` | Disassembly (iced-x86), function discovery, coverage report |
| `runtime` | Guest memory, CPU state, dispatch/override table, and later the kernel replacement |
| `cli`     | `h2` binary: `info`, `ls`, `extract`, `analyze` |

## Milestones

1. Parse the disc and XBE. *(done)*
2. Discover functions until coverage of `.text` is close to 100%.
3. Lift the code and run from the entry point to the first kernel call.
4. Replace the kernel imports and reach D3D device creation.
5. Draw the first frame (`mainmenu.map`) through the D3D replacement.
