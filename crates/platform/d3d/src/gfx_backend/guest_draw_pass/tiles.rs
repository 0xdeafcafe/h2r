//! E3 lighting's local-light shadow tiles on the native frame graph
//! (docs/graphics/e3-lighting.md, "Shadows from local lights").
//!
//! A guest shadow map is a `D24S8` depth level not of the frame's size.
//! Each shadowed light clears it, draws its casters into it (layer 10) and
//! then draws receivers that compare against it (layer 11). In E3 mode the
//! graph draws those casters into a tile at
//! [`crate::scaled::shadow_scale`] times the level's size, and compares
//! every receiver of a tile with the Sharp edge. The guest's commands,
//! their order and the scene are unchanged; only where the graph draws
//! them and the receivers' filter differ. Retail mode never plans tiles.
//!
//! A level the graph cannot redraw at another size keeps the guest's size
//! and retail's comparison, and is counted in [`TileWork::refused`]: one
//! copied, uploaded or aliased, read by a typed stage or an earlier
//! frame's view, seeded from earlier contents, or bound with a frame-sized
//! attachment. Colour levels bound with a tile are drawn at the tile's
//! size too, since attachments of one pass share a grid.

use std::collections::{HashMap, HashSet};

use frame_types::TargetRef;
use gfx::D3DFormat;

use crate::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture, TargetSeed};

/// What one graph draw did with its tiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileWork {
    /// Shadow maps drawn as tiles: one per shadowed light, counted at the
    /// first caster after each whole depth clear.
    pub tiles: u32,
    /// Caster draws into a tile.
    pub tile_draws: u32,
    /// Receiver draws comparing against a tile with the Sharp edge.
    pub marks: u32,
    /// Shadow maps left at the guest's size with retail's comparison: a
    /// level the plan refused, or a tile sample that fell back to it.
    pub refused: u32,
}

/// The tile plan of one sealed frame and its counts while it replays.
#[derive(Debug)]
pub struct ShadowTiles {
    factor: u32,
    /// Every target drawn at [`Self::factor`]: shadow levels and the
    /// colour levels bound with them.
    members: HashSet<TargetRef>,
    /// The shadow levels among them.
    depths: HashSet<TargetRef>,
    bound: TargetRef,
    /// Tiles that have had a caster since their last whole depth clear.
    drawn: HashSet<TargetRef>,
    /// Sample ids that read a tile.
    samples: HashSet<u32>,
    work: TileWork,
}

impl ShadowTiles {
    fn empty(factor: u32) -> Self {
        Self {
            factor,
            members: HashSet::new(),
            depths: HashSet::new(),
            bound: TargetRef::None,
            drawn: HashSet::new(),
            samples: HashSet::new(),
            work: TileWork::default(),
        }
    }
}

/// A `D24S8` depth level not of the frame's size.
pub(crate) fn is_shadow_level(frame: &ReplayFrame, target: TargetRef) -> bool {
    let (Some(desc), Some(colour)) = (frame.desc(target), frame.colour) else {
        return false;
    };
    desc.depth
        && matches!(
            D3DFormat::from_raw(desc.format),
            Some(D3DFormat::D24S8 | D3DFormat::LinD24S8)
        )
        && (desc.width, desc.height) != (colour.width, colour.height)
}

fn root(parent: &mut HashMap<TargetRef, TargetRef>, t: TargetRef) -> TargetRef {
    let mut r = t;
    while let Some(&p) = parent.get(&r) {
        if p == r {
            break;
        }
        r = p;
    }
    parent.insert(t, r);
    r
}

fn join(parent: &mut HashMap<TargetRef, TargetRef>, a: TargetRef, b: TargetRef) {
    if a == TargetRef::None || b == TargetRef::None {
        return;
    }
    let (ra, rb) = (root(parent, a), root(parent, b));
    parent.entry(ra).or_insert(ra);
    parent.entry(rb).or_insert(rb);
    if ra != rb {
        parent.insert(ra, rb);
    }
}

impl ShadowTiles {
    /// Plans the tiles of `frame` at `factor` times each level's size.
    pub fn plan(frame: &ReplayFrame, factor: u32) -> Self {
        let mut parent = HashMap::new();
        let mut bad = HashSet::from([
            TargetRef::SceneColour,
            TargetRef::SceneDepth,
            frame.presented,
        ]);
        for (_, op) in &frame.ops {
            match op {
                ReplayOp::Bind { colour, depth }
                | ReplayOp::Clear { colour, depth, .. }
                | ReplayOp::ClearRects { colour, depth, .. } => join(&mut parent, *colour, *depth),
                ReplayOp::Copy { from, to } => {
                    bad.extend([*from, *to]);
                }
                ReplayOp::Upload { to, .. } => {
                    bad.insert(*to);
                }
                ReplayOp::Sample {
                    of,
                    how: ReplaySample::Alias { .. },
                    ..
                } => {
                    bad.insert(*of);
                }
                ReplayOp::Stage(stage) => {
                    let s = &stage.surfaces;
                    bad.extend([s.color, s.depth]);
                    bad.extend(s.sampled.iter().copied());
                    bad.extend(s.slots.iter().flatten().copied());
                }
                ReplayOp::Draw { textures, .. } => {
                    for t in textures.iter().flatten() {
                        if let ReplayTexture::Live { target, .. } = t {
                            bad.insert(*target);
                        }
                    }
                }
                _ => {}
            }
        }
        for (target, seed) in &frame.initial {
            if matches!(seed, TargetSeed::Upload { .. }) {
                bad.insert(*target);
            }
        }
        if let Ok(seeds) = frame.frozen_target_seeds() {
            bad.extend(seeds.iter().map(|(t, _)| *t));
        }
        let frame_size = frame.colour.map(|c| (c.width, c.height));
        let targets: Vec<TargetRef> = (0..frame.aux.len())
            .filter_map(|i| u16::try_from(i).ok().map(TargetRef::Aux))
            .chain([TargetRef::SceneDepth])
            .collect();
        let mut groups: HashMap<TargetRef, Vec<TargetRef>> = HashMap::new();
        for &t in &targets {
            let r = root(&mut parent, t);
            groups.entry(r).or_default().push(t);
        }
        let mut plan = Self::empty(factor);
        for members in groups.values() {
            let levels: Vec<TargetRef> = members
                .iter()
                .copied()
                .filter(|&t| is_shadow_level(frame, t))
                .collect();
            if levels.is_empty() {
                continue;
            }
            let refused = factor < 2
                || members.iter().any(|t| {
                    bad.contains(t) || frame.desc(*t).map(|d| (d.width, d.height)) == frame_size
                });
            if refused {
                plan.work.refused += levels.len() as u32;
            } else {
                plan.members.extend(members.iter().copied());
                plan.depths.extend(levels);
            }
        }
        plan
    }

    /// The tile factor, when `target` is drawn as part of a tile.
    pub fn factor_of(&self, target: TargetRef) -> Option<u32> {
        self.members.contains(&target).then_some(self.factor)
    }

    /// Whether `target` is a shadow level drawn as a tile.
    pub fn is_tile(&self, target: TargetRef) -> bool {
        self.depths.contains(&target)
    }

    /// The shadow levels drawn as tiles.
    pub fn tile_levels(&self) -> impl Iterator<Item = TargetRef> + '_ {
        self.depths.iter().copied()
    }

    /// Counts `op` before it runs. True for a receiver of a tile, which
    /// the caller draws with the Sharp edge.
    pub fn note(&mut self, op: &ReplayOp) -> bool {
        match op {
            ReplayOp::Bind { depth, .. } => self.bound = *depth,
            ReplayOp::Clear { depth, clear, .. }
                if clear.depth.is_some() && self.depths.contains(depth) =>
            {
                self.drawn.remove(depth);
            }
            ReplayOp::Sample {
                of,
                id,
                how: ReplaySample::Depth | ReplaySample::InPlace,
                ..
            } if self.depths.contains(of) => {
                self.samples.insert(*id);
            }
            ReplayOp::Draw { textures, .. } => {
                if self.depths.contains(&self.bound) {
                    self.work.tile_draws += 1;
                    if self.drawn.insert(self.bound) {
                        self.work.tiles += 1;
                    }
                }
                let reads = textures
                    .iter()
                    .flatten()
                    .any(|t| matches!(t, ReplayTexture::Sample(id) if self.samples.contains(id)));
                self.work.marks += u32::from(reads);
                return reads;
            }
            _ => {}
        }
        false
    }

    /// A sample of a tile that was taken from the guest-size level
    /// instead: its receivers compare as retail.
    pub fn fell_back(&mut self, id: u32) {
        if self.samples.remove(&id) {
            self.work.refused += 1;
        }
    }

    pub fn work(&self) -> TileWork {
        self.work
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ResolvedDraw, TargetInfo, Viewport};
    use crate::shader::{ShaderInputs, VertexProcessing};
    use crate::state::StateWords;
    use frame_types::{AuxTarget, Slot, SlotAt};
    use gfx::Clear;
    use gfx::vertex::{PrimitiveType, VertexLayout, fvf};
    use std::sync::Arc;

    const FRAME: AuxTarget = AuxTarget {
        width: 64,
        height: 48,
        format: D3DFormat::LinA8R8G8B8 as u32,
        depth: false,
    };
    const MAP: AuxTarget = AuxTarget {
        width: 32,
        height: 32,
        format: D3DFormat::LinD24S8 as u32,
        depth: true,
    };
    const MAP_COLOUR: AuxTarget = AuxTarget {
        width: 32,
        height: 32,
        ..FRAME
    };
    const LEVEL: TargetRef = TargetRef::Aux(0);
    const PARTNER: TargetRef = TargetRef::Aux(1);

    fn frame(ops: Vec<ReplayOp>) -> ReplayFrame {
        let at = SlotAt {
            slot: Slot::Opaque,
            view: 0,
        };
        ReplayFrame {
            colour: Some(FRAME),
            depth: Some(AuxTarget {
                format: D3DFormat::LinD24S8 as u32,
                depth: true,
                ..FRAME
            }),
            aux: vec![MAP, MAP_COLOUR],
            presented: TargetRef::SceneColour,
            ops: ops.into_iter().map(|op| (at, op)).collect(),
            ..Default::default()
        }
    }

    fn bind(colour: TargetRef, depth: TargetRef) -> ReplayOp {
        ReplayOp::Bind { colour, depth }
    }

    fn clear_depth() -> ReplayOp {
        ReplayOp::Clear {
            colour: PARTNER,
            depth: LEVEL,
            clear: Clear {
                depth: Some(1.0),
                stencil: Some(0),
                ..Clear::NONE
            },
        }
    }

    fn sample(id: u32) -> ReplayOp {
        ReplayOp::Sample {
            of: LEVEL,
            id,
            how: ReplaySample::Depth,
            into: None,
        }
    }

    /// The plan reads only a draw's textures and its bound attachments.
    fn packet() -> Arc<ResolvedDraw> {
        let raw = StateWords::defaults(false);
        let fvf = fvf::XYZRHW;
        Arc::new(ResolvedDraw {
            entry: "synthetic tile draw",
            primitive: PrimitiveType::TriangleFan,
            vertex_count: 0,
            layout: VertexLayout::from_fvf(fvf).unwrap(),
            streams: Vec::new(),
            indices: None,
            state: raw.translate(&Default::default()),
            extras: raw.stage_extras(),
            raw: Arc::new(raw),
            target: TargetInfo {
                width: 1,
                height: 1,
                format: D3DFormat::A8R8G8B8,
                depth_format: None,
            },
            texture_info: [None; 4],
            shader: ShaderInputs {
                vertex: VertexProcessing::FixedFunction { fvf },
                provided_inputs: 1,
                vertex_defaults: [[0.0; 4]; 16],
                constants: Arc::new([[0.0; 4]; 192]),
                pixel_shader: None,
                combiner_state: Arc::new([0; 57]),
                final_combiner_latch: false,
                affine_projection: false,
            },
            viewport: Viewport {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                min_z: 0.0,
                max_z: 1.0,
            },
            clip: None,
            clip_rects: Vec::new(),
            hw: Default::default(),
            textures: [None; 4],
            retail_constants: None,
            ui_fit: None,
            glyph_cache: 0,
        })
    }

    fn draw(texture: Option<u32>) -> ReplayOp {
        ReplayOp::Draw {
            draw: packet(),
            textures: [texture.map(ReplayTexture::Sample), None, None, None],
            camera: frame_types::carried::CameraUse::Unknown,
            external: None,
        }
    }

    /// Two shadowed lights over one level: two tiles, each counted at its
    /// first caster, and only the receivers of a tile sample are marks.
    #[test]
    fn each_cleared_and_drawn_level_is_one_tile_and_its_receivers_are_marks() {
        let ops = vec![
            bind(PARTNER, LEVEL),
            clear_depth(),
            draw(None),
            draw(None),
            bind(TargetRef::SceneColour, TargetRef::SceneDepth),
            sample(0),
            draw(Some(0)),
            bind(PARTNER, LEVEL),
            clear_depth(),
            draw(None),
            bind(TargetRef::SceneColour, TargetRef::SceneDepth),
            sample(1),
            draw(Some(1)),
            draw(None),
        ];
        let f = frame(ops);
        let mut tiles = ShadowTiles::plan(&f, 2);
        assert!(tiles.is_tile(LEVEL));
        assert_eq!(tiles.factor_of(LEVEL), Some(2));
        assert_eq!(
            tiles.factor_of(PARTNER),
            Some(2),
            "a colour bound with a tile shares its grid"
        );
        assert_eq!(tiles.factor_of(TargetRef::SceneColour), None);
        let sharp: Vec<bool> = f.ops.iter().map(|(_, op)| tiles.note(op)).collect();
        assert_eq!(sharp.iter().filter(|&&s| s).count(), 2);
        assert!(sharp[6] && sharp[12] && !sharp[13]);
        assert_eq!(
            tiles.work(),
            TileWork {
                tiles: 2,
                tile_draws: 3,
                marks: 2,
                refused: 0
            }
        );
    }

    /// A level the graph cannot redraw at another size keeps retail's map
    /// and is counted; retail mode's factor of 1 plans nothing.
    #[test]
    fn copied_levels_and_factor_one_keep_the_guest_map() {
        let light = || vec![bind(PARTNER, LEVEL), clear_depth(), draw(None)];
        let with = |copy: ReplayOp| {
            let mut ops = light();
            ops.push(copy);
            frame(ops)
        };
        let tiles = ShadowTiles::plan(
            &with(ReplayOp::Copy {
                from: LEVEL,
                to: LEVEL,
            }),
            2,
        );
        assert!(!tiles.is_tile(LEVEL));
        assert_eq!(tiles.factor_of(PARTNER), None);
        assert_eq!(tiles.work().refused, 1);
        assert!(!ShadowTiles::plan(&frame(light()), 1).is_tile(LEVEL));
        assert!(ShadowTiles::plan(&frame(light()), 2).is_tile(LEVEL));
        let read = with(ReplayOp::Copy {
            from: PARTNER,
            to: TargetRef::SceneColour,
        });
        assert!(
            !ShadowTiles::plan(&read, 2).is_tile(LEVEL),
            "a copied partner refuses its level"
        );
    }

    /// The frame's own depth is never a shadow level, and a sample that
    /// fell back stops its receivers being marks.
    #[test]
    fn a_fallen_back_sample_compares_as_retail() {
        let ops = vec![
            bind(PARTNER, LEVEL),
            clear_depth(),
            draw(None),
            // The receiver draws into the frame, not into the tile.
            bind(TargetRef::SceneColour, TargetRef::SceneDepth),
            sample(4),
            draw(Some(4)),
        ];
        let f = frame(ops);
        assert!(!is_shadow_level(&f, TargetRef::SceneDepth));
        let mut tiles = ShadowTiles::plan(&f, 3);
        for (_, op) in &f.ops[..5] {
            tiles.note(op);
        }
        tiles.fell_back(4);
        assert!(!tiles.note(&f.ops[5].1));
        assert_eq!(
            tiles.work(),
            TileWork {
                tiles: 1,
                tile_draws: 1,
                marks: 0,
                refused: 1
            }
        );
    }
}
