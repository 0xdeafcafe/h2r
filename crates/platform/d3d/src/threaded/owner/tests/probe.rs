use std::sync::mpsc::{Receiver, channel};

use super::*;

fn ask(gpu: &mut ThreadedBackend<Pixels>, surface: u32) -> Receiver<Result<Vec<u8>, String>> {
    let (send, answer) = channel();
    gpu.queue_frame_probe_read(
        surface,
        1,
        1,
        Box::new(move |result| send.send(result).unwrap()),
    );
    answer
}

#[test]
fn only_the_chosen_renderer_reads_snapshots_and_original_reads_keep_their_position() {
    for eligible in [true, false] {
        let (mut gpu, owned, target) = device(eligible);
        let before = ask(&mut gpu, target);
        gpu.clear(target, None, clear(1)).unwrap();
        let after = ask(&mut gpu, target);
        gpu.present(target, 0).unwrap();
        let (reads, shown) = gpu
            .with_inner(|b| (b.probe_reads, b.shown.clone()))
            .unwrap();
        assert_eq!(shown, [1]);
        assert_eq!(owned.load(Ordering::Relaxed), usize::from(eligible));
        if eligible {
            assert_eq!(reads, 0);
            assert!(before.recv().is_err());
            assert!(after.recv().is_err());
        } else {
            assert_eq!(reads, 2);
            assert_eq!(before.recv().unwrap().unwrap(), [0, 0, 0, 255]);
            assert_eq!(after.recv().unwrap().unwrap(), [0, 0, 1, 255]);
        }
    }
}

#[test]
fn a_probe_before_an_independent_frame_barrier_preserves_pixels_and_cannot_block_adoption() {
    for eligible in [true, false] {
        let (mut gpu, owned, target) = device(eligible);
        let before = ask(&mut gpu, target);
        gpu.with_surface(target, |backend, physical| {
            backend.surfaces[physical as usize] = 93;
            Ok(())
        })
        .unwrap();
        // No decision has been made for this frame.
        assert!(matches!(
            before.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        gpu.clear(target, None, clear(1)).unwrap();
        gpu.present(target, 0).unwrap();
        gpu.finish().unwrap();
        assert_eq!(owned.load(Ordering::Relaxed), usize::from(eligible));
        if eligible {
            assert!(before.recv().is_err());
        } else {
            assert_eq!(before.recv().unwrap().unwrap(), [0, 0, 0, 255]);
        }
    }
}

#[test]
fn an_ordinary_snapshot_error_is_reported_to_the_probe_and_the_worker_still_presents() {
    let (mut gpu, _, target) = device(false);
    gpu.clear(target, None, clear(1)).unwrap();
    let missing = ask(&mut gpu, u32::MAX);
    gpu.present(target, 0).unwrap();
    let error = missing.recv().unwrap().unwrap_err();
    assert!(error.contains("not live"), "{error}");
    assert_eq!(gpu.with_inner(|b| b.shown.clone()).unwrap(), [1]);
    assert_eq!(gpu.misses(), 0);
}
