//! XeLoadSection (327) and XeUnloadSection (328) on the loaded XBE's section
//! headers.
//!
//! XBEIMAGE_SECTION, as used here: +0x04 VirtualAddress, +0x08 VirtualSize,
//! +0x14 SectionName, +0x18 SectionReferenceCount, +0x1c/+0x20 pointers to the
//! head/tail shared-page reference counts (16-bit). Runtime's loader maps
//! every section resident with its pristine bytes, so attachment mirrors the
//! retail loader's accounting: each PRELOAD section starts with one reference
//! and its head/tail pages counted, and non-preload sections start at zero.
//!
//! A load that takes a section from zero to one reference rewrites its
//! virtual range with the pristine bytes captured at attachment (raw data,
//! then zero fill), as retail re-reads it from the image, and counts its
//! head/tail pages. Unloading the last reference uncounts them. Runtime has no
//! unmap, so an unloaded section stays mapped; retail would decommit pages
//! whose shared counts reach zero. Reloading a discarded PRELOAD section
//! stops: its pristine bytes are not retained.
use crate::{Action, Core, Error, status};
use runtime::{
    Guest, GuestFault,
    guest::{PAGE_SIZE, Permissions},
};
use std::collections::BTreeMap;

const VIRTUAL_ADDRESS: u32 = 0x04;
const VIRTUAL_SIZE: u32 = 0x08;
const REFERENCE_COUNT: u32 = 0x18;
const HEAD_SHARED: u32 = 0x1c;
const TAIL_SHARED: u32 = 0x20;
const HEADER_SIZE: u32 = 56;
const FLAG_PRELOAD: u32 = 0x02;

pub(crate) struct Section {
    va: u32,
    vsize: u32,
    /// Pristine bytes for a section that can be reloaded.
    pristine: Option<Vec<u8>>,
}

/// Read the loaded image's section table and set up retail accounting.
pub(crate) fn attach(memory: &mut Guest) -> Result<BTreeMap<u32, Section>, Error> {
    let base = runtime::loader::HEADER_BASE;
    let count = memory.u32(base + 0x11c)?;
    let table = memory.u32(base + 0x120)?;
    if count > 4096 {
        return Err(Error::Invalid("XBE section count"));
    }
    let mut sections = BTreeMap::new();
    for i in 0..count {
        let header = table
            .checked_add(i * HEADER_SIZE)
            .ok_or(Error::Invalid("XBE section table"))?;
        let bytes = memory.read::<{ HEADER_SIZE as usize }>(header)?;
        let word = |at: u32| u32::from_le_bytes(bytes[at as usize..][..4].try_into().unwrap());
        let (flags, va, vsize) = (word(0), word(VIRTUAL_ADDRESS), word(VIRTUAL_SIZE));
        let preload = flags & FLAG_PRELOAD != 0;
        let pristine = if preload || vsize == 0 {
            None
        } else {
            let mut data = vec![0; vsize as usize];
            memory.read_bytes(va, &mut data, 1)?;
            Some(data)
        };
        if preload && vsize != 0 && word(REFERENCE_COUNT) == 0 {
            memory.set_u32(header + REFERENCE_COUNT, 1)?;
            shared(memory, &bytes, 1)?;
        }
        sections.insert(
            header,
            Section {
                va,
                vsize,
                pristine,
            },
        );
    }
    Ok(sections)
}

/// Adjust the head and tail shared-page counts (once when they coincide).
fn shared(memory: &mut Guest, header: &[u8], delta: i32) -> Result<(), GuestFault> {
    let word = |at: u32| u32::from_le_bytes(header[at as usize..][..4].try_into().unwrap());
    let (head, tail) = (word(HEAD_SHARED), word(TAIL_SHARED));
    for pointer in [head, if tail == head { 0 } else { tail }] {
        if pointer != 0 {
            let count = memory.u16(pointer)? as i32;
            memory.set_u16(pointer, (count + delta).clamp(0, u16::MAX as i32) as u16)?;
        }
    }
    Ok(())
}

impl Core {
    pub(crate) fn section_call(
        &mut self,
        memory: &mut Guest,
        header: u32,
        load: bool,
    ) -> Result<Action, GuestFault> {
        let Some(section) = self.sections.get(&header) else {
            return Ok(Action::Stop("not a section header of the loaded image"));
        };
        let bytes = memory.read::<{ HEADER_SIZE as usize }>(header)?;
        let count = u32::from_le_bytes(bytes[REFERENCE_COUNT as usize..][..4].try_into().unwrap());
        if !load {
            if count == 0 {
                return Ok(Action::status(status::INVALID_PARAMETER));
            }
            memory.set_u32(header + REFERENCE_COUNT, count - 1)?;
            if count == 1 {
                shared(memory, &bytes, -1)?;
            }
            return Ok(Action::status(status::SUCCESS));
        }
        let Some(next) = count.checked_add(1) else {
            return Ok(Action::Stop("section reference count overflow"));
        };
        if count == 0 && section.vsize != 0 {
            let Some(pristine) = section.pristine.as_ref() else {
                return Ok(Action::Stop(
                    "reloading a discarded preload section needs the image bytes",
                ));
            };
            restore(memory, section.va, pristine)?;
            shared(memory, &bytes, 1)?;
        }
        memory.set_u32(header + REFERENCE_COUNT, next)?;
        Ok(Action::status(status::SUCCESS))
    }
}

/// Write pristine bytes through each page's permissions, then restore them.
fn restore(memory: &mut Guest, va: u32, bytes: &[u8]) -> Result<(), GuestFault> {
    let end = va as u64 + bytes.len() as u64;
    let mut page = va & !(PAGE_SIZE - 1);
    while (page as u64) < end {
        let start = (page as u64).max(va as u64);
        let stop = (page as u64 + PAGE_SIZE as u64).min(end);
        let permissions = memory
            .mappings()
            .iter()
            .find(|m| m.virtual_start == page && m.len == PAGE_SIZE)
            .map(|m| m.permissions);
        if let Some(permissions) = permissions
            && permissions != Permissions::RW
        {
            memory.protect(page, Permissions::RW).expect("page mapping");
        }
        let chunk = &bytes[(start - va as u64) as usize..(stop - va as u64) as usize];
        let written = memory.write_bytes(start as u32, chunk, 1);
        if let Some(permissions) = permissions {
            memory.protect(page, permissions).expect("page mapping");
        }
        written?;
        let Some(next) = page.checked_add(PAGE_SIZE) else {
            break;
        };
        page = next;
    }
    Ok(())
}
