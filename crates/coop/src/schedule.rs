//! The two-port pad schedule.
//!
//! Frame *f* of a co-op game has one pad per port, the same on both
//! machines. Each machine owns one port's stream: when it starts frame *f*
//! it samples its own pad and appends it as frame *f + delay*, so the sample
//! has `delay` frames to reach the other machine. The first `delay` frames
//! of a stream are neutral. The other port's stream arrives over the link,
//! in order.
//!
//! A stream is only a sequence of pads; which wall-clock moment a pad was
//! sampled at is not part of it. That is why the delay can differ between
//! the two machines and can change during a game ([`Schedule::set_delay`])
//! without the machines disagreeing: each one just reads the two sequences.

use std::collections::VecDeque;

/// Bytes of an `XINPUT_GAMEPAD`: what one port holds for one frame.
pub const PAD: usize = 18;
/// One port's pad for one frame. All zero is neutral.
pub type Pad = [u8; PAD];
/// Ports in a co-op game.
pub const PORTS: usize = 2;
/// The longest input delay, in frames.
pub const MAX_DELAY: u32 = 8;

/// One port's frames from `first` on, contiguous.
#[derive(Clone, Debug, Default)]
struct Stream {
    first: u32,
    pads: VecDeque<Pad>,
}

impl Stream {
    fn end(&self) -> u32 {
        self.first + self.pads.len() as u32
    }

    fn get(&self, frame: u32) -> Option<&Pad> {
        self.pads.get(frame.checked_sub(self.first)? as usize)
    }

    fn drop_before(&mut self, frame: u32) {
        while self.first < frame && self.pads.pop_front().is_some() {
            self.first += 1;
        }
    }
}

/// What the guest reads for one port in the running frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Shown {
    pad: Pad,
    /// `dwPacketNumber`: goes up when the port's pad changes, as a real
    /// pad's does. Derived from the agreed stream, never from a device.
    packet: u32,
}

#[derive(Clone, Debug)]
pub struct Schedule {
    local_port: usize,
    delay: u32,
    streams: [Stream; PORTS],
    /// The frame the guest is in, once one has started.
    current: Option<u32>,
    shown: [Shown; PORTS],
    /// The local stream's frames the other machine has acknowledged.
    acked: u32,
}

impl Schedule {
    /// A schedule for the machine whose own pad is `local_port` (0 for the
    /// session's host, 1 for the guest), with `delay` frames of input delay
    /// (clamped to 1..=[`MAX_DELAY`]).
    pub fn new(local_port: usize, delay: u32) -> Schedule {
        assert!(local_port < PORTS, "co-op has two ports");
        let delay = delay.clamp(1, MAX_DELAY);
        let mut streams: [Stream; PORTS] = Default::default();
        streams[local_port]
            .pads
            .extend(std::iter::repeat_n([0; PAD], delay as usize));
        Schedule {
            local_port,
            delay,
            streams,
            current: None,
            shown: Default::default(),
            acked: 0,
        }
    }

    pub fn local_port(&self) -> usize {
        self.local_port
    }

    pub fn remote_port(&self) -> usize {
        1 - self.local_port
    }

    pub fn delay(&self) -> u32 {
        self.delay
    }

    /// The next frame to start.
    pub fn next_frame(&self) -> u32 {
        self.current.map_or(0, |f| f + 1)
    }

    /// Whether both ports' pads for the next frame are in hand.
    pub fn ready(&self) -> bool {
        let f = self.next_frame();
        self.streams.iter().all(|s| s.get(f).is_some())
    }

    /// Starts the next frame and returns its number: `local` is this
    /// machine's pad now, appended to its own stream `delay` frames ahead.
    /// `None`, and nothing changes, when the frame is not [`ready`](Self::ready).
    pub fn begin(&mut self, local: Pad) -> Option<u32> {
        if !self.ready() {
            return None;
        }
        let f = self.next_frame();
        for (port, shown) in self.shown.iter_mut().enumerate() {
            let pad = *self.streams[port].get(f).expect("ready");
            if pad != shown.pad {
                shown.packet = shown.packet.wrapping_add(1);
                shown.pad = pad;
            }
        }
        self.current = Some(f);
        // The remote stream is only read forwards. The local one is kept
        // until the other machine has it.
        let remote = self.remote_port();
        self.streams[remote].drop_before(f);
        // Keep the local stream exactly `delay` ahead of the frame started.
        let mine = &mut self.streams[self.local_port];
        let want_end = f + 1 + self.delay;
        // More than one when the delay grew: the frames in between repeat
        // this sample.
        while mine.end() < want_end {
            mine.pads.push_back(local);
        }
        // A delay that shrank leaves the stream ahead: this sample is
        // dropped, and sampling resumes when the frame catches up.
        self.trim();
        Some(f)
    }

    /// Changes this machine's input delay from the next frame started. The
    /// stream stays contiguous, so the other machine needs no notice.
    pub fn set_delay(&mut self, delay: u32) {
        self.delay = delay.clamp(1, MAX_DELAY);
    }

    /// The `XINPUT_STATE` (packet number, then pad) of `port` in the running
    /// frame. `None` before the first frame, and for a port that is not in
    /// the game.
    pub fn state(&self, port: usize) -> Option<[u8; PAD + 4]> {
        self.current?;
        let shown = self.shown.get(port)?;
        let mut out = [0; PAD + 4];
        out[..4].copy_from_slice(&shown.packet.to_le_bytes());
        out[4..].copy_from_slice(&shown.pad);
        Some(out)
    }

    /// Both ports' pads in the running frame.
    pub fn pads(&self) -> Option<[Pad; PORTS]> {
        self.current?;
        Some([self.shown[0].pad, self.shown[1].pad])
    }

    /// The next remote frame this machine needs: what it acknowledges.
    pub fn remote_end(&self) -> u32 {
        self.streams[self.remote_port()].end()
    }

    /// Frames of the other port's stream from `first` on, as received.
    /// Frames already held are skipped, a gap is ignored (the sender repeats
    /// from the acknowledgement, so the missing frames come again). Returns
    /// how many frames were new.
    pub fn receive(&mut self, first: u32, pads: &[Pad]) -> usize {
        let remote = self.remote_port();
        let stream = &mut self.streams[remote];
        let end = stream.end();
        if first > end {
            return 0;
        }
        let skip = (end - first) as usize;
        let new = pads.get(skip..).unwrap_or(&[]);
        stream.pads.extend(new.iter().copied());
        new.len()
    }

    /// The other machine has every local frame before `frame`.
    pub fn acknowledge(&mut self, frame: u32) {
        // Never past what exists, and never backwards (a stale datagram).
        let frame = frame.min(self.streams[self.local_port].end());
        self.acked = self.acked.max(frame);
        self.trim();
    }

    /// Drops local frames that the other machine has and the guest here has
    /// already read.
    fn trim(&mut self) {
        let read = self.current.map_or(0, |f| f + 1);
        self.streams[self.local_port].drop_before(self.acked.min(read));
    }

    /// The local frames the other machine has not acknowledged: the first
    /// frame's number and the pads, at most `max`.
    pub fn unacknowledged(&self, max: usize) -> (u32, Vec<Pad>) {
        let mine = &self.streams[self.local_port];
        let from = self.acked.max(mine.first);
        let skip = (from - mine.first) as usize;
        (
            from,
            mine.pads.iter().skip(skip).take(max).copied().collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(n: u8) -> Pad {
        [n; PAD]
    }

    #[test]
    fn a_sample_lands_delay_frames_later() {
        let mut a = Schedule::new(0, 2);
        let mut b = Schedule::new(1, 2);
        let mut seen = Vec::new();
        for n in 1..=6u8 {
            // Hand each side everything the other has.
            let (first, pads) = a.unacknowledged(usize::MAX);
            b.receive(first, &pads);
            let (first, pads) = b.unacknowledged(usize::MAX);
            a.receive(first, &pads);
            let fa = a.begin(pad(n)).unwrap();
            let fb = b.begin(pad(n + 100)).unwrap();
            assert_eq!(fa, fb);
            assert_eq!(a.pads(), b.pads());
            seen.push(a.pads().unwrap());
        }
        // Two neutral frames, then the samples in order, on both ports.
        let port0: Vec<u8> = seen.iter().map(|p| p[0][0]).collect();
        let port1: Vec<u8> = seen.iter().map(|p| p[1][0]).collect();
        assert_eq!(port0, [0, 0, 1, 2, 3, 4]);
        assert_eq!(port1, [0, 0, 101, 102, 103, 104]);
    }

    #[test]
    fn a_frame_waits_for_the_other_port() {
        let mut a = Schedule::new(0, 2);
        assert!(!a.ready());
        assert_eq!(a.begin(pad(1)), None);
        assert_eq!(a.state(0), None, "nothing to read before a frame");
        a.receive(0, &[pad(9)]);
        assert_eq!(a.begin(pad(1)), Some(0));
        assert_eq!(a.begin(pad(2)), None, "frame 1 of port 1 is missing");
        assert_eq!(a.next_frame(), 1);
    }

    #[test]
    fn receive_is_in_order_whatever_arrives() {
        let mut a = Schedule::new(0, 1);
        assert_eq!(a.receive(2, &[pad(3)]), 0, "a gap is ignored");
        assert_eq!(a.receive(0, &[pad(1), pad(2)]), 2);
        assert_eq!(
            a.receive(0, &[pad(7), pad(7), pad(3)]),
            1,
            "old frames skipped"
        );
        assert_eq!(a.receive(1, &[pad(7)]), 0);
        assert_eq!(a.remote_end(), 3);
        let mut got = Vec::new();
        while a.begin(pad(0)).is_some() {
            got.push(a.pads().unwrap()[1][0]);
        }
        assert_eq!(got, [1, 2, 3]);
    }

    #[test]
    fn packet_numbers_follow_the_stream() {
        let mut a = Schedule::new(0, 1);
        a.receive(0, &[pad(0), pad(5), pad(5), pad(6)]);
        let mut numbers = Vec::new();
        while a.begin(pad(0)).is_some() {
            let s = a.state(1).unwrap();
            numbers.push(u32::from_le_bytes(s[..4].try_into().unwrap()));
            assert_eq!(s[4..], a.pads().unwrap()[1]);
        }
        assert_eq!(numbers, [0, 1, 1, 2]);
        assert_eq!(a.state(2), None, "ports 2 and 3 are not in the game");
    }

    #[test]
    fn unacknowledged_frames_are_kept_and_repeated() {
        let mut a = Schedule::new(0, 2);
        a.receive(0, &[pad(0); 4]);
        for n in 1..=3 {
            a.begin(pad(n)).unwrap();
        }
        let (first, pads) = a.unacknowledged(usize::MAX);
        assert_eq!((first, pads.len()), (0, 5));
        a.acknowledge(3);
        let (first, pads) = a.unacknowledged(usize::MAX);
        assert_eq!(first, 3);
        assert_eq!(pads, [pad(2), pad(3)]);
        a.acknowledge(1); // stale
        assert_eq!(a.unacknowledged(1), (3, vec![pad(2)]));
        a.acknowledge(99); // nonsense
        assert_eq!(a.unacknowledged(usize::MAX), (5, vec![]));
    }

    #[test]
    fn the_delay_can_change_without_a_gap() {
        let mut a = Schedule::new(0, 1);
        a.receive(0, &[pad(0); 12]);
        let mut seen = Vec::new();
        for n in 1..=10u8 {
            if n == 3 {
                a.set_delay(3);
            }
            if n == 7 {
                a.set_delay(1);
            }
            a.begin(pad(n)).unwrap();
            seen.push(a.pads().unwrap()[0][0]);
        }
        // Delay 1: sample n shows at frame n. Growing to 3 at sample 3
        // repeats it over the new gap; shrinking back drops samples until
        // the stream is one ahead again.
        assert_eq!(seen, [0, 1, 2, 3, 3, 3, 4, 5, 6, 9]);
        let (first, pads) = a.unacknowledged(usize::MAX);
        assert_eq!(first + pads.len() as u32, a.next_frame() + 1);
    }
}
