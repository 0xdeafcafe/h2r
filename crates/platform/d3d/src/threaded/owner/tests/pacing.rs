use super::*;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
enum Swap {
    Entered(u8),
    Returned(u8),
}

#[test]
fn display_capacity_releases_owned_frames_on_take_and_originals_on_completion() {
    for eligible in [true, false] {
        let (presenting, begun) = mpsc::channel();
        let (resume, show) = mpsc::channel();
        let (progress, swaps) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            let (mut gpu, count, target) = device(eligible);
            gpu.with_inner(move |pixels| pixels.hold_present = Some((presenting, show))).unwrap();
            for n in 1..=4 {
                gpu.clear(target, None, clear(n)).unwrap();
                progress.send(Swap::Entered(n)).unwrap();
                gpu.present(target, 0).unwrap();
                progress.send(Swap::Returned(n)).unwrap();
            }
            let shown = gpu.with_inner(|b| b.shown.clone()).unwrap();
            (shown, count.load(Ordering::Relaxed))
        });

        let timeout = Duration::from_secs(5);
        begun.recv_timeout(timeout).unwrap();
        // The physical presenter holds the first frame. Once taken by the
        // native worker it occupies no waiting slot. A declined frame
        // still owns one slot until its original Swap returns.
        let through = if eligible { 3 } else { 2 };
        for n in 1..=through {
            assert_eq!(swaps.recv_timeout(timeout).unwrap(), Swap::Entered(n));
            assert_eq!(swaps.recv_timeout(timeout).unwrap(), Swap::Returned(n));
        }
        assert_eq!(swaps.recv_timeout(timeout).unwrap(), Swap::Entered(through + 1));
        assert_eq!(swaps.recv_timeout(Duration::from_millis(30)), Err(mpsc::RecvTimeoutError::Timeout));

        resume.send(()).unwrap();
        assert_eq!(swaps.recv_timeout(timeout).unwrap(), Swap::Returned(through + 1));
        let (shown, adopted) = producer.join().unwrap();
        assert_eq!(shown, [1, 2, 3, 4]);
        assert_eq!(adopted, if eligible { 4 } else { 0 });
    }
}
