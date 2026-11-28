//! Safe, deterministic Xbox kernel HLE. Unsupported contracts retain the
//! runtime's diagnostic stop behavior; success always describes a real effect.
#![forbid(unsafe_code)]
pub mod abi;
pub mod clock;
pub mod console;
pub mod content;
pub mod crypto;
mod dispatcher;
pub mod dos;
pub use dispatcher::{DeferredSignal, DispatcherReference};
mod exports;
mod files;
pub mod handles;
pub mod image;
pub mod launch;
pub mod layout;
pub mod level;
pub mod memory;
pub mod net;
mod objects;
pub mod ord;
pub mod pacer;
pub mod periodic;
pub mod quiesce;
pub mod rawdisk;
mod rtl;
mod sections;
pub mod snapshot;
pub mod status;
mod strings;
mod sync;
pub mod threads;
mod timers;

use abi::{Args, Export, Value};
use clock::Clock;
pub use console::ConsoleConfig;
pub use exports::EXPORTS;
use handles::HandleTable;
pub use quiesce::{Blocker, Quiescence};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, LoadedImage, Runtime, TransferAbi};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, MutexGuard, Weak},
};
pub use threads::{Machine, RunOutcome, Thread};

/// Host-memory budgets for state that accumulates over a long run. Guest code
/// controls how often these grow, so each has a lifetime bound, not only a
/// per-item bound. History buffers drop their older half when full and count
/// what they dropped; live tables refuse new entries with a resource status.
pub mod limits {
    /// DbgPrint messages retained by `Kernel::log_bytes`.
    pub const MAX_LOG_MESSAGES: usize = 4096;
    /// Total DbgPrint bytes retained (each message is at most 4096 bytes).
    pub const MAX_LOG_BYTES: usize = 1024 * 1024;
    /// Diagnostic stop reasons retained by `Kernel::stop_reasons`.
    pub const MAX_STOP_REASONS: usize = 1024;
    /// Scheduled thread IDs retained by `Machine::schedule`.
    pub const MAX_SCHEDULE: usize = 65536;
    /// Live guest handles (including duplicates); more report
    /// STATUS_INSUFFICIENT_RESOURCES.
    pub const MAX_HANDLES: usize = 65536;
    /// Distinct RTL critical sections tracked; more stop.
    pub const MAX_CRITICAL_SECTIONS: usize = 65536;
    /// Failed deferred completions retained by
    /// `Kernel::take_deferred_signal_failures`.
    pub const MAX_DEFERRED_FAILURES: usize = 256;
    /// Distinct 512-byte sectors `rawdisk::RawDisk` retains (2 MiB),
    /// including the cache database. A write touching more new sectors than
    /// remain fails with STATUS_DISK_FULL and changes nothing.
    pub const MAX_RAW_DISK_SECTORS: usize = 4096;
    /// Largest host scratch buffer used for one guest file I/O chunk.
    pub const IO_CHUNK: usize = 64 * 1024;
}

/// Append to a bounded history, dropping the older half when it is full.
pub(crate) fn push_bounded<T>(history: &mut Vec<T>, limit: usize, dropped: &mut u64, item: T) {
    if history.len() >= limit {
        let half = history.len().div_ceil(2);
        history.drain(..half);
        *dropped = dropped.saturating_add(half as u64);
    }
    history.push(item);
}

pub(crate) enum Action {
    Complete(Value),
    /// The call is completed and its guest continuation saved before yielding.
    Block(Value),
    Exit(u32),
    Stop(&'static str),
}
impl Action {
    pub(crate) fn value(v: u32) -> Self {
        Self::Complete(Value::U32(v))
    }
    pub(crate) fn status(v: u32) -> Self {
        Self::value(v)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub tls: Option<xbe::TlsDirectory>,
    pub data_exports: BTreeMap<u32, u32>,
    /// Explicit NT epoch in 100 ns units, recorded with the replay input.
    pub epoch_100ns: u64,
    pub console: ConsoleConfig,
    /// Title whose cache partition the raw disk's cache database records.
    /// `for_image` reads it from the XBE certificate.
    pub title_id: Option<u32>,
    /// Cache partition (3..=5) assigned to `title_id`; Z: once XAPI mounts it.
    pub utility_partition: u8,
    /// XeImageFileName (326 DATA): the launched image's device path.
    pub image_file_name: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            tls: None,
            data_exports: BTreeMap::new(),
            epoch_100ns: 0,
            console: ConsoleConfig::default(),
            title_id: None,
            utility_partition: 5,
            image_file_name: DEFAULT_IMAGE_FILE_NAME.to_owned(),
        }
    }
}
/// A title launched from the DVD drive.
pub const DEFAULT_IMAGE_FILE_NAME: &str = "\\Device\\CdRom0\\default.xbe";
/// HalDiskCachePartitionCount (40 DATA): retail Partition3..=5.
pub const CACHE_PARTITION_COUNT: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Fault(#[from] GuestFault),
    #[error("kernel operation failed: NTSTATUS {0:#010x}")]
    Resource(u32),
    #[error("invalid kernel configuration: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Clock(#[from] clock::ClockError),
}

pub(crate) struct Core {
    memory: memory::Memory,
    handles: HandleTable,
    objects: BTreeMap<u32, sync::Object>,
    owned_objects: BTreeSet<u32>,
    critical: BTreeMap<u32, sync::Critical>,
    threads: BTreeMap<u32, threads::Record>,
    wait_order: VecDeque<u32>,
    ready: VecDeque<u32>,
    /// The thread whose NtYieldExecution handed its slice to another ready
    /// thread; the next pick passes it over (threads.rs `pop_ready`).
    yielder: Option<u32>,
    yielded: BTreeMap<u32, TransferAbi>,
    next_tid: u32,
    kpcr: u32,
    bridge: u32,
    tls: Option<xbe::TlsDirectory>,
    clock: Clock,
    /// Charged accesses not yet a whole clock tick ([`Core::charge_work`]).
    work_carry: u64,
    data: BTreeMap<u32, u32>,
    saved_av: u32,
    /// PhyInitialize (253) has run.
    phy_ready: bool,
    log: Vec<Vec<u8>>,
    log_bytes: usize,
    dropped_log: u64,
    stops: Vec<(u32, &'static str)>,
    dropped_stops: u64,
    deferred_failures: Vec<DeferredSignal>,
    dropped_deferred_failures: u64,
    storage: Option<Arc<Mutex<vfs::Vfs>>>,
    /// The raw disk's written sectors, when the kernel mounted it
    /// (`set_shared_vfs`).
    raw_disk: Option<rawdisk::RawSectors>,
    title_id: Option<u32>,
    utility_partition: u8,
    console: ConsoleConfig,
    encoder: console::EncoderState,
    launch_page: u32,
    references: BTreeMap<u32, usize>,
    files: BTreeMap<u32, files::File>,
    file_handles: BTreeMap<u32, vfs::Handle>,
    /// Symbolic-link objects opened by 203: pool object -> target captured at open.
    links: BTreeMap<u32, String>,
    timers: BTreeMap<u32, timers::Timer>,
    /// Owned mutants: guest address -> owning thread ID.
    mutant_owners: BTreeMap<u32, u32>,
    /// ObReferenceObjectByHandle counts not yet released by ObfDereferenceObject
    /// (also counted in `references`).
    guest_references: BTreeMap<u32, usize>,
    dpcs: VecDeque<timers::Queued>,
    /// Periodic host interrupt sources by ID ([`Kernel::add_periodic`]).
    periodic: BTreeMap<u32, periodic::Periodic>,
    next_periodic: u32,
    /// Set by a DPC routine's return continuation.
    dpc_complete: bool,
    /// HalRegisterShutdownNotification records, highest priority first.
    shutdown: Vec<ShutdownNotification>,
    /// Pages marked by MmPersistContiguousMemory.
    persistent: BTreeSet<u32>,
    sections: BTreeMap<u32, sections::Section>,
}

/// One HAL_SHUTDOWN_REGISTRATION recorded by HalRegisterShutdownNotification.
/// Nothing runs these yet: every shutdown/reboot path (49 and friends) stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownNotification {
    pub registration: u32,
    pub routine: u32,
    pub priority: i32,
}

pub struct Kernel {
    runtime: Weak<Runtime>,
    pub(crate) core: Mutex<Core>,
    /// Guest addresses of pinned events queued by `DispatcherReference::defer_signal`.
    /// A leaf lock: nothing else is ever acquired while it is held.
    pub(crate) deferred: Mutex<Vec<u32>>,
}
impl Kernel {
    /// Installs only implemented exports plus explicit diagnostic policies.
    /// Existing runtime stops for all other ordinals remain registered.
    pub fn install(runtime: &Arc<Runtime>, config: Config) -> Result<Arc<Self>, Error> {
        Self::install_inner(runtime, config, &[])
    }
    fn install_inner(
        runtime: &Arc<Runtime>,
        config: Config,
        reclaimed: &[runtime::guest::Mapping],
    ) -> Result<Arc<Self>, Error> {
        image::install(&mut runtime.memory(), reclaimed)?;
        let mut memory_state = memory::Memory::default();
        let kpcr = memory_state
            .internal(&mut runtime.memory(), 4096)
            .map_err(Error::Resource)?;
        // Runtime validates Execute permission on exports outside its current
        // fixed import inventory. Give supplemental exports mapped adapters.
        let bridge = memory_state
            .internal(&mut runtime.memory(), 4096)
            .map_err(Error::Resource)?;
        runtime
            .memory()
            .protect(bridge, runtime::guest::Permissions::RX)
            .map_err(|_| Error::Invalid("adapter mapping"))?;
        let mut data = config.data_exports;
        if let std::collections::btree_map::Entry::Vacant(entry) = data.entry(ord::LAUNCH_DATA_PAGE)
        {
            entry.insert(
                memory_state
                    .internal(&mut runtime.memory(), 4)
                    .map_err(Error::Resource)?,
            );
        }
        runtime.memory().set_u32(data[&ord::LAUNCH_DATA_PAGE], 0)?;
        if !(3..=5).contains(&config.utility_partition) {
            return Err(Error::Invalid("utility partition must be 3..=5"));
        }
        if let Some(&address) = data.get(&ord::HAL_DISK_CACHE_PARTITION_COUNT) {
            runtime.memory().set_u32(address, CACHE_PARTITION_COUNT)?;
        }
        if let Some(&address) = data.get(&ord::XBOX_HD_KEY) {
            runtime
                .memory()
                .write_bytes(address, &crypto::XBOX_HD_KEY, 1)?;
        }
        if let Some(&address) = data.get(&ord::XE_IMAGE_FILE_NAME) {
            let name = config.image_file_name.as_bytes();
            if name.len() >= 4096 || !name.is_ascii() || name.contains(&0) {
                return Err(Error::Invalid("image file name"));
            }
            // ANSI_STRING { Length, MaximumLength, Buffer } over NUL-terminated
            // kernel-owned bytes.
            let buffer = memory_state
                .internal(&mut runtime.memory(), name.len() as u32 + 1)
                .map_err(Error::Resource)?;
            let mut memory = runtime.memory();
            memory.write_bytes(buffer, name, 1)?;
            memory.set_u16(address, name.len() as u16)?;
            memory.set_u16(address + 2, name.len() as u16 + 1)?;
            memory.set_u32(address + 4, buffer)?;
        }
        let kernel = Arc::new(Self {
            runtime: Arc::downgrade(runtime),
            core: Mutex::new(Core {
                memory: memory_state,
                handles: HandleTable::default(),
                objects: BTreeMap::new(),
                owned_objects: BTreeSet::new(),
                critical: BTreeMap::new(),
                threads: BTreeMap::new(),
                wait_order: VecDeque::new(),
                ready: VecDeque::new(),
                yielder: None,
                yielded: BTreeMap::new(),
                next_tid: 1,
                kpcr,
                bridge,
                tls: config.tls,
                clock: Clock::new(config.epoch_100ns),
                work_carry: 0,
                data,
                console: config.console,
                encoder: console::EncoderState::default(),
                launch_page: 0,
                references: BTreeMap::new(),
                saved_av: 0,
                phy_ready: false,
                log: Vec::new(),
                log_bytes: 0,
                dropped_log: 0,
                stops: Vec::new(),
                dropped_stops: 0,
                deferred_failures: Vec::new(),
                dropped_deferred_failures: 0,
                storage: None,
                raw_disk: None,
                title_id: config.title_id,
                utility_partition: config.utility_partition,
                files: BTreeMap::new(),
                file_handles: BTreeMap::new(),
                links: BTreeMap::new(),
                timers: BTreeMap::new(),
                mutant_owners: BTreeMap::new(),
                guest_references: BTreeMap::new(),
                dpcs: VecDeque::new(),
                periodic: BTreeMap::new(),
                next_periodic: 1,
                dpc_complete: false,
                shutdown: Vec::new(),
                persistent: BTreeSet::new(),
                sections: BTreeMap::new(),
            }),
            deferred: Mutex::new(Vec::new()),
        });
        kernel.lock().write_tick(&mut runtime.memory())?;
        // rdtsc follows the logical clock. Generated code reads it holding
        // no lock, so this is the outermost lock taken, as for a service.
        let clock = Arc::downgrade(&kernel);
        runtime.set_guest_clock(Some(Arc::new(move || {
            Some(clock.upgrade()?.lock().clock.counter())
        })));
        runtime.edit_dispatch(|dispatch| {
            for import in runtime::kernel::FUNCTION_IMPORTS {
                if dispatch
                    .override_name(runtime::kernel::function_token(import.ordinal))
                    .is_none()
                {
                    let export = Export {
                        ordinal: import.ordinal,
                        name: import.name,
                        convention: abi::Convention::Stdcall,
                        words: import.stack_words,
                        variadic: import.variadic,
                    };
                    dispatch.register_override(
                        runtime::kernel::function_token(import.ordinal),
                        import.name,
                        move |cpu, ctx, _| {
                            ctx.measure(runtime::perf::Scope::Kernel, |ctx| {
                                Ok(abi::stop(export, cpu, ctx))
                            })
                        },
                    );
                }
            }
            for &export in EXPORTS {
                let kernel = Arc::clone(&kernel);
                let entry = kernel.entry(export.ordinal).expect("declared export");
                dispatch.register_override(entry, export.name, move |cpu, ctx, _| {
                    ctx.measure(runtime::perf::Scope::Kernel, |ctx| {
                        kernel.invoke(export, cpu, ctx)
                    })
                });
            }
        });
        Ok(kernel)
    }
    pub fn for_image(image: &mut LoadedImage) -> Result<Arc<Self>, Error> {
        Self::for_image_with(image, Config::default())
    }
    /// `for_image` with explicit settings: `tls`, `data_exports` and
    /// `title_id` come from the image; every other field (console profile,
    /// epoch, utility partition, image name) from
    /// `config`.
    pub fn for_image_with(image: &mut LoadedImage, config: Config) -> Result<Arc<Self>, Error> {
        // Certificate TitleID; synthetic images without a certificate have none.
        let title_id = {
            let memory = image.runtime.memory();
            memory
                .u32(runtime::loader::HEADER_BASE + 0x118)
                .ok()
                .and_then(|certificate| certificate.checked_add(8))
                .and_then(|at| memory.u32(at).ok())
        };
        let reclaimed = image::reclaim_data(image)?;
        let kernel = Self::install_inner(
            &image.runtime,
            Config {
                tls: image.tls,
                data_exports: image.data_exports.clone(),
                title_id,
                ..config
            },
            &reclaimed,
        )?;
        let sections = sections::attach(&mut image.runtime.memory())?;
        kernel.lock().sections = sections;
        Ok(kernel)
    }
    /// Registered shutdown notifications, in the order a shutdown would call them.
    pub fn shutdown_notifications(&self) -> Vec<ShutdownNotification> {
        self.lock().shutdown.clone()
    }
    /// Page addresses marked persistent by MmPersistContiguousMemory.
    pub fn persistent_pages(&self) -> Vec<u32> {
        self.lock().persistent.iter().copied().collect()
    }
    pub(crate) fn lock(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Apply before guest execution or between cooperative slices. No host
    /// EEPROM/settings are read or changed by this profile.
    pub fn set_console_config(&self, console: ConsoleConfig) {
        self.lock().console = console;
    }
    pub fn console_config(&self) -> ConsoleConfig {
        self.lock().console.clone()
    }
    /// Blank/flicker/luma settings sent through AvSendTVEncoderOption (2).
    pub fn encoder_state(&self) -> console::EncoderState {
        self.lock().encoder
    }
    pub fn clock(&self) -> Clock {
        self.lock().clock.clone()
    }
    pub fn log(&self) -> Vec<String> {
        self.lock()
            .log
            .iter()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect()
    }
    pub fn log_bytes(&self) -> Vec<Vec<u8>> {
        self.lock().log.clone()
    }
    /// DbgPrint messages dropped from the front of `log_bytes` by its budget.
    pub fn dropped_log_messages(&self) -> u64 {
        self.lock().dropped_log
    }
    /// Retained stop reasons, oldest first; see `limits::MAX_STOP_REASONS`.
    pub fn stop_reasons(&self) -> Vec<(u32, &'static str)> {
        self.lock().stops.clone()
    }
    pub fn dropped_stop_reasons(&self) -> u64 {
        self.lock().dropped_stops
    }
    pub fn kpcr(&self) -> u32 {
        self.lock().kpcr
    }
    /// Attach one shared VFS configured by the caller. No default host paths,
    /// disc images or persistent directories are opened by kernel startup.
    pub fn set_vfs(&self, storage: vfs::Vfs) -> Result<(), Error> {
        self.set_shared_vfs(Arc::new(Mutex::new(storage)))
    }
    /// Also mounts the kernel's raw disk device (`rawdisk::RAW_DISK_DEVICE`)
    /// unless this VFS already resolves that name.
    pub fn set_shared_vfs(&self, storage: Arc<Mutex<vfs::Vfs>>) -> Result<(), Error> {
        let mut core = self.lock();
        if core.storage.is_some() {
            return Err(Error::Invalid("VFS already attached"));
        }
        {
            let mut shared = storage.lock().unwrap_or_else(|e| e.into_inner());
            if shared.query_path(rawdisk::RAW_DISK_DEVICE) == Err(vfs::Error::ObjectPathNotFound) {
                let cache: Vec<_> = core
                    .title_id
                    .map(|title_id| rawdisk::CacheEntry {
                        title_id,
                        partition: core.utility_partition,
                    })
                    .into_iter()
                    .collect();
                let disk = rawdisk::RawDisk::new(rawdisk::DISK_SIZE + shared.extra_disk_bytes(), &cache)
                    .map_err(|_| Error::Invalid("raw disk"))?;
                let sectors = disk.sectors();
                shared
                    .mount(rawdisk::RAW_DISK_DEVICE, disk)
                    .map_err(|_| Error::Invalid("raw disk mount"))?;
                core.raw_disk = Some(sectors);
            }
        }
        core.storage = Some(storage);
        Ok(())
    }
    /// Drop the VFS guard before calling kernel APIs or taking guest memory.
    /// Kernel services lock kernel state -> guest memory -> VFS.
    pub fn vfs(&self) -> Option<Arc<Mutex<vfs::Vfs>>> {
        self.lock().storage.clone()
    }
    /// One positioned read through a guest file handle: the VFS read
    /// `NtReadFile` makes (README "VFS boundaries"), for the native
    /// streaming reads that stand in for the guest's I/O worker. The guest
    /// handle needs read access; the file position is not touched. Fewer
    /// bytes than `out` is the end of the file; none is `Ok(0)`.
    pub fn read_at(&self, guest_handle: u32, offset: u64, out: &mut [u8]) -> Result<usize, vfs::Error> {
        let (storage, handle) = {
            let core = self.lock();
            (core.storage.clone().ok_or(vfs::Error::InvalidHandle)?, core.file_handle(guest_handle)?)
        };
        let mut storage = storage.lock().unwrap_or_else(|e| e.into_inner());
        match storage.read(handle, vfs::Offset::Absolute(offset), out) {
            Err(vfs::Error::EndOfFile) => Ok(0),
            other => other,
        }
    }
    /// The raw disk's written sectors, when the kernel mounted the device.
    pub fn raw_disk(&self) -> Option<rawdisk::RawSectors> {
        self.lock().raw_disk.clone()
    }
    pub fn data_export(&self, ordinal: u32) -> Option<u32> {
        self.lock().data.get(&ordinal).copied()
    }
    fn runtime(&self) -> Result<Arc<Runtime>, Error> {
        self.runtime
            .upgrade()
            .ok_or(Error::Invalid("runtime has been dropped"))
    }
    /// Supplemental exports use checked executable adapters until Runtime's
    /// loader knows their ordinals. Imported Halo functions keep their tokens.
    pub fn entry(&self, ordinal: u32) -> Option<u32> {
        EXPORTS.iter().find(|e| e.ordinal == ordinal)?;
        Some(if runtime::kernel::function(ordinal).is_some() {
            runtime::kernel::function_token(ordinal)
        } else {
            self.lock().bridge + 4 + ordinal * 4
        })
    }
    /// Add a periodic host interrupt (`periodic`): its first deadline is one
    /// period from now. Returns its ID.
    pub fn add_periodic(&self, source: periodic::Source) -> Result<u32, Error> {
        let mut core = self.lock();
        let now = core.clock.counter();
        let source = periodic::Periodic::new(source, now).ok_or(Error::Invalid(
            "periodic rate must be nonzero and under 10 MHz",
        ))?;
        let id = core.next_periodic;
        core.next_periodic = id
            .checked_add(1)
            .ok_or(Error::Invalid("periodic source IDs exhausted"))?;
        core.periodic.insert(id, source);
        Ok(id)
    }
    /// Remove a periodic source; periods it still owed are not delivered.
    pub fn remove_periodic(&self, id: u32) -> bool {
        let mut core = self.lock();
        core.dpcs.retain(|q| *q != timers::Queued::Host(id));
        core.periodic.remove(&id).is_some()
    }
    pub fn periodic_stats(&self, id: u32) -> Option<periodic::Stats> {
        self.lock().periodic.get(&id).map(|p| p.stats)
    }
    pub fn advance_time(&self, runtime: &Runtime, ticks_100ns: u64) -> Result<(), Error> {
        let mut core = self.lock();
        let mut memory = runtime.memory();
        core.clock.advance(ticks_100ns)?;
        core.write_tick(&mut memory)?;
        core.wake(&mut memory)?;
        Ok(())
    }
    pub fn stopped_ordinals(&self) -> Vec<u32> {
        let has_storage = self.lock().storage.is_some();
        runtime::kernel::FUNCTION_IMPORTS
            .iter()
            .filter(|f| {
                (!has_storage && files::is_file_export(f.ordinal))
                    || !EXPORTS.iter().any(|e| {
                        e.ordinal == f.ordinal
                            && !matches!(
                                e.ordinal,
                                ord::INTERLOCKED_FLUSH_S_LIST
                                    ..=ord::INTERLOCKED_PUSH_ENTRY_S_LIST | ord::MM_MAP_IO_SPACE
                            )
                    })
            })
            .map(|f| f.ordinal)
            .chain([56, 57, 58, 177])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    fn invoke(
        &self,
        export: Export,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
    ) -> Result<TransferAbi, GuestFault> {
        let spent = ctx.take_slice_spent();
        let (args, action) = {
            // Every service takes locks in this order; neither escapes a call.
            let mut core = self.lock();
            let mut memory = ctx.runtime().memory();
            // Settle the generated work before this call first, so what it
            // reads of time, and any relative deadline it sets, starts
            // after that work rather than before it.
            core.charge_work(&mut memory, ctx.thread_id, spent)?;
            let args = Args::read(export, cpu, &memory)?;
            core.write_tick(&mut memory)?;
            let action = core.call(&mut memory, &args, ctx.thread_id)?;
            (args, action)
        };
        match action {
            Action::Complete(value) => {
                let resume = args.finish(cpu, &ctx.runtime().memory(), value)?;
                // A DPC queued by this call (KeSetTimer with a due time already
                // reached, for example) runs before the caller continues, as
                // when retail lowers IRQL below DISPATCH_LEVEL on the way out.
                if self
                    .lock()
                    .dispatch_pending(&mut ctx.runtime().memory(), ctx.thread_id)?
                {
                    self.lock().yielded.insert(ctx.thread_id, resume);
                    return Ok(TransferAbi::stop(0));
                }
                Ok(resume)
            }
            Action::Stop(reason) => {
                let mut core = self.lock();
                let core = &mut *core;
                push_bounded(
                    &mut core.stops,
                    limits::MAX_STOP_REASONS,
                    &mut core.dropped_stops,
                    (export.ordinal, reason),
                );
                Ok(abi::stop(export, cpu, ctx))
            }
            Action::Block(value) => {
                let resume = args.finish(cpu, &ctx.runtime().memory(), value)?;
                self.lock().yielded.insert(ctx.thread_id, resume);
                Ok(TransferAbi::stop(0))
            }
            Action::Exit(status) => {
                let mut core = self.lock();
                core.terminate(ctx.thread_id, status, &mut ctx.runtime().memory())?;
                core.yielded.insert(ctx.thread_id, TransferAbi::jump(0));
                Ok(TransferAbi::stop(0))
            }
        }
    }
}

impl Core {
    /// Retain DbgPrint output within `limits::MAX_LOG_MESSAGES`/`MAX_LOG_BYTES`.
    pub(crate) fn push_log(&mut self, message: Vec<u8>) {
        while !self.log.is_empty()
            && (self.log.len() >= limits::MAX_LOG_MESSAGES
                || self.log_bytes.saturating_add(message.len()) > limits::MAX_LOG_BYTES)
        {
            let half = self.log.len().div_ceil(2);
            let removed: usize = self.log.drain(..half).map(|m| m.len()).sum();
            self.log_bytes -= removed;
            self.dropped_log = self.dropped_log.saturating_add(half as u64);
        }
        self.log_bytes += message.len();
        self.log.push(message);
    }
    /// Charge `accesses` of executed generated work by thread `tid` to
    /// logical time at [`threads::ACCESSES_PER_TICK`], carrying the
    /// remainder, and to the thread's work since it last polled.
    pub(crate) fn charge_work(
        &mut self,
        memory: &mut Guest,
        tid: u32,
        accesses: u64,
    ) -> Result<(), GuestFault> {
        if let Some(record) = self.threads.get_mut(&tid) {
            record.work_since_poll = record.work_since_poll.saturating_add(accesses);
        }
        let carry = self.work_carry.saturating_add(accesses);
        self.work_carry = carry % threads::ACCESSES_PER_TICK;
        if carry >= threads::ACCESSES_PER_TICK {
            self.clock.charge(carry / threads::ACCESSES_PER_TICK);
            self.write_tick(memory)?;
        }
        Ok(())
    }
    fn write_tick(&self, memory: &mut Guest) -> Result<(), GuestFault> {
        if let Some(&address) = self.data.get(&ord::KE_TICK_COUNT) {
            memory.set_u32(address, self.clock.tick_count())?;
        }
        Ok(())
    }
    fn call(&mut self, memory: &mut Guest, a: &Args, tid: u32) -> Result<Action, GuestFault> {
        if matches!(a.export.ordinal, ord::NT_CLOSE | ord::NT_DUPLICATE_OBJECT)
            && self.file_handles.contains_key(&a.get(0))
        {
            return self.file_call(memory, a);
        }
        match a.export.ordinal {
            ord::RTL_NT_STATUS_TO_DOS_ERROR => Ok(match dos::from_ntstatus(a.get(0)) {
                Some(value) => Action::value(value),
                None => Action::Stop("unmapped NTSTATUS to DOS error"),
            }),
            ord::EX_QUERY_NON_VOLATILE_SETTING | ord::AV_SEND_TV_ENCODER_OPTION => {
                self.console_call(memory, a)
            }
            ord::AV_GET_SAVED_DATA_ADDRESS => Ok(Action::value(self.saved_av)),
            ord::AV_SET_SAVED_DATA_ADDRESS => {
                self.saved_av = a.get(0);
                Ok(Action::Complete(Value::Void))
            }
            ord::DBG_PRINT => strings::debug_print(self, memory, a),
            ord::IO_CREATE_SYMBOLIC_LINK
            | ord::IO_DELETE_SYMBOLIC_LINK
            | ord::NT_CREATE_FILE
            | ord::NT_FLUSH_BUFFERS_FILE
            | ord::NT_OPEN_FILE
            | ord::NT_OPEN_SYMBOLIC_LINK_OBJECT
            | ord::NT_QUERY_SYMBOLIC_LINK_OBJECT
            | ord::NT_QUERY_DIRECTORY_FILE
            | ord::NT_QUERY_FULL_ATTRIBUTES_FILE
            | ord::NT_QUERY_INFORMATION_FILE
            | ord::NT_QUERY_VOLUME_INFORMATION_FILE
            | ord::NT_READ_FILE
            | ord::NT_SET_INFORMATION_FILE
            | ord::NT_WRITE_FILE => self.file_call(memory, a),
            ord::EX_ALLOCATE_POOL
            | ord::EX_ALLOCATE_POOL_WITH_TAG
            | ord::EX_FREE_POOL
            | ord::EX_QUERY_POOL_BLOCK_SIZE
            | ord::MM_ALLOCATE_CONTIGUOUS_MEMORY
            | ord::MM_ALLOCATE_CONTIGUOUS_MEMORY_EX
            | ord::MM_FREE_CONTIGUOUS_MEMORY
            | ord::MM_GET_PHYSICAL_ADDRESS
            | ord::MM_CREATE_KERNEL_STACK
            | ord::MM_DELETE_KERNEL_STACK
            | ord::MM_MAP_IO_SPACE
            | ord::MM_PERSIST_CONTIGUOUS_MEMORY
            | ord::MM_SET_ADDRESS_PROTECT
            | ord::MM_QUERY_ADDRESS_PROTECT
            | ord::MM_QUERY_ALLOCATION_SIZE
            | ord::MM_QUERY_STATISTICS
            | ord::NT_ALLOCATE_VIRTUAL_MEMORY
            | ord::NT_FREE_VIRTUAL_MEMORY
            | ord::NT_QUERY_VIRTUAL_MEMORY => {
                if a.export.ordinal == ord::MM_PERSIST_CONTIGUOUS_MEMORY {
                    return self.persist_call(memory, a);
                }
                let freeing = (a.export.ordinal == ord::MM_FREE_CONTIGUOUS_MEMORY)
                    .then(|| {
                        self.memory
                            .contiguous
                            .get(&a.get(0))
                            .map(|(n, _)| (a.get(0), *n))
                    })
                    .flatten();
                let action = self.memory.call(memory, a)?;
                // Freed pages no longer persist.
                if let (Some((base, size)), Action::Complete(_)) = (freeing, &action) {
                    self.persistent.retain(|p| {
                        !(base as u64..base as u64 + size as u64).contains(&(*p as u64))
                    });
                }
                Ok(action)
            }
            ord::HAL_REGISTER_SHUTDOWN_NOTIFICATION => {
                let registration = a.get(0);
                // { NotificationRoutine, Priority, LIST_ENTRY }; the list
                // itself is kernel state held in Rust.
                let bytes = memory.read::<16>(registration)?;
                self.shutdown.retain(|n| n.registration != registration);
                if a.boolean(1) {
                    let routine = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                    let priority = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    if self.shutdown.len() >= limits::MAX_HANDLES {
                        return Ok(Action::Stop("shutdown notification budget exhausted"));
                    }
                    let at = self
                        .shutdown
                        .iter()
                        .position(|n| n.priority < priority)
                        .unwrap_or(self.shutdown.len());
                    self.shutdown.insert(
                        at,
                        ShutdownNotification {
                            registration,
                            routine,
                            priority,
                        },
                    );
                }
                Ok(Action::Complete(Value::Void))
            }
            ord::KE_CANCEL_TIMER
            | ord::KE_INITIALIZE_DPC
            | ord::KE_INITIALIZE_TIMER_EX
            | ord::KE_SET_TIMER
            | ord::KE_SET_TIMER_EX => self.timer_call(memory, a),
            ord::OB_REFERENCE_OBJECT_BY_HANDLE | ord::OBF_DEREFERENCE_OBJECT => {
                self.object_call(memory, a, tid)
            }
            ord::XC_SHA_INIT | ord::XC_SHA_UPDATE | ord::XC_SHA_FINAL => self.sha_call(memory, a),
            ord::XC_HMAC => self.hmac_call(memory, a),
            // No NIC model; see net.rs for the decoded contract.
            ord::PHY_INITIALIZE | ord::PHY_GET_LINK_STATE => Ok(self.phy_call(a)),
            ord::RTL_COMPARE_MEMORY_ULONG
            | ord::RTL_TIME_FIELDS_TO_TIME
            | ord::RTL_TIME_TO_TIME_FIELDS => self.rtl_call(memory, a),
            ord::XE_LOAD_SECTION | ord::XE_UNLOAD_SECTION => {
                self.section_call(memory, a.get(0), a.export.ordinal == ord::XE_LOAD_SECTION)
            }
            ord::INTERLOCKED_COMPARE_EXCHANGE..=ord::INTERLOCKED_EXCHANGE_ADD => {
                let old = memory.u32(a.get(0))?;
                let new = match a.export.ordinal {
                    ord::INTERLOCKED_COMPARE_EXCHANGE => {
                        if old == a.get(2) {
                            a.get(1)
                        } else {
                            old
                        }
                    }
                    ord::INTERLOCKED_DECREMENT => old.wrapping_sub(1),
                    ord::INTERLOCKED_INCREMENT => old.wrapping_add(1),
                    ord::INTERLOCKED_EXCHANGE => a.get(1),
                    ord::INTERLOCKED_EXCHANGE_ADD => old.wrapping_add(a.get(1)),
                    _ => return Ok(Action::Stop("unrouted service case")),
                };
                // Read/write both happen under the same Runtime memory guard.
                memory.set_u32(a.get(0), new)?;
                Ok(Action::value(
                    if matches!(
                        a.export.ordinal,
                        ord::INTERLOCKED_DECREMENT | ord::INTERLOCKED_INCREMENT
                    ) {
                        new
                    } else {
                        old
                    },
                ))
            }
            ord::INTERLOCKED_FLUSH_S_LIST..=ord::INTERLOCKED_PUSH_ENTRY_S_LIST => Ok(Action::Stop(
                "SLIST header sequence/depth policy unverified",
            )),
            ord::KE_DELAY_EXECUTION_THREAD
            | ord::KE_ENTER_CRITICAL_REGION
            | ord::KE_GET_CURRENT_THREAD
            | ord::KE_LEAVE_CRITICAL_REGION
            | ord::KE_SET_BASE_PRIORITY_THREAD
            | ord::NT_RESUME_THREAD
            | ord::NT_YIELD_EXECUTION
            | ord::PS_CREATE_SYSTEM_THREAD_EX
            | ord::PS_TERMINATE_SYSTEM_THREAD => self.thread_call(memory, a, tid),
            ord::KE_GET_CURRENT_IRQL => Ok(Action::value(
                memory.u8(self.kpcr + crate::layout::kpcr::IRQL)? as u32,
            )),
            ord::KE_RAISE_IRQL_TO_DPC_LEVEL | ord::KF_RAISE_IRQL | ord::KF_LOWER_IRQL => {
                let old = memory.u8(self.kpcr + crate::layout::kpcr::IRQL)?;
                let new = if a.export.ordinal == ord::KE_RAISE_IRQL_TO_DPC_LEVEL {
                    2
                } else {
                    a.get(0) as u8
                };
                if new > 31
                    || a.export.ordinal == ord::KF_LOWER_IRQL && new > old
                    || a.export.ordinal != ord::KF_LOWER_IRQL && new < old
                {
                    return Ok(Action::Stop("invalid IRQL transition"));
                }
                // Lowering below DISPATCH_LEVEL with DPCs queued delivers them
                // before the caller continues (`Core::dispatch_pending`).
                memory.set_u8(self.kpcr + crate::layout::kpcr::IRQL, new)?;
                Ok(Action::Complete(
                    if a.export.ordinal == ord::KF_LOWER_IRQL {
                        Value::Void
                    } else {
                        Value::U32(old as u32)
                    },
                ))
            }
            ord::KE_QUERY_PERFORMANCE_COUNTER => {
                Ok(Action::Complete(Value::U64(self.clock.counter())))
            }
            ord::KE_QUERY_PERFORMANCE_FREQUENCY => {
                Ok(Action::Complete(Value::U64(Clock::FREQUENCY)))
            }
            ord::KE_QUERY_SYSTEM_TIME => {
                memory.set_u64(a.get(0), self.clock.system_time())?;
                Ok(Action::Complete(Value::Void))
            }
            ord::KE_INITIALIZE_EVENT
            | ord::KE_INITIALIZE_SEMAPHORE
            | ord::KE_RELEASE_SEMAPHORE
            | ord::KE_RESET_EVENT
            | ord::KE_SET_EVENT
            | ord::KE_WAIT_FOR_MULTIPLE_OBJECTS
            | ord::KE_WAIT_FOR_SINGLE_OBJECT
            | ord::NT_CLOSE
            | ord::NT_CREATE_EVENT
            | ord::NT_CREATE_MUTANT
            | ord::NT_CREATE_SEMAPHORE
            | ord::NT_DUPLICATE_OBJECT
            | ord::NT_RELEASE_MUTANT
            | ord::NT_RELEASE_SEMAPHORE
            | ord::NT_SET_EVENT
            | ord::NT_WAIT_FOR_SINGLE_OBJECT
            | ord::NT_WAIT_FOR_SINGLE_OBJECT_EX
            | ord::NT_WAIT_FOR_MULTIPLE_OBJECTS_EX
            | ord::RTL_ENTER_CRITICAL_SECTION
            | ord::RTL_INITIALIZE_CRITICAL_SECTION
            | ord::RTL_LEAVE_CRITICAL_SECTION
            | ord::RTL_TRY_ENTER_CRITICAL_SECTION => self.sync_call(memory, a, tid),
            ord::RTL_ANSI_STRING_TO_UNICODE_STRING
            | ord::RTL_EQUAL_STRING
            | ord::RTL_FREE_ANSI_STRING
            | ord::RTL_FREE_UNICODE_STRING
            | ord::RTL_INIT_ANSI_STRING
            | ord::RTL_INIT_UNICODE_STRING
            | ord::RTL_UNICODE_STRING_TO_ANSI_STRING => strings::call(self, memory, a),
            _ => Ok(Action::Stop("declared export has no handler")),
        }
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn an_unrouted_export_stops_instead_of_panicking_the_host() {
        let mut memory = Guest::new();
        memory
            .map_zeroed(0x10000, 4096, runtime::guest::Permissions::RW)
            .unwrap();
        memory
            .set_u32(0x10000, runtime::exec::HARNESS_EXIT)
            .unwrap();
        let runtime = Runtime::new(memory, runtime::Dispatch::default());
        let kernel = Kernel::install(&runtime, Config::default()).unwrap();
        let cpu = CpuAbi {
            esp: 0x10000,
            ..CpuAbi::xbox_thread()
        };
        let args = Args::read(
            Export::stdcall(999, "unrouted fixture", 0),
            &cpu,
            &runtime.memory(),
        )
        .unwrap();
        assert!(matches!(
            kernel.lock().call(&mut runtime.memory(), &args, 1).unwrap(),
            Action::Stop(_)
        ));
    }
}
