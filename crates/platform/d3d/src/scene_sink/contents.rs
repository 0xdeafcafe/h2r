//! A private target has no pixels inherited from the previous present.
//! Track which whole attachment aspects are defined by this frame. A draw
//! can make a defined aspect depend on an unknown input, but only a full
//! overwrite can make an unknown aspect independent of its old contents.

use std::collections::HashMap;

use frame_scene::{AuxTarget, TargetRef};
use gfx::state::DepthBufferMode;
use gfx::D3DFormat;

use super::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};

const RGB: u8 = 0b111;
const ALPHA: u8 = 8;
const COLOUR: u8 = RGB | ALPHA;
const DEPTH: u8 = 16;
const STENCIL: u8 = 32;

fn channels(red: bool, green: bool, blue: bool, alpha: bool) -> u8 {
    u8::from(red) | (u8::from(green) << 1) | (u8::from(blue) << 2) | (u8::from(alpha) << 3)
}

impl ReplayFrame {
    fn desc(&self, target: TargetRef) -> Option<AuxTarget> {
        match target {
            TargetRef::SceneColour => self.colour,
            TargetRef::SceneDepth => self.depth,
            TargetRef::Aux(i) => self.aux.get(usize::from(i)).copied(),
            TargetRef::None => None,
        }
    }

    fn aspects(&self, target: TargetRef) -> u8 {
        let Some(desc) = self.desc(target) else { return 0 };
        if !desc.depth { return COLOUR }
        match D3DFormat::from_raw(desc.format) {
            Some(D3DFormat::D24S8 | D3DFormat::LinD24S8 | D3DFormat::F24S8 | D3DFormat::LinF24S8) => DEPTH | STENCIL,
            _ => DEPTH,
        }
    }

    /// A reason this stream would produce a different image if its
    /// attachments were freshly allocated. The in-place owner can still
    /// draw it against its persistent attachments.
    pub fn private_target_refusal(&self) -> Option<&'static str> {
        self.read_refusal(self.ops.len(), self.presented, false)
    }

    /// Fresh display targets must also reproduce every guest-visible count.
    pub fn display_target_refusal(&self) -> Option<&'static str> {
        self.read_refusal(self.ops.len(), self.presented, true)
    }

    /// Whether a pass snapshot through the first `end` commands can read
    /// all of `colour` without pixels inherited from another frame.
    pub fn private_read_refusal(&self, end: usize, colour: TargetRef) -> Option<&'static str> {
        self.read_refusal(end, colour, false)
    }

    fn read_refusal(&self, end: usize, colour: TargetRef, feedback: bool) -> Option<&'static str> {
        let mut state = Contents::default();
        let mut attachments = None;
        let mut samples = HashMap::new();
        let mut query_defined = false;
        for (_, op) in self.ops.iter().take(end) {
            match op {
                ReplayOp::Bind { colour, depth } => attachments = Some((*colour, *depth)),
                ReplayOp::Clear { colour, depth, clear } => {
                    if clear.color.is_some() { state.define(*colour, COLOUR); }
                    if clear.depth.is_some() { state.define(*depth, DEPTH); }
                    if clear.stencil.is_some() { state.define(*depth, STENCIL); }
                }
                ReplayOp::ClearRects { colour, depth, clear } => {
                    let covers = |target| self.desc(target).is_some_and(|d| clear.rects.iter().any(|r| r.is_whole(d.width, d.height)));
                    if covers(*colour) && let Some((_, m)) = clear.color {
                        state.define(*colour, channels(m.red, m.green, m.blue, m.alpha));
                    }
                    if covers(*depth) {
                        if clear.depth.is_some() { state.define(*depth, DEPTH); }
                        if clear.stencil.is_some() { state.define(*depth, STENCIL); }
                    }
                }
                ReplayOp::Upload { to, desc, .. } => {
                    if self.desc(*to).is_some_and(|t| (t.width, t.height) == (desc.width, desc.height)) {
                        state.define(*to, self.aspects(*to));
                    }
                }
                ReplayOp::Copy { from, to } => {
                    let from_size = self.desc(*from).map(|t| (t.width, t.height));
                    let to_size = self.desc(*to).map(|t| (t.width, t.height));
                    // A smaller source overwrites only its rectangle at
                    // (0, 0). The rest still depends on the destination.
                    let mut bits = state.bits(*from);
                    if from_size != to_size { bits &= state.bits(*to); }
                    state.targets.insert(*to, bits);
                }
                ReplayOp::Sample { id, of, how } => {
                    // A snapshot freezes this version; an in-place view
                    // can additionally depend on intervening writes.
                    let mut mask = self.aspects(*of);
                    if matches!(how, ReplaySample::Snapshot { opaque: true }) { mask &= !ALPHA; }
                    let live = matches!(how, ReplaySample::InPlace);
                    samples.insert(*id, (state.has(*of, mask), live.then_some((*of, mask))));
                }
                ReplayOp::Draw { draw, textures, .. } => {
                    let Some((colour, depth)) = attachments else { return Some("draw_without_bind") };
                    let r = draw.state.render;
                    let mut inputs = textures.iter().flatten().all(|t| match t {
                        ReplayTexture::Upload(_) => true,
                        ReplayTexture::Placeholder(_) => false,
                        ReplayTexture::Sample(id) => samples.get(id).is_some_and(|(defined, live)| {
                            *defined && live.is_none_or(|(target, mask)| state.has(target, mask))
                        }),
                    });
                    if depth != TargetRef::None {
                        if r.depth_mode != DepthBufferMode::Disabled {
                            inputs &= state.has(depth, DEPTH);
                        }
                        if r.stencil.enable {
                            inputs &= state.has(depth, self.aspects(depth) & STENCIL);
                        }
                    }
                    if draw.hw.zpass_count_enable { query_defined &= inputs; }
                    if r.blend_enable { inputs &= state.has(colour, COLOUR); }
                    if !inputs {
                        let c = r.color_write;
                        state.forget(colour, channels(c.red, c.green, c.blue, c.alpha));
                        if draw.writes_depth_stencil() { state.forget(depth, DEPTH | STENCIL); }
                    }
                }
                ReplayOp::Stage(stage) => {
                    // A typed pass may blend, test depth/stencil and read
                    // any populated attachment supplied by its plan.
                    let s = &stage.surfaces;
                    let complete = [s.color, s.depth].into_iter()
                        .chain(s.sampled.iter().copied())
                        .chain(s.slots.iter().flatten().copied())
                        .all(|t| state.has(t, self.aspects(t)));
                    if !complete {
                        state.forget(s.color, COLOUR);
                        state.forget(s.depth, DEPTH | STENCIL);
                        for (slot, resource) in stage.plan.slots.iter().zip(&s.slots) {
                            if slot.is_some_and(|p| p.written) && let Some(target) = resource {
                                state.forget(*target, COLOUR);
                            }
                        }
                    }
                }
                ReplayOp::BeginFlareQuery => query_defined = true,
                ReplayOp::EndFlareQuery(_) if feedback && !query_defined => {
                    return Some("flare_needs_previous_depth");
                }
                ReplayOp::EndFlareQuery(_) => {}
            }
        }
        (!state.has(colour, COLOUR)).then_some("frame_needs_previous_target_contents")
    }
}

#[derive(Default)]
struct Contents {
    targets: HashMap<TargetRef, u8>,
}

impl Contents {
    fn bits(&self, target: TargetRef) -> u8 {
        self.targets.get(&target).copied().unwrap_or(0)
    }
    fn has(&self, target: TargetRef, mask: u8) -> bool {
        self.bits(target) & mask == mask
    }
    fn define(&mut self, target: TargetRef, mask: u8) {
        *self.targets.entry(target).or_default() |= mask;
    }
    fn forget(&mut self, target: TargetRef, mask: u8) {
        *self.targets.entry(target).or_default() &= !mask;
    }
}
