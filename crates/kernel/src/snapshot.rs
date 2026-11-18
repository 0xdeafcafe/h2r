//! Machine snapshots, phase 3 (docs/runtime/snapshots.md): the kernel's
//! `Core` and the machine's threads as `kcore` and `machine` sections.
//!
//! Every saved type lists all its fields ([`persist_struct!`] destructures
//! without `..`), so a field added later fails to compile until someone
//! decides whether it is saved. `Core` itself is written field by field in
//! [`save_core`] for the same reason: each field is saved, checked against
//! the booted machine, or kept from it (boot state, host handles,
//! diagnostics), and says so there.
//!
//! A load goes into a machine booted from the same image and switches,
//! after guest RAM and the TSC (`snapshot::guest`). It replaces `Core`
//! whole and makes fresh thread contexts carrying the saved CPUs and their
//! rebuilt continuations.
use crate::threads::{Machine, Thread};
use crate::{Core, Kernel, abi::Value};
use runtime::{
    Continuation, ContinuationLayout, CpuAbi, ExecCtx, LiveRecord, SlotRecord, TransferAbi,
};
use snapshot::codec::{Reader, Writer};
use snapshot::{Codec, Snapshot, SnapshotWriter, Tag};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use vfs::{SavedFile, VfsState};

pub use snapshot::Error as CodecError;

pub const KCORE: Tag = Tag::new("kcore");
pub const MACHINE: Tag = Tag::new("machine");
/// 2: the raw disk's written sectors follow the periodic schedules.
const KCORE_VERSION: u32 = 2;
const MACHINE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The machine is not between slices with nothing live
    /// ([`Machine::quiescent`]).
    #[error("machine is not quiescent: {0:?}")]
    Busy(Vec<String>),
    #[error(transparent)]
    Continuation(#[from] runtime::LayoutError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// The snapshot was taken on a machine booted differently.
    #[error("snapshot does not fit this machine: {0}")]
    Mismatch(String),
    #[error(transparent)]
    Kernel(#[from] crate::Error),
}

/// Plain data a section can hold. Little-endian through the snapshot
/// codec; containers carry a u32 count.
pub(crate) trait Persist: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError>;
}

/// `Persist` for a struct, every field in order. The destructuring has no
/// `..`: a new field does not compile until it is listed or the struct's
/// impl is written by hand.
macro_rules! persist_struct {
    ($ty:ident { $($field:ident),* $(,)? }) => {
        impl $crate::snapshot::Persist for $ty {
            fn put(&self, w: &mut ::snapshot::codec::Writer) {
                let $ty { $($field),* } = self;
                $( $crate::snapshot::Persist::put($field, w); )*
            }
            fn get(
                r: &mut ::snapshot::codec::Reader<'_>,
            ) -> Result<Self, $crate::snapshot::CodecError> {
                Ok($ty { $($field: $crate::snapshot::Persist::get(r)?),* })
            }
        }
    };
}
pub(crate) use persist_struct;

pub(crate) fn corrupt(what: &str) -> CodecError {
    CodecError::Corrupt("kernel snapshot", what.to_owned())
}

macro_rules! persist_int {
    ($($t:ty: $put:ident $get:ident $via:ty),*) => {$(
        impl Persist for $t {
            fn put(&self, w: &mut Writer) {
                w.$put(*self as $via);
            }
            fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
                <$t>::try_from(r.$get()?).map_err(|_| corrupt("integer out of range"))
            }
        }
    )*};
}
persist_int!(u8: u8 u8 u8, u32: u32 u32 u32, u64: u64 u64 u64, usize: u64 u64 u64);

impl Persist for i8 {
    fn put(&self, w: &mut Writer) {
        w.u8(*self as u8);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(r.u8()? as i8)
    }
}
impl Persist for u16 {
    fn put(&self, w: &mut Writer) {
        w.u32(*self as u32);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        u16::try_from(r.u32()?).map_err(|_| corrupt("u16 out of range"))
    }
}
impl Persist for i32 {
    fn put(&self, w: &mut Writer) {
        w.u32(*self as u32);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(r.u32()? as i32)
    }
}
impl Persist for bool {
    fn put(&self, w: &mut Writer) {
        w.bool(*self);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        r.bool()
    }
}
impl Persist for String {
    fn put(&self, w: &mut Writer) {
        w.str(self);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(r.str()?.to_owned())
    }
}
impl<T: Persist> Persist for Option<T> {
    fn put(&self, w: &mut Writer) {
        w.bool(self.is_some());
        if let Some(v) = self {
            v.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(if r.bool()? { Some(T::get(r)?) } else { None })
    }
}
impl<T: Persist> Persist for Vec<T> {
    fn put(&self, w: &mut Writer) {
        w.list(self, |w, v| v.put(w));
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        r.list(1, T::get)
    }
}
impl<T: Persist> Persist for VecDeque<T> {
    fn put(&self, w: &mut Writer) {
        w.u32(self.len() as u32);
        for v in self {
            v.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Vec::get(r)?.into())
    }
}
impl<T: Persist + Ord> Persist for BTreeSet<T> {
    fn put(&self, w: &mut Writer) {
        w.u32(self.len() as u32);
        for v in self {
            v.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Vec::get(r)?.into_iter().collect())
    }
}
impl<K: Persist + Ord, V: Persist> Persist for BTreeMap<K, V> {
    fn put(&self, w: &mut Writer) {
        w.u32(self.len() as u32);
        for (k, v) in self {
            k.put(w);
            v.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(r.list(2, |r| Ok((K::get(r)?, V::get(r)?)))?
            .into_iter()
            .collect())
    }
}
impl<A: Persist, B: Persist> Persist for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok((A::get(r)?, B::get(r)?))
    }
}
impl<T: Persist + Copy + Default, const N: usize> Persist for [T; N] {
    fn put(&self, w: &mut Writer) {
        for v in self {
            v.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        let mut out = [T::default(); N];
        for v in &mut out {
            *v = T::get(r)?;
        }
        Ok(out)
    }
}

use runtime::cpu::X87Abi;
persist_struct!(X87Abi {
    st,
    control,
    status,
    tag,
    opcode,
    instruction_pointer,
    data_pointer,
    instruction_selector,
    data_selector,
});
persist_struct!(CpuAbi {
    abi_version,
    struct_size,
    eax,
    ecx,
    edx,
    ebx,
    esp,
    ebp,
    esi,
    edi,
    eip,
    fs_base,
    eflags,
    eflags_defined,
    x87,
    xmm,
    mxcsr,
});
persist_struct!(TransferAbi {
    abi_version,
    struct_size,
    kind,
    route,
    target_pc,
    resume_pc,
    event_id,
});
persist_struct!(LiveRecord {
    origin,
    target,
    state
});
persist_struct!(SlotRecord { generation, live });
persist_struct!(ContinuationLayout { slots, reusable });

impl Persist for vfs::Handle {
    fn put(&self, w: &mut Writer) {
        w.u64(self.raw());
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(vfs::Handle::from_raw(r.u64()?))
    }
}
persist_struct!(SavedFile {
    id,
    device,
    path,
    access,
    share,
    disposition,
    want_directory,
    attributes,
    position,
    directory,
    search,
});
persist_struct!(VfsState {
    next_handle,
    handles,
    objects,
});

impl Persist for Value {
    fn put(&self, w: &mut Writer) {
        match *self {
            Value::Void => {
                w.u8(0);
            }
            Value::U32(v) => {
                w.u8(1).u32(v);
            }
            Value::U64(v) => {
                w.u8(2).u64(v);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(match r.u8()? {
            0 => Value::Void,
            1 => Value::U32(r.u32()?),
            2 => Value::U64(r.u64()?),
            _ => return Err(corrupt("value kind")),
        })
    }
}

impl Persist for crate::ShutdownNotification {
    fn put(&self, w: &mut Writer) {
        let Self {
            registration,
            routine,
            priority,
        } = self;
        (registration, routine, priority).put_all(w);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, CodecError> {
        Ok(Self {
            registration: u32::get(r)?,
            routine: u32::get(r)?,
            priority: i32::get(r)?,
        })
    }
}

/// Three fields at once, for hand-written impls.
trait PutAll {
    fn put_all(self, w: &mut Writer);
}
impl<A: Persist, B: Persist, C: Persist> PutAll for (&A, &B, &C) {
    fn put_all(self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
        self.2.put(w);
    }
}

/// The kernel's `Core`, every field accounted for.
fn save_core(core: &Core, w: &mut Writer) -> Result<(), SnapshotError> {
    let Core {
        // Saved.
        memory,
        handles,
        objects,
        owned_objects,
        critical,
        threads,
        wait_order,
        ready,
        yielder,
        yielded,
        next_tid,
        clock,
        work_carry,
        saved_av,
        phy_ready,
        encoder,
        launch_page,
        references,
        files,
        file_handles,
        // Saved through the VFS's own state: open objects by path.
        storage,
        // The raw disk's sectors (the cache database), which live in host
        // memory behind the VFS.
        raw_disk,
        links,
        timers,
        mutant_owners,
        guest_references,
        dpcs,
        periodic,
        next_periodic,
        dpc_complete,
        shutdown,
        persistent,
        // Checked: set by the boot, so equal on a machine booted the same way.
        kpcr,
        bridge,
        data,
        title_id,
        utility_partition,
        // Kept from the booted machine: boot configuration and host handles.
        tls: _,
        console: _,
        sections: _,
        // Diagnostics.
        log: _,
        log_bytes: _,
        dropped_log: _,
        stops: _,
        dropped_stops: _,
        deferred_failures: _,
        dropped_deferred_failures: _,
    } = core;
    let vfs = match storage {
        Some(storage) => Some(
            storage
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state()
                .map_err(|e| SnapshotError::Busy(vec![format!("vfs: {e}")]))?,
        ),
        None => None,
    };
    (kpcr, bridge, data).put_all(w);
    (title_id, utility_partition, memory).put_all(w);
    (handles, objects, owned_objects).put_all(w);
    (critical, threads, wait_order).put_all(w);
    (ready, yielder, yielded).put_all(w);
    (next_tid, clock, work_carry).put_all(w);
    (saved_av, phy_ready, encoder).put_all(w);
    (launch_page, references, links).put_all(w);
    (timers, mutant_owners, guest_references).put_all(w);
    (dpcs, next_periodic, dpc_complete).put_all(w);
    (files, file_handles, &vfs).put_all(w);
    (shutdown, persistent, &periodic.len()).put_all(w);
    for (id, p) in periodic {
        id.put(w);
        p.save(w);
    }
    raw_disk.as_ref().map(|d| d.lock().clone()).put(w);
    Ok(())
}

/// Read a `kcore` section into `core`, which a machine booted the same way
/// owns. Everything is decoded and checked before `core` changes.
fn load_core(core: &mut Core, bytes: &[u8]) -> Result<(), SnapshotError> {
    let r = &mut Reader::new(bytes, "kcore");
    let check = |what: &str, same: bool| {
        if same {
            Ok(())
        } else {
            Err(SnapshotError::Mismatch(format!(
                "{what} differs from the booted machine's"
            )))
        }
    };
    check("KPCR", u32::get(r)? == core.kpcr)?;
    check("export bridge", u32::get(r)? == core.bridge)?;
    check(
        "data exports",
        <BTreeMap<u32, u32> as Persist>::get(r)? == core.data,
    )?;
    check(
        "title ID",
        <Option<u32> as Persist>::get(r)? == core.title_id,
    )?;
    check("utility partition", u8::get(r)? == core.utility_partition)?;
    let memory = Persist::get(r)?;
    let handles = Persist::get(r)?;
    let objects = Persist::get(r)?;
    let owned_objects = Persist::get(r)?;
    let critical = Persist::get(r)?;
    let threads = Persist::get(r)?;
    let wait_order = Persist::get(r)?;
    let ready = Persist::get(r)?;
    let yielder = Persist::get(r)?;
    let yielded = Persist::get(r)?;
    let next_tid = Persist::get(r)?;
    let clock = Persist::get(r)?;
    let work_carry = Persist::get(r)?;
    let saved_av = Persist::get(r)?;
    let phy_ready = Persist::get(r)?;
    let encoder = Persist::get(r)?;
    let launch_page = Persist::get(r)?;
    let references = Persist::get(r)?;
    let links = Persist::get(r)?;
    let timers = Persist::get(r)?;
    let mutant_owners = Persist::get(r)?;
    let guest_references = Persist::get(r)?;
    let dpcs = Persist::get(r)?;
    let next_periodic = Persist::get(r)?;
    let dpc_complete = Persist::get(r)?;
    let files = Persist::get(r)?;
    let file_handles = Persist::get(r)?;
    let vfs: Option<VfsState> = Persist::get(r)?;
    let shutdown = Persist::get(r)?;
    let persistent = Persist::get(r)?;
    // Periodic sources are registered by the boot with Rust handlers: the
    // saved schedule goes onto the booted source with the same ID and name.
    let count = usize::get(r)?;
    if count != core.periodic.len() {
        return Err(SnapshotError::Mismatch(format!(
            "{count} periodic sources saved, {} registered",
            core.periodic.len()
        )));
    }
    let mut schedules = Vec::with_capacity(count);
    for _ in 0..count {
        let id = u32::get(r)?;
        let schedule = crate::periodic::Schedule::get(r)?;
        match core.periodic.get(&id) {
            Some(p) if p.name == schedule.name => schedules.push((id, schedule)),
            _ => {
                return Err(SnapshotError::Mismatch(format!(
                    "periodic source {id} ({}) is not registered",
                    schedule.name
                )));
            }
        }
    }
    let disk: Option<crate::rawdisk::Sectors> = Persist::get(r)?;
    r.finish()?;
    if disk.is_some() != core.raw_disk.is_some() {
        return Err(SnapshotError::Mismatch(
            "the kernel's raw disk is mounted on one machine only".into(),
        ));
    }
    if disk
        .as_ref()
        .is_some_and(|d| d.len() > crate::limits::MAX_RAW_DISK_SECTORS)
    {
        return Err(corrupt("more raw disk sectors than the disk keeps").into());
    }
    // The VFS checks every saved file before it changes anything.
    match (&vfs, &core.storage) {
        (Some(state), Some(storage)) => storage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .restore_state(state)
            .map_err(|e| SnapshotError::Mismatch(format!("vfs: {e}")))?,
        (None, None) => {}
        _ => {
            return Err(SnapshotError::Mismatch(
                "storage is attached on one machine only".into(),
            ));
        }
    }
    core.memory = memory;
    core.handles = handles;
    core.objects = objects;
    core.owned_objects = owned_objects;
    core.critical = critical;
    core.threads = threads;
    core.wait_order = wait_order;
    core.ready = ready;
    core.yielder = yielder;
    core.yielded = yielded;
    core.next_tid = next_tid;
    core.clock = clock;
    core.work_carry = work_carry;
    core.saved_av = saved_av;
    core.phy_ready = phy_ready;
    core.encoder = encoder;
    core.launch_page = launch_page;
    core.references = references;
    core.files = files;
    core.file_handles = file_handles;
    core.links = links;
    core.timers = timers;
    core.mutant_owners = mutant_owners;
    core.guest_references = guest_references;
    core.dpcs = dpcs;
    core.next_periodic = next_periodic;
    core.dpc_complete = dpc_complete;
    core.shutdown = shutdown;
    core.persistent = persistent;
    for (id, schedule) in schedules {
        core.periodic.get_mut(&id).unwrap().restore(schedule);
    }
    if let (Some(saved), Some(sectors)) = (disk, &core.raw_disk) {
        *sectors.lock() = saved;
    }
    Ok(())
}

/// One guest thread's context, beyond its kernel record.
struct ThreadState {
    id: u32,
    object: u32,
    tls: u32,
    cpu: CpuAbi,
    interrupts_masked: bool,
    continuations: ContinuationLayout,
}
persist_struct!(ThreadState {
    id,
    object,
    tls,
    cpu,
    interrupts_masked,
    continuations,
});

/// Makes a rebuildable continuation again from its target and state
/// ([`runtime::ExecCtx::call_guest_rebuildable`]).
pub type Rebuild = Box<dyn Fn(u32, [u32; 4]) -> Continuation + Send + Sync>;

/// Rebuilders by continuation origin. The kernel's own (a created thread's
/// return) is always present; the boot adds the ones its HLE crates make
/// (the kernel-call trace's return).
pub struct Rebuilders {
    by_origin: BTreeMap<&'static str, Rebuild>,
}

impl Rebuilders {
    pub fn new() -> Self {
        Self {
            by_origin: BTreeMap::new(),
        }
    }
    pub fn add(&mut self, origin: &'static str, rebuild: Rebuild) -> &mut Self {
        self.by_origin.insert(origin, rebuild);
        self
    }
    fn rebuild(
        &self,
        origin: &str,
        target: u32,
        state: [u32; 4],
    ) -> Option<(&'static str, Continuation)> {
        let (&name, f) = self.by_origin.get_key_value(origin)?;
        Some((name, f(target, state)))
    }
}

impl Default for Rebuilders {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    /// Add the `kcore` and `machine` sections. The machine must be between
    /// slices with nothing live ([`Machine::quiescent`]); guest RAM and the
    /// TSC are the caller's (`snapshot::guest`).
    pub fn save_snapshot(&self, out: &mut SnapshotWriter) -> Result<(), SnapshotError> {
        let q = self.quiescent();
        if !q.is_quiescent() {
            return Err(SnapshotError::Busy(
                q.blockers.iter().map(|b| format!("{b:?}")).collect(),
            ));
        }
        let mut core = Writer::new();
        save_core(&self.kernel.lock(), &mut core)?;
        let mut w = Writer::new();
        let threads = self
            .threads()
            .map(|t| {
                Ok(ThreadState {
                    id: t.id,
                    object: t.object,
                    tls: t.tls,
                    cpu: t.cpu,
                    interrupts_masked: t.ctx.interrupts_masked(),
                    continuations: t.ctx.continuation_layout()?,
                })
            })
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        threads.put(&mut w);
        self.save_schedule(&mut w);
        out.section(KCORE, KCORE_VERSION, Codec::Zstd, core.into_bytes())?;
        out.section(MACHINE, MACHINE_VERSION, Codec::Zstd, w.into_bytes())?;
        Ok(())
    }

    /// Replace this machine's kernel state and threads with a snapshot's.
    /// The machine must be booted from the same image and switches, and
    /// guest RAM and the TSC already loaded. Continuations live at the save
    /// are made again through `rebuilders` and the kernel's own.
    pub fn load_snapshot(
        &mut self,
        snapshot: &Snapshot<'_>,
        rebuilders: &Rebuilders,
    ) -> Result<(), SnapshotError> {
        let core_bytes = snapshot.require(KCORE, KCORE_VERSION..=KCORE_VERSION)?;
        let machine_bytes = snapshot.require(MACHINE, MACHINE_VERSION..=MACHINE_VERSION)?;
        let r = &mut Reader::new(&machine_bytes, "machine");
        let saved: Vec<ThreadState> = Persist::get(r)?;
        let schedule = self.read_schedule(r)?;
        r.finish()?;
        let kernel = Arc::clone(&self.kernel);
        let mut threads = BTreeMap::new();
        for t in saved {
            let mut ctx = ExecCtx::new(Arc::clone(&self.runtime), t.id);
            ctx.set_interrupts_masked(t.interrupts_masked);
            ctx.restore_continuations(&t.continuations, |origin, target, state| {
                if origin == crate::quiesce::THREAD_RETURN {
                    return Some((crate::quiesce::THREAD_RETURN, thread_return(&kernel)));
                }
                rebuilders.rebuild(origin, target, state)
            })?;
            threads.insert(
                t.id,
                Thread {
                    id: t.id,
                    object: t.object,
                    tls: t.tls,
                    cpu: t.cpu,
                    ctx,
                },
            );
        }
        load_core(&mut self.kernel.lock(), &core_bytes)?;
        self.kernel
            .deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.replace_threads(threads, schedule);
        Ok(())
    }
}

/// The continuation every created thread's start routine returns into:
/// terminate the thread with success (`Machine::collect_threads`).
pub(crate) fn thread_return(kernel: &Arc<Kernel>) -> Continuation {
    let kernel = Arc::clone(kernel);
    Box::new(move |_, ctx| {
        let mut core = kernel.lock();
        core.terminate(
            ctx.thread_id,
            crate::status::SUCCESS,
            &mut ctx.runtime().memory(),
        )?;
        core.yielded.insert(ctx.thread_id, TransferAbi::jump(0));
        Ok(TransferAbi::stop(0))
    })
}
