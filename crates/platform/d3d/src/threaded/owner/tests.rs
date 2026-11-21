//! The frame owner runs through the real worker queue; the invented
//! backend keeps one colour byte per surface so that reads and subsequent
//! frames can observe exactly which target a command wrote.

mod pacing;

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
    parity_inputs: Vec<u8>,
    hold_present: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
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
    fn snapshot_surface(&mut self, surface: u32, _: bool) -> Result<Option<u32>, String> {
        Ok(Some(u32::from(self.surfaces[surface as usize])))
    }
    fn present(&mut self, surface: u32, _: u32) -> Result<(), String> {
        if let Some((entered, resume)) = self.hold_present.take() {
            entered.send(()).map_err(|e| e.to_string())?;
            resume.recv().map_err(|e| e.to_string())?;
        }
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
                Some(Submission::Native(Candidate {
                    frame: Arc::new(frame(n, byte)),
                    eligible: Box::new(move |_, _| eligible),
                    draw: Box::new(move |gpu: &mut Pixels, targets| {
                        count.fetch_add(1, Ordering::Relaxed);
                        let target = targets[&TargetRef::SceneColour];
                        gpu.clear(target, None, clear(byte))?;
                        gpu.present(target, 0)?;
                        Ok(Outputs::default())
                    }),
                }))
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
    assert!(target_map(&replay, &same, &HashMap::new(), 1).is_none());
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

#[test]
fn a_snapshot_keeps_its_value_and_guest_handle_across_native_or_legacy_present() {
    use crate::scene_sink::ReplaySample;

    for matches in [true, false] {
        for release_before_swap in [true, false] {
            let owned = Arc::new(AtomicUsize::new(0));
            let count = owned.clone();
            let mut gpu = ThreadedBackend::new(Pixels::default()).unwrap().with_frame_owner(FrameOwner {
                select: Box::new(move |n| {
                    let mut replay = frame(n, 11);
                    let name = TargetRef::SceneColour;
                    let at = replay.ops[0].0;
                    replay.ops.push((at, ReplayOp::Sample {
                        of: name,
                        id: 17,
                        how: ReplaySample::Snapshot { opaque: matches },
                    }));
                    replay.ops.extend(frame(n, 22).ops);
                    let count = count.clone();
                    Some(Submission::Native(Candidate {
                        frame: Arc::new(replay),
                        eligible: Box::new(|_, _| true),
                        draw: Box::new(move |device: &mut Pixels, targets| {
                            count.fetch_add(1, Ordering::Relaxed);
                            let target = targets[&name];
                            device.clear(target, None, clear(11))?;
                            let frozen = device.snapshot_surface(target, true)?.unwrap();
                            device.clear(target, None, clear(22))?;
                            device.present(target, 0)?;
                            Ok(Outputs {
                                samples: HashMap::from([(17, frozen)]),
                                ..Default::default()
                            })
                        }),
                    }))
                }),
                declined: |_| {},
                failed: |_| {},
                stopped: || {},
            });
            let target = gpu.create_surface(D3DFormat::A8R8G8B8, 1, 1, false).unwrap();
            gpu.clear(target, None, clear(11)).unwrap();
            let mut frozen = gpu.snapshot_surface(target, true).unwrap();
            let handle = frozen.as_ref().unwrap().id;
            gpu.clear(target, None, clear(22)).unwrap();
            if release_before_swap {
                drop(frozen.take());
            }
            gpu.present(target, 0).unwrap();
            let stored = gpu.call(move |w| Ok(w.textures.get(&handle).copied())).unwrap();
            assert_eq!(stored, (!release_before_swap).then_some(11));
            assert_eq!(owned.load(Ordering::Relaxed), usize::from(matches));
            assert_eq!(value(&mut gpu, target), 22);
            drop(frozen);
            assert!(gpu.call(move |w| Ok(!w.textures.contains_key(&handle))).unwrap());
        }
    }
}

#[test]
fn previous_present_flare_reports_can_land_while_the_next_frame_is_retained() {
    use crate::RecordingBackend;

    let mut inner = RecordingBackend::new();
    inner.pixel_count = 47;
    inner.pixel_counts_pending = true; // block only the polling operation
    let owned = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&owned);
    let mut gpu = ThreadedBackend::new(inner).unwrap().with_frame_owner(FrameOwner {
        select: Box::new(move |n| {
            let count = Arc::clone(&count);
            Some(Submission::Native(Candidate {
                frame: Arc::new(frame(n, 13)),
                eligible: Box::new(|_, _| true),
                draw: Box::new(move |gpu: &mut RecordingBackend, targets| {
                    count.fetch_add(1, Ordering::Relaxed);
                    let target = targets[&TargetRef::SceneColour];
                    gpu.clear(target, None, clear(13))?;
                    gpu.present(target, 0)?;
                    Ok(Outputs::default())
                }),
            }))
        }),
        declined: |_| {}, failed: |_| {}, stopped: || {},
    });
    let color = gpu.create_surface(D3DFormat::A8R8G8B8, 1, 1, false).unwrap();
    let read = gpu.report_pixel_count().unwrap().unwrap();
    let discarded = gpu.report_pixel_count().unwrap().unwrap();
    gpu.present(color, 0).unwrap();
    gpu.clear(color, None, clear(13)).unwrap();
    assert_eq!(gpu.try_pixel_count(read).unwrap(), None);
    gpu.drop_pixel_count(discarded);
    assert_eq!(gpu.read_pixel_count(read).unwrap(), 47);
    gpu.present(color, 0).unwrap();
    assert_eq!(gpu.with_inner(|b| b.pixel_reports.len()).unwrap(), 0);
    assert_eq!(owned.load(Ordering::Relaxed), 1);

    // A read from within the current frame must still flush the saved report.
    gpu.clear(color, None, clear(13)).unwrap();
    let current = gpu.report_pixel_count().unwrap().unwrap();
    assert_eq!(gpu.read_pixel_count(current).unwrap(), 47);
    gpu.present(color, 0).unwrap();
    gpu.finish().unwrap();
    assert_eq!(owned.load(Ordering::Relaxed), 1);
}

#[test]
fn a_failed_native_write_faults_the_worker_without_replaying_into_the_same_target() {
    let (mut gpu, _, target) = device(true);
    gpu.frame_owner.as_mut().unwrap().select = Box::new(|n| {
        Some(Submission::Native(Candidate {
            frame: Arc::new(frame(n, 1)),
            eligible: Box::new(|_, _| true),
            draw: Box::new(|gpu, targets| {
                gpu.clear(targets[&TargetRef::SceneColour], None, clear(42))?;
                Err("synthetic submission failure".into())
            }),
        }))
    });
    gpu.clear(target, None, clear(1)).unwrap();
    gpu.present(target, 0).unwrap();
    // The public interface reports the first worker error. Inspect the
    // fake backend after that transaction to distinguish one write from
    // a rewrite by the guest path.
    let (state, observed) = std::sync::mpsc::channel();
    gpu.post(move |worker| {
        let target = worker.surface(target)?;
        let backend = &worker.inner;
        state.send((backend.clears, backend.surfaces[target as usize], backend.shown.clone())).unwrap();
        Ok(())
    });
    assert!(gpu.finish().unwrap_err().contains("synthetic submission failure"));
    drop(gpu);
    assert_eq!(observed.recv().unwrap(), (1, 42, Vec::new()));
}

#[test]
fn a_reference_freezes_the_input_before_the_original_draw_or_refuses_after_a_read() {
    let (mut gpu, _, target) = device(true);
    gpu.clear(target, None, clear(1)).unwrap();
    gpu.present(target, 0).unwrap();
    gpu.frame_owner.as_mut().unwrap().select = Box::new(|n| {
        Some(Submission::Reference {
            frame: Arc::new(frame(n, n as u8)),
            freeze: |gpu, _, targets| {
                let target = targets[&TargetRef::SceneColour];
                gpu.parity_inputs.push(gpu.surfaces[target as usize]);
                // The guest reference must still render even if the requested
                // attachment format cannot be copied to the private renderer.
                Err("synthetic private-copy refusal".into())
            },
        })
    });
    gpu.clear(target, None, clear(2)).unwrap();
    gpu.present(target, 0).unwrap();
    gpu.clear(target, None, clear(3)).unwrap();
    assert_eq!(value(&mut gpu, target), 3);
    gpu.present(target, 0).unwrap();
    assert_eq!(gpu.with_inner(|b| (b.parity_inputs.clone(), b.shown.clone())).unwrap(),
               (vec![1], vec![1, 2, 3]));
}
