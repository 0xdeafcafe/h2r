# Particles: the setup's routines, read

Evidence for [retire-d3d-r2.md](../retire-d3d-r2.md), "R2's materials". Nothing here states what the code does today; the design page does. Entries are newest first.

## 2026-11-28: the particle setup `0x0023ea80` and what it calls

Read at `fa25b82bd` with the static disassembler:
`h2 disasm "$HOME/Library/Application Support/h2recomp/data/disc/default.xbe" <addr> <count>`.

| Routine | What it does |
|---|---|
| `0x0023ea80` | The particle setup (ECX shader, EAX view; stack class, layer, pass). Shader to template: a `shad`'s `[data + 0x24]`, a `PRTM`/`prt3` through `0x00137bd0` then `[vtable + 0x28]`. Writes `[0x00470a2c] = 1`, `[0x004b8348] = 0`, `[0x004075e4] = 0`; `STIPPLEENABLE` off (`0x003f5cf0`, `0x4147c`); the view's 3x4 to `c78..c80` (`0x003f7330`, ECX `0xae`), and to `0x00408218` when `[0x00404fe8]` bit 4 is clear. Then `0x0001bbf0`, `0x0001cf50`, program `class + 7` of `vrtx` `[impl + 0x100]` (`0x0001c590`, BL 1). Returns AL 0 when the `vrtx` is -1, the index is past its count or the program is empty (`+0xc >> 4` zero): the walk does not run. Then the declaration (below), `0x0023e460`, `0x0023ecd0`; AL 1. |
| `0x00137bd0` | DX a tag: `PRTM` sets singleton `0x00479874`, `prt3` sets `0x00479868` (`[vtable + 0x50](datum)`), and returns it; any other group returns 0. |
| `0x002243a0` | Both singletons' `[vtable + 0x50]`: `+4` the datum, `+8` its tag data. |
| `0x002242a0` | `prt3`'s `[vtable + 0x28]`: `[[+8] + 0x90]`, the template reference the `prt3` carries itself. |
| `0x00224d70` | `PRTM`'s `[vtable + 0x28]`: the `shad` at `[[+8] + 0x1c]`, its `[data + 0x24]`. |
| `0x0001bbf0` | The shader begin (EDI shader). With `[0x004670bc]` set: a device reset (`0x00016b10` on `0x00485b48`, `0x003f5b40(0)`, `0x003f54f0(2)`, state `0x40354` = `0x203`, `0x003f59b0(0)`, state `0x40300` = 0, `0x003f5b20(0)`, state `0x40340` = 0, `0x003f53d0(0x900)`, `0x003f5450(0x900)`, `[0x00407490] |= 0xf`, stage words `0x00404df8 + 0x80 n` and `0x0051f3d8..e4` zeroed), then the byte cleared. A `shad` writes the engine inputs `[0x00485a64..70]` from its `+0x3e`, `+0x40` (u16), `+0x44`, `+0x48`; any other tag zeroes them. Then `0x00016b90(0x00485b48, shader, 0, layer, pass, 0, 10000)`. |
| `0x00016b90` | The material setup: the template as `0x0023ea80` finds it, its `+0x5c` lod/layer/pass tables, the pass's implementation (`0x132` bytes each). |
| `0x0001ccb0` | EAX a stream type: `0x0001c6b0` on the stream cache `0x0051f0f0`, stream 0's format the type's record (`0x0043f408 + 0x15 type`), the declaration dirty when it was another, then `0x0001c710`. The particle setup runs it for `0x2b` with `[0x00470a38]` and `[0x00470a39]` both set, and inline for `0x2a` otherwise (`0x0023ec7f`). |
| `0x0001c6b0` | Clears the stream cache: `+0x2d0` count 0, `+0x2d4` and `+0x20c` dirty, the sixteen `+0x210` stream records, `+0xcc` and `+0x8c` (the format pointers). |
| `0x0001c710` | When dirty, builds the declaration (`0x0001c330`) and copies it (`0x100` bytes) to `0x0040852c` for D3D. |
| `0x0001c330` | The declaration: the program's inputs (`vrtx` element `+4` count, `+8` u16 list; register `i` reads entry `i`; with no program semantics 0, 3, 9 in `v0`, `v3`, `v9`), then per stream its record's (semantic, code) pairs: a register's (stream, offset, `0x00035790` type), `0xfe` skips bytes, another negative ends it. |
| `0x0023ecd0` | The emitter's register table `0x00470a3c..58` = 1..8. |
| `0x0023ed30` | The emitter: nothing with `[0x00470a38]` clear; writes the registers the table names, then four inline `SHORT4` corners. |
| `0x0023e460` | Runs with `[0x00485af0]` set: builds a table of sines and cosines (`0x0045dbe0`, `0x0045e418`, `0x0045e410`); not read further. |

Stream records on the disc: `0x2a` (`0x0043f77a`) is float4, float3,
float1, float4, float3, float2, float4, float4 and a `D3DCOLOR` (semantics
0 to 7 and 9, 104 bytes); `0x2b` (`0x0043f78f`) is semantic 0 as four
shorts (8 bytes).
