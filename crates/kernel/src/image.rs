//! A synthetic PE32 image describing the HLE, never copied kernel firmware.
//! See KERNEL-IMAGE.md for the retail header probe and the no-INIT policy.
use crate::Error;
use runtime::{Guest, guest::Permissions};
pub const BASE: u32 = 0x8001_0000;
pub const SIZE: u32 = 0x2000;
pub const NT_OFFSET: u32 = 0x80;
pub const OPTIONAL_SIZE: u16 = 0xe0;
pub const SECTION_OFFSET: u32 = NT_OFFSET + 24 + OPTIONAL_SIZE as u32;

pub(crate) fn install(
    memory: &mut Guest,
    reclaimed: &[runtime::guest::Mapping],
) -> Result<(), Error> {
    let overlaps: Vec<_> = memory
        .mappings()
        .iter()
        .filter(|m| {
            (m.virtual_start as u64) < (BASE + SIZE) as u64
                && m.virtual_start as u64 + m.len as u64 > BASE as u64
        })
        .cloned()
        .collect();
    if overlaps != reclaimed {
        return Err(Error::Invalid("synthetic kernel image range is occupied"));
    }
    let mut cursor = BASE;
    for m in &overlaps {
        if cursor < m.virtual_start {
            memory
                .map_zeroed(cursor, m.virtual_start - cursor, Permissions::RW)
                .map_err(|_| Error::Invalid("synthetic kernel image mapping"))?;
        }
        memory
            .protect(m.virtual_start, Permissions::RW)
            .map_err(|_| Error::Invalid("reclaimed DATA protection"))?;
        cursor = m.virtual_start + m.len;
    }
    if cursor < BASE + SIZE {
        memory
            .map_zeroed(cursor, BASE + SIZE - cursor, Permissions::RW)
            .map_err(|_| Error::Invalid("synthetic kernel image mapping"))?;
    }
    memory.write_bytes(BASE, &vec![0; SIZE as usize], 1)?;
    memory.set_u16(BASE, 0x5a4d)?; // MZ
    memory.set_u32(BASE + 0x3c, NT_OFFSET)?;
    let nt = BASE + NT_OFFSET;
    memory.set_u32(nt, 0x4550)?; // PE\0\0
    memory.set_u16(nt + 4, 0x14c)?; // IMAGE_FILE_MACHINE_I386
    memory.set_u16(nt + 6, 1)?;
    memory.set_u16(nt + 20, OPTIONAL_SIZE)?;
    memory.set_u16(nt + 22, 0x102)?; // executable, 32 bit
    let optional = nt + 24;
    memory.set_u16(optional, 0x10b)?; // PE32
    memory.set_u32(optional + 8, 0x1000)?; // initialized data size
    memory.set_u32(optional + 24, 0x1000)?; // BaseOfData
    memory.set_u32(optional + 28, BASE)?;
    memory.set_u32(optional + 32, 0x1000)?; // SectionAlignment
    memory.set_u32(optional + 36, 0x200)?; // FileAlignment
    memory.set_u32(optional + 56, SIZE)?;
    memory.set_u32(optional + 60, 0x1000)?;
    memory.set_u16(optional + 68, 1)?; // native subsystem
    memory.set_u32(optional + 92, 16)?; // empty data directories, including exports
    let section = BASE + SECTION_OFFSET;
    memory.write_bytes(section, b".rdata\0\0", 1)?;
    memory.set_u32(section + 8, 0x1000)?;
    memory.set_u32(section + 12, 0x1000)?;
    memory.set_u32(section + 36, 0x4000_0040)?; // initialized, readable data
    let starts: Vec<_> = memory
        .mappings()
        .iter()
        .filter(|m| m.virtual_start >= BASE && m.virtual_start < BASE + SIZE)
        .map(|m| m.virtual_start)
        .collect();
    for start in starts {
        memory
            .protect(start, Permissions::READ)
            .map_err(|_| Error::Invalid("synthetic kernel image protection"))?;
    }
    Ok(())
}

/// Work around the loader's DATA area reaching the real kernel header base.
/// This runs before entry, while the thunk table still describes loader imports.
pub(crate) fn reclaim_data(
    image: &mut runtime::LoadedImage,
) -> Result<Vec<runtime::guest::Mapping>, Error> {
    use runtime::{
        guest::{Access, Mapping},
        kernel,
    };
    let mut memory = image.runtime.memory();
    let overlapping: Vec<Mapping> = memory
        .mappings()
        .iter()
        .filter(|m| {
            (m.virtual_start as u64) < (BASE + SIZE) as u64
                && m.virtual_start as u64 + m.len as u64 > BASE as u64
        })
        .cloned()
        .collect();
    if overlapping.is_empty() {
        return Ok(overlapping);
    }
    let mut moves = Vec::new();
    for m in &overlapping {
        let (&ordinal, _) = image
            .data_exports
            .iter()
            .find(|(_, addr)| **addr == m.virtual_start)
            .ok_or(Error::Invalid("kernel header overlaps non-DATA mapping"))?;
        if m.virtual_start < BASE
            || m.virtual_start as u64 + m.len as u64 > (BASE + SIZE) as u64
            || kernel::data(ordinal).is_none_or(|d| d.size != m.len)
        {
            return Err(Error::Invalid(
                "kernel header overlaps unexpected DATA extent",
            ));
        }
        let mut bytes = vec![0; m.len as usize];
        memory.read_bytes(m.virtual_start, &mut bytes, 1)?;
        moves.push((ordinal, m.virtual_start, bytes));
    }
    // Select the same retail/debug keys the loader selected. Do not scan arbitrary
    // guest RAM for matching values, which could alter unrelated pointers.
    let entry = memory.u32(runtime::loader::HEADER_BASE + 0x128)?;
    let thunk_encoded = memory.u32(runtime::loader::HEADER_BASE + 0x158)?;
    let thunk = [
        (0xa8fc_57ab, 0x5b6d_40b6),
        (0x9485_9d4b, 0xefb1_f152),
        (0x40b5_c16e, 0x2290_059d),
    ]
    .into_iter()
    .find_map(|(ek, tk)| (entry ^ ek == image.cpu.eip).then_some(thunk_encoded ^ tk))
    .ok_or(Error::Invalid(
        "DATA relocation requires pristine loaded entry",
    ))?;
    let mut slots = Vec::new();
    let mut terminated = false;
    for i in 0..4096u32 {
        let slot = thunk
            .checked_add(i * 4)
            .ok_or(Error::Invalid("thunk overflow"))?;
        let target = memory.u32(slot)?;
        if target == 0 {
            terminated = true;
            break;
        }
        if !image.data_exports.values().any(|p| *p == target)
            && !kernel::FUNCTION_IMPORTS
                .iter()
                .any(|f| kernel::function_token(f.ordinal) == target)
        {
            return Err(Error::Invalid(
                "DATA relocation requires untouched loader thunks",
            ));
        }
        if moves.iter().any(|(_, old, _)| *old == target) {
            slots.push((slot, target));
        }
    }
    if !terminated
        || moves
            .iter()
            .any(|(_, old, _)| !slots.iter().any(|(_, v)| v == old))
    {
        return Err(Error::Invalid("missing DATA import slot"));
    }
    let mut allocator = crate::memory::Memory::default();
    for (ordinal, old, bytes) in moves {
        let address = allocator
            .internal(&mut memory, bytes.len() as u32)
            .map_err(Error::Resource)?;
        memory.write_bytes(address, &bytes, 1)?;
        for &(slot, _) in slots.iter().filter(|(_, v)| *v == old) {
            let mappings: Vec<_> = memory
                .mappings()
                .iter()
                .filter(|m| {
                    (m.virtual_start as u64) < slot as u64 + 4
                        && m.virtual_start as u64 + m.len as u64 > slot as u64
                })
                .cloned()
                .collect();
            for m in &mappings {
                memory
                    .protect(m.virtual_start, Permissions::RW)
                    .map_err(|_| Error::Invalid("thunk protection"))?;
            }
            let result = memory.set_u32(slot, address);
            for m in &mappings {
                memory
                    .protect(m.virtual_start, m.permissions)
                    .map_err(|_| Error::Invalid("thunk protection restore"))?;
            }
            result?;
            memory.check(slot, 4, Access::Read, 1)?;
        }
        image.data_exports.insert(ordinal, address);
    }
    Ok(overlapping)
}
