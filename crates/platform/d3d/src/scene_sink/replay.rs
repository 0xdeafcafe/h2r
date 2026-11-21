//! Owned work for drawing a sealed frame. Texture inputs are either immutable
//! uploads or the result of a sample at a particular point in this stream.

use std::sync::Arc;

use frame_scene::{AuxTarget, SlotAt, TargetRef};
use frame_scene::carried::CameraUse;
use gfx::texture::{DecodeOptions, DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{RectClear, ResolvedDraw, SceneStage};

#[derive(Clone, Debug)]
pub enum ReplayTexture {
    Upload(Arc<DecodedTexture>),
    Placeholder(TextureDesc),
    Sample(u32),
    #[cfg(feature = "gpu")]
    Frozen(super::FrozenSample),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaySample {
    Snapshot { opaque: bool },
    InPlace,
    Depth,
    Alias {
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: TextureDesc,
        options: DecodeOptions,
    },
}

impl std::fmt::Debug for SceneStage<TargetRef> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneStage")
            .field("plan", &self.plan)
            .field("colour", &self.surfaces.color)
            .field("depth", &self.surfaces.depth)
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ReplayOp {
    /// The typed stage at the exact point where the guest released its draws.
    Stage(SceneStage<TargetRef>),
    BeginFlareQuery,
    /// The original ticket issued to the guest. Private playback skips it.
    EndFlareQuery(u64),
    Bind { colour: TargetRef, depth: TargetRef },
    Draw {
        draw: Arc<ResolvedDraw>,
        textures: [Option<ReplayTexture>; 4],
        camera: CameraUse,
    },
    Clear { colour: TargetRef, depth: TargetRef, clear: Clear },
    ClearRects { colour: TargetRef, depth: TargetRef, clear: RectClear },
    Copy { from: TargetRef, to: TargetRef },
    Upload { to: TargetRef, desc: TextureDesc, bytes: Arc<[u8]> },
    Sample { of: TargetRef, id: u32, how: ReplaySample },
}

impl ReplayOp {
    fn map_targets(&mut self, map: impl Fn(TargetRef) -> TargetRef) {
        match self {
            Self::Stage(stage) => {
                stage.surfaces.color = map(stage.surfaces.color);
                stage.surfaces.depth = map(stage.surfaces.depth);
                for s in &mut stage.surfaces.sampled { *s = map(*s); }
                for s in stage.surfaces.slots.iter_mut().flatten() { *s = map(*s); }
            }
            Self::Bind { colour, depth } | Self::Clear { colour, depth, .. } | Self::ClearRects { colour, depth, .. } => {
                *colour = map(*colour);
                *depth = map(*depth);
            }
            Self::Copy { from, to } => {
                *from = map(*from);
                *to = map(*to);
            }
            Self::Upload { to, .. } => *to = map(*to),
            Self::Sample { of, .. } => *of = map(*of),
            Self::Draw { .. } | Self::BeginFlareQuery | Self::EndFlareQuery(_) => {}
        }
    }
}

/// A present's resources and commands. A frame with a refusal cannot be
/// redrawn independently. Frozen texture inputs require the producing GPU device.
#[derive(Debug)]
pub struct ReplayFrame {
    /// Guest presents, including those without captured ops; first present is 1.
    pub present: u64,
    pub flags: u32,
    pub gamma: Option<Box<[u8; 768]>>,
    /// Exact contents last established by an earlier clear or whole upload.
    /// Existing display attachments already hold these pixels.
    pub initial: Vec<(TargetRef, super::TargetSeed)>,
    /// Private target contents retained by the display worker before the
    /// first write of this present. The cell belongs to this sealed frame.
    #[cfg(feature = "gpu")]
    pub target_snapshots: super::TargetSnapshots,
    pub ops: Vec<(SlotAt, ReplayOp)>,
    pub aux: Vec<AuxTarget>,
    pub colour: Option<AuxTarget>,
    pub depth: Option<AuxTarget>,
    pub presented: TargetRef,
    pub refusals: Vec<&'static str>,
}

impl Default for ReplayFrame {
    fn default() -> Self {
        Self {
            present: 0,
            flags: 0,
            gamma: None,
            initial: Vec::new(),
            #[cfg(feature = "gpu")]
            target_snapshots: Default::default(),
            ops: Vec::new(),
            aux: Vec::new(),
            colour: None,
            depth: None,
            presented: TargetRef::None,
            refusals: Vec::new(),
        }
    }
}

impl ReplayFrame {
    /// True when the reference backend received the guest's original draws
    /// throughout this captured frame. A typed pass may already have been
    /// accepted before a parity request arrived in the middle of a frame.
    pub fn guest_reference_complete(&self) -> bool {
        self.refusals.is_empty() && !self.ops.iter().any(|(_, op)| matches!(op, ReplayOp::Stage(_)))
    }

    pub(super) fn refuse(&mut self, cause: &'static str) {
        if !self.refusals.contains(&cause) {
            self.refusals.push(cause);
        }
    }


}

pub(super) struct Recorder<S> {
    pub frame: ReplayFrame,
    // Physical handles can be destroyed and recycled before present. Ops
    // retain the number of the resource, independently of its live handle.
    live: Vec<(S, u16)>,
    seeds: Vec<(S, super::TargetSeed)>,
    bound: Option<(SlotAt, TargetRef, TargetRef)>,
    pub seq: u64,
    next_sample: u32,
    output: Box<dyn FnMut(ReplayFrame) + Send>,
}

impl<S: Copy + Eq> Recorder<S> {
    pub fn new(output: impl FnMut(ReplayFrame) + Send + 'static) -> Self {
        Self {
            frame: ReplayFrame::default(),
            live: Vec::new(),
            seeds: Vec::new(),
            bound: None,
            seq: 0,
            next_sample: 0,
            output: Box::new(output),
        }
    }

    pub fn target(&mut self, surface: S, surfaces: &[(S, AuxTarget)]) -> TargetRef {
        if let Some((_, id)) = self.live.iter().find(|(s, _)| *s == surface) {
            return TargetRef::Aux(*id);
        }
        let Some((_, desc)) = surfaces.iter().find(|(s, _)| *s == surface) else {
            self.frame.refuse("unknown_surface");
            return TargetRef::None;
        };
        let Ok(id) = u16::try_from(self.frame.aux.len()) else {
            self.frame.refuse("too_many_surfaces");
            return TargetRef::None;
        };
        if let Some((_, seed)) = self.seeds.iter().find(|(s, _)| *s == surface) {
            self.frame.initial.push((TargetRef::Aux(id), seed.clone()));
        }
        self.frame.aux.push(*desc);
        self.live.push((surface, id));
        TargetRef::Aux(id)
    }

    pub fn destroy(&mut self, surface: S) {
        self.seeds.retain(|(s, _)| *s != surface);
        self.live.retain(|(s, _)| *s != surface);
    }

    pub fn untracked_write(&mut self) {
        self.seeds.clear();
        self.frame.initial.clear();
        if !self.frame.ops.is_empty() {
            self.frame.refuse("write_outside_scene");
        }
    }

    pub fn bind(&mut self, at: SlotAt, colour: TargetRef, depth: TargetRef) {
        if self.bound != Some((at, colour, depth)) {
            self.frame.ops.push((at, ReplayOp::Bind { colour, depth }));
            self.bound = Some((at, colour, depth));
        }
    }

    pub fn sample(&mut self, at: SlotAt, of: TargetRef, how: ReplaySample) -> Origin {
        let id = self.next_sample;
        self.next_sample += 1;
        let origin = Origin::sample(self.seq, Some(id), &how);
        self.frame.ops.push((at, ReplayOp::Sample { of, id, how }));
        origin
    }

    pub fn finish(&mut self, present: TargetRef) {
        let mut frame = std::mem::take(&mut self.frame);
        if frame.refusals.is_empty() {
            let mut end = super::seeds::final_contents(&frame);
            for &(surface, id) in &self.live {
                self.seeds.retain(|(s, _)| *s != surface);
                if let Some(seed) = end[usize::from(id)].take() {
                    self.seeds.push((surface, seed));
                }
            }
        } else {
            // An external/unrecorded write could have touched any surface.
            self.seeds.clear();
        }
        // Only the actual present surface is the final colour. In particular
        // the source of a copy swap must remain distinct from its destination.
        let depth = frame.ops.iter().find_map(|(_, op)| match op {
            ReplayOp::Stage(stage) if stage.surfaces.color == present && stage.surfaces.depth != present => Some(stage.surfaces.depth),
            ReplayOp::Bind { colour, depth } | ReplayOp::Clear { colour, depth, .. }
            | ReplayOp::ClearRects { colour, depth, .. } if *colour == present && *depth != TargetRef::None => Some(*depth),
            _ => None,
        });
        let mut names = Vec::with_capacity(frame.aux.len());
        for (i, desc) in std::mem::take(&mut frame.aux).into_iter().enumerate() {
            let physical = TargetRef::Aux(i as u16);
            names.push(if physical == present {
                frame.colour = Some(desc);
                TargetRef::SceneColour
            } else if Some(physical) == depth {
                frame.depth = Some(desc);
                TargetRef::SceneDepth
            } else {
                let next = frame.aux.len() as u16;
                frame.aux.push(desc);
                TargetRef::Aux(next)
            });
        }
        let map = |r| match r { TargetRef::Aux(i) => names[usize::from(i)], other => other };
        frame.presented = map(present);
        for (target, _) in &mut frame.initial {
            *target = map(*target);
        }
        for (_, op) in &mut frame.ops {
            op.map_targets(map);
        }
        self.live.clear();
        self.bound = None;
        self.next_sample = 0;
        self.seq = self.seq.wrapping_add(1);
        (self.output)(frame);
    }
}

pub(super) enum Origin {
    Upload(Arc<DecodedTexture>),
    Placeholder(TextureDesc),
    Sample {
        seq: u64,
        id: Option<u32>,
        immutable: bool,
        #[cfg(feature = "gpu")]
        frozen: std::sync::OnceLock<Option<super::FrozenSample>>,
    },
}

impl Origin {
    pub fn sample(seq: u64, id: Option<u32>, how: &ReplaySample) -> Self {
        Self::Sample {
            seq, id,
            immutable: !matches!(how, ReplaySample::InPlace),
            #[cfg(feature = "gpu")]
            frozen: Default::default(),
        }
    }
}

/// The original backend's texture plus the information needed to bind
/// it to a private draw. The census path does not retain texture bytes.
pub struct SinkTexture<T> {
    pub(super) inner: T,
    pub(super) origin: Option<Origin>,
}
