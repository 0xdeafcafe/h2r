//! A map change while a sealed-scene observer is still returning.
#![cfg(feature = "native-render")]

use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use guest_layout::memory::FlatMemory;
use frame_scene::{Cut, FrameScene, SceneId};
use render_bridge::{native_map_work, scene_step};

static PIN: Mutex<Option<SceneId>> = Mutex::new(None);
static PARK: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = Mutex::new(None);

fn observe(_: u64, scene: Arc<FrameScene>) {
    *PIN.lock().unwrap() = Some(scene.id);
    let gate = PARK.lock().unwrap().take();
    if let Some((arrived, resume)) = gate {
        arrived.send(()).unwrap();
        resume.recv_timeout(Duration::from_secs(10)).unwrap();
    }
}

fn forget_pin() {
    PIN.lock().unwrap().take();
}

#[test]
fn activation_retires_the_open_scene_last_seal_and_pin_before_another_frame() {
    let guest = FlatMemory::new(0, 64);
    scene_step::set_seal_observer(Some(scene_step::SealObserver { sealed: observe, reset: forget_pin }));
    let (arrived, started) = mpsc::channel();
    let (resume, gate) = mpsc::channel();
    *PARK.lock().unwrap() = Some((arrived, gate));

    scene_step::begin(&guest, None);
    let (_, retired) = scene_step::open_identity().unwrap();
    let late = thread::spawn(scene_step::seal);
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(*PIN.lock().unwrap(), Some(retired));

    // A second old-map render is still open when activation occurs.
    scene_step::begin(&guest, None);
    native_map_work::reset();
    assert!(scene_step::open_identity().is_none());
    assert!(scene_step::last().is_none());
    assert!(PIN.lock().unwrap().is_none());

    // Neither the late observer nor a seal of the abandoned open interval
    // can publish into the new timeline.
    resume.send(()).unwrap();
    late.join().unwrap();
    scene_step::seal();
    assert!(scene_step::last().is_none());
    assert!(PIN.lock().unwrap().is_none());

    scene_step::begin(&guest, None);
    scene_step::seal();
    let current = scene_step::last().unwrap();
    assert_ne!(retired.map_epoch, current.id.map_epoch);
    assert!(current.cut.contains(Cut::MAP));
    assert_eq!(current.stamp.ticks_run, 0);
    assert_eq!(*PIN.lock().unwrap(), Some(current.id));
    scene_step::set_seal_observer(None);
}
