# Synthetic kernel headers

`image::BASE` is 0x80010000. Installation maps 8 KiB of read-only guest RAM
containing an authored MZ/PE32 header and one non-executable `.rdata` section.
This describes the HLE: it contains no Microsoft kernel code or firmware bytes,
no discardable `INIT` section, and no PE exports. Guest imports continue through
Runtime's tokens. This is not a general replacement for an Xbox kernel image.

## Evidence from the supplied retail XBE

Local disassembly with `h2 disasm disc/default.xbe` establishes this path:

| Guest address | Operation |
| --- | --- |
| 0x2d1e15 | Read DOS `e_lfanew` at 0x8001003c, add kernel base. |
| 0x2d1e21 | Read PE `NumberOfSections` (NT header + 6). |
| 0x2d1e26 | Read `SizeOfOptionalHeader` (NT header + 20). |
| 0x2d1e2f | Form last section: NT + 24 + optional size + (count − 1) × 40. |
| 0x2d1e33 | Compare section name against `INIT`; return at 0x2d1e72 if absent. |
| 0x2d1e3c | If present, read that section's RVA at section + 12. |
| 0x2d1e44–0x2d1e62 | Turn base + RVA into a page-granular x86 code-segment limit. |
| 0x2d643c–0x2d6456 | SGDT, CLI, exchange GDT entry 1 (selector 8), STI, far jump to reload CS; return previous descriptor in EDX:EAX. |
| 0x2d1e67–0x2d1e6c | Save previous descriptor at 0x55f140 / 0x55f144. |
| 0x2d60f9–0x2d610c | Read saved descriptor and restore it with the same helper if nonzero. |

Thus this is a code-segment boundary workaround, not a version/export lookup.
The purpose inferred from its effects is limiting executable reach at the
kernel's discardable initialization boundary, with restoration during shutdown.
The adjacent initialization routine checks old kernel build/hardware data for
other patches and reaches this probe when PRCB + 0x250 is zero. This finding does
not establish that all later kernel-image accesses have been discovered.

The synthetic HLE has no discardable initialization code, so its final section
is `.rdata`. The generated guest routine takes its own no-INIT return branch;
HLE does not replace it or pretend to execute the privileged GDT helper. Export
data-directory RVA/size are zero because this path never walks them. A future
actual export walker requires another investigated contract, not invented code
addresses or firmware contents.

## Loader overlap workaround

The current loader allocates imported DATA cells upward from 0x80000000, one
page apart. The supplied retail XBE's seventeenth cell, HalDiskModelNumber (41),
occupies the first eight bytes at 0x80010000. `Kernel::for_image` therefore takes
`&mut LoadedImage`: before guest entry it copies conflicting known DATA cells to
separate internal RAM, patches only their validated import-thunk slots, restores
thunk permissions, and updates `LoadedImage::data_exports`. The old mapping is
reused as a fragment of the read-only synthetic header. Other mappings are never
silently overwritten. `Kernel::install` on synthetic runtimes rejects overlaps.

This is a kernel-only workaround, not a runtime source change. The desired
loader improvement is to reserve this fixed image interval before assigning
DATA addresses; an eventual runtime-owned header mapping can replace this code.
`LoadedImage::kernel_thunks: BTreeMap<u32, Vec<u32>>` would also avoid re-decoding
its pristine encoded table address for relocation. Attachment must precede any
guest execution; pointers already copied by guest code cannot be relocated.

`tests/image.rs` uses an authored field-walker and a synthetic loader fixture to
check layout, read/write/execute permissions, collisions, duplicate thunk slots,
copy preservation and restored permissions. Its optional local-XBE test checks
the actual retail overlap without keeping proprietary bytes in the repository.
