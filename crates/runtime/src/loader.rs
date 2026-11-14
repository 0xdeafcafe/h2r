//! Minimal entry-thread loader. TLS metadata is exposed, not pre-executed:
//! entry code must calculate and store its own TLS index/sizing globals.

use crate::{
    CpuAbi, Dispatch, ExecCtx, Guest, GuestFault, Runtime,
    exec::HARNESS_EXIT,
    guest::{MapError, PAGE_SIZE, Permissions},
    kernel,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const HEADER_BASE: u32 = 0x10000;
pub const STACK_TOP: u32 = 0x7000_0000;
pub const KERNEL_DATA_BASE: u32 = 0x8000_0000;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Xbe(#[from] xbe::Error),
    #[error(transparent)]
    Mapping(#[from] MapError),
    #[error(transparent)]
    Fault(#[from] GuestFault),
    #[error("invalid XBE layout: {0}")]
    Invalid(&'static str),
    #[error("unknown Xbox kernel export {0}")]
    UnknownImport(u32),
}

/// A shared page grants more access than at least one contributing region
/// requested. The union is necessary for packed Xbox image sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagePermissionNote {
    pub page: u32,
    pub effective: Permissions,
    pub contributors: Vec<(String, Permissions)>,
}

pub struct LoadedImage {
    pub runtime: Arc<Runtime>,
    pub cpu: CpuAbi,
    pub tls: Option<xbe::TlsDirectory>,
    /// Aligned TLS payload plus its leading pointer slot.
    pub tls_data_size: Option<u32>,
    pub stack_bottom: u32,
    pub stack_top: u32,
    /// Guest storage addresses, suitable for HLE updates under memory's lock.
    pub data_exports: BTreeMap<u32, u32>,
    pub permission_notes: Vec<PagePermissionNote>,
}
impl LoadedImage {
    pub fn context(&self) -> ExecCtx {
        ExecCtx::new(Arc::clone(&self.runtime), 1)
    }
}

fn reserved(va: u32, len: u32) -> bool {
    let end = va as u64 + len as u64;
    ((va as u64) < crate::exec::CONTINUATION_END as u64
        && end > crate::exec::CONTINUATION_BASE as u64)
        || ((va as u64) < kernel::KERNEL_TOKEN_END as u64 && end > kernel::KERNEL_TOKEN_BASE as u64)
        || ((va as u64) <= HARNESS_EXIT as u64 && end > HARNESS_EXIT as u64)
}

/// Load only user-supplied bytes; no executable data is retained outside RAM.
/// All sections are initially resident for this milestone. Section permissions
/// are restored after patching kernel thunk slots; startup headers stay writable.
pub fn load_xbe(bytes: &[u8]) -> Result<LoadedImage, LoadError> {
    let image = xbe::Xbe::parse(bytes)?;
    if image.base != HEADER_BASE || image.size_of_headers < 0x178 {
        return Err(LoadError::Invalid("header base/size"));
    }
    let header = bytes
        .get(..image.size_of_headers as usize)
        .ok_or(LoadError::Invalid("truncated headers"))?;
    let image_end = image
        .base
        .checked_add(image.size_of_image)
        .ok_or(LoadError::Invalid("image range overflow"))?;
    if image.size_of_headers > image.size_of_image {
        return Err(LoadError::Invalid("headers exceed image"));
    }
    let mut memory = Guest::new();
    let mut pages = BTreeMap::<u32, Permissions>::new();
    let mut contributors = BTreeMap::<u32, Vec<(String, Permissions)>>::new();
    let round_end = |end: u32| {
        end.checked_add(PAGE_SIZE - 1)
            .map(|v| v & !(PAGE_SIZE - 1))
            .ok_or(LoadError::Invalid("page range overflow"))
    };
    let header_end = image.base + image.size_of_headers;
    for page in (image.base..round_end(header_end)?).step_by(PAGE_SIZE as usize) {
        pages.insert(page, Permissions::RW);
        contributors.insert(page, vec![("headers".into(), Permissions::RW)]);
    }
    let mut extents = Vec::new();
    for section in &image.sections {
        let end = section
            .va
            .checked_add(section.vsize)
            .ok_or(LoadError::Invalid("section range overflow"))?;
        if section.va < header_end || end > image_end || reserved(section.va, section.vsize) {
            return Err(LoadError::Invalid(
                "section outside image or in reserved control range",
            ));
        }
        if section.vsize == 0 {
            continue;
        }
        extents.push((section.va, end));
        let first = section.va & !(PAGE_SIZE - 1);
        let last = (end - 1) & !(PAGE_SIZE - 1);
        for page in (first..round_end(end)?).step_by(PAGE_SIZE as usize) {
            // Each section contributes permissions to each page it occupies.
            // A shared page is writable if any resident section needs writes
            // there. XBE bits 0x10/0x20 remove that section's head/tail write.
            let writable = section.flags.writable()
                && !(page == first && section.flags.0 & 0x10 != 0)
                && !(page == last && section.flags.0 & 0x20 != 0);
            let permissions = match (writable, section.flags.executable()) {
                (true, true) => Permissions::RWX,
                (true, false) => Permissions::RW,
                (false, true) => Permissions::RX,
                (false, false) => Permissions::READ,
            };
            contributors
                .entry(page)
                .or_default()
                .push((section.name.clone(), permissions));
            pages
                .entry(page)
                .and_modify(|p| *p = p.union(permissions))
                .or_insert(permissions);
        }
    }
    let permission_notes = contributors
        .into_iter()
        .filter_map(|(page, contributors)| {
            let effective = pages[&page];
            contributors
                .iter()
                .any(|(_, requested)| *requested != effective)
                .then_some(PagePermissionNote {
                    page,
                    effective,
                    contributors,
                })
        })
        .collect();
    extents.sort_unstable();
    if extents.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(LoadError::Invalid("overlapping section bytes"));
    }
    for &page in pages.keys() {
        memory.map_zeroed(page, PAGE_SIZE, Permissions::RW)?;
    }
    memory.write_bytes(image.base, header, 1)?;
    for section in &image.sections {
        let data = image.section_bytes(section)?;
        if section.vsize != 0 {
            memory.write_bytes(
                section.va,
                &data[..data.len().min(section.vsize as usize)],
                1,
            )?;
        }
    }
    // Parse TLS from the loaded virtual image, including any BSS-backed bytes.
    let tls = if image.tls_addr == 0 {
        None
    } else {
        let bytes = memory.read::<24>(image.tls_addr)?;
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        Some(xbe::TlsDirectory {
            raw_start: word(0),
            raw_end: word(4),
            index_addr: word(8),
            callbacks_addr: word(12),
            zero_fill: word(16),
            characteristics: word(20),
        })
    };
    let stack_size = image
        .stack_commit
        .checked_add(PAGE_SIZE - 1)
        .map(|v| v & !(PAGE_SIZE - 1))
        .filter(|v| *v >= PAGE_SIZE && *v <= STACK_TOP)
        .ok_or(LoadError::Invalid("stack commit"))?;
    let stack_bottom = STACK_TOP - stack_size;
    memory.map_zeroed(stack_bottom, stack_size, Permissions::RW)?;
    memory.set_u32(STACK_TOP - 4, HARNESS_EXIT)?;

    // Each DATA ordinal has one identity even if multiple thunk slots import
    // it. Fixed-width descriptors/pointer cells stay guest-backed and zeroed;
    // this loader does not pretend to implement their kernel semantics.
    let mut dispatch = Dispatch::default();
    let mut data_exports = BTreeMap::new();
    let mut next_data = KERNEL_DATA_BASE;
    let mut functions = BTreeSet::new();
    for import in image.kernel_imports()? {
        let target = if let Some(data) = kernel::data(import.ordinal) {
            if let Some(&addr) = data_exports.get(&import.ordinal) {
                addr
            } else {
                let addr = next_data;
                memory.map_zeroed(addr, data.size, Permissions::RW)?;
                data_exports.insert(import.ordinal, addr);
                next_data = addr
                    .checked_add(PAGE_SIZE)
                    .ok_or(LoadError::Invalid("DATA export range"))?;
                addr
            }
        } else {
            let function = *kernel::function(import.ordinal)
                .ok_or(LoadError::UnknownImport(import.ordinal))?;
            let token = kernel::function_token(function.ordinal);
            if functions.insert(token) {
                dispatch.register_override(token, function.name, move |cpu, ctx, _| {
                    kernel::stop(cpu, ctx, function)
                });
            }
            token
        };
        memory.set_u32(import.slot_va, target)?;
    }
    for (page, permissions) in pages {
        memory.protect(page, permissions)?;
    }
    let tls_data_size = if let Some(tls) = tls {
        let raw_size = tls
            .raw_end
            .checked_sub(tls.raw_start)
            .ok_or(LoadError::Invalid("TLS raw bounds"))?;
        let size = raw_size
            .checked_add(tls.zero_fill)
            .and_then(|v| v.checked_add(15))
            .map(|v| v & !15)
            .and_then(|v| v.checked_add(4))
            .ok_or(LoadError::Invalid("TLS payload size overflow"))?;
        memory.check(
            tls.raw_start,
            raw_size as usize,
            crate::guest::Access::Read,
            1,
        )?;
        // Validate the final page permissions, not temporary loader writes.
        memory.check(tls.index_addr, 4, crate::guest::Access::Write, 1)?;
        if tls.callbacks_addr != 0 && memory.u32(tls.callbacks_addr)? != 0 {
            return Err(LoadError::Invalid("TLS callbacks are not supported yet"));
        }
        Some(size)
    } else {
        None
    };
    let mut cpu = CpuAbi::xbox_thread();
    cpu.eip = image.entry;
    cpu.esp = STACK_TOP - 4;
    Ok(LoadedImage {
        runtime: Runtime::new(memory, dispatch),
        cpu,
        tls,
        tls_data_size,
        stack_bottom,
        stack_top: STACK_TOP,
        data_exports,
        permission_notes,
    })
}
