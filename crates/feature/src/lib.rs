//! The feature registry (docs/engine/feature-engine.md, "Hooks").
//!
//! A feature declares what it attaches to through a [`Builder`]: frame and
//! game events (the [`frame`](Builder::frame) and [`game`](Builder::game)
//! bridges), or raw hooks on named [`points`](engine::points). Nothing is
//! installed while features build. [`Registry::build`] turns every
//! declaration into a [`Plan`]: one ordered chain per guest address, or a
//! list of [`Conflict`]s naming both parties. It is pure (no runtime, no
//! guest), so the conflicts are unit-tested. [`Plan::install`] then places
//! exactly one override per address, and refuses an address that already
//! has an override the plan did not place.
//!
//! Chain order at one address, outermost first: observers before, `before`
//! hooks, the implementation (or the generated original), `after` hooks in
//! reverse, observers after. See [`plan`].

pub mod arena;
pub mod bridge;
pub mod entry;
pub mod install;
pub mod leaves;
pub mod locale;
pub mod occlusion;
pub mod plan;
pub mod pipeline_render;
pub mod render_census;
pub mod render_loop;
pub mod render_timing;
pub mod snapshot;
pub mod switches;

use std::sync::Arc;

pub use engine::points;
use engine::points::Point;
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, GuestFault, Runtime, TransferAbi};

pub use arena::Arena;
pub use bridge::{Frame, Game};
pub use engine::entry::{Cx, EntryFuture, GuestCall, Ret};
pub use entry::Entry;
pub use install::{InstallError, Installed};
pub use plan::{Base, Conflict, ImageAddrs, Plan, Remap};
pub use snapshot::{FeatureSnapshot, SnapshotPolicy};
pub use switches::Switches;

/// What a feature is, for switches and the plan text.
#[derive(Clone, Debug)]
pub struct Manifest {
    /// `features.<id>` switches it; also the tie-break in chain order.
    pub id: &'static str,
    pub title: &'static str,
    /// On unless switched off.
    pub default_on: bool,
    pub doc: &'static str,
}

impl Manifest {
    pub fn new(id: &'static str, title: &'static str) -> Self {
        Manifest {
            id,
            title,
            default_on: true,
            doc: "",
        }
    }
    pub fn off_by_default(mut self) -> Self {
        self.default_on = false;
        self
    }
    pub fn doc(mut self, doc: &'static str) -> Self {
        self.doc = doc;
        self
    }
}

/// An opt-in enhancement.
pub trait Feature: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn build(&self, b: &mut Builder) -> Result<(), BuildError>;
}

/// A feature refused to build; the registry leaves it out with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError(pub String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a hook sits relative to the others of its kind at one point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Order {
    First,
    Default,
    Last,
}

/// A `before` hook's verdict.
pub enum Flow {
    /// Run the rest of the chain.
    Continue,
    /// Return this transfer to the caller; nothing further in the chain runs
    /// (no original, no `after`, no observers after).
    Return(TransferAbi),
}

/// What a read-only observer sees: the registers at that moment and the
/// runtime, for guest memory and the clock. Observers must not write guest
/// memory (by convention: the guard is mutable).
pub struct Obs<'a> {
    pub cpu: &'a CpuAbi,
    pub runtime: &'a Arc<Runtime>,
    /// The address the chain is installed at.
    pub addr: u32,
    pub point: &'static Point,
}

impl Obs<'_> {
    /// The guest's memory. Do not hold it across anything that takes it.
    pub fn guest(&self) -> runtime::ffi::MemoryGuard<'_> {
        self.runtime.memory()
    }
    /// The logical clock, when the runtime has one.
    pub fn clock(&self) -> Option<u64> {
        self.runtime.guest_time()
    }
    /// A `u32` argument by name, from the point's signature (at entry: for
    /// stack arguments only an observer before is meaningful).
    pub fn arg_u32(&self, name: &str) -> Option<u32> {
        arg_u32(self.point, self.cpu, &self.guest(), name)
    }
}

/// What a mutating hook gets: the CPU and the execution context.
pub struct Call<'a> {
    pub cpu: &'a mut CpuAbi,
    pub ctx: &'a mut ExecCtx,
    pub addr: u32,
    pub point: &'static Point,
}

impl Call<'_> {
    /// Return to the caller now, popping `cleanup` argument bytes (for a
    /// `before` that answers in place of the original).
    pub fn ret(&mut self, cleanup: u32) -> Result<TransferAbi, GuestFault> {
        let rt = Arc::clone(self.ctx.runtime());
        rt.memory().ret(self.cpu, cleanup)
    }
    /// Answer in place of the rest of the chain with an async body (see
    /// [`entry`]): it may await guest calls, and its [`Ret`] returns to the
    /// caller.
    pub fn answer_async(
        &mut self,
        make: impl FnOnce(Cx) -> EntryFuture,
    ) -> Result<Flow, GuestFault> {
        entry::start_with(self.cpu, self.ctx, self.addr, make).map(Flow::Return)
    }
    pub fn arg_u32(&mut self, name: &str) -> Option<u32> {
        let rt = Arc::clone(self.ctx.runtime());
        let mem = rt.memory();
        arg_u32(self.point, self.cpu, &mem, name)
    }
}

fn arg_u32(point: &Point, cpu: &CpuAbi, mem: &runtime::Guest, name: &str) -> Option<u32> {
    use engine::points::Loc;
    Some(match point.sig.arg(name)? {
        Loc::Eax => cpu.eax,
        Loc::Al => cpu.eax & 0xff,
        Loc::Ecx => cpu.ecx,
        Loc::Edx => cpu.edx,
        Loc::Ebx => cpu.ebx,
        Loc::Esi => cpu.esi,
        Loc::Edi => cpu.edi,
        Loc::Xmm0 => u32::from_le_bytes(cpu.xmm[0][..4].try_into().ok()?),
        Loc::Stack(n) => mem.u32(cpu.esp.wrapping_add(4 + 4 * n as u32)).ok()?,
    })
}

pub type ObserveFn = dyn Fn(&Obs) + Send + Sync;
pub type BeforeFn = dyn Fn(&mut Call) -> Result<Flow, GuestFault> + Send + Sync;
pub type AfterFn = dyn Fn(&mut Call) -> Result<(), GuestFault> + Send + Sync;
/// A whole implementation, as a dispatch override is: it owns the ABI.
pub type ImplFn =
    dyn Fn(&mut CpuAbi, &mut ExecCtx, u32) -> Result<TransferAbi, GuestFault> + Send + Sync;

/// The body of one hook.
#[derive(Clone)]
pub enum Run {
    ObserveBefore(Arc<ObserveFn>),
    Before(Arc<BeforeFn>),
    Implement(Arc<ImplFn>),
    After(Arc<AfterFn>),
    ObserveAfter(Arc<ObserveFn>),
}

/// The kind of a hook (docs/engine/feature-engine.md, "Kinds").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    ObserveBefore,
    Before,
    Implement,
    After,
    ObserveAfter,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ObserveBefore => "observe_before",
            Kind::Before => "before",
            Kind::Implement => "implement",
            Kind::After => "after",
            Kind::ObserveAfter => "observe_after",
        }
    }
    /// May write guest state or change control flow.
    pub fn mutates(self) -> bool {
        matches!(self, Kind::Before | Kind::After)
    }
}

impl Run {
    pub fn kind(&self) -> Kind {
        match self {
            Run::ObserveBefore(_) => Kind::ObserveBefore,
            Run::Before(_) => Kind::Before,
            Run::Implement(_) => Kind::Implement,
            Run::After(_) => Kind::After,
            Run::ObserveAfter(_) => Kind::ObserveAfter,
        }
    }
}

/// One declared hook.
#[derive(Clone)]
pub struct Hook {
    pub feature: &'static str,
    pub point: &'static Point,
    pub run: Run,
    pub order: Order,
    /// Features whose hooks at this point sit outside this one: their
    /// `before` runs first and their `after` last.
    pub after_features: Vec<&'static str>,
    /// Dropped with a note when the point is missing, rather than turning
    /// the feature off.
    pub optional: bool,
    /// What the hook is for, in the plan text and the "active" line.
    pub label: &'static str,
    /// The hook acts only on the context it runs on: the registers, the
    /// guest stack and the memory that context sees, plus host state it
    /// only reads. A chain made only of such hooks, with nothing after the
    /// body, may run on the pipelined render thread
    /// ([`HookRef::context_only`], [`Chain::render_thread`](crate::plan::Chain::render_thread)).
    pub context_only: bool,
}

impl Hook {
    pub fn kind(&self) -> Kind {
        self.run.kind()
    }
}

/// A declared hook, for setting its order. Returned by [`Raw`]'s methods.
pub struct HookRef<'a>(&'a mut Hook);

impl HookRef<'_> {
    pub fn first(self) -> Self {
        self.0.order = Order::First;
        self
    }
    pub fn last(self) -> Self {
        self.0.order = Order::Last;
        self
    }
    /// Sit inside `feature`'s hooks of the same kind at this point.
    pub fn after_feature(self, feature: &'static str) -> Self {
        self.0.after_features.push(feature);
        self
    }
    pub fn optional(self) -> Self {
        self.0.optional = true;
        self
    }
    /// Says the hook acts only on the context it runs on: it reads and
    /// writes the registers, the guest stack and memory through its
    /// `Call`/`Obs` (or `ExecCtx`), never live RAM through the runtime,
    /// never host state it shares with the game thread except to read it,
    /// and returns without a Rust continuation (no guest call it waits
    /// on). The pipelined render runs such a chain on its own thread
    /// against its view instead of falling back
    /// (docs/graphics/pipelined-render.md).
    pub fn context_only(self) -> Self {
        self.0.context_only = true;
        self
    }
}

/// A guest allocation the plan maps at install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alloc {
    pub feature: &'static str,
    pub addr: u32,
    pub len: u32,
    pub perms: Permissions,
}

/// Collects every feature's declarations. See the crate docs.
pub struct Registry {
    switches: Switches,
    features: Vec<plan::FeatureState>,
    hooks: Vec<Hook>,
    arena: Arena,
    allocs: Vec<Alloc>,
    entries: Vec<Entry>,
    sources: Vec<locale::Source>,
    locale: locale::Sources,
    /// The feature whose `LOCALE_FIND` implementation serves the sources.
    locale_server: Option<&'static str>,
    /// Each built feature's snapshot policy ([`Builder::snapshot`]).
    snapshot: Vec<snapshot::FeatureSnapshot>,
}

impl Registry {
    pub fn new(switches: Switches) -> Self {
        Registry {
            switches,
            features: Vec::new(),
            hooks: Vec::new(),
            arena: Arena::default(),
            allocs: Vec::new(),
            entries: Vec::new(),
            sources: Vec::new(),
            locale: locale::Sources::default(),
            locale_server: None,
            snapshot: Vec::new(),
        }
    }

    /// Builds `feature` when its switch is on. `Ok(false)`: switched off.
    /// A build error leaves the feature out (nothing it declared is kept)
    /// and is returned. A second feature with the same id is a conflict at
    /// [`build`](Self::build).
    pub fn add(&mut self, feature: &dyn Feature) -> Result<bool, BuildError> {
        let m = feature.manifest();
        let on = self.switches.feature(m.id, m.default_on);
        if !on {
            self.features
                .push(plan::FeatureState::new(&m, Some("switched off".into())));
            return Ok(false);
        }
        let mut b = Builder {
            feature: m.id,
            hooks: Vec::new(),
            arena: self.arena.clone(),
            allocs: Vec::new(),
            entries: Vec::new(),
            entry_page: None,
            switches: &self.switches,
            sources: Vec::new(),
            locale: self.locale.clone(),
            serves_locale: false,
            snapshot: None,
        };
        match feature.build(&mut b) {
            Ok(()) => {
                let Builder {
                    hooks,
                    arena,
                    allocs,
                    entries,
                    sources,
                    serves_locale,
                    snapshot,
                    ..
                } = b;
                self.snapshot.push(snapshot::FeatureSnapshot {
                    id: m.id,
                    policy: snapshot.unwrap_or(SnapshotPolicy::Unaudited),
                });
                self.entries.extend(entries);
                self.sources.extend(sources);
                if serves_locale {
                    self.locale_server = Some(m.id);
                }
                self.hooks.extend(hooks);
                self.arena = arena;
                self.allocs.extend(allocs);
                self.features.push(plan::FeatureState::new(&m, None));
                Ok(true)
            }
            Err(e) => {
                self.features
                    .push(plan::FeatureState::new(&m, Some(format!("build: {e}"))));
                Err(e)
            }
        }
    }

    pub fn switches(&self) -> &Switches {
        &self.switches
    }

    /// The plan, or every conflict found. Pure.
    pub fn build(self, addrs: &dyn ImageAddrs) -> Result<Plan, Vec<Conflict>> {
        let locale = plan::LocaleInput {
            sources: self.sources,
            shared: self.locale,
            server: self.locale_server,
        };
        let snapshot = self.snapshot;
        plan::build(
            self.features,
            self.hooks,
            self.allocs,
            self.entries,
            self.arena,
            locale,
            addrs,
        )
        .map(|mut plan| {
            plan.snapshot = snapshot;
            plan
        })
    }
}

/// What a feature declares through. See [`Feature::build`].
pub struct Builder<'r> {
    feature: &'static str,
    hooks: Vec<Hook>,
    arena: Arena,
    allocs: Vec<Alloc>,
    entries: Vec<Entry>,
    /// The page this feature's entries are packed in, and its next free
    /// offset.
    entry_page: Option<(u32, u32)>,
    switches: &'r Switches,
    sources: Vec<locale::Source>,
    locale: locale::Sources,
    serves_locale: bool,
    snapshot: Option<SnapshotPolicy>,
}

impl<'r> Builder<'r> {
    /// What this feature holds outside guest RAM, for machine snapshots
    /// (docs/runtime/snapshots.md). A feature that never says is
    /// [`SnapshotPolicy::Unaudited`] and blocks every snapshot.
    pub fn snapshot(&mut self, policy: SnapshotPolicy) {
        self.snapshot = Some(policy);
    }
    /// The feature being built.
    pub fn id(&self) -> &'static str {
        self.feature
    }
    /// The run's switches (`impl.*` for a feature that backs a subsystem).
    pub fn switches(&self) -> &Switches {
        self.switches
    }
    pub fn frame(&mut self) -> Frame<'_, 'r> {
        Frame(self)
    }
    pub fn game(&mut self) -> Game<'_, 'r> {
        Game(self)
    }
    /// Our strings (see [`locale`]).
    pub fn locale(&mut self) -> locale::Locale<'_, 'r> {
        locale::Locale(self)
    }
    /// The escape hatch: hooks on points. A feature that needs it is a sign
    /// of a missing extension point.
    pub fn raw(&mut self) -> Raw<'_, 'r> {
        Raw(self)
    }
    /// Guest memory for this feature, from the one reserved range, mapped
    /// (zeroed) at install. Page-granular.
    pub fn guest_alloc(&mut self, len: u32, perms: Permissions) -> Result<u32, BuildError> {
        let addr = self
            .arena
            .alloc(len)
            .ok_or_else(|| BuildError(format!("guest arena full ({len:#x} bytes)")))?;
        self.allocs.push(Alloc {
            feature: self.feature,
            addr,
            len,
            perms,
        });
        Ok(addr)
    }

    /// A function of ours at a fresh guest address: `f` runs (as an async
    /// entry, see [`entry`]) whenever the guest calls that address. For
    /// loaders, vtable slots and callbacks the guest is handed. Entries are
    /// [`ENTRY_STRIDE`] bytes apart on one executable arena page per feature.
    pub fn entry(
        &mut self,
        label: &'static str,
        f: impl Fn(Cx) -> EntryFuture + Send + Sync + 'static,
    ) -> Result<u32, BuildError> {
        let (page, next) = match self.entry_page {
            Some((page, next)) if next < 0x1000 => (page, next),
            _ => (self.guest_alloc(0x1000, Permissions::RX)?, 0),
        };
        self.entry_page = Some((page, next + ENTRY_STRIDE));
        let addr = page + next;
        self.entries.push(Entry {
            feature: self.feature,
            label,
            addr,
            run: Arc::new(f),
        });
        Ok(addr)
    }

    fn push(&mut self, point: &'static Point, run: Run, label: &'static str) -> HookRef<'_> {
        self.hooks.push(Hook {
            feature: self.feature,
            point,
            run,
            order: Order::Default,
            after_features: Vec::new(),
            optional: false,
            label,
            context_only: false,
        });
        HookRef(self.hooks.last_mut().expect("just pushed"))
    }
}

/// Bytes between two entries on a feature's entry page.
pub const ENTRY_STRIDE: u32 = 0x10;

/// Raw hooks. See [`Builder::raw`].
pub struct Raw<'b, 'r>(&'b mut Builder<'r>);

impl<'b> Raw<'b, '_> {
    pub fn observe_before(
        self,
        point: &'static Point,
        label: &'static str,
        f: impl Fn(&Obs) + Send + Sync + 'static,
    ) -> HookRef<'b> {
        self.0.push(point, Run::ObserveBefore(Arc::new(f)), label)
    }
    pub fn observe_after(
        self,
        point: &'static Point,
        label: &'static str,
        f: impl Fn(&Obs) + Send + Sync + 'static,
    ) -> HookRef<'b> {
        self.0.push(point, Run::ObserveAfter(Arc::new(f)), label)
    }
    pub fn before(
        self,
        point: &'static Point,
        label: &'static str,
        f: impl Fn(&mut Call) -> Result<Flow, GuestFault> + Send + Sync + 'static,
    ) -> HookRef<'b> {
        self.0.push(point, Run::Before(Arc::new(f)), label)
    }
    pub fn after(
        self,
        point: &'static Point,
        label: &'static str,
        f: impl Fn(&mut Call) -> Result<(), GuestFault> + Send + Sync + 'static,
    ) -> HookRef<'b> {
        self.0.push(point, Run::After(Arc::new(f)), label)
    }
    pub fn implement(
        self,
        point: &'static Point,
        label: &'static str,
        f: impl Fn(&mut CpuAbi, &mut ExecCtx, u32) -> Result<TransferAbi, GuestFault>
        + Send
        + Sync
        + 'static,
    ) -> HookRef<'b> {
        self.0.push(point, Run::Implement(Arc::new(f)), label)
    }
    /// An implementation as an async entry ([`entry`]): the body may await
    /// guest calls.
    pub fn implement_async(
        self,
        point: &'static Point,
        label: &'static str,
        f: impl Fn(Cx) -> EntryFuture + Send + Sync + 'static,
    ) -> HookRef<'b> {
        let f: Arc<engine::entry::EntryFn> = Arc::new(f);
        self.0.push(
            point,
            Run::Implement(Arc::new(move |cpu, ctx, at| {
                entry::start(&*f, cpu, ctx, at)
            })),
            label,
        )
    }
}

/// A feature from a closure, for features with no state of their own and
/// for tests.
pub struct FnFeature<F> {
    pub manifest: Manifest,
    pub build: F,
}

impl<F> Feature for FnFeature<F>
where
    F: Fn(&mut Builder) -> Result<(), BuildError> + Send + Sync,
{
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        (self.build)(b)
    }
}

/// `FnFeature` with a default manifest for `id`.
pub fn from_fn<F>(id: &'static str, build: F) -> FnFeature<F>
where
    F: Fn(&mut Builder) -> Result<(), BuildError> + Send + Sync,
{
    FnFeature {
        manifest: Manifest::new(id, id),
        build,
    }
}
