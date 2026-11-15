//! The HID state machine and thread over a fake device. Report bytes are
//! built here from the layout in `dualsense.rs` (offsets from Linux
//! `hid-playstation`), never captured from a controller.
use super::*;
use crate::dualsense::{BT_OUTPUT_LEN, USB_OUTPUT_LEN};
use std::collections::VecDeque;
use std::sync::Mutex;

/// A USB input report (0x01, 64 bytes) with the mute button up or down.
fn usb_input(mute: bool) -> Vec<u8> {
    let mut r = vec![0u8; 64];
    r[0] = 0x01;
    r[10] = if mute { 0x04 } else { 0 };
    r
}

const ON: PadHidConfig = PadHidConfig { mute: true };
const OFF: PadHidConfig = PadHidConfig { mute: false };

/// The USB output report's common block starts at byte 1.
fn common(r: &[u8]) -> &[u8] {
    assert_eq!((r.len(), r[0]), (USB_OUTPUT_LEN, 0x02));
    &r[1..]
}

#[test]
fn opening_sends_the_speaker_path_and_the_led_state() {
    // Output playing, mute on: one report with both.
    let mut l = PadLink::new(Transport::Usb, ON, false, true);
    let r = l.on_open().unwrap();
    let c = common(&r);
    assert_eq!(c[0], 0x80 | 0x20, "audio control and speaker volume valid");
    assert_eq!(
        c[1],
        0x01 | 0x02 | 0x80,
        "mute LED, power save, audio control 2 valid"
    );
    assert_eq!((c[5], c[7], c[8], c[9], c[37]), (0x64, 0x30, 0, 0, 2));
    // Nothing else touched: rumble, lightbar, player LEDs.
    assert!(
        c[2..5]
            .iter()
            .chain(&c[10..37])
            .chain(&c[38..])
            .all(|&b| b == 0)
    );
    // Not playing and mute off: nothing to send at all.
    assert_eq!(
        PadLink::new(Transport::Usb, OFF, false, false).on_open(),
        None
    );
    // Not playing, mute on and carried over as muted: the LED only.
    let r = PadLink::new(Transport::Usb, ON, true, false)
        .on_open()
        .unwrap();
    let c = common(&r);
    assert_eq!((c[0], c[1], c[8], c[9]), (0, 0x03, 1, 0x10));
}

#[test]
fn the_mute_button_toggles_the_led_on_each_press() {
    let mut l = PadLink::new(Transport::Usb, ON, false, false);
    assert_eq!(l.on_input(&usb_input(false)), None);
    // Press: muted, LED solid and the mic muted in hardware.
    let r = l.on_input(&usb_input(true)).unwrap();
    assert_eq!(
        (common(&r)[1], common(&r)[8], common(&r)[9]),
        (0x03, 1, 0x10)
    );
    assert!(l.muted());
    // Held, then released: nothing.
    assert_eq!(l.on_input(&usb_input(true)), None);
    assert_eq!(l.on_input(&usb_input(false)), None);
    // Second press: unmuted, LED off.
    let r = l.on_input(&usb_input(true)).unwrap();
    assert_eq!((common(&r)[1], common(&r)[8], common(&r)[9]), (0x03, 0, 0));
    assert!(!l.muted());
    // A short or foreign report is ignored.
    assert_eq!(l.on_input(&usb_input(true)[..20]), None);
    assert_eq!(l.on_input(&[0x05; 64]), None);
}

#[test]
fn with_mute_off_the_button_does_nothing() {
    let mut l = PadLink::new(Transport::Usb, OFF, false, false);
    assert_eq!(l.on_input(&usb_input(true)), None);
    assert!(!l.muted());
}

#[test]
fn the_speaker_switch_sends_only_changes() {
    let mut l = PadLink::new(Transport::Usb, ON, false, false);
    let r = l.set_speaker(true).unwrap();
    let c = common(&r);
    assert_eq!((c[0], c[1], c[7]), (0xa0, 0x80, 0x30));
    assert_eq!(l.set_speaker(true), None);
    // Off: the audio control byte valid, path 0 (the jack), LED untouched.
    let r = l.set_speaker(false).unwrap();
    let c = common(&r);
    assert_eq!((c[0], c[1], c[7]), (0x80, 0, 0));
    assert_eq!(l.set_speaker(false), None);
}

#[test]
fn bluetooth_reports_count_their_sequence() {
    let mut l = PadLink::new(Transport::Bluetooth, ON, false, true);
    let a = l.on_open().unwrap();
    let b = l.set_speaker(false).unwrap();
    assert_eq!(
        (a.len(), a[0], a[1], a[2]),
        (BT_OUTPUT_LEN, 0x31, 0x00, 0x10)
    );
    assert_eq!(b[1], 0x10, "sequence 1 in the high nibble");
}

#[test]
fn h2_pad_mute_defaults_on() {
    assert_eq!(PadHidConfig::from_env(|_| None), Ok(ON));
    assert_eq!(PadHidConfig::from_env(|_| Some("0".into())), Ok(OFF));
    assert_eq!(PadHidConfig::from_env(|_| Some(" 1 ".into())), Ok(ON));
    assert!(PadHidConfig::from_env(|_| Some("yes".into())).is_err());
}

/// What a fake pad was sent, and what it will report.
#[derive(Default)]
struct Wire {
    written: Vec<Vec<u8>>,
    inputs: VecDeque<Vec<u8>>,
    gone: bool,
    opens: u32,
}

struct FakePad(Arc<Mutex<Wire>>);

impl HidPad for FakePad {
    fn transport(&self) -> Transport {
        Transport::Usb
    }
    fn read(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let mut w = self.0.lock().unwrap();
        if w.gone {
            return Err(io::ErrorKind::NotConnected.into());
        }
        if let Some(r) = w.inputs.pop_front() {
            return Ok(Some(r));
        }
        drop(w);
        std::thread::sleep(timeout);
        Ok(None)
    }
    fn write(&mut self, report: &[u8]) -> io::Result<()> {
        let mut w = self.0.lock().unwrap();
        if w.gone {
            return Err(io::ErrorKind::NotConnected.into());
        }
        w.written.push(report.to_vec());
        Ok(())
    }
}

/// An opener for the fake pad, there while `present` is set.
fn opener(wire: &Arc<Mutex<Wire>>, present: Arc<AtomicBool>) -> HidOpener {
    let wire = Arc::clone(wire);
    Box::new(move || {
        if !present.load(Ordering::Acquire) {
            return None;
        }
        let mut w = wire.lock().unwrap();
        w.gone = false;
        w.opens += 1;
        Some(Box::new(FakePad(Arc::clone(&wire))) as Box<dyn HidPad>)
    })
}

const FAST: Timing = Timing {
    retry: Duration::from_millis(5),
    poll: Duration::from_millis(2),
};

fn wait(mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    f()
}

/// The audio-path byte (`audio_control`) and the mute LED byte, with their
/// valid flags, of each report written.
fn summary(w: &Wire) -> Vec<(Option<u8>, Option<u8>)> {
    w.written
        .iter()
        .map(|r| {
            let c = common(r);
            (
                (c[0] & 0x80 != 0).then_some(c[7]),
                (c[1] & 0x01 != 0).then_some(c[8]),
            )
        })
        .collect()
}

#[test]
fn the_thread_follows_the_speaker_the_button_and_a_replug() {
    let wire = Arc::new(Mutex::new(Wire::default()));
    let present = Arc::new(AtomicBool::new(false));
    let hid = spawn(opener(&wire, Arc::clone(&present)), ON, FAST);
    let switch = hid.speaker_switch();
    // The controller output opens before the pad is found: remembered.
    switch.set(true);
    std::thread::sleep(Duration::from_millis(20));
    assert!(!hid.status().connected);
    present.store(true, Ordering::Release);
    assert!(wait(|| wire.lock().unwrap().written.len() == 1));
    assert!(hid.status().connected);
    // Opened: speaker path 3 and the LED off, in one report.
    assert_eq!(summary(&wire.lock().unwrap()), [(Some(0x30), Some(0))]);
    // Press and release the button: muted, LED on.
    wire.lock()
        .unwrap()
        .inputs
        .extend([usb_input(true), usb_input(false)]);
    assert!(wait(|| hid.status().muted));
    assert_eq!(summary(&wire.lock().unwrap())[1], (None, Some(1)));
    // Unplugged, then back: the mute state and the speaker carry over.
    present.store(false, Ordering::Release);
    wire.lock().unwrap().gone = true;
    assert!(wait(|| !hid.status().connected));
    present.store(true, Ordering::Release);
    assert!(wait(|| {
        let w = wire.lock().unwrap();
        w.opens == 2 && w.written.len() == 3
    }));
    assert_eq!(summary(&wire.lock().unwrap())[2], (Some(0x30), Some(1)));
    // The output stops: back to the jack.
    switch.set(false);
    assert!(wait(|| wire.lock().unwrap().written.len() == 4));
    assert_eq!(summary(&wire.lock().unwrap())[3], (Some(0), None));
    drop(hid);
    assert_eq!(
        wire.lock().unwrap().written.len(),
        4,
        "already on the jack: nothing more"
    );
}

#[test]
fn dropping_the_thread_puts_the_speaker_back() {
    let wire = Arc::new(Mutex::new(Wire::default()));
    let hid = spawn(opener(&wire, Arc::new(AtomicBool::new(true))), OFF, FAST);
    hid.speaker_switch().set(true);
    assert!(wait(
        || summary(&wire.lock().unwrap()).last() == Some(&(Some(0x30), None))
    ));
    drop(hid);
    assert_eq!(
        summary(&wire.lock().unwrap()).last(),
        Some(&(Some(0), None))
    );
    assert_eq!(wire.lock().unwrap().opens, 1);
}
