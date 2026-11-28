//! The targets a game view's frame draws, declared by the graph beside the
//! replay (native-only plan, `nor-8b`; docs/graphics/framescene.md,
//! "Slice 6"). Each target the frame's ops touch is one [`Declared`]: its
//! size, how its first use loads it, what the frame does with it and the
//! ops and slots that do it, and its role. [`Owned`] is the graph's own
//! set of them: one surface per [`TargetRole`], allocated by the graph,
//! kept across frames while its shape holds. [`Store`] is it on the render
//! thread: the device makes every render target and depth surface through
//! it (docs/graphics/retire-d3d-display.md, "The graph's targets"), and
//! its map is the one the replay draws on (`draw_display_on`'s `targets`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use d3d::backend::Backend;
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use frame_scene::{Slot, TargetRef, TargetRole};
use gfx::D3DFormat;

/// How a target's first use in the frame loads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// The first op that touches it clears every aspect it has (colour, or
    /// depth and stencil): nothing from before the frame is read.
    Clear,
    /// Its contents are the seed an earlier clear or whole upload left
    /// (`ReplayFrame::initial`).
    Seeded,
    /// Its contents are whatever the frame before left.
    Keep,
}

/// What the frame does with a target, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uses(u8);

impl Uses {
    pub const COLOUR: Uses = Uses(1);
    pub const DEPTH: Uses = Uses(1 << 1);
    pub const COPY_FROM: Uses = Uses(1 << 2);
    pub const COPY_TO: Uses = Uses(1 << 3);
    pub const UPLOAD: Uses = Uses(1 << 4);
    pub const SAMPLED: Uses = Uses(1 << 5);
    pub const PRESENTED: Uses = Uses(1 << 6);

    pub fn contains(self, other: Uses) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the frame writes it: an attachment, a copy's or an upload's
    /// destination.
    pub fn written(self) -> bool {
        self.0 & (Self::COLOUR.0 | Self::DEPTH.0 | Self::COPY_TO.0 | Self::UPLOAD.0) != 0
    }
}

impl std::ops::BitOr for Uses {
    type Output = Uses;
    fn bitor(self, other: Uses) -> Uses {
        Uses(self.0 | other.0)
    }
}

/// One target of the frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declared {
    pub target: TargetRef,
    /// What it is to the game, the same in every frame
    /// ([`ReplayFrame::roles`]); `None` for a target made with none.
    pub role: Option<TargetRole>,
    /// Its size in guest pixels, from the frame's descriptors; `None` when
    /// the frame names none.
    pub size: Option<(u32, u32)>,
    pub depth: bool,
    /// Its guest `D3DFORMAT`, from the same descriptor as `size`.
    pub format: Option<u32>,
    pub load: Load,
    pub uses: Uses,
    /// The ops that touch it, first to last.
    pub ops: Range<usize>,
    /// The slots those ops are in.
    pub slots: BTreeSet<Slot>,
}

/// Every target the frame's ops touch, in the order of their first use,
/// and the presented one. [`TargetRef::None`] is no target.
pub fn declare(frame: &ReplayFrame) -> Vec<Declared> {
    let mut out: Vec<Declared> = Vec::new();
    let mut bound = None;
    for (i, (at, op)) in frame.ops.iter().enumerate() {
        let mut touch = |target: TargetRef, uses: Uses, clears: bool| {
            if target == TargetRef::None {
                return;
            }
            if let Some(d) = out.iter_mut().find(|d| d.target == target) {
                d.uses = d.uses | uses;
                d.ops.end = i + 1;
                d.slots.insert(at.slot);
                return;
            }
            let (size, depth, format) = describe(frame, target);
            let load = if clears {
                Load::Clear
            } else if frame.initial.iter().any(|(t, _)| *t == target) {
                Load::Seeded
            } else {
                Load::Keep
            };
            out.push(Declared { target, role: frame.role(target), size, depth, format, load, uses, ops: i..i + 1, slots: BTreeSet::from([at.slot]) });
        };
        match op {
            ReplayOp::Bind { colour, depth } => bound = Some((*colour, *depth)),
            ReplayOp::Draw { .. } => {
                if let Some((colour, depth)) = bound {
                    touch(colour, Uses::COLOUR, false);
                    touch(depth, Uses::DEPTH, false);
                }
            }
            ReplayOp::Stage(stage) => {
                touch(stage.surfaces.color, Uses::COLOUR, false);
                touch(stage.surfaces.depth, Uses::DEPTH, false);
                for s in stage.surfaces.sampled.iter().chain(stage.surfaces.slots.iter().flatten()) {
                    touch(*s, Uses::SAMPLED, false);
                }
            }
            ReplayOp::Clear { colour, depth, clear } => {
                touch(*colour, Uses::COLOUR, clear.color.is_some());
                touch(*depth, Uses::DEPTH, clear.depth.is_some() && clear.stencil.is_some());
            }
            ReplayOp::ClearRects { colour, depth, .. } => {
                touch(*colour, Uses::COLOUR, false);
                touch(*depth, Uses::DEPTH, false);
            }
            ReplayOp::Copy { from, to } => {
                touch(*from, Uses::COPY_FROM, false);
                touch(*to, Uses::COPY_TO, false);
            }
            ReplayOp::Upload { to, .. } => touch(*to, Uses::UPLOAD, false),
            ReplayOp::Sample { of, .. } => touch(*of, Uses::SAMPLED, false),
            ReplayOp::BeginFlareQuery | ReplayOp::EndFlareQuery(_) | ReplayOp::ObjectShadow(_) => {}
        }
    }
    if frame.presented != TargetRef::None {
        match out.iter_mut().find(|d| d.target == frame.presented) {
            Some(d) => d.uses = d.uses | Uses::PRESENTED,
            None => {
                let (size, depth, format) = describe(frame, frame.presented);
                let load = if frame.initial.iter().any(|(t, _)| *t == frame.presented) { Load::Seeded } else { Load::Keep };
                let end = frame.ops.len();
                out.push(Declared { target: frame.presented, role: frame.role(frame.presented), size, depth, format, load, uses: Uses::PRESENTED, ops: end..end, slots: BTreeSet::new() });
            }
        }
    }
    out
}

/// The ops of `frame` that touch a target `declared` does not name, with
/// the target: empty for a declaration of the same frame.
pub fn undeclared(frame: &ReplayFrame, declared: &[Declared]) -> Vec<(usize, TargetRef)> {
    let named = |t: TargetRef| t == TargetRef::None || declared.iter().any(|d| d.target == t);
    let mut out = Vec::new();
    let mut bound = None;
    for (i, (_, op)) in frame.ops.iter().enumerate() {
        let targets = op_targets(op, &mut bound);
        out.extend(targets.into_iter().filter(|t| !named(*t)).map(|t| (i, t)));
    }
    out
}

/// The targets `op` touches, as [`undeclared`] names them; a bind touches
/// none and moves `bound`, which the draws after it use.
pub(super) fn op_targets(op: &ReplayOp, bound: &mut Option<(TargetRef, TargetRef)>) -> Vec<TargetRef> {
    match op {
        ReplayOp::Bind { colour, depth } => {
            *bound = Some((*colour, *depth));
            Vec::new()
        }
        ReplayOp::Draw { .. } => bound.map(|(c, d)| vec![c, d]).unwrap_or_default(),
        ReplayOp::Stage(stage) => [stage.surfaces.color, stage.surfaces.depth]
            .into_iter()
            .chain(stage.surfaces.sampled.iter().copied())
            .chain(stage.surfaces.slots.iter().flatten().copied())
            .collect(),
        ReplayOp::Clear { colour, depth, .. } | ReplayOp::ClearRects { colour, depth, .. } => vec![*colour, *depth],
        ReplayOp::Copy { from, to } => vec![*from, *to],
        ReplayOp::Upload { to, .. } => vec![*to],
        ReplayOp::Sample { of, .. } => vec![*of],
        ReplayOp::BeginFlareQuery | ReplayOp::EndFlareQuery(_) | ReplayOp::ObjectShadow(_) => Vec::new(),
    }
}

fn describe(frame: &ReplayFrame, target: TargetRef) -> (Option<(u32, u32)>, bool, Option<u32>) {
    let aux = match target {
        TargetRef::SceneColour => frame.colour.as_ref(),
        TargetRef::SceneDepth => frame.depth.as_ref(),
        TargetRef::Aux(i) => frame.aux.get(usize::from(i)),
        TargetRef::None => None,
    };
    match aux {
        Some(a) => (Some((a.width, a.height)), a.depth, Some(a.format)),
        None => (None, target == TargetRef::SceneDepth, None),
    }
}

/// What the graph allocates a target as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub size: (u32, u32),
    pub depth: bool,
    pub format: u32,
    /// A texture's level bound as a target: a surface the draws after it
    /// sample in place.
    pub texture: bool,
}

impl Shape {
    /// The shape of `d`; `None` when the frame names no size or format
    /// for it.
    pub fn of(d: &Declared) -> Option<Shape> {
        let texture = !d.depth && matches!(d.role, Some(TargetRole::GameLevel { .. }));
        Some(Shape { size: d.size?, depth: d.depth, format: d.format?, texture })
    }

    /// The shape the device asks for.
    pub fn of_desc(t: &d3d::TargetDesc) -> Shape {
        Shape { size: (t.width, t.height), depth: t.depth, format: u32::from(t.format as u8), texture: t.texture }
    }
}

/// Makes and frees the graph's surfaces (the backend's).
pub trait Allocate {
    type Surface: Copy;
    /// A surface of `shape` for `role`; `None`: refused.
    fn allocate(&mut self, role: TargetRole, shape: Shape) -> Option<Self::Surface>;
    fn release(&mut self, surface: Self::Surface);
}

/// Why a declared target has no surface this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// The frame names no size or format for it.
    Unsized,
    /// The device made it with no role.
    Unroled,
    /// The allocator refused its shape.
    Allocation,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Refusal::Unsized => "unsized",
            Refusal::Unroled => "unroled",
            Refusal::Allocation => "allocation",
        }
    }
}

/// The game's targets the graph owns, one surface per [`TargetRole`]. A
/// role keeps its surface from frame to frame while its shape holds,
/// whichever per-frame [`TargetRef`] names it; a new shape replaces the
/// surface. A role a frame does not declare is kept for the next that
/// does (a menu frame between two game frames drops nothing). A target
/// with no surface is refused by name and counted, and the frame's map
/// leaves it out: no other surface stands in.
#[derive(Debug)]
pub struct Owned<S> {
    held: BTreeMap<TargetRole, (Shape, S)>,
    pub made: u64,
    pub kept: u64,
    pub released: u64,
    pub refused: BTreeMap<Refusal, u64>,
}

impl<S> Default for Owned<S> {
    fn default() -> Self {
        Self { held: BTreeMap::new(), made: 0, kept: 0, released: 0, refused: BTreeMap::new() }
    }
}

/// What [`Owned::take`] did for a role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken<S> {
    /// The held surface, of the same shape.
    Kept(S),
    /// A new surface; `replaced` is the one of the old shape, no longer
    /// held, for the caller to release when nothing draws on it.
    Made { surface: S, replaced: Option<S> },
}

impl<S> Taken<S> {
    pub fn surface(&self) -> &S {
        match self {
            Taken::Kept(s) | Taken::Made { surface: s, .. } => s,
        }
    }
}

impl<S: Copy> Owned<S> {
    /// The surface for `role` at `shape`: kept, or made through `alloc`.
    /// `None`: the allocator refused (the old surface, if any, is still
    /// held).
    pub fn take<A: Allocate<Surface = S>>(&mut self, role: TargetRole, shape: Shape, alloc: &mut A) -> Option<Taken<S>> {
        if let Some((held, surface)) = self.held.get(&role)
            && *held == shape
        {
            self.kept += 1;
            return Some(Taken::Kept(*surface));
        }
        let surface = alloc.allocate(role, shape)?;
        self.made += 1;
        let replaced = self.held.insert(role, (shape, surface)).map(|(_, old)| old);
        Some(Taken::Made { surface, replaced })
    }

    /// The surfaces for `declared`, made or kept through `alloc` (a
    /// replaced surface released at once), and the targets refused.
    /// [`TargetRef::None`] is never declared.
    pub fn own<A: Allocate<Surface = S>>(
        &mut self,
        declared: &[Declared],
        alloc: &mut A,
    ) -> (HashMap<TargetRef, S>, Vec<(TargetRef, Refusal)>) {
        let mut map = HashMap::new();
        let mut refused = Vec::new();
        for d in declared {
            let Some(shape) = Shape::of(d) else {
                refused.push((d.target, Refusal::Unsized));
                continue;
            };
            let Some(role) = d.role else {
                refused.push((d.target, Refusal::Unroled));
                continue;
            };
            match self.take(role, shape, alloc) {
                Some(taken) => {
                    if let Taken::Made { replaced: Some(old), .. } = taken {
                        alloc.release(old);
                        self.released += 1;
                    }
                    map.insert(d.target, *taken.surface());
                }
                None => refused.push((d.target, Refusal::Allocation)),
            }
        }
        for (_, r) in &refused {
            *self.refused.entry(*r).or_default() += 1;
        }
        (map, refused)
    }

    /// The surface held for `role`.
    pub fn surface(&self, role: TargetRole) -> Option<S> {
        self.held.get(&role).map(|(_, s)| *s)
    }

    /// The surface each of `declared` is on, by its role, held now; the
    /// first that has none, named.
    pub fn map(&self, declared: &[Declared]) -> Result<HashMap<TargetRef, S>, String> {
        let mut map = HashMap::new();
        for d in declared {
            let role = d.role.ok_or_else(|| format!("{:?} has no role", d.target))?;
            let surface = self.surface(role).ok_or_else(|| format!("{:?} ({role:?}) is not held", d.target))?;
            map.insert(d.target, surface);
        }
        Ok(map)
    }

    /// Releases every surface (a device reset, the renderer's end).
    pub fn release_all<A: Allocate<Surface = S>>(&mut self, alloc: &mut A) {
        for (_, (_, surface)) in std::mem::take(&mut self.held) {
            alloc.release(surface);
            self.released += 1;
        }
    }

    /// `render.scene`'s counters: `graph_targets_<made|kept|released>` and
    /// `graph_targets_refused_<cause>`.
    pub fn counters(&self) -> Vec<(String, u64)> {
        let mut out = vec![
            ("graph_targets_made".to_string(), self.made),
            ("graph_targets_kept".to_string(), self.kept),
            ("graph_targets_released".to_string(), self.released),
        ];
        out.extend(self.refused.iter().map(|(r, n)| (format!("graph_targets_refused_{}", r.name()), *n)));
        out
    }
}

/// The backend's surfaces as the graph's allocator.
struct Make<'a, B>(&'a mut B);

impl<B: Backend> Allocate for Make<'_, B> {
    type Surface = B::Surface;
    fn allocate(&mut self, _: TargetRole, shape: Shape) -> Option<B::Surface> {
        let format = D3DFormat::from_raw(shape.format)?;
        let (w, h) = shape.size;
        let made = if shape.texture { self.0.create_texture_target(format, w, h) } else { self.0.create_surface(format, w, h, shape.depth) };
        made.ok()
    }
    fn release(&mut self, surface: B::Surface) {
        // A surface is released when nothing draws on it; a refusal is
        // the backend's and leaves nothing to do.
        let _ = self.0.destroy_surface(surface);
    }
}

/// The frame graph's store on the render thread
/// (`d3d::threaded::owner::TargetStore`): every render target and depth
/// surface the device makes is one of [`Owned`]'s, by role. The device's
/// handles for a role come and go (`claims`, made and given back); a
/// surface a new shape replaces while a handle to it is live is retired
/// and freed when that handle comes back, since commands queued before
/// the new one can still draw on it.
pub struct Store<B: Backend> {
    owned: Owned<B::Surface>,
    claims: BTreeMap<TargetRole, u32>,
    retired: Vec<B::Surface>,
}

impl<B: Backend> Default for Store<B> {
    fn default() -> Self {
        Self { owned: Owned::default(), claims: BTreeMap::new(), retired: Vec::new() }
    }
}

impl<B: Backend> Store<B> {
    /// The graph's targets: [`Owned`].
    pub fn owned(&self) -> &Owned<B::Surface> {
        &self.owned
    }
}

impl<B: Backend> d3d::threaded::owner::TargetStore<B> for Store<B> {
    fn target(&mut self, backend: &mut B, target: d3d::TargetDesc) -> Result<B::Surface, String> {
        let role = target.role;
        let taken = self
            .owned
            .take(role, Shape::of_desc(&target), &mut Make(backend))
            .ok_or_else(|| format!("graph targets: {role:?} refused at {}x{} {:?}", target.width, target.height, target.format))?;
        if let Taken::Made { replaced: Some(old), .. } = taken {
            if self.claims.get(&role).copied().unwrap_or(0) == 0 {
                Make(backend).release(old);
                self.owned.released += 1;
            } else {
                self.retired.push(old);
            }
        }
        *self.claims.entry(role).or_default() += 1;
        Ok(*taken.surface())
    }

    fn released(&mut self, backend: &mut B, role: TargetRole, surface: B::Surface) -> Result<(), String> {
        let claims = self.claims.entry(role).or_default();
        *claims = claims.checked_sub(1).ok_or_else(|| format!("graph targets: {role:?} given back unmade"))?;
        if let Some(i) = self.retired.iter().position(|s| *s == surface) {
            self.retired.swap_remove(i);
            self.owned.released += 1;
            backend.destroy_surface(surface)?;
        }
        Ok(())
    }

    fn map(&self, frame: &ReplayFrame) -> Result<HashMap<TargetRef, B::Surface>, String> {
        self.owned.map(&declare(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{AuxTarget, SlotAt};
    use gfx::{Clear, D3DFormat};

    fn at(slot: Slot) -> SlotAt {
        SlotAt { slot, view: 0 }
    }

    /// The scene colour and depth bound and cleared, an auxiliary target
    /// copied into and sampled, another only presented: each declared once
    /// with its size, its load and its uses, and no op left undeclared.
    #[test]
    fn the_graph_declares_every_target_it_draws() {
        let colour = AuxTarget { width: 640, height: 480, depth: false, format: D3DFormat::LinA8R8G8B8 as u32 };
        let depth = AuxTarget { depth: true, format: D3DFormat::LinD24S8 as u32, ..colour };
        let small = AuxTarget { width: 320, height: 240, ..colour };
        let mut frame = ReplayFrame {
            presented: TargetRef::Aux(1),
            colour: Some(colour),
            depth: Some(depth),
            aux: vec![small, colour],
            ..Default::default()
        };
        let (main, z, snap) = (TargetRef::SceneColour, TargetRef::SceneDepth, TargetRef::Aux(0));
        frame.ops = vec![
            (at(Slot::Opaque), ReplayOp::Bind { colour: main, depth: z }),
            (at(Slot::Opaque), ReplayOp::Clear { colour: main, depth: z, clear: Clear { color: Some([0.; 4]), depth: Some(1.), stencil: Some(0) } }),
            (at(Slot::Opaque), ReplayOp::ObjectShadow(None)),
            (at(Slot::AfterOpaque), ReplayOp::Copy { from: main, to: snap }),
            (at(Slot::AfterOpaque), ReplayOp::Sample { of: snap, id: 0, how: d3d::scene_sink::ReplaySample::InPlace, into: None }),
            (at(Slot::AfterOpaque), ReplayOp::Bind { colour: snap, depth: TargetRef::None }),
            (at(Slot::AfterOpaque), ReplayOp::ClearRects { colour: snap, depth: TargetRef::None, clear: d3d::backend::RectClear { rects: Vec::new(), color: None, depth: None, stencil: None } }),
        ];
        let d = declare(&frame);
        assert_eq!(d.iter().map(|d| d.target).collect::<Vec<_>>(), [main, z, snap, TargetRef::Aux(1)]);
        assert_eq!((d[0].size, d[0].load, d[0].ops.clone()), (Some((640, 480)), Load::Clear, 1..4));
        assert!(d[0].uses.contains(Uses::COLOUR | Uses::COPY_FROM) && d[0].uses.written());
        assert!(d[1].depth && d[1].load == Load::Clear && d[1].uses.contains(Uses::DEPTH));
        assert_eq!((d[2].size, d[2].load), (Some((320, 240)), Load::Keep), "a copy is not a clear");
        assert!(d[2].uses.contains(Uses::COPY_TO | Uses::SAMPLED | Uses::COLOUR));
        assert_eq!(d[2].slots, BTreeSet::from([Slot::AfterOpaque]));
        assert_eq!((d[3].uses, d[3].ops.clone()), (Uses::PRESENTED, 7..7), "presented, never drawn");
        assert!(undeclared(&frame, &d).is_empty());
        assert_eq!(undeclared(&frame, &d[..2]), [(3, snap), (4, snap), (6, snap)], "every op names its target");
    }

    #[derive(Default)]
    struct Pool {
        next: u32,
        live: Vec<u32>,
        refuse: Option<TargetRole>,
    }

    impl Allocate for Pool {
        type Surface = u32;
        fn allocate(&mut self, role: TargetRole, _: Shape) -> Option<u32> {
            if self.refuse == Some(role) {
                return None;
            }
            self.next += 1;
            self.live.push(self.next);
            Some(self.next)
        }
        fn release(&mut self, surface: u32) {
            self.live.retain(|s| *s != surface);
        }
    }

    /// The graph owns its targets: the same shape keeps its surface into
    /// the next frame, a new shape gets a new one and frees the old, and a
    /// target with no size or a refused allocation is named, not stood in
    /// for.
    #[test]
    fn the_graph_owns_its_targets_across_frames() {
        let colour = AuxTarget { width: 640, height: 480, depth: false, format: D3DFormat::LinA8R8G8B8 as u32 };
        let depth = AuxTarget { depth: true, format: D3DFormat::LinD24S8 as u32, ..colour };
        let roles = vec![(TargetRef::SceneColour, TargetRole::Display(0)), (TargetRef::SceneDepth, TargetRole::DisplayDepth)];
        let mut frame = ReplayFrame { presented: TargetRef::SceneColour, colour: Some(colour), depth: Some(depth), roles, ..Default::default() };
        let clear = Clear { color: Some([0.; 4]), depth: Some(1.), stencil: Some(0) };
        frame.ops = vec![(at(Slot::Opaque), ReplayOp::Clear { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth, clear })];
        let (mut owned, mut pool) = (Owned::default(), Pool::default());
        let (first, refused) = owned.own(&declare(&frame), &mut pool);
        assert!(refused.is_empty());
        assert_eq!(first.len(), 2);
        let (second, _) = owned.own(&declare(&frame), &mut pool);
        assert_eq!(second, first, "the same shape keeps its surfaces");
        assert_eq!((owned.made, owned.kept), (2, 2));
        frame.colour = Some(AuxTarget { width: 1280, height: 720, ..colour });
        let (third, _) = owned.own(&declare(&frame), &mut pool);
        assert_ne!(third[&TargetRef::SceneColour], first[&TargetRef::SceneColour], "a new size, a new surface");
        assert_eq!(third[&TargetRef::SceneDepth], first[&TargetRef::SceneDepth]);
        assert_eq!((owned.released, pool.live.len()), (1, 2), "the old colour freed");
        let menu = ReplayFrame::default();
        assert!(owned.own(&declare(&menu), &mut pool).0.is_empty());
        assert_eq!(owned.surface(TargetRole::DisplayDepth), Some(first[&TargetRef::SceneDepth]), "a frame that declares none drops none");
        frame.colour = None;
        pool.refuse = Some(TargetRole::DisplayDepth);
        frame.depth = Some(AuxTarget { width: 320, ..depth });
        let (map, refused) = owned.own(&declare(&frame), &mut pool);
        assert_eq!(refused, [(TargetRef::SceneColour, Refusal::Unsized), (TargetRef::SceneDepth, Refusal::Allocation)]);
        assert!(map.is_empty(), "nothing stands in");
        let c: BTreeMap<_, _> = owned.counters().into_iter().collect();
        assert_eq!((c["graph_targets_refused_unsized"], c["graph_targets_refused_allocation"]), (1, 1));
        owned.release_all(&mut pool);
        assert!(pool.live.is_empty());
    }

    /// `Aux(0)` is the freeze frame's target in one frame and the post
    /// colour's in the next: each role keeps its own surface, whichever
    /// number names it, and a target with no role is refused, not stood in
    /// for.
    #[test]
    fn a_role_keeps_its_surface_when_its_number_moves() {
        let side = AuxTarget { width: 320, height: 240, depth: false, format: D3DFormat::LinA8R8G8B8 as u32 };
        let frame = |roles: [TargetRole; 2]| {
            let mut f = ReplayFrame { aux: vec![side, side], ..Default::default() };
            f.ops = vec![(at(Slot::AfterPost), ReplayOp::Copy { from: TargetRef::Aux(0), to: TargetRef::Aux(1) })];
            f.roles = vec![(TargetRef::Aux(0), roles[0]), (TargetRef::Aux(1), roles[1])];
            f
        };
        let (freeze, post) = (TargetRole::Game(0x12), TargetRole::Game(1));
        let (mut owned, mut pool) = (Owned::default(), Pool::default());
        let (a, _) = owned.own(&declare(&frame([freeze, post])), &mut pool);
        let (b, _) = owned.own(&declare(&frame([post, freeze])), &mut pool);
        assert_eq!(b[&TargetRef::Aux(0)], a[&TargetRef::Aux(1)], "the post colour's surface");
        assert_eq!(b[&TargetRef::Aux(1)], a[&TargetRef::Aux(0)], "the freeze frame's surface");
        assert_eq!((owned.made, owned.kept), (2, 2));
        assert_eq!(owned.map(&declare(&frame([post, freeze]))).unwrap(), b);
        let mut unnamed = frame([post, freeze]);
        unnamed.roles.pop();
        let (map, refused) = owned.own(&declare(&unnamed), &mut pool);
        assert_eq!(refused, [(TargetRef::Aux(1), Refusal::Unroled)]);
        assert!(!map.contains_key(&TargetRef::Aux(1)));
        assert!(owned.map(&declare(&unnamed)).unwrap_err().contains("no role"));
    }

    /// The device's handles for a role through the store: the same shape
    /// is the same surface; a new shape while the old handle is live
    /// retires the old surface until that handle comes back, and with no
    /// live handle frees it at once.
    #[test]
    fn the_store_frees_a_replaced_surface_when_its_handle_comes_back() {
        use d3d::threaded::owner::TargetStore;
        use d3d::{Recorded, RecordingBackend};
        let desc = |width| d3d::TargetDesc {
            role: TargetRole::Game(1), format: D3DFormat::LinA8R8G8B8, width, height: 240, depth: false, texture: false,
        };
        let destroyed = |b: &RecordingBackend| b.calls.iter().filter(|c| matches!(c, Recorded::DestroySurface { .. })).count();
        let (mut store, mut gpu) = (Store::<RecordingBackend>::default(), RecordingBackend::new());
        let first = store.target(&mut gpu, desc(320)).unwrap();
        store.released(&mut gpu, TargetRole::Game(1), first).unwrap();
        assert_eq!(store.target(&mut gpu, desc(320)).unwrap(), first, "a role's surface outlives its handle");
        let second = store.target(&mut gpu, desc(640)).unwrap();
        assert_ne!(second, first);
        assert_eq!(destroyed(&gpu), 0, "the old handle is live: retired");
        store.released(&mut gpu, TargetRole::Game(1), first).unwrap();
        assert_eq!(destroyed(&gpu), 1, "freed when it comes back");
        store.released(&mut gpu, TargetRole::Game(1), second).unwrap();
        let third = store.target(&mut gpu, desc(160)).unwrap();
        assert_eq!(destroyed(&gpu), 2, "no live handle: freed at once");
        assert_eq!(store.owned().surface(TargetRole::Game(1)), Some(third));
        assert!(store.released(&mut gpu, TargetRole::Game(2), third).is_err(), "a role never made");
    }

    /// A depth clear that leaves the stencil loads the depth; a seeded
    /// target says so.
    #[test]
    fn a_partial_clear_keeps_and_a_seed_is_named() {
        let colour = AuxTarget { width: 8, height: 4, depth: false, format: D3DFormat::LinA8R8G8B8 as u32 };
        let mut frame = ReplayFrame { presented: TargetRef::SceneColour, colour: Some(colour), ..Default::default() };
        frame.initial.push((TargetRef::SceneColour, d3d::scene_sink::TargetSeed::Colour([0.; 4])));
        frame.ops = vec![(at(Slot::Opaque), ReplayOp::Clear {
            colour: TargetRef::SceneColour,
            depth: TargetRef::SceneDepth,
            clear: Clear { color: None, depth: Some(1.), stencil: None },
        })];
        let d = declare(&frame);
        assert_eq!((d[0].target, d[0].load), (TargetRef::SceneColour, Load::Seeded));
        assert!(d[0].uses.contains(Uses::PRESENTED));
        assert_eq!((d[1].target, d[1].load), (TargetRef::SceneDepth, Load::Keep));
    }
}
