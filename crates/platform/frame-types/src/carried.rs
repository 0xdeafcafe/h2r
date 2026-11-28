//! The carried part of a frame (docs/graphics/framescene.md, section 4.4
//! and slice 6): the guest's own D3D work, in the frame's order, at the
//! slot of the frame the frame step declared current.
//!
//! [`Slot`] names a position in a game view's frame, in the guest's order.
//! [`SlotTracker`] moves it on the frame step's events (the frame, each
//! game view and the entries around the light pass, the transparents and
//! the post passes). [`Carried`] is what a sink of the guest's device
//! (`d3d::SceneSink`) fills: each op with the slot and view it came in,
//! the targets it names as the scene's own colour and depth or as a side
//! target ([`TargetRef::Aux`]), and the readbacks the guest made, which a
//! scene drawn once at the display scale cannot answer (section 8.3).
//!
//! The draw type is a parameter: `d3d` carries its `ResolvedDraw`, a
//! census carries nothing. This crate does not know either.

/// Whether a carried draw can follow the camera between sealed ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraUse {
    Screen,
    /// Hardware constant index of the verified four-row view projection.
    View { view: u8, first: u8 },
    Unknown,
}

/// A position in the frame, in the guest's order (section 4.4). Within a
/// game view the slots run in declaration order; `Overlay` is the HUD
/// and UI after a game view, `Outside` a frame or a part of one that
/// never enters a game view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    /// The view's opaque block: from the view's entry to the light pass.
    Opaque,
    /// The light pass (`0x00031c80`) and what it leaves to the guest.
    AfterOpaque,
    /// From the light pass's exit to the sorted transparents: layer 2,
    /// the fog, the water, layer 15.
    AfterLights,
    /// From the sorted transparents (`0x0001e370`) to the end of the post
    /// passes: the transparents, the first person, layer 17, the flares,
    /// `render_screen_effects`.
    Transparents,
    /// From `render_screen_effects`' exit to the view's end.
    AfterPost,
    /// The HUD, and the UI after a frame's game views.
    Overlay,
    /// A frame, or the part of one, outside every game view.
    #[default]
    Outside,
}

impl Slot {
    pub const COUNT: usize = 7;
    pub const ALL: [Slot; Slot::COUNT] =
        [Slot::Opaque, Slot::AfterOpaque, Slot::AfterLights, Slot::Transparents, Slot::AfterPost, Slot::Overlay, Slot::Outside];

    pub const fn index(self) -> usize {
        self as usize
    }

    /// The name `render.scene` reports it by.
    pub const fn name(self) -> &'static str {
        match self {
            Slot::Opaque => "opaque",
            Slot::AfterOpaque => "after_opaque",
            Slot::AfterLights => "after_lights",
            Slot::Transparents => "transparents",
            Slot::AfterPost => "after_post",
            Slot::Overlay => "overlay",
            Slot::Outside => "outside",
        }
    }

    /// One of a game view's own slots (not the overlay, not outside).
    pub const fn in_game_view(self) -> bool {
        !matches!(self, Slot::Overlay | Slot::Outside)
    }
}

/// Where the frame step is: the slot, and the guest view index of the
/// game view it is in (the last one, for `Overlay`; 0 outside).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SlotAt {
    pub slot: Slot,
    pub view: u8,
}

/// The frame step's events that move the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotEvent {
    /// `render_frame_and_present`'s entry or exit.
    Frame,
    /// A game view's entry (`render_game_view` `0x0002b790`), with its
    /// guest view index.
    GameView(u8),
    /// The light pass's entry and exit (`0x00031c80`).
    LightsBegin,
    LightsEnd,
    /// The sorted transparents' entry (`0x0001e370`).
    Transparents,
    /// `render_screen_effects`' exit (`0x00013870`).
    PostEnd,
    /// A game view's exit, the HUD's draw (`0x001917eb`) or the end of
    /// `render_views`: the overlay, when the frame has had a game view.
    Overlay,
}

/// The slot the frame step declares current, moved by [`SlotEvent`]s.
/// Inside a game view the slot only moves forward, so an entry that runs
/// twice, or out of the usual order, cannot take it back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotTracker {
    at: SlotAt,
    /// Game views entered since the frame's start.
    views: u32,
}

impl SlotTracker {
    /// Outside every game view, as [`Default`] (usable in a `static`).
    pub const fn new() -> Self {
        Self { at: SlotAt { slot: Slot::Outside, view: 0 }, views: 0 }
    }

    pub fn at(&self) -> SlotAt {
        self.at
    }

    /// Game views entered since the frame's start.
    pub fn views(&self) -> u32 {
        self.views
    }

    pub fn on(&mut self, event: SlotEvent) {
        let forward = |at: &mut SlotAt, to: Slot| {
            if at.slot.in_game_view() && to > at.slot {
                at.slot = to;
            }
        };
        match event {
            SlotEvent::Frame => *self = Self::new(),
            SlotEvent::GameView(view) => {
                self.at = SlotAt { slot: Slot::Opaque, view };
                self.views += 1;
            }
            SlotEvent::LightsBegin => forward(&mut self.at, Slot::AfterOpaque),
            SlotEvent::LightsEnd => forward(&mut self.at, Slot::AfterLights),
            SlotEvent::Transparents => forward(&mut self.at, Slot::Transparents),
            SlotEvent::PostEnd => forward(&mut self.at, Slot::AfterPost),
            SlotEvent::Overlay => {
                if self.views > 0 {
                    self.at.slot = Slot::Overlay;
                }
            }
        }
    }
}

/// A target a carried op names. `SceneColour` and `SceneDepth` are the
/// frame's own (the surfaces the guest presents and the depth bound with
/// them); `Aux(n)` the frame's `n`th other surface, numbered in the order
/// first named, described by [`Carried::aux`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetRef {
    SceneColour,
    SceneDepth,
    Aux(u16),
    /// No surface (a colour-only bind's depth).
    None,
}

/// What a target is to the game, the same from frame to frame. A
/// [`TargetRef::Aux`] is numbered per frame by first use, so one number
/// names another target in the next frame; a role does not. The frame
/// graph owns one surface per role (`native_render::frame_graph::targets`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetRole {
    /// Swap-chain buffer `n` (0 the back buffer).
    Display(u8),
    /// The device's own depth buffer.
    DisplayDepth,
    /// Entry `n` of the game's render-target table: its surface.
    Game(u8),
    /// A level of entry `n`'s texture bound as a target.
    GameLevel(u8),
}

/// How a carried op samples a target on the GPU, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetSample {
    /// A copy of a colour target as it is now (a framebuffer snapshot:
    /// camouflage, screen effects, water).
    Snapshot,
    /// A render-to-texture target sampled in place.
    InPlace,
    /// A depth target's bits as a texture (a depth level or a depth alias).
    Depth,
}

/// One guest op, in the frame's order (section 4.4). `D` is the draw.
#[derive(Clone, Debug, PartialEq)]
pub enum GuestOp<D> {
    Draw(D),
    /// The attachments the ops after it draw into. Emitted when they
    /// change, and again at a new slot or view.
    Bind { colour: TargetRef, depth: TargetRef },
    /// A clear, whole (`rects` false) or of rectangles.
    Clear { colour: TargetRef, depth: TargetRef, rects: bool },
    /// A whole-surface copy.
    Copy { from: TargetRef, to: TargetRef },
    /// Guest texels written into a target.
    Upload { to: TargetRef },
    /// A target sampled by the draws after it.
    Sample { of: TargetRef, how: TargetSample },
}

impl<D> GuestOp<D> {
    /// The targets the op names, colour first.
    pub fn targets(&self) -> [Option<TargetRef>; 2] {
        match *self {
            GuestOp::Draw(_) => [None, None],
            GuestOp::Bind { colour, depth } | GuestOp::Clear { colour, depth, .. } => [Some(colour), Some(depth)],
            GuestOp::Copy { from, to } => [Some(from), Some(to)],
            GuestOp::Upload { to } => [Some(to), None],
            GuestOp::Sample { of, .. } => [Some(of), None],
        }
    }
}

/// A guest read of what was drawn, which a scene drawn once at the display
/// scale cannot answer (section 8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Readback {
    /// A colour target's texels read back to guest memory.
    Surface,
    /// A depth target's words read back to guest memory.
    Depth,
    /// The alias mirror read back.
    Mirror,
    /// A pixel count asked for (`GET_REPORT`: the lens flares' visibility).
    PixelCount,
}

impl Readback {
    pub const COUNT: usize = 4;
    pub const ALL: [Readback; Readback::COUNT] = [Readback::Surface, Readback::Depth, Readback::Mirror, Readback::PixelCount];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn name(self) -> &'static str {
        match self {
            Readback::Surface => "surface",
            Readback::Depth => "depth",
            Readback::Mirror => "mirror",
            Readback::PixelCount => "pixel_count",
        }
    }
}

/// A side target: a surface other than the scene's colour and depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AuxTarget {
    pub width: u32,
    pub height: u32,
    /// The guest's `D3DFORMAT` code.
    pub format: u32,
    pub depth: bool,
}

/// An op with where it came.
#[derive(Clone, Debug, PartialEq)]
pub struct CarriedOp<D> {
    pub at: SlotAt,
    pub op: GuestOp<D>,
}

/// One frame's carried part.
#[derive(Clone, Debug, PartialEq)]
pub struct Carried<D> {
    pub ops: Vec<CarriedOp<D>>,
    /// [`TargetRef::Aux`]'s targets, by number.
    pub aux: Vec<AuxTarget>,
    pub readbacks: Vec<(SlotAt, Readback)>,
}

impl<D> Default for Carried<D> {
    fn default() -> Self {
        Self { ops: Vec::new(), aux: Vec::new(), readbacks: Vec::new() }
    }
}

/// Ops of each kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OpCounts {
    pub draws: u32,
    pub binds: u32,
    pub clears: u32,
    pub copies: u32,
    pub uploads: u32,
    pub samples: u32,
}

impl OpCounts {
    fn add<D>(&mut self, op: &GuestOp<D>) {
        match op {
            GuestOp::Draw(_) => self.draws += 1,
            GuestOp::Bind { .. } => self.binds += 1,
            GuestOp::Clear { .. } => self.clears += 1,
            GuestOp::Copy { .. } => self.copies += 1,
            GuestOp::Upload { .. } => self.uploads += 1,
            GuestOp::Sample { .. } => self.samples += 1,
        }
    }

    pub fn total(&self) -> u32 {
        self.draws + self.binds + self.clears + self.copies + self.uploads + self.samples
    }
}

/// A side target named in a game view, with the ops that named it by slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideTarget {
    pub aux: u16,
    pub target: AuxTarget,
    pub ops: [u32; Slot::COUNT],
}

/// A frame's carried part, counted (`render.scene`'s `carried`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CarriedSummary {
    /// Ops by slot.
    pub slots: [OpCounts; Slot::COUNT],
    /// Per guest view index in the frame's order, its game-view slots' ops.
    pub views: Vec<(u8, [OpCounts; Slot::COUNT])>,
    /// Side targets named in a game view (section 8.3's census).
    pub side_targets: Vec<SideTarget>,
    /// Readbacks in a game view, and outside one, by [`Readback`].
    pub readbacks: [u32; Readback::COUNT],
    pub readbacks_outside: [u32; Readback::COUNT],
}

impl CarriedSummary {
    /// Draws of every slot.
    pub fn draws(&self) -> u64 {
        self.slots.iter().map(|c| u64::from(c.draws)).sum()
    }

    /// Readbacks in a game view: section 8.3's `scene_readback`.
    pub fn readback_total(&self) -> u64 {
        self.readbacks.iter().map(|&n| u64::from(n)).sum()
    }
}

impl<D> Carried<D> {
    /// Empty, keeping the buffers.
    pub fn clear(&mut self) {
        self.ops.clear();
        self.aux.clear();
        self.readbacks.clear();
    }

    pub fn push(&mut self, at: SlotAt, op: GuestOp<D>) {
        self.ops.push(CarriedOp { at, op });
    }

    pub fn readback(&mut self, at: SlotAt, kind: Readback) {
        self.readbacks.push((at, kind));
    }

    /// The ops of `slot`, in order.
    pub fn in_slot(&self, slot: Slot) -> impl Iterator<Item = &GuestOp<D>> {
        self.ops.iter().filter(move |o| o.at.slot == slot).map(|o| &o.op)
    }

    pub fn summary(&self) -> CarriedSummary {
        let mut s = CarriedSummary::default();
        let mut side: Vec<SideTarget> = Vec::new();
        for o in &self.ops {
            let slot = o.at.slot.index();
            s.slots[slot].add(&o.op);
            if !o.at.slot.in_game_view() {
                continue;
            }
            let view = match s.views.iter().position(|(v, _)| *v == o.at.view) {
                Some(i) => i,
                None => {
                    s.views.push((o.at.view, [OpCounts::default(); Slot::COUNT]));
                    s.views.len() - 1
                }
            };
            s.views[view].1[slot].add(&o.op);
            let mut named: [Option<u16>; 2] = [None; 2];
            for (i, t) in o.op.targets().into_iter().enumerate() {
                if let Some(TargetRef::Aux(n)) = t
                    && !named[..i].contains(&Some(n))
                {
                    named[i] = Some(n);
                }
            }
            for n in named.into_iter().flatten() {
                let at = match side.iter().position(|t| t.aux == n) {
                    Some(i) => i,
                    None => {
                        let target = self.aux.get(usize::from(n)).copied().unwrap_or_default();
                        side.push(SideTarget { aux: n, target, ops: [0; Slot::COUNT] });
                        side.len() - 1
                    }
                };
                side[at].ops[slot] += 1;
            }
        }
        side.sort_by_key(|t| t.aux);
        s.side_targets = side;
        for &(at, kind) in &self.readbacks {
            if at.slot.in_game_view() {
                s.readbacks[kind.index()] += 1;
            } else {
                s.readbacks_outside[kind.index()] += 1;
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(slot: Slot, view: u8) -> SlotAt {
        SlotAt { slot, view }
    }

    #[test]
    fn a_game_view_runs_its_slots_in_order_and_the_overlay_follows() {
        let mut t = SlotTracker::default();
        assert_eq!(t.at(), at(Slot::Outside, 0));
        t.on(SlotEvent::Overlay);
        assert_eq!(t.at().slot, Slot::Outside, "no game view yet: still outside");
        t.on(SlotEvent::GameView(1));
        assert_eq!(t.at(), at(Slot::Opaque, 1));
        t.on(SlotEvent::LightsBegin);
        assert_eq!(t.at().slot, Slot::AfterOpaque);
        t.on(SlotEvent::LightsEnd);
        assert_eq!(t.at().slot, Slot::AfterLights);
        t.on(SlotEvent::Transparents);
        assert_eq!(t.at().slot, Slot::Transparents);
        t.on(SlotEvent::PostEnd);
        assert_eq!(t.at().slot, Slot::AfterPost);
        t.on(SlotEvent::Overlay);
        assert_eq!(t.at(), at(Slot::Overlay, 1));
        t.on(SlotEvent::Frame);
        assert_eq!((t.at(), t.views()), (at(Slot::Outside, 0), 0));
    }

    #[test]
    fn a_new_tracker_is_the_default() {
        assert_eq!(SlotTracker::new(), SlotTracker::default());
    }

    #[test]
    fn inside_a_view_the_slot_only_moves_forward_and_outside_it_does_not_move() {
        let mut t = SlotTracker::default();
        t.on(SlotEvent::LightsBegin);
        t.on(SlotEvent::PostEnd);
        assert_eq!(t.at().slot, Slot::Outside);
        t.on(SlotEvent::GameView(0));
        t.on(SlotEvent::Transparents);
        t.on(SlotEvent::LightsBegin);
        t.on(SlotEvent::LightsEnd);
        assert_eq!(t.at().slot, Slot::Transparents);
        t.on(SlotEvent::Overlay);
        t.on(SlotEvent::PostEnd);
        assert_eq!(t.at().slot, Slot::Overlay, "the overlay is not a game view's slot");
        t.on(SlotEvent::GameView(1));
        assert_eq!((t.at(), t.views()), (at(Slot::Opaque, 1), 2), "a second view starts again");
    }

    #[test]
    fn slots_have_distinct_names_in_order() {
        for (i, s) in Slot::ALL.into_iter().enumerate() {
            assert_eq!(s.index(), i);
            assert!(Slot::ALL[..i].iter().all(|o| o.name() != s.name() && *o < s));
        }
        assert_eq!(Slot::ALL.iter().filter(|s| s.in_game_view()).count(), 5);
        for (i, r) in Readback::ALL.into_iter().enumerate() {
            assert_eq!(r.index(), i);
        }
    }

    #[test]
    fn a_summary_counts_by_slot_and_view_and_names_the_side_targets_of_game_views() {
        let mut c: Carried<u32> = Carried::default();
        c.aux.push(AuxTarget { width: 256, height: 256, format: 6, depth: false });
        c.aux.push(AuxTarget { width: 64, height: 64, format: 0x2c, depth: true });
        let scene = GuestOp::Bind { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth };
        c.push(at(Slot::Outside, 0), GuestOp::Bind { colour: TargetRef::Aux(0), depth: TargetRef::None });
        c.push(at(Slot::Outside, 0), GuestOp::Draw(1));
        c.push(at(Slot::Opaque, 0), scene.clone());
        c.push(at(Slot::Opaque, 0), GuestOp::Draw(2));
        c.push(at(Slot::AfterLights, 0), GuestOp::Bind { colour: TargetRef::Aux(0), depth: TargetRef::Aux(1) });
        c.push(at(Slot::AfterLights, 0), GuestOp::Clear { colour: TargetRef::Aux(0), depth: TargetRef::Aux(1), rects: false });
        c.push(at(Slot::AfterLights, 0), GuestOp::Draw(3));
        c.push(at(Slot::Transparents, 0), GuestOp::Sample { of: TargetRef::SceneColour, how: TargetSample::Snapshot });
        c.push(at(Slot::Opaque, 1), scene);
        c.push(at(Slot::Opaque, 1), GuestOp::Draw(4));
        c.push(at(Slot::Overlay, 1), GuestOp::Copy { from: TargetRef::Aux(0), to: TargetRef::Aux(0) });
        c.readback(at(Slot::Transparents, 0), Readback::PixelCount);
        c.readback(at(Slot::Outside, 0), Readback::Surface);
        let s = c.summary();
        assert_eq!(s.draws(), 4);
        assert_eq!(s.slots[Slot::Opaque.index()], OpCounts { draws: 2, binds: 2, ..Default::default() });
        assert_eq!(s.slots[Slot::Outside.index()].total(), 2);
        assert_eq!(s.slots[Slot::Overlay.index()].copies, 1);
        assert_eq!(s.views.iter().map(|(v, _)| *v).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(s.views[0].1[Slot::AfterLights.index()], OpCounts { draws: 1, binds: 1, clears: 1, ..Default::default() });
        assert_eq!(s.views[1].1[Slot::Opaque.index()].draws, 1);
        // Aux 0 outside and in the overlay does not count; in the view, its
        // bind and clear do, once each; aux 1 the same.
        assert_eq!(s.side_targets.len(), 2);
        assert_eq!(s.side_targets[0].aux, 0);
        assert_eq!(s.side_targets[0].target.width, 256);
        assert_eq!(s.side_targets[0].ops[Slot::AfterLights.index()], 2);
        assert_eq!(s.side_targets[0].ops.iter().sum::<u32>(), 2);
        assert!(s.side_targets[1].target.depth);
        assert_eq!(s.readbacks[Readback::PixelCount.index()], 1);
        assert_eq!(s.readback_total(), 1);
        assert_eq!(s.readbacks_outside[Readback::Surface.index()], 1);
        assert_eq!(c.in_slot(Slot::Opaque).filter(|o| matches!(o, GuestOp::Draw(_))).count(), 2);
    }

    #[test]
    fn a_copy_from_a_side_target_to_itself_names_it_once() {
        let mut c: Carried<()> = Carried::default();
        c.aux.push(AuxTarget::default());
        c.push(at(Slot::Transparents, 0), GuestOp::Copy { from: TargetRef::Aux(0), to: TargetRef::Aux(0) });
        assert_eq!(c.summary().side_targets[0].ops.iter().sum::<u32>(), 1);
    }

    #[test]
    fn clearing_keeps_nothing() {
        let mut c: Carried<()> = Carried::default();
        c.aux.push(AuxTarget::default());
        c.push(SlotAt::default(), GuestOp::Draw(()));
        c.readback(SlotAt::default(), Readback::Mirror);
        c.clear();
        assert_eq!(c, Carried::default());
        assert_eq!(c.summary(), CarriedSummary::default());
    }
}
