//! The pad schedule as the guest's input: an `xapi::tap::InputTap` that
//! answers every `XInputGetState` from the frame both machines agreed on.
//!
//! It never lets the local device through. The live packet `xapi` hands in
//! is overwritten for ports 0 and 1, and a read with no frame started, or
//! for a port that is not in the game, is answered "not connected" and
//! counted, so a runner that let the guest run without a frame shows up in
//! [`ScheduleTap::starved`] instead of as a quiet desync.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use xapi::tap::{InputTap, PACKET};

use crate::schedule::Schedule;

/// `ERROR_DEVICE_NOT_CONNECTED`.
pub const NOT_CONNECTED: u32 = 1167;

pub struct ScheduleTap {
    schedule: Arc<Mutex<Schedule>>,
    port_of: Box<dyn Fn(u32) -> Option<usize> + Send>,
    starved: Arc<AtomicU64>,
}

impl ScheduleTap {
    pub fn new(
        schedule: Arc<Mutex<Schedule>>,
        port_of: impl Fn(u32) -> Option<usize> + Send + 'static,
    ) -> ScheduleTap {
        ScheduleTap {
            schedule,
            port_of: Box::new(port_of),
            starved: Arc::default(),
        }
    }

    /// Reads of a game port that came with no frame started. Always 0 when
    /// the runner holds the guest as it should.
    pub fn starved(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.starved)
    }
}

impl InputTap for ScheduleTap {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        let port = (self.port_of)(handle);
        let state = port.and_then(|p| {
            let s = self.schedule.lock().unwrap_or_else(|e| e.into_inner());
            let state = s.state(p);
            if state.is_none() && p < crate::schedule::PORTS {
                self.starved.fetch_add(1, Ordering::Relaxed);
            }
            state
        });
        match state {
            Some(state) => {
                *status = 0;
                *packet = state;
            }
            None => {
                *status = NOT_CONNECTED;
                *packet = [0; PACKET];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::PAD;

    const H0: u32 = 0x5800_0000;
    const H1: u32 = 0x5800_0004;
    const H2: u32 = 0x5800_0008;

    fn port_of(handle: u32) -> Option<usize> {
        match handle {
            H0 => Some(0),
            H1 => Some(1),
            H2 => Some(2),
            _ => None,
        }
    }

    fn read(tap: &mut ScheduleTap, handle: u32) -> (u32, [u8; PACKET]) {
        // What the local device would have given: it must never get through.
        let (mut status, mut packet) = (0, [0xaa; PACKET]);
        tap.read(handle, &mut status, &mut packet);
        (status, packet)
    }

    #[test]
    fn reads_come_from_the_frame_and_never_from_the_device() {
        let schedule = Arc::new(Mutex::new(Schedule::new(0, 1)));
        let mut tap = ScheduleTap::new(Arc::clone(&schedule), port_of);
        let starved = tap.starved();

        // No frame yet: not connected, and counted.
        assert_eq!(read(&mut tap, H0), (NOT_CONNECTED, [0; PACKET]));
        assert_eq!(starved.load(Ordering::Relaxed), 1);

        {
            let mut s = schedule.lock().unwrap();
            s.receive(0, &[[7; PAD], [7; PAD]]);
            s.begin([3; PAD]).unwrap();
        }
        let (status, p0) = read(&mut tap, H0);
        assert_eq!(
            (status, &p0[4..]),
            (0, &[0u8; PAD][..]),
            "port 0, frame 0: neutral"
        );
        let (status, p1) = read(&mut tap, H1);
        assert_eq!(status, 0);
        assert_eq!(p1[..4], 1u32.to_le_bytes(), "the pad changed: packet 1");
        assert_eq!(p1[4..], [7; PAD]);
        // Reading again in the frame gives the same bytes.
        assert_eq!(read(&mut tap, H1), (0, p1));

        schedule.lock().unwrap().begin([4; PAD]).unwrap();
        let (_, p0) = read(&mut tap, H0);
        assert_eq!(p0[4..], [3; PAD], "the local sample, one frame later");
        assert_eq!(
            read(&mut tap, H1).1,
            p1,
            "unchanged pad, unchanged packet number"
        );

        // Ports outside the game and unknown handles: absent, not starved.
        assert_eq!(read(&mut tap, H2), (NOT_CONNECTED, [0; PACKET]));
        assert_eq!(read(&mut tap, 0x1234), (NOT_CONNECTED, [0; PACKET]));
        assert_eq!(starved.load(Ordering::Relaxed), 1);
    }
}
