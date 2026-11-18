//! The communicator's media objects against a fake microphone and output,
//! driven the way the engine drives them (docs/live/live-voice.md, 3.3):
//! two 640-byte packets in flight per object, resubmitted from the
//! completion, one tick per 20 ms.

use voice::{
    HOST_FRAME,
    io::{FakeVoiceIo, Frame},
    xmo::{Completion, Delivery, Fault, Format, Handle, Kind, MediaObjects, Memory, hresult},
};

struct Ram(Vec<u8>);

impl Ram {
    fn new() -> Self {
        Self(vec![0xcc; 0x10000])
    }
    fn u32(&self, a: u32) -> u32 {
        let mut b = [0; 4];
        self.read(a, &mut b).unwrap();
        u32::from_le_bytes(b)
    }
    fn samples(&self, a: u32, n: usize) -> Vec<i16> {
        let mut b = vec![0; n * 2];
        self.read(a, &mut b).unwrap();
        b.as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect()
    }
    fn put_samples(&mut self, a: u32, s: &[i16]) {
        let b: Vec<u8> = s.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.write(a, &b).unwrap();
    }
    /// An XMEDIAPACKET at `at`, as 0x334c43 builds it.
    fn packet(&mut self, at: u32, buffer: u32, size: u32, cells: u32, context: u32) {
        let words = [
            buffer,
            size,
            cells,
            if cells == 0 { 0 } else { cells + 4 },
            context,
            0,
        ];
        let b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.write(at, &b).unwrap();
    }
}

impl Memory for Ram {
    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<(), Fault> {
        let a = address as usize;
        let src = self.0.get(a..a + bytes.len()).ok_or(Fault {
            address,
            length: bytes.len(),
            write: false,
        })?;
        bytes.copy_from_slice(src);
        Ok(())
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Fault> {
        let a = address as usize;
        let len = bytes.len();
        let dst = self.0.get_mut(a..a + len).ok_or(Fault {
            address,
            length: len,
            write: true,
        })?;
        dst.copy_from_slice(bytes);
        Ok(())
    }
}

const CALLBACK_MIC: u32 = 0x0033_51a9;
const CALLBACK_PHONE: u32 = 0x0033_51c0;
const STACK: u32 = 0x100;

fn open(objects: &mut MediaObjects, kind: Kind, port: u32) -> Handle {
    let callback = match kind {
        Kind::Microphone => CALLBACK_MIC,
        Kind::Headphone => CALLBACK_PHONE,
    };
    objects
        .create(kind, port, 2, Format::ENGINE, callback, 0x7000 + port)
        .unwrap()
}

/// Process a 640-byte packet built on the "stack", then clobber the stack
/// as the engine's next call would.
fn submit(objects: &mut MediaObjects, ram: &mut Ram, h: Handle, buffer: u32, index: u32) -> u32 {
    ram.packet(STACK, buffer, 0x280, 0, index);
    let (src, dst) = match objects.kind(h).unwrap() {
        Kind::Microphone => (0, STACK),
        Kind::Headphone => (STACK, 0),
    };
    let r = objects.process(ram, h, src, dst).unwrap();
    ram.write(STACK, &[0xee; 24]).unwrap();
    r
}

fn constant(v: i16) -> Frame {
    [v; HOST_FRAME]
}

#[test]
fn create_checks_and_errors() {
    let mut o = MediaObjects::new();
    let fmt = Format::ENGINE;
    assert_eq!(
        o.create(Kind::Microphone, 0, 2, fmt, 1, 1),
        Err(hresult::DEVICE_NOT_CONNECTED)
    );
    o.set_present(0b0001);
    assert_eq!(
        o.create(Kind::Microphone, 4, 2, fmt, 1, 1),
        Err(hresult::E_INVALIDARG)
    );
    assert_eq!(
        o.create(Kind::Microphone, 0, 0, fmt, 1, 1),
        Err(hresult::E_INVALIDARG)
    );
    let cd = Format {
        rate: 44_100,
        average_bytes: 88_200,
        ..fmt
    };
    assert_eq!(
        o.create(Kind::Microphone, 0, 2, cd, 1, 1),
        Err(hresult::E_INVALIDARG),
        "outside mask 0x1f"
    );
    let odd = Format {
        rate: 11_025,
        average_bytes: 22_050,
        ..fmt
    };
    assert_eq!(
        o.create(Kind::Microphone, 0, 2, odd, 1, 1),
        Err(hresult::E_INVALIDARG),
        "allowed by the original, not implemented by the host"
    );
    assert_eq!(
        o.create(Kind::Microphone, 1, 2, fmt, 1, 1),
        Err(hresult::DEVICE_NOT_CONNECTED)
    );
    let mic = o.create(Kind::Microphone, 0, 2, fmt, 1, 1).unwrap();
    assert_eq!(
        o.create(Kind::Microphone, 0, 2, fmt, 1, 1),
        Err(hresult::SHARING_VIOLATION)
    );
    let phone = o.create(Kind::Headphone, 0, 2, fmt, 1, 1).unwrap();
    assert_ne!(mic, phone);
    for rate in [8_000, 24_000] {
        let f = Format {
            rate,
            average_bytes: rate * 2,
            ..fmt
        };
        assert!(f.supported());
    }
}

#[test]
fn format_parses_the_engine_waveformatex() {
    // 0x484290 as 0x332f67 fills it.
    let bytes = [1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0];
    assert_eq!(Format::parse(&bytes), Format::ENGINE);
    assert_eq!(Format::ENGINE.frame_bytes(), 0x280);
}

#[test]
fn process_copies_the_packet_and_marks_it_pending() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    o.set_present(1);
    let mic = open(&mut o, Kind::Microphone, 0);
    ram.packet(STACK, 0x1000, 0x280, 0x200, 0);
    assert_eq!(
        o.process(&mut ram, mic, STACK, 0).unwrap(),
        hresult::E_INVALIDARG
    );
    assert_eq!(o.process(&mut ram, mic, 0, STACK).unwrap(), hresult::S_OK);
    assert_eq!((ram.u32(0x200), ram.u32(0x204)), (0, hresult::E_PENDING));
    assert_eq!(submit(&mut o, &mut ram, mic, 0x1280, 1), hresult::S_OK);
    assert_eq!(
        submit(&mut o, &mut ram, mic, 0x1500, 2),
        hresult::E_FAIL,
        "more than max_packets"
    );
    assert_eq!(o.queued(mic), 2);
    assert_eq!(
        o.process(&mut ram, Handle(99), 0, STACK).unwrap(),
        hresult::E_INVALIDARG
    );
}

#[test]
fn microphone_fills_only_the_talkers_port() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(0b0011);
    let mic0 = open(&mut o, Kind::Microphone, 0);
    let mic1 = open(&mut o, Kind::Microphone, 1);
    submit(&mut o, &mut ram, mic0, 0x1000, 0);
    submit(&mut o, &mut ram, mic1, 0x2000, 0);
    o.set_talker(Some(0));
    io.speak(constant(1000));

    let done = o.tick(&mut ram, &mut io).unwrap();
    assert_eq!(done.len(), 2);
    assert_eq!(io.pending(), 0, "one host frame a tick, shared");
    assert!(ram.samples(0x1000, 320).iter().all(|&s| s == 1000));
    assert!(ram.samples(0x2000, 320).iter().all(|&s| s == 0));
    assert_eq!(
        done[0].delivery,
        Delivery::Callback {
            function: CALLBACK_MIC,
            context: 0x7000,
            packet_context: 0,
            status: 0
        }
    );
    assert_eq!(done[0].size, 0x280);
}

#[test]
fn no_talker_means_silence() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(1);
    let mic = open(&mut o, Kind::Microphone, 0);
    submit(&mut o, &mut ram, mic, 0x1000, 0);
    io.speak(constant(1000));
    o.tick(&mut ram, &mut io).unwrap();
    assert!(ram.samples(0x1000, 320).iter().all(|&s| s == 0));
}

#[test]
fn completion_writes_size_and_status_then_reports() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(1);
    let mic = o
        .create(Kind::Microphone, 0, 2, Format::ENGINE, 0, 0)
        .unwrap();
    ram.packet(STACK, 0x1000, 0x280, 0x200, 0x5555);
    o.process(&mut ram, mic, 0, STACK).unwrap();
    let done: Vec<Completion> = o.tick(&mut ram, &mut io).unwrap();
    done[0].write(&mut ram).unwrap();
    assert_eq!((ram.u32(0x200), ram.u32(0x204)), (0x280, 0));
    assert_eq!(done[0].delivery, Delivery::Event(0x5555), "no callback");
}

#[test]
fn a_two_frame_packet_takes_two_ticks() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(1);
    o.set_talker(Some(0));
    let mic = open(&mut o, Kind::Microphone, 0);
    ram.packet(STACK, 0x1000, 0x500, 0, 0);
    o.process(&mut ram, mic, 0, STACK).unwrap();
    io.speak(constant(10));
    io.speak(constant(20));
    assert!(o.tick(&mut ram, &mut io).unwrap().is_empty());
    let done = o.tick(&mut ram, &mut io).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].size, 0x500);
    let s = ram.samples(0x1000, 640);
    assert!(s[..320].iter().all(|&x| x == 10) && s[320..].iter().all(|&x| x == 20));
}

#[test]
fn headphones_mix_into_one_output() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(0b0011);
    let a = open(&mut o, Kind::Headphone, 0);
    let b = open(&mut o, Kind::Headphone, 1);
    for (h, buf, v) in [(a, 0x1000, 300i16), (b, 0x2000, 200)] {
        for i in 0..2 {
            ram.put_samples(buf + i * 0x280, &[v; 320]);
            submit(&mut o, &mut ram, h, buf + i * 0x280, i);
        }
    }
    let first = o.tick(&mut ram, &mut io).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first[0].delivery,
        Delivery::Callback {
            function: CALLBACK_PHONE,
            context: 0x7000,
            packet_context: 0,
            status: 0
        }
    );
    o.tick(&mut ram, &mut io).unwrap();
    let played = io.take_played();
    assert_eq!(played.len(), 2, "one output frame a tick");
    // The first frame ramps up from silence over one 16 kHz sample.
    assert_eq!(&played[0][..3], &[166, 333, 500]);
    assert!(played[0][3..].iter().all(|&s| s == 500));
    assert!(
        played[1].iter().all(|&s| s == 500),
        "no click at the frame edge"
    );
}

#[test]
fn the_volumes_scale_the_microphone_and_the_mix() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(0b0011);
    let mic = open(&mut o, Kind::Microphone, 0);
    let phone = open(&mut o, Kind::Headphone, 1);
    o.set_talker(Some(0));
    // Input at 50%, output at 25%.
    o.set_gains(0.5, 0.25);
    for i in 0..2 {
        submit(&mut o, &mut ram, mic, 0x1000 + i * 0x280, i);
        ram.put_samples(0x2000 + i * 0x280, &[400; 320]);
        submit(&mut o, &mut ram, phone, 0x2000 + i * 0x280, i);
        io.speak(constant(1000));
    }
    o.tick(&mut ram, &mut io).unwrap();
    o.tick(&mut ram, &mut io).unwrap();
    assert!(ram.samples(0x1000, 320).iter().all(|&s| s == 500));
    let played = io.take_played();
    assert!(played[1].iter().all(|&s| s == 100), "{:?}", &played[1][..3]);

    // Muted (gain 0) is silence each way; full is unchanged.
    for (gains, mic_want, out_want) in [((0.0, 0.0), 0, 0), ((1.0, 1.0), 1000, 400)] {
        o.set_gains(gains.0, gains.1);
        for i in 0..2 {
            submit(&mut o, &mut ram, mic, 0x1000 + i * 0x280, i);
            submit(&mut o, &mut ram, phone, 0x2000 + i * 0x280, i);
            io.speak(constant(1000));
        }
        o.tick(&mut ram, &mut io).unwrap();
        o.tick(&mut ram, &mut io).unwrap();
        assert!(
            ram.samples(0x1280, 320).iter().all(|&s| s == mic_want),
            "{gains:?}"
        );
        assert!(
            io.take_played()[1].iter().all(|&s| s == out_want),
            "{gains:?}"
        );
    }
}

#[test]
fn the_mix_saturates() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(0b0011);
    for port in 0..2 {
        let h = open(&mut o, Kind::Headphone, port);
        let buf = 0x1000 + port * 0x1000;
        ram.put_samples(buf, &[30_000; 320]);
        submit(&mut o, &mut ram, h, buf, 0);
    }
    o.tick(&mut ram, &mut io).unwrap();
    assert_eq!(io.take_played()[0][HOST_FRAME - 1], i16::MAX);
}

#[test]
fn speech_survives_microphone_to_headphone() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(1);
    o.set_talker(Some(0));
    let mic = open(&mut o, Kind::Microphone, 0);
    let phone = open(&mut o, Kind::Headphone, 0);
    // 500 Hz at 48 kHz, five frames.
    let tone =
        |n: usize| ((n as f64 * 500.0 * std::f64::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
    let mut said = Vec::new();
    for f in 0..5 {
        let frame: Frame = std::array::from_fn(|i| tone(f * HOST_FRAME + i));
        said.extend_from_slice(&frame);
        io.speak(frame);
    }
    // The engine's loop: the mic packet comes back, its PCM goes to the
    // headphone (here directly; on the Xbox, through the codec).
    for _ in 0..5 {
        submit(&mut o, &mut ram, mic, 0x1000, 0);
        o.tick(&mut ram, &mut io).unwrap();
        let pcm = ram.samples(0x1000, 320);
        ram.put_samples(0x3000, &pcm);
        submit(&mut o, &mut ram, phone, 0x3000, 0);
    }
    o.tick(&mut ram, &mut io).unwrap();
    let heard: Vec<i16> = io.take_played().concat();
    // Output frame k carries input frame k - 1 (one tick of latency); skip
    // the silent first frame and compare the rest, allowing the small
    // phase lag of averaging then interpolating.
    let heard = &heard[HOST_FRAME..];
    let lag = 1;
    let mut err = 0f64;
    let mut energy = 0f64;
    for i in 0..said.len() - HOST_FRAME {
        let (h, s) = (f64::from(heard[i + lag]), f64::from(said[i]));
        err += (h - s).powi(2);
        energy += s * s;
    }
    assert!(err / energy < 0.01, "relative error {}", err / energy);
}

#[test]
fn pulling_the_communicator_fails_queued_packets() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(1);
    let mic = open(&mut o, Kind::Microphone, 0);
    submit(&mut o, &mut ram, mic, 0x1000, 0);
    submit(&mut o, &mut ram, mic, 0x1280, 1);
    let done = o.set_present(0);
    assert_eq!(done.len(), 2);
    assert!(
        done.iter()
            .all(|c| c.status == hresult::DEVICE_NOT_CONNECTED)
    );
    assert_eq!(
        submit(&mut o, &mut ram, mic, 0x1000, 0),
        hresult::DEVICE_NOT_CONNECTED
    );
    assert!(o.tick(&mut ram, &mut io).unwrap().is_empty());
}

#[test]
fn release_flushes_and_frees_the_port() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    o.set_present(1);
    let mic = open(&mut o, Kind::Microphone, 0);
    submit(&mut o, &mut ram, mic, 0x1000, 0);
    let done = o.release(mic);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].status, hresult::E_ABORT);
    assert_eq!(o.kind(mic), None);
    assert!(o.release(mic).is_empty());
    open(&mut o, Kind::Microphone, 0);
}

#[test]
fn steady_cadence_with_two_packets_in_flight() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    o.set_present(1);
    let mic = open(&mut o, Kind::Microphone, 0);
    let phone = open(&mut o, Kind::Headphone, 0);
    for i in 0..2 {
        submit(&mut o, &mut ram, mic, 0x1000 + i * 0x280, i);
        submit(&mut o, &mut ram, phone, 0x2000 + i * 0x280, i);
    }
    for tick in 0..50u32 {
        let done = o.tick(&mut ram, &mut io).unwrap();
        assert_eq!(done.len(), 2, "tick {tick}");
        for c in done {
            assert_eq!(c.packet.context, tick % 2);
            let base = if c.object == mic { 0x1000 } else { 0x2000 };
            submit(
                &mut o,
                &mut ram,
                c.object,
                base + c.packet.context * 0x280,
                c.packet.context,
            );
        }
    }
    assert_eq!(io.take_played().len(), 50);
}

#[test]
fn nothing_open_touches_no_device() {
    let mut o = MediaObjects::new();
    let mut ram = Ram::new();
    let mut io = FakeVoiceIo::new();
    io.speak(constant(1));
    o.set_present(0xf);
    assert!(o.tick(&mut ram, &mut io).unwrap().is_empty());
    assert_eq!(io.pending(), 1);
    assert!(io.take_played().is_empty());
}
