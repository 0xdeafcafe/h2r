//! Find the attachment values the private reads actually inherit. Model a
//! missing value through exactly the same proof used to admit a private
//! draw, including copies, colour masks, snapshots and partial clears.

use super::{ALPHA, DEPTH, RGB, STENCIL};
use crate::scene_sink::ReplayFrame;
use frame_scene::TargetRef;

#[cfg(feature = "gpu")]
use crate::gfx_backend::FrozenTarget;

#[cfg(feature = "gpu")]
#[derive(Debug, Default)]
pub struct TargetSnapshots(std::sync::OnceLock<Result<Vec<(TargetRef, FrozenTarget)>, String>>);

impl ReplayFrame {
    /// A minimal set of inherited attachment aspects sufficient for every
    /// requested (number of completed ops, colour target) read. CPU seeds
    /// from earlier uploads and clears already participate in the proof.
    /// A frame that still fails with every input present cannot be repaired
    /// by copying attachments. The returned channel bits are RGBA (0..3),
    /// depth (4) and stencil (5).
    pub fn private_seed_requirements(
        &self,
        reads: &[(usize, TargetRef)],
    ) -> Option<Vec<(TargetRef, u8)>> {
        if !self.refusals.is_empty() {
            return None;
        }
        let mut reads = reads.to_vec();
        reads.sort_by_key(|(end, _)| *end);
        reads.dedup();
        let supplied = |v: &[(TargetRef, u8)]| self.read_refusal(&reads, false, false, v).is_none();
        if supplied(&[]) {
            return Some(Vec::new());
        }

        let mut candidates: Vec<_> = self
            .colour
            .iter()
            .map(|_| TargetRef::SceneColour)
            .chain(self.depth.iter().map(|_| TargetRef::SceneDepth))
            .chain((0..self.aux.len()).map(|i| TargetRef::Aux(i as u16)))
            .map(|t| (t, self.aspects(t)))
            .collect();
        if !supplied(&candidates) {
            return None;
        }
        // Discard entire subtrees of unrelated resources with one walk. A
        // frame that merely copies an old side target into scene colour
        // does not retain either the old display pixels or other targets.
        fn reduce(
            candidates: &mut [(TargetRef, u8)],
            offset: usize,
            n: usize,
            supplied: &impl Fn(&[(TargetRef, u8)]) -> bool,
        ) {
            let range = offset..offset + n;
            let removed: Vec<_> = candidates[range.clone()]
                .iter_mut()
                .map(|(_, mask)| std::mem::take(mask))
                .collect();
            if supplied(candidates) {
                return;
            }
            for ((_, mask), old) in candidates[range].iter_mut().zip(removed) {
                *mask = old;
            }
            if n > 1 {
                let half = n / 2;
                reduce(candidates, offset, half, supplied);
                reduce(candidates, offset + half, n - half, supplied);
            }
        }
        let count = candidates.len();
        reduce(&mut candidates, 0, count, &supplied);
        candidates.retain(|(_, mask)| *mask != 0);
        for i in 0..candidates.len() {
            // Colour channels usually travel together. Check RGB as a
            // group first, then its individual channels if needed.
            for mask in [RGB, 1, 2, 4, ALPHA, DEPTH, STENCIL] {
                let old = candidates[i].1;
                if old & mask == 0 {
                    continue;
                }
                candidates[i].1 &= !mask;
                if !supplied(&candidates) {
                    candidates[i].1 = old;
                }
            }
        }
        Some(candidates)
    }

    /// Publish one set, obtained before this sealed frame's first write.
    #[cfg(feature = "gpu")]
    pub fn seal_target_seeds(
        &self,
        result: Result<Vec<(TargetRef, FrozenTarget)>, String>,
    ) -> Result<(), String> {
        self.target_snapshots
            .0
            .set(result)
            .map_err(|_| "frame target snapshots already sealed".to_owned())
    }

    #[cfg(feature = "gpu")]
    pub fn target_seeds_sealed(&self) -> bool {
        self.target_snapshots.0.get().is_some()
    }

    /// A pending worker or failed copy never counts as a defined attachment.
    #[cfg(feature = "gpu")]
    pub fn frozen_target_seeds(&self) -> Result<&[(TargetRef, FrozenTarget)], String> {
        self.target_snapshots
            .0
            .get()
            .ok_or("frame target snapshots pending")?
            .as_deref()
            .map_err(Clone::clone)
    }

    pub(super) fn copied_aspects(&self) -> Vec<(TargetRef, u8)> {
        #[cfg(feature = "gpu")]
        if let Ok(seeds) = self.frozen_target_seeds() {
            return seeds.iter().map(|(t, _)| (*t, self.aspects(*t))).collect();
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ChannelMask, PixelRect, RectClear};
    use crate::scene_sink::ReplayOp;
    use frame_scene::{AuxTarget, Slot, SlotAt};
    use gfx::{Clear, D3DFormat};

    #[test]
    fn copy_only_the_inherited_channels_reaching_the_requested_pass() {
        let shown = TargetRef::SceneColour;
        let source = TargetRef::Aux(1);
        let at = SlotAt {
            slot: Slot::Opaque,
            view: 0,
        };
        let desc = AuxTarget {
            width: 5,
            height: 3,
            format: D3DFormat::LinA8R8G8B8 as u32,
            depth: false,
        };
        let clear_rgb = ReplayOp::ClearRects {
            colour: source,
            depth: TargetRef::None,
            clear: RectClear {
                rects: vec![PixelRect {
                    x: 0,
                    y: 0,
                    width: 5,
                    height: 3,
                }],
                color: Some((
                    [0.1, 0.2, 0.3, 1.0],
                    ChannelMask {
                        red: true,
                        green: true,
                        blue: true,
                        alpha: false,
                    },
                )),
                depth: None,
                stencil: None,
            },
        };
        let frame = ReplayFrame {
            presented: shown,
            colour: Some(desc),
            aux: vec![desc, desc, desc],
            ops: vec![
                (at, clear_rgb),
                (
                    at,
                    ReplayOp::Copy {
                        from: source,
                        to: shown,
                    },
                ),
                (
                    at,
                    ReplayOp::Clear {
                        colour: shown,
                        depth: TargetRef::None,
                        clear: Clear::all([0.6; 4]),
                    },
                ),
            ],
            ..Default::default()
        };
        assert_eq!(frame.private_seed_requirements(&[(3, shown)]), Some(vec![]));
        assert_eq!(
            frame.private_seed_requirements(&[(3, shown), (2, shown)]),
            Some(vec![(source, ALPHA)])
        );
        assert_eq!(frame.private_seed_requirements(&[(4, shown)]), None);
        assert!(
            frame.private_read_refusal(2, shown).is_some(),
            "asking for the copy does not make it available"
        );
    }

    #[test]
    #[cfg(feature = "gpu")]
    fn failed_or_missing_initial_copy_never_claims_defined_pixels() {
        let scene = TargetRef::SceneColour;
        let frame = ReplayFrame {
            presented: scene,
            colour: Some(AuxTarget {
                width: 4,
                height: 3,
                format: D3DFormat::LinA8R8G8B8 as u32,
                depth: false,
            }),
            ..Default::default()
        };
        assert!(!frame.target_seeds_sealed());
        assert_eq!(
            frame.private_seed_requirements(&[(0, scene)]),
            Some(vec![(scene, RGB | ALPHA)])
        );
        assert!(frame.private_target_refusal().is_some());
        frame
            .seal_target_seeds(Err("source was released".into()))
            .unwrap();
        assert!(frame.target_seeds_sealed());
        assert_eq!(
            frame.frozen_target_seeds().unwrap_err(),
            "source was released"
        );
        assert!(frame.seal_target_seeds(Ok(vec![])).is_err());
        assert!(frame.private_target_refusal().is_some());
        assert!(!frame.all_targets_defined());
    }
}
