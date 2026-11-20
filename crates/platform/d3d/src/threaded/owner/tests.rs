//! The frame owner runs through the real worker queue; the invented
//! backend keeps one colour byte per surface so that reads and subsequent
//! frames can observe exactly which target a command wrote.

use super::*;
use crate::backend::{GuestLayoutKind, ResolvedDraw};
use frame_scene::{AuxTarget, Slot, SlotAt};
use gfx::D3DFormat;
use gfx::texture::DecodedTexture;
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct Pixels {
    surfaces: Vec<u8>,
    shown: Vec<u8>,
    clears: usize,
}

impl Backend for Pixels {
    type Surface = u32;
    type Texture = u32;

    fn create_surface(&mut self, _: D3DFormat, _: u32, _: u32, _: bool) -> Result<u32, String> {
        self.surfaces.push(0);
        Ok((self.surfaces.len() - 1) as u32)
    }
    fn destroy_surface(&mut self, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn upload_texture(&mut self, _: &DecodedTexture) -> Result<u32, String> {
        unreachable!()
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn draw(
        &mut self,
        _: u32,
        _: Option<u32>,
        _: &ResolvedDraw,
        _: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        unreachable!()
    }
    fn clear(&mut self, colour: u32, _: Option<u32>, clear: Clear) -> Result<(), String> {
        self.clears += 1;
        self.surfaces[colour as usize] = (clear.color.unwrap()[0] * 255.).round() as u8;
        Ok(())
    }
    fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
        self.surfaces[dst as usize] = self.surfaces[src as usize];
        Ok(true)
    }
    fn present(&mut self, surface: u32, _: u32) -> Result<(), String> {
        self.shown.push(self.surfaces[surface as usize]);
        Ok(())
    }
    fn read_surface(
        &mut self,
        surface: u32,
        _: D3DFormat,
        _: u32,
        _: u32,
        _: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        Ok(vec![self.surfaces[surface as usize]])
    }
}

fn clear(byte: u8) -> Clear {
    Clear {
        color: Some([f32::from(byte) / 255., 0., 0., 1.]),
        ..Clear::NONE
    }
}

fn frame(n: u64, byte: u8) -> ReplayFrame {
    let target = TargetRef::SceneColour;
    let at = SlotAt {
        slot: Slot::Opaque,
        view: 0,
    };
    ReplayFrame {
        present: n,
        presented: target,
        colour: Some(AuxTarget {
            width: 1,
            height: 1,
            format: D3DFormat::A8R8G8B8 as u32,
            depth: false,
        }),
        ops: vec![(
            at,
            ReplayOp::Clear {
                colour: target,
                depth: TargetRef::None,
                clear: clear(byte),
            },
        )],
        ..Default::default()
    }
}

fn device(eligible: bool) -> (ThreadedBackend<Pixels>, Arc<AtomicUsize>, u32) {
    let owned = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&owned);
    let mut gpu = ThreadedBackend::new(Pixels::default())
        .unwrap()
        .with_frame_owner(FrameOwner {
            select: Box::new(move |n| {
                let byte = u8::try_from(n).unwrap();
                let count = Arc::clone(&count);
                Some(Candidate {
                    frame: Arc::new(frame(n, byte)),
                    eligible: Box::new(move |_, _| eligible),
                    draw: Box::new(move |gpu: &mut Pixels, targets| {
                        count.fetch_add(1, Ordering::Relaxed);
                        let target = targets[&TargetRef::SceneColour];
                        gpu.clear(target, None, clear(byte))?;
                        gpu.present(target, 0)?;
                        Ok(Outputs::default())
                    }),
                })
            }),
            declined: |_| {},
            failed: |_| {},
            stopped: || {},
        });
    let target = gpu
        .create_surface(D3DFormat::A8R8G8B8, 1, 1, false)
        .unwrap();
    (gpu, owned, target)
}

fn value(gpu: &mut ThreadedBackend<Pixels>, target: u32) -> u8 {
    gpu.read_surface(
        target,
        D3DFormat::A8R8G8B8,
        1,
        1,
        GuestLayoutKind::Linear { pitch: 4 },
    )
    .unwrap()[0]
}

#[test]
fn an_owned_frame_and_the_following_legacy_frame_share_the_physical_surface() {
    let (mut gpu, accepted, target) = device(true);
    gpu.clear(target, None, clear(1)).unwrap();
    gpu.present(target, 0).unwrap();
    assert_eq!(value(&mut gpu, target), 1);
    // A menu frame shows the retained colour with no new draw.
    gpu.present(target, 0).unwrap();
    assert_eq!(
        gpu.with_inner(|b| (b.clears, b.shown.clone())).unwrap(),
        (1, vec![1, 1])
    );
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
}

#[test]
fn a_mid_frame_guest_read_flushes_the_originals_and_the_next_frame_can_commit() {
    let (mut gpu, accepted, target) = device(true);
    gpu.clear(target, None, clear(1)).unwrap();
    assert_eq!(value(&mut gpu, target), 1);
    gpu.present(target, 0).unwrap();
    gpu.clear(target, None, clear(2)).unwrap();
    gpu.present(target, 0).unwrap();
    assert_eq!(
        gpu.with_inner(|b| (b.clears, b.shown.clone())).unwrap(),
        (2, vec![1, 2])
    );
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
}

#[test]
fn a_different_clear_or_ineligible_target_keeps_every_original_write_once() {
    for eligible in [true, false] {
        let (mut gpu, accepted, target) = device(eligible);
        gpu.clear(target, None, clear(98)).unwrap(); // sealed frame expects 1
        gpu.present(target, 0).unwrap();
        gpu.clear(target, None, clear(2)).unwrap();
        gpu.present(target, 0).unwrap();
        assert_eq!(
            gpu.with_inner(|b| (b.clears, b.shown.clone())).unwrap(),
            (2, vec![98, 2])
        );
        assert_eq!(accepted.load(Ordering::Relaxed), usize::from(eligible));
    }
}

#[test]
fn two_logical_names_cannot_alias_one_physical_target_during_admission() {
    let a = TargetRef::Aux(0);
    let b = TargetRef::SceneColour;
    let at = SlotAt {
        slot: Slot::Opaque,
        view: 0,
    };
    let replay = ReplayFrame {
        presented: b,
        ops: vec![(at, ReplayOp::Copy { from: a, to: b })],
        ..Default::default()
    };
    let same = [Cmd::<Pixels>::Render(RenderOp::Copy { src: 1, dst: 1 })];
    assert!(target_map(&replay, &same, 1).is_none());
}


#[test]
fn a_fresh_allocation_after_a_draw_preserves_that_frames_admission() {
    let (mut gpu, accepted, target) = device(true);
    gpu.clear(target, None, clear(1)).unwrap();
    let spare = gpu.create_surface(D3DFormat::A8R8G8B8, 1, 1, false).unwrap();
    gpu.present(target, 0).unwrap();
    assert_eq!(value(&mut gpu, target), 1);
    assert_eq!(value(&mut gpu, spare), 0);
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
}
