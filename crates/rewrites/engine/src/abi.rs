//! Guest calling-convention adapters: one per overridable function.
//!
//! Each adapter takes the guest registers at the function's entry (ESP
//! pointing at the return address), runs the Rust implementation, writes the
//! result where the original leaves it, pops the return address and stack
//! arguments exactly as its `ret n` would, and returns the return target.
//! That is the shape the runtime's override handlers need; see
//! [`crate::runtime_glue`] (feature `runtime-glue`) for registration.
//!
//! These are LTCG functions: arguments arrive in whatever registers the
//! compiler chose for each one, documented per adapter and in
//! `docs/engine/`. Registers the original preserves are preserved. Registers
//! it clobbers ([`Override::clobbers`]) are *also* left untouched here,
//! which is safe (no caller can depend on a clobbered value) and is what a
//! differential comparison must mask. Flags are never preserved by the
//! originals and are not modelled.

use crate::datum::{DataIterator, DatumArray};
use crate::handle::{DatumHandle, raw_or_none};
use crate::locale::{LocaleTable, StringId};
use crate::memory::{GuestMemory, MemFault};
use crate::tags::{self, GroupTag};

/// The integer registers an adapter reads and writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Regs {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    Eax,
    Ecx,
    Edx,
    Ebx,
    Ebp,
    Esi,
    Edi,
}

/// What the original leaves in EAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// All 32 bits are the result.
    Eax,
    /// Only AL is the result; the original leaves EAX's upper 24 bits as
    /// they were on entry.
    Al,
    /// Nothing meaningful (EAX is still written, see the adapter).
    None,
}

pub type AdapterFn = fn(&mut Regs, &mut dyn GuestMemory) -> Result<u32, MemFault>;

/// One overridable guest function.
#[derive(Clone, Copy)]
pub struct Override {
    pub addr: u32,
    /// Assigned name (`docs/engine/engine-map.md` where it has one).
    pub name: &'static str,
    pub run: AdapterFn,
    pub output: Output,
    /// Registers the original may change besides its output. A
    /// differential comparison ignores these; everything else must match.
    pub clobbers: &'static [Reg],
}

impl std::fmt::Debug for Override {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Override({:#x} {})", self.addr, self.name)
    }
}

/// `ret n`: pop the return address, drop `cleanup` argument bytes.
fn ret(r: &mut Regs, m: &dyn GuestMemory, cleanup: u32) -> Result<u32, MemFault> {
    let target = m.u32(r.esp)?;
    r.esp = r.esp.wrapping_add(4).wrapping_add(cleanup);
    Ok(target)
}

/// The `n`th 32-bit stack argument (1-based) at entry.
fn arg(r: &Regs, m: &dyn GuestMemory, n: u32) -> Result<u32, MemFault> {
    m.u32(r.esp.wrapping_add(4 * n))
}

fn ptr(p: Option<u32>) -> u32 {
    p.unwrap_or(0)
}

use Reg::*;

/// Every adapter in this crate, by guest address.
pub static OVERRIDES: &[Override] = &[
    Override {
        addr: 0x0012_2c10,
        name: "tag_get_checked",
        run: tag_get_checked,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0012_2d00,
        name: "tag_group_find",
        run: tag_group_find,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0016_b5f0,
        name: "data_initialize_contiguous",
        run: data_initialize_contiguous,
        output: Output::None,
        clobbers: &[Eax, Ecx, Edx],
    },
    Override {
        addr: 0x0016_b650,
        name: "data_initialize",
        run: data_initialize,
        output: Output::None,
        clobbers: &[Eax, Ecx, Edx],
    },
    Override {
        addr: 0x0016_b6b0,
        name: "data_attach",
        run: data_attach,
        output: Output::None,
        clobbers: &[Eax, Ecx, Edx],
    },
    Override {
        addr: 0x0016_b790,
        name: "data_make_valid",
        run: data_make_valid,
        output: Output::None,
        clobbers: &[Eax, Ecx, Edx],
    },
    Override {
        addr: 0x0016_b7a0,
        name: "data_delete_all",
        run: data_delete_all,
        output: Output::None,
        clobbers: &[Eax, Ecx, Edx],
    },
    Override {
        addr: 0x0016_b840,
        name: "datum_new",
        run: datum_new,
        output: Output::Eax,
        clobbers: &[Ecx],
    },
    Override {
        addr: 0x0016_b910,
        name: "datum_new_at_index",
        run: datum_new_at_index,
        output: Output::Eax,
        clobbers: &[Ecx],
    },
    Override {
        addr: 0x0016_b990,
        name: "datum_new_at_absolute_index",
        run: datum_new_at_absolute_index,
        output: Output::Eax,
        clobbers: &[Ecx],
    },
    Override {
        addr: 0x0016_ba00,
        name: "datum_initialize",
        run: datum_initialize,
        output: Output::None,
        clobbers: &[Eax, Ecx],
    },
    Override {
        addr: 0x0016_ba40,
        name: "datum_delete",
        run: datum_delete,
        output: Output::Eax,
        clobbers: &[Ecx],
    },
    Override {
        addr: 0x0016_bae0,
        name: "datum_try_and_get",
        run: datum_try_and_get,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0016_bb20,
        name: "datum_get_absolute",
        run: datum_get_absolute,
        output: Output::Eax,
        clobbers: &[Ecx],
    },
    Override {
        addr: 0x0016_bb50,
        name: "datum_absolute_index_to_index",
        run: datum_absolute_index_to_index,
        output: Output::Eax,
        clobbers: &[Edx],
    },
    Override {
        addr: 0x0016_bb70,
        name: "data_iterator_next",
        run: data_iterator_next,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0016_bbc0,
        name: "data_next_datum",
        run: data_next_datum,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0016_bc00,
        name: "data_next_index",
        run: data_next_index,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0016_bc40,
        name: "data_last_datum",
        run: data_last_datum,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x0016_bcc0,
        name: "data_previous_datum",
        run: data_previous_datum,
        output: Output::Eax,
        clobbers: &[Ecx, Edx],
    },
    Override {
        addr: 0x001a_02d0,
        name: "locale_string_id_find",
        run: locale_string_id_find,
        output: Output::Eax,
        clobbers: &[Ecx],
    },
    Override {
        addr: 0x001a_0310,
        name: "locale_string_id_exists",
        run: locale_string_id_exists,
        output: Output::Al,
        clobbers: &[Ecx, Edx],
    },
];

pub fn find(addr: u32) -> Option<&'static Override> {
    OVERRIDES.iter().find(|o| o.addr == addr)
}

/// `0x122c10`: ECX = tag datum, EBX = expected group. EAX = metadata or 0.
fn tag_get_checked(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = ptr(tags::tag_get_checked(
        m,
        DatumHandle(r.ecx),
        GroupTag(r.ebx),
    )?);
    ret(r, m, 0)
}

/// `0x122d00`: ECX = group. EAX = group record or 0.
fn tag_group_find(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = ptr(tags::tag_group_find(m, GroupTag(r.ecx))?);
    ret(r, m, 0)
}

/// `0x16b5f0`: ECX = alignment bits, EDX = element size, EBX = capacity;
/// stack (buffer, name, allocator); `ret 0xc`. The original reads the
/// arguments in the order buffer, allocator, name, before any access other
/// than its own pushes.
fn data_initialize_contiguous(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let buffer = arg(r, m, 1)?;
    let allocator = arg(r, m, 3)?;
    let name = arg(r, m, 2)?;
    DatumArray::at(buffer).initialize_contiguous(
        m,
        name,
        r.ebx as i32,
        r.edx as i32,
        r.ecx as u8,
        allocator,
    )?;
    ret(r, m, 0xc)
}

/// `0x16b650`: ESI = header; stack (name, capacity, element size, alignment
/// byte, allocator, bitmap); `ret 0x18`. The arguments are read from the
/// stack between the header stores, at the points the original reads them.
fn data_initialize(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    DatumArray::at(r.esi).initialize_from_stack(m, r.esp.wrapping_add(4))?;
    ret(r, m, 0x18)
}

/// `0x16b6b0`: ESI = header, EAX = capacity, ECX = existing storage.
fn data_attach(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    DatumArray::at(r.esi).attach(m, r.eax as i32, r.ecx)?;
    ret(r, m, 0)
}

/// `0x16b790`: EAX = header. ESI is used internally and restored.
fn data_make_valid(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    DatumArray::at(r.eax).make_valid(m)?;
    ret(r, m, 0)
}

/// `0x16b7a0`: ESI = header.
fn data_delete_all(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    DatumArray::at(r.esi).delete_all(m)?;
    ret(r, m, 0)
}

/// `0x16b840`: EDX = header. EAX = handle or -1.
fn datum_new(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = raw_or_none(DatumArray::at(r.edx).new_datum(m)?);
    ret(r, m, 0)
}

/// `0x16b910`: EAX = requested handle, EDX = header. EAX = handle or -1.
/// Callers read `[edx+0x44]` straight after, relying on EDX surviving.
fn datum_new_at_index(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = raw_or_none(DatumArray::at(r.edx).new_datum_at(m, DatumHandle(r.eax))?);
    ret(r, m, 0)
}

/// `0x16b990`: EDI = absolute index, EDX = header. EAX = handle or -1.
fn datum_new_at_absolute_index(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = raw_or_none(DatumArray::at(r.edx).new_datum_at_absolute(m, r.edi as i32)?);
    ret(r, m, 0)
}

/// `0x16ba00`: EDX = header, ESI = element. Zeroes the element and stamps
/// the next salt; called from inside `0x16b910` and `0x16b990` (at
/// `0x16b969`, `0x16b9da`), so generated copies of those reach it.
fn datum_initialize(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    DatumArray::at(r.edx).initialize_datum(m, r.esi)?;
    ret(r, m, 0)
}

/// `0x16ba40`: EAX = handle, EDX = header. EAX = new live count.
fn datum_delete(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = DatumArray::at(r.edx).delete(m, DatumHandle(r.eax))? as u32;
    ret(r, m, 0)
}

/// `0x16bae0`: EAX = handle, ESI = header. EAX = element or 0.
fn datum_try_and_get(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = ptr(DatumArray::at(r.esi).try_get(m, DatumHandle(r.eax))?);
    ret(r, m, 0)
}

/// `0x16bb20`: EDX = absolute index, ESI = header. EAX = element or 0.
fn datum_get_absolute(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = ptr(DatumArray::at(r.esi).get_absolute(m, r.edx as i32)?);
    ret(r, m, 0)
}

/// `0x16bb50`: ECX = absolute index, EDX = header. EAX = handle or -1.
fn datum_absolute_index_to_index(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = raw_or_none(DatumArray::at(r.edx).handle_of(m, r.ecx as i32)?);
    ret(r, m, 0)
}

/// `0x16bb70`: ESI = iterator. EAX = element or 0; iterator updated.
fn data_iterator_next(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = ptr(DataIterator::at(r.esi).next(m)?);
    ret(r, m, 0)
}

/// `0x16bbc0`: EAX = previous handle or -1, ESI = header. EAX = next or -1.
fn data_next_datum(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = raw_or_none(DatumArray::at(r.esi).next_datum(m, DatumHandle::from_raw(r.eax))?);
    ret(r, m, 0)
}

/// `0x16bc00`: ECX = header, EDX = start index. EAX = index or -1.
fn data_next_index(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = DatumArray::at(r.ecx)
        .next_index(m, r.edx as i32)?
        .unwrap_or(-1) as u32;
    ret(r, m, 0)
}

/// `0x16bc40`: stack (header); `ret 4`. EAX = last handle or -1.
///
/// The original keeps its cursor in its own argument slot: after reading
/// the header pointer it stores -1 there, then the current handle at the
/// top of each step (0x16bc4e). So on return the slot holds the result,
/// and after a fault it holds the last handle reached. Reproduced: it is
/// caller-visible memory. This is the walk of
/// [`DatumArray::last_datum`] with those stores.
fn data_last_datum(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let slot = r.esp.wrapping_add(4);
    let array = DatumArray::at(m.u32(slot)?);
    let mut cur = None;
    loop {
        m.set_u32(slot, raw_or_none(cur))?;
        match array.next_datum(m, cur)? {
            Some(h) if !h.is_none() => cur = Some(h),
            _ => break,
        }
    }
    r.eax = raw_or_none(cur);
    ret(r, m, 4)
}

/// `0x16bcc0`: EAX = header, EDX = next handle or -1. EAX = previous or -1.
fn data_previous_datum(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = raw_or_none(DatumArray::at(r.eax).previous_datum(m, DatumHandle::from_raw(r.edx))?);
    ret(r, m, 0)
}

/// `0x1a02d0`: EAX = first entry, ECX = entry count, EDX = locale record,
/// EDI = StringID. EAX = string address.
fn locale_string_id_find(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    r.eax = LocaleTable::at(r.edx).find(m, r.eax as i32, r.ecx as i32, StringId(r.edi))?;
    ret(r, m, 0)
}

/// `0x1a0310`: ESI = locale record, ECX = first entry, EBX = StringID;
/// stack (entry count); `ret 4`. AL = found. The original tests the loaded
/// flag first and reads the count only when the table is loaded.
fn locale_string_id_exists(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let t = LocaleTable::at(r.esi);
    let found = t.is_loaded(m)? && {
        let count = arg(r, m, 1)? as i32;
        t.contains_loaded(m, r.ecx as i32, count, StringId(r.ebx))?
    };
    r.eax = (r.eax & !0xff) | found as u32;
    ret(r, m, 4)
}
