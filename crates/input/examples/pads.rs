//! Print live pad state for every host pad, as the game would see it.
//!
//! `cargo run -p input --example pads` (add `-- --rumble` to buzz port 0 once
//! it has a pad). The keyboard backend is not attached here because a plain
//! terminal has no key events; unit tests cover it.

use std::time::Duration;

use input::device::PORT_COUNT;
use input::gamepad::{AnalogButton, Rumble};
use input::device::FeedbackRequest;
use input::host::ConvertConfig;
use input::{InputManager, PortIndex};

fn main() {
    let rumble = std::env::args().any(|a| a == "--rumble");
    let mut mgr = InputManager::new(ConvertConfig::default()).expect("valid config");

    // On macOS the GameController backend sees the same pads as gilrs (and adds
    // pressure buttons and rumble), so use only one of them.
    #[cfg(all(feature = "game-controller", target_os = "macos"))]
    mgr.add_backend(Box::new(input::backend::game_controller::GameControllerPads::new()));
    #[cfg(all(feature = "gilrs", not(all(feature = "game-controller", target_os = "macos"))))]
    match input::backend::gilrs_pads::GilrsPads::new() {
        Ok(b) => mgr.add_backend(Box::new(b)),
        Err(e) => eprintln!("gilrs unavailable: {e}"),
    }

    let mut last = String::new();
    let mut buzzed = false;
    let mut first = true;
    loop {
        mgr.sync();
        let _ = mgr.take_completions();
        let (ins, rem) = mgr.device_changes();
        if ins | rem != 0 {
            println!("-- device changes: inserted {ins:04b}, removed {rem:04b}");
        }
        let mut out = String::new();
        for p in 0..PORT_COUNT {
            let port = PortIndex::new(p).unwrap();
            let Ok(h) = mgr.open(port) else { continue };
            let s = mgr.get_state(h).unwrap();
            let g = s.gamepad;
            if rumble && !buzzed {
                let _ = mgr.set_state(h, FeedbackRequest { addr: 0, h_event: 0, rumble: Rumble { left_motor: 40000, right_motor: 20000 } });
                buzzed = true;
            }
            out += &format!(
                "port {p} pkt {:>5} btn {:016b} A{:3} B{:3} X{:3} Y{:3} blk{:3} wht{:3} LT{:3} RT{:3} L({:6},{:6}) R({:6},{:6})\n",
                s.packet_number,
                g.buttons.0,
                g.analog(AnalogButton::A),
                g.analog(AnalogButton::B),
                g.analog(AnalogButton::X),
                g.analog(AnalogButton::Y),
                g.analog(AnalogButton::Black),
                g.analog(AnalogButton::White),
                g.analog(AnalogButton::LeftTrigger),
                g.analog(AnalogButton::RightTrigger),
                g.thumb_lx,
                g.thumb_ly,
                g.thumb_rx,
                g.thumb_ry,
            );
        }
        if first || out != last {
            first = false;
            if out.is_empty() {
                println!("(no pads connected)");
            } else {
                print!("{out}");
            }
            last = out;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
