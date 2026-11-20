//! What generated modules import beyond `h2abi.h`'s accessors, and the
//! loop that registers a module's tables. The contract is
//! `crates/recompiler/emit/RUNTIME-CONTRACT.md`; the C declarations live in the
//! emitter's `h2gen.h`, which is written next to every generated module.

use crate::{
    Dispatch, ExecCtx, ExecCtxOpaque, GuestFn,
    cpu::ABI_VERSION,
    dispatch::{Handler, ModuleTable, RegistrationError},
    ffi::GeneratedAbiVersion,
};
use std::{collections::HashMap, os::raw::c_char};

/// The `h2x87_v1_*` helpers generated x87 code calls (`h2x87.h`) are
/// `crates/recompiler/x87`'s `#[no_mangle]` entry points; naming the crate links them
/// into every binary that has the runtime.
pub use x87::abi::ffi as x87_helpers;

/// Record a non-memory guest exception (codes 1-7, 13) or a generated stop
/// (codes 8-12) and return its event id, which is never zero. Generated
/// code returns FAULT (exceptions) or STOP (stops) with that id and
/// `cpu->eip = pc`. Exceptions are latched like failed memory accesses: the
/// invocation ends in FAULT with this id even if the handler carries on. An
/// unknown code is recorded as `Event::UnknownShimEvent` and latched too.
///
/// # Safety
/// `exec` is the current exclusive handle supplied to the running handler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2shim_v1_event(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    saved_eip: u32,
    code: u32,
    arg: u32,
) -> u32 {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    ctx.shim_event(pc, saved_eip, code, arg)
}

/// `rdtsc` at `pc`: the guest's time-stamp counter ([`crate::tsc`]). Never
/// fails, records nothing, and returns a different value every call.
///
/// # Safety
/// `exec` is the current exclusive handle supplied to the running handler.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2shim_v1_rdtsc(exec: *mut ExecCtxOpaque, _pc: u32) -> u64 {
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    ctx.runtime().read_tsc()
}

/// Longest string kept from a module's diagnostic, in input bytes.
pub const MAX_DIAGNOSTIC: usize = 512;

/// Copy a diagnostic string, reading at most `MAX_DIAGNOSTIC` bytes: the
/// scan stops at the first NUL or at the cap, whichever comes first, and
/// never touches a byte past either.
///
/// # Safety
/// `p` is null, or readable from `p` up to and including its NUL terminator
/// or up to `MAX_DIAGNOSTIC` bytes, whichever is shorter. The bytes must not
/// change during the call.
unsafe fn owned(p: *const c_char) -> String {
    if p.is_null() {
        return "<null>".to_owned();
    }
    let p = p.cast::<u8>();
    let mut len = 0;
    // SAFETY: every byte read is before the terminator or within the cap,
    // which the caller guarantees is readable.
    while len < MAX_DIAGNOSTIC && unsafe { p.add(len).read() } != 0 {
        len += 1;
    }
    // SAFETY: the `len` bytes just scanned are readable and unchanged.
    let bytes = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Record an instruction the lifter does not implement and return its
/// event id. Generated code then returns STOP with `cpu->eip = pc`. Both
/// strings are copied, each truncated to `MAX_DIAGNOSTIC` bytes; null
/// pointers are reported as `<null>`.
///
/// # Safety
/// `exec` is the current exclusive handle. `mnemonic` and `detail` are each
/// null, or readable through their NUL terminator or through the first
/// `MAX_DIAGNOSTIC` bytes, whichever is shorter, for the duration of this
/// call. An unterminated buffer is only safe if it has that many readable
/// bytes; a raw pointer alone cannot make a shorter one safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2shim_v1_unsupported(
    exec: *mut ExecCtxOpaque,
    pc: u32,
    mnemonic: *const c_char,
    detail: *const c_char,
) -> u32 {
    // SAFETY: string validity is the caller's contract.
    let (mnemonic, detail) = unsafe { (owned(mnemonic), owned(detail)) };
    // SAFETY: invocation-scoped exclusive handle, as required by the ABI.
    let ctx = unsafe { &mut *exec.cast::<ExecCtx>() };
    ctx.unsupported(pc, mnemonic, detail)
}

/// `H2GenEntry` from `h2gen.h`. Kind 0 is a function entry, kind 1 an
/// address-taken interior label of the same handler.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct H2GenEntry {
    pub addr: u32,
    pub kind: u32,
    pub handler: Option<GuestFn>,
    pub abi_version: Option<GeneratedAbiVersion>,
}

/// `H2GenResume`: a return to `pc` resumes `owner`'s handler at `start`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H2GenResume {
    pub pc: u32,
    pub owner: u32,
    pub start: u32,
}

/// `H2GenModule`: both tables have `count` rows followed by a zero row.
/// A module with [`FEATURE_ENTRY_TABLE`] continues as
/// [`H2GenEntryModule`].
#[repr(C)]
#[derive(Debug)]
pub struct H2GenModule {
    pub abi_version: u32,
    pub entry_count: u32,
    pub resume_count: u32,
    /// A set of `FEATURE_*` bits. A bit this runtime does not know is a
    /// registration error, so a newer module never runs on assumptions an
    /// older runtime cannot honour.
    pub features: u32,
    pub entries: *const H2GenEntry,
    pub resumes: *const H2GenResume,
}

/// `H2GenModule` as h2gen.h declares it for a module with
/// [`FEATURE_ENTRY_TABLE`]: the head, then the entry table's `pcmap`.
#[repr(C)]
#[derive(Debug)]
pub struct H2GenEntryModule {
    pub module: H2GenModule,
    /// `text_hi - text_lo` entry indices, one per byte of
    /// `[text_lo, text_hi)`: entry row `r` (index `r + 1`) at its address,
    /// 0 elsewhere. Null when the module has no entries.
    pub pcmap: *const u16,
    pub text_lo: u32,
    pub text_hi: u32,
}

/// `H2GenModule` as h2gen.h declares it for a module with
/// [`FEATURE_FLAT_RAM`]: the entry-table layout, then the fixup table
/// (`H2_FIX_BOUNDS`, h2fastmem.h).
#[repr(C)]
#[derive(Debug)]
pub struct H2GenFlatModule {
    pub entry: H2GenEntryModule,
    /// The fixup rows, `[fix_start, fix_end)`, in [`hostfault::Format`]
    /// `fix_format`; both null for none.
    pub fix_start: *const u8,
    pub fix_end: *const u8,
    pub fix_format: u32,
    pub fix_pad: u32,
}

const _: () = assert!(size_of::<H2GenResume>() == 12);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<H2GenEntry>() == 24
        && size_of::<H2GenModule>() == 32
        && size_of::<H2GenEntryModule>() == 48
        && size_of::<H2GenFlatModule>() == 72
);

pub const ENTRY_FUNCTION: u32 = 0;
pub const ENTRY_INTERIOR: u32 = 1;

/// Module feature (`H2GEN_FEATURE_ACCOUNTING`): the module's handlers count
/// their own guest memory accesses (access accounting, version 1). They use
/// the uncharged `h2abi_v1_read_counted`/`write_counted`, settle the count
/// with `h2abi_v1_charge` on every way out of a handler and before anything
/// that spends or observes the slice (atomics, which still charge
/// themselves, and `rdtsc`), and leave a loop for the runner once the count
/// reaches `h2abi_v1_budget`, where the charged accessors would have set the
/// slice bit. REP string loops poll after each iteration. Charged totals at
/// those points equal an unflagged module's; only when a REP loop gives up
/// its slice differs (it can now leave mid-instruction, at its own PC).
pub const FEATURE_ACCOUNTING: u32 = 1;
/// Module feature (`H2GEN_FEATURE_DIRECT_CALLS`, `H2_DIRECT_CALLS`): the
/// module's handlers read the direct-call block through the context's second
/// word and may invoke each other's handlers directly ([`crate::direct`]).
pub const FEATURE_DIRECT_CALLS: u32 = 2;
/// Module feature (`H2GEN_FEATURE_DIRECT_VIEW`, with `H2_DIRECT_CALLS`,
/// `H2_FASTMEM` and `H2_DIRECT_VIEW`): a direct caller lends its inline RAM
/// view to the callee through the direct-call block's last word, so the
/// callee neither acquires nor releases one ([`crate::direct`]). A runtime
/// whose block ends before that word must refuse the module.
pub const FEATURE_DIRECT_VIEW: u32 = 4;
/// Module feature (`H2GEN_FEATURE_ENTRY_TABLE`): entry row `r` has entry
/// index `r + 1`, the module exports `pcmap` ([`H2GenEntryModule`]), and
/// its loop and bulk-copy polls mask
/// [`IRQ_QUEUED`](crate::preempt::IRQ_QUEUED) out of the preempt word while
/// its direct calls test the whole word. Registering it makes the runtime
/// keep its entry table ([`crate::direct`]) and turns on
/// [`Runtime::enable_leave_word`](crate::Runtime::enable_leave_word).
/// [`FEATURE_DIRECT_CALLS`] needs it: an image built before it is refused.
pub const FEATURE_ENTRY_TABLE: u32 = 8;
/// Module feature (`H2GEN_FEATURE_HOST_CALLS`, every direct-call module):
/// host calls in place. The direct-call block carries the runtime's
/// host-call thunk (`h2rt_thunk`, [`crate::direct`]); a direct call invokes
/// whatever the entry table holds, which is the thunk for an overridden
/// entry, and an indirect call or jump outside the module's text calls the
/// thunk. [`FEATURE_DIRECT_CALLS`] needs it: an image built before it reads
/// the block at the old offsets and is refused.
pub const FEATURE_HOST_CALLS: u32 = 32;
/// Module feature (`H2GEN_FEATURE_FLAT_RAM`, `H2_FLAT_RAM=1` at build):
/// the flat memory model (docs/runtime/contract-thin-transfers-flat-ram.md,
/// section 4.1). Every access is one host instruction at the runtime's flat
/// RAM window base plus the guest address, read through the flat block at
/// the context's third word ([`crate::ffi::H2Mem`]), with no gate; a
/// faulting one resumes at its fixup row's landing label and takes the
/// checked helper. The module carries its fixup table
/// ([`H2GenFlatModule`]), which registration checks and registers with
/// `hostfault`; it needs [`FEATURE_ENTRY_TABLE`] (the layout before it) and
/// a host with a window. A runtime with such a module makes guest RAM
/// aliasable, sends window faults to the fixups and parks generated code
/// for memory guards ([`crate::Runtime::enable_parking`]). An image without
/// it keeps the paged model.
pub const FEATURE_FLAT_RAM: u32 = 16;
/// Every module feature this runtime implements.
pub const FEATURES: u32 = FEATURE_ACCOUNTING
    | FEATURE_DIRECT_CALLS
    | FEATURE_DIRECT_VIEW
    | FEATURE_ENTRY_TABLE
    | FEATURE_FLAT_RAM
    | FEATURE_HOST_CALLS;

/// What `register_generated_module` added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredModule {
    pub entries: usize,
    pub resumes: usize,
    /// The module's features ([`FEATURE_ACCOUNTING`]).
    pub features: u32,
}

fn row(table: ModuleTable, index: usize, error: RegistrationError) -> RegistrationError {
    RegistrationError::ModuleRow {
        table,
        index,
        error: Box::new(error),
    }
}

/// # Safety
/// `ptr` is null or valid for `count + 1` rows.
unsafe fn table<'a, T>(
    ptr: *const T,
    count: u32,
    which: ModuleTable,
) -> Result<&'a [T], RegistrationError> {
    if ptr.is_null() {
        return if count == 0 {
            Ok(&[])
        } else {
            Err(RegistrationError::NullTable(which))
        };
    }
    // SAFETY: the caller guarantees count rows plus the terminator.
    Ok(unsafe { std::slice::from_raw_parts(ptr, count as usize + 1) })
}

impl Dispatch {
    /// Register every handler and resume of a loaded generated module, after
    /// RUNTIME-CONTRACT.md §3. `module_version` is the module's
    /// `h2gen_module_abi_version`; every entry row is checked with its own
    /// object's probe. Entry rows (kinds 0 and 1) register as generated
    /// originals that dispatch with `start = addr`; then every resume row
    /// registers against its kind-0 owner.
    ///
    /// The whole module is validated before anything is added, so an error
    /// leaves this dispatch unchanged. Errors name the row.
    ///
    /// A module with [`FEATURE_ENTRY_TABLE`] also has its `pcmap` checked
    /// against its rows (every row's byte maps to the row), and becomes
    /// this dispatch's entry table; a second one is refused. A module with
    /// [`FEATURE_DIRECT_CALLS`] and no entry table, or without
    /// [`FEATURE_HOST_CALLS`], is refused. A module with
    /// [`FEATURE_FLAT_RAM`] needs the entry table and a host with a flat RAM
    /// window, and its fixup table must be sorted
    /// ([`RegistrationError::FixupTable`]); it is registered with
    /// `hostfault` for as long as this dispatch (or a fork of it) lives.
    ///
    /// # Safety
    /// `module` and its tables (each `count` rows plus a zero row) must be
    /// valid, and its handlers and probes must obey the ABI v1 contract in
    /// ffi.rs and h2abi.h and stay loaded while registered. With
    /// [`FEATURE_ENTRY_TABLE`], `module` must be the head of an
    /// [`H2GenEntryModule`] whose `pcmap` is null or valid for
    /// `text_hi - text_lo` elements. With [`FEATURE_FLAT_RAM`], the head
    /// of an [`H2GenFlatModule`] whose fixup rows are readable, in their
    /// format, and stay loaded with the module.
    pub unsafe fn register_generated_module(
        &mut self,
        module: &H2GenModule,
        module_version: GeneratedAbiVersion,
    ) -> Result<RegisteredModule, RegistrationError> {
        // SAFETY: the caller supplies a loaded probe.
        let probe = unsafe { module_version() };
        if probe != ABI_VERSION || module.abi_version != ABI_VERSION {
            return Err(RegistrationError::ModuleAbiVersion {
                probe,
                table: module.abi_version,
                runtime: ABI_VERSION,
            });
        }
        if module.features & !FEATURES != 0 {
            return Err(RegistrationError::UnknownFeatures(module.features & !FEATURES));
        }
        let with_table = module.features & FEATURE_ENTRY_TABLE != 0;
        if module.features & FEATURE_DIRECT_CALLS != 0 && !with_table {
            return Err(RegistrationError::DirectCallsWithoutEntryTable);
        }
        if module.features & FEATURE_DIRECT_CALLS != 0 && module.features & FEATURE_HOST_CALLS == 0 {
            return Err(RegistrationError::DirectCallsWithoutHostCalls);
        }
        if with_table && self.entry_table().is_some() {
            return Err(RegistrationError::SecondEntryTable);
        }
        let flat = module.features & FEATURE_FLAT_RAM != 0;
        if flat && !with_table {
            return Err(RegistrationError::FlatRamWithoutEntryTable);
        }
        if flat && !crate::window::supported() {
            return Err(RegistrationError::FlatRamUnsupported);
        }
        // SAFETY: the caller guarantees the table shapes.
        let entries = unsafe { table(module.entries, module.entry_count, ModuleTable::Entries)? };
        // SAFETY: as above.
        let resumes = unsafe { table(module.resumes, module.resume_count, ModuleTable::Resumes)? };
        let entries = counted(entries, ModuleTable::Entries, |e| {
            e.addr == 0 && e.kind == 0 && e.handler.is_none() && e.abi_version.is_none()
        })?;
        let resumes = counted(resumes, ModuleTable::Resumes, |r| {
            *r == H2GenResume {
                pc: 0,
                owner: 0,
                start: 0,
            }
        })?;
        entries_ok(self, entries)?;
        let entry_table = if with_table {
            // SAFETY: the caller guarantees the extended layout with the
            // feature.
            let ext = unsafe { &*std::ptr::from_ref(module).cast::<H2GenEntryModule>() };
            // SAFETY: as above, for pcmap.
            Some(unsafe { entry_table(entries, ext)? })
        } else {
            None
        };
        // Last of the checks that may fail: the table registers as it is
        // checked, and unregisters if anything after it refuses.
        let fixups = if flat {
            // SAFETY: the caller guarantees the flat layout with the feature.
            let ext = unsafe { &*std::ptr::from_ref(module).cast::<H2GenFlatModule>() };
            // SAFETY: as above, for the rows.
            Some(unsafe { fixup_table(ext)? })
        } else {
            None
        };
        let kinds: HashMap<u32, u32> = entries.iter().map(|e| (e.addr, e.kind)).collect();
        let mut seen = std::collections::HashSet::with_capacity(resumes.len());
        for (index, r) in resumes.iter().enumerate() {
            let error = if r.pc == 0 {
                Some(RegistrationError::ZeroAddress)
            } else if kinds.get(&r.owner) != Some(&ENTRY_FUNCTION) {
                Some(RegistrationError::OwnerNotFunction(r.owner))
            } else if self.has_resume(r.pc) || !seen.insert(r.pc) {
                Some(RegistrationError::DuplicateResume(r.pc))
            } else {
                None
            };
            if let Some(error) = error {
                return Err(row(ModuleTable::Resumes, index, error));
            }
        }
        for e in entries {
            // Validated above: no duplicates, handler present.
            let handler = e.handler.expect("validated entry row");
            self.register_generated_handler(e.addr, Handler::Native(handler))
                .expect("validated entry row");
        }
        for r in resumes {
            self.register_resume(r.pc, r.owner, r.start)
                .expect("validated resume row");
        }
        if let Some(t) = entry_table {
            self.set_entry_table(t);
        }
        if let Some(t) = fixups {
            self.set_flat_fixups(t);
        }
        Ok(RegisteredModule {
            entries: entries.len(),
            resumes: resumes.len(),
            features: module.features,
        })
    }
}

/// The entry table of a module with [`FEATURE_ENTRY_TABLE`], checked: at
/// most 65,535 rows, and `pcmap` is non-zero exactly at the rows'
/// addresses, where it holds the row's own index (a row its range leaves
/// out is called through the runner).
///
/// # Safety
/// `ext.pcmap` is null or valid for `text_hi - text_lo` elements.
unsafe fn entry_table(
    rows: &[H2GenEntry],
    ext: &H2GenEntryModule,
) -> Result<crate::direct::EntryTable, RegistrationError> {
    let bad = |what: String| Err(RegistrationError::EntryTable(what));
    if rows.len() > u16::MAX as usize {
        return bad(format!("{} rows, more than 16-bit indices hold", rows.len()));
    }
    let (lo, hi) = (ext.text_lo, ext.text_hi);
    if hi < lo || (hi > lo && ext.pcmap.is_null()) {
        return bad(format!("pcmap {:p} for [{lo:#x}, {hi:#x})", ext.pcmap));
    }
    let map: &[u16] = if hi > lo {
        // SAFETY: the caller guarantees pcmap covers [lo, hi).
        unsafe { std::slice::from_raw_parts(ext.pcmap, (hi - lo) as usize) }
    } else {
        &[]
    };
    for (r, e) in rows.iter().enumerate() {
        if !(lo..hi).contains(&e.addr) {
            continue;
        }
        let at = map[(e.addr - lo) as usize];
        if usize::from(at) != r + 1 {
            return bad(format!("pcmap at {:#010x} is {at}, row {r} has index {}", e.addr, r + 1));
        }
    }
    // And nothing else: an index anywhere else would send an indirect call
    // to a handler that does not start there.
    for (off, &at) in map.iter().enumerate() {
        let addr = lo + off as u32;
        if at != 0 && rows.get(usize::from(at) - 1).is_none_or(|e| e.addr != addr) {
            return bad(format!("pcmap at {addr:#010x} is {at}, which is no row there"));
        }
    }
    let table = rows
        .iter()
        .map(|e| (e.addr, (e.kind == ENTRY_FUNCTION).then_some(e.handler).flatten()))
        .collect();
    Ok(crate::direct::EntryTable::new(table, ext.pcmap, (lo, hi)))
}

/// A flat module's fixup table, checked and registered with `hostfault`
/// (rows sorted by instruction address, never sorted here).
///
/// # Safety
/// `ext`'s fixup bounds are null or a readable table in `fix_format` that
/// stays loaded while the returned table lives.
unsafe fn fixup_table(ext: &H2GenFlatModule) -> Result<hostfault::Table, RegistrationError> {
    let format = hostfault::Format::from_raw(ext.fix_format)
        .ok_or(RegistrationError::FixupTable(hostfault::Error::BadTable))?;
    if ext.fix_start.is_null() != ext.fix_end.is_null() {
        return Err(RegistrationError::FixupTable(hostfault::Error::BadTable));
    }
    // SAFETY: the caller's contract, passed on; null bounds are an empty
    // table, which registers nothing.
    unsafe { hostfault::register_table(ext.fix_start, ext.fix_end, format) }
        .map_err(RegistrationError::FixupTable)
}

/// Drop the zero row after the counted rows, checking it is one.
fn counted<T>(
    rows: &[T],
    which: ModuleTable,
    is_zero: impl Fn(&T) -> bool,
) -> Result<&[T], RegistrationError> {
    match rows.split_last() {
        None => Ok(rows),
        Some((last, rows)) if is_zero(last) => Ok(rows),
        Some(_) => Err(RegistrationError::MissingTerminator(which)),
    }
}

fn entries_ok(dispatch: &Dispatch, rows: &[H2GenEntry]) -> Result<(), RegistrationError> {
    let mut seen = std::collections::HashSet::with_capacity(rows.len());
    for (index, e) in rows.iter().enumerate() {
        let error = if e.addr == 0 {
            Some(RegistrationError::ZeroAddress)
        } else if !matches!(e.kind, ENTRY_FUNCTION | ENTRY_INTERIOR) {
            Some(RegistrationError::EntryKind {
                addr: e.addr,
                kind: e.kind,
            })
        } else if let (Some(_), Some(probe)) = (e.handler, e.abi_version) {
            // SAFETY: the module's caller guarantees its probes are loaded.
            let version = unsafe { probe() };
            if version != ABI_VERSION {
                Some(RegistrationError::AbiVersion(version))
            } else if dispatch.has_generated(e.addr) || !seen.insert(e.addr) {
                Some(RegistrationError::DuplicateGenerated(e.addr))
            } else {
                None
            }
        } else {
            Some(RegistrationError::NullEntry(e.addr))
        };
        if let Some(error) = error {
            return Err(row(ModuleTable::Entries, index, error));
        }
    }
    Ok(())
}
