//! A private target has no pixels inherited from the previous present.
//! Track which whole attachment aspects are defined by this frame. A draw
//! can make a defined aspect depend on an unknown input, but only a full
//! overwrite can make an unknown aspect independent of its old contents.

mod rects;
#[cfg(feature = "gpu")]
mod overwrite;

use crate::backend::PixelRect;
use rects::Holes;
use std::collections::HashMap;

use frame_scene::{AuxTarget, TargetRef};
use gfx::state::{CompareFunc, DepthBufferMode};
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

    fn upload_aspects(&self, target: TargetRef, format: D3DFormat) -> u8 {
        let mask = self.aspects(target);
        // A D16 upload into a stencil-capable surface fills its depth only.
        if matches!(format, D3DFormat::D16 | D3DFormat::LinD16) { mask & !STENCIL } else { mask }
    }

    /// A reason this stream would produce a different image if its
    /// attachments were freshly allocated. The in-place owner can still
    /// draw it against its persistent attachments.
    pub fn private_target_refusal(&self) -> Option<&'static str> {
        self.read_refusal(self.ops.len(), self.presented, false, false)
    }

    /// Fresh display targets must also reproduce every guest-visible count.
    pub fn display_target_refusal(&self) -> Option<&'static str> {
        self.read_refusal(self.ops.len(), self.presented, true, false)
    }

    /// Whether a pass snapshot through the first `end` commands can read
    /// all of `colour` without pixels inherited from another frame.
    pub fn private_read_refusal(&self, end: usize, colour: TargetRef) -> Option<&'static str> {
        self.read_refusal(end, colour, false, false)
    }

    /// A handoff between live target sets with different histories needs
    /// every attachment and every frozen sampled output to agree. Do not
    /// count retained seeds: in-place replay leaves the live pixels alone.
    pub fn all_targets_defined(&self) -> bool {
        self.refusals.is_empty() && self.read_refusal(self.ops.len(), self.presented, true, true).is_none()
    }

    fn read_refusal(&self, end: usize, colour: TargetRef, feedback: bool, all: bool) -> Option<&'static str> {
        let mut state = Contents::default();
        if !all {
            for (target, seed) in &self.initial {
                let mask = match seed {
                    super::TargetSeed::Colour(_) => COLOUR,
                    super::TargetSeed::Upload { desc, .. } => self.upload_aspects(*target, desc.format),
                };
                state.define(*target, mask);
            }
        }
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
                    if let Some((_, m)) = clear.color {
                        state.cover(*colour, self.desc(*colour), channels(m.red, m.green, m.blue, m.alpha), &clear.rects);
                    }
                    let z = if clear.depth.is_some() { DEPTH } else { 0 };
                    let stencil = if clear.stencil.is_some() { STENCIL } else { 0 };
                    state.cover(*depth, self.desc(*depth), z | stencil, &clear.rects);
                }
                ReplayOp::Upload { to, desc, .. } => {
                    if self.desc(*to).is_some_and(|t| (t.width, t.height) == (desc.width, desc.height)) {
                        state.define(*to, self.upload_aspects(*to, desc.format));
                    }
                }
                ReplayOp::Copy { from, to } => {
                    if from == to { continue; }
                    if let Some(src) = self.desc(*from) {
                        let mask = self.aspects(*to);
                        let known = state.bits(*from) & mask;
                        state.forget(*to, mask & !known);
                        let area = PixelRect { x: 0, y: 0, width: src.width, height: src.height };
                        state.cover(*to, self.desc(*to), known, &[area]);
                    } else {
                        state.forget(*to, self.aspects(*to));
                    }
                }
                ReplayOp::Sample { id, of, how } => {
                    // A snapshot freezes this version. An in-place view reads
                    // the contents present when the draw binds it.
                    let mut mask = self.aspects(*of);
                    if matches!(how, ReplaySample::Snapshot { opaque: true }) { mask &= !ALPHA; }
                    let live = matches!(how, ReplaySample::InPlace);
                    if all && !live && !state.has(*of, mask) { return Some("sample_needs_previous_target_contents"); }
                    samples.insert(*id, (state.has(*of, mask), live.then_some((*of, mask))));
                }
                ReplayOp::Draw { draw, textures, .. } => {
                    let Some((colour, depth)) = attachments else { return Some("draw_without_bind") };
                    let r = draw.state.render;
                    let mut inputs = textures.iter().flatten().all(|t| match t {
                        ReplayTexture::Upload(_) => true,
                        // Immutable input has complete pixels. The display
                        // worker validates its device and reference/view pair
                        // before it changes a live attachment.
                        #[cfg(feature = "gpu")]
                        ReplayTexture::Frozen(_) => true,
                        ReplayTexture::Placeholder(_) => false,
                        ReplayTexture::Sample(id) => samples.get(id).is_some_and(|(defined, live)| {
                            live.map_or(*defined, |(target, mask)| state.has(target, mask))
                        }),
                    });
                    if depth != TargetRef::None {
                        if r.depth_mode != DepthBufferMode::Disabled && !matches!(r.depth_func, CompareFunc::Always | CompareFunc::Never) {
                            inputs &= state.has(depth, DEPTH);
                        }
                        if r.stencil.enable {
                            inputs &= state.has(depth, self.aspects(depth) & STENCIL);
                        }
                    }
                    if draw.hw.zpass_count_enable { query_defined &= inputs; }
                    if r.blend_enable { inputs &= state.has(colour, COLOUR); }
                    let c = r.color_write;
                    if !inputs {
                        state.forget(colour, channels(c.red, c.green, c.blue, c.alpha));
                        if draw.writes_depth_stencil() { state.forget(depth, DEPTH | STENCIL); }
                    } else {
                        #[cfg(feature = "gpu")]
                        if !state.has(colour, channels(c.red, c.green, c.blue, c.alpha))
                            && let Some(target) = self.desc(colour)
                            && overwrite::covers_colour(draw, textures.iter().any(Option::is_some), target)
                        {
                            state.define(colour, channels(c.red, c.green, c.blue, c.alpha));
                        }
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
        if all {
            let declared = self.colour.iter().map(|_| TargetRef::SceneColour)
                .chain(self.depth.iter().map(|_| TargetRef::SceneDepth))
                .chain((0..self.aux.len()).map(|i| TargetRef::Aux(i as u16)));
            if declared.into_iter().any(|t| !state.has(t, self.aspects(t))) {
                return Some("frame_needs_previous_target_contents");
            }
        }
        (!state.has(colour, COLOUR)).then_some("frame_needs_previous_target_contents")
    }
}

#[derive(Default)]
struct Contents {
    targets: HashMap<TargetRef, u8>,
    partial: HashMap<(TargetRef, u8), Holes>,
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
        if mask == 0 { return; }
        *self.targets.entry(target).or_default() &= !mask;
        self.partial.retain(|(t, bit), _| *t != target || *bit & mask == 0);
    }

    fn cover(&mut self, target: TargetRef, desc: Option<AuxTarget>, mask: u8, rects: &[PixelRect]) {
        let Some(desc) = desc else { return };
        let mask = mask & !self.bits(target);
        if mask == 0 || rects.is_empty() || desc.width == 0 || desc.height == 0 { return; }
        if rects.iter().any(|r| r.is_whole(desc.width, desc.height)) {
            self.define(target, mask);
            return;
        }
        for bit in [1, 2, 4, ALPHA, DEPTH, STENCIL] {
            if mask & bit == 0 { continue; }
            let key = (target, bit);
            for &rect in rects {
                let holes = self.partial.entry(key).or_insert_with(|| Holes::whole(desc.width, desc.height));
                match holes.cover(rect) {
                    Ok(true) => {
                        self.define(target, bit);
                        self.partial.remove(&key);
                        break;
                    }
                    Ok(false) => {}
                    Err(()) => { self.partial.remove(&key); }
                }
            }
        }
    }
}
