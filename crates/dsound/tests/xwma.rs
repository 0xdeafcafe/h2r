//! The native WMA codec at the game's boundary, through the real Runner:
//! the input callback is a guest call, the container's driver is played by
//! the test the way `0x388533`/`0x3886b0` drive it. Synthetic tables and
//! packets only (no retail data); see the ignored test at the end for a
//! stream from the disc.
use dsound::register::RegisterError;
use dsound::xwma::{self, BEGIN, CREATE, DECODE, DESTROY, GET_PCM, RESET, WmaHle};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, abi, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use wma::boundary::{NO_DATA, OK, PACKET_DONE, UNSUPPORTED};

const STACK_TOP: u32 = 0x0061_0000;
const ESP: u32 = STACK_TOP - 0x100;
const SCRATCH: u32 = 0x0070_0000;
/// The game's input callback stands here (an override, like guest code).
const INPUT: u32 = 0x0012_3460;
const CONTEXT: u32 = 0x00c0_ffee;

/// Bytes the "container" has left to hand out, and what the callback saw.
#[derive(Default)]
struct Input {
    packets: VecDeque<Vec<u8>>,
    offset: usize,
    /// The reader has only this many bytes of the front packet yet.
    stall_at: Option<usize>,
    calls: Vec<u32>,
}

struct World {
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    hle: Arc<WmaHle>,
    input: Arc<Mutex<Input>>,
}

fn world_with_tables(tables: wma::Tables) -> World {
    let mut g = Guest::new();
    g.map_zeroed(0x0038_a000, 0x1000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0012_3000, 0x1000, Permissions::RWX).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x2_0000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, Dispatch::default());
    let hle = WmaHle::new(Arc::new(tables));
    runtime.edit_dispatch(|d| xwma::register(d, &hle)).unwrap();
    let input = Arc::new(Mutex::new(Input::default()));
    let served = Arc::clone(&input);
    // The callback ABI at 0x388777: (data**, size*, context, staging),
    // ret 10h; at most 128 bytes, return 3 on a codec packet's first chunk.
    runtime.edit_dispatch(|d| {
        d.register_override(INPUT, "test::wma_input", move |cpu, ctx, _| {
            let mut m = ctx.memory();
            let (out_data, out_size, context, staging) = (
                m.u32(cpu.esp + 4)?,
                m.u32(cpu.esp + 8)?,
                m.u32(cpu.esp + 12)?,
                m.u32(cpu.esp + 16)?,
            );
            let mut i = served.lock().unwrap();
            i.calls.push(context);
            let mut result = 0;
            let chunk = match i.packets.front() {
                Some(p) => {
                    if i.offset == 0 {
                        result = 3;
                    }
                    let end = (i.offset + 128)
                        .min(p.len())
                        .min(i.stall_at.unwrap_or(usize::MAX));
                    let c = p[i.offset..end].to_vec();
                    i.offset = end;
                    if end == i.packets[0].len() {
                        i.packets.pop_front();
                        i.offset = 0;
                    }
                    c
                }
                None => Vec::new(),
            };
            m.write_bytes(staging, &chunk, 1)?;
            m.set_u32(out_data, staging)?;
            m.set_u32(out_size, chunk.len() as u32)?;
            cpu.eax = result;
            m.ret(cpu, 16)
        })
    });
    World {
        ctx: ExecCtx::new(Arc::clone(&runtime), 1),
        runtime,
        cpu: CpuAbi::default(),
        hle,
        input,
    }
}

fn world() -> World {
    world_with_tables(wma::Tables::synthetic())
}

impl World {
    /// Calls `target` from HARNESS_EXIT; requires a return with the stdcall
    /// cleanup, and returns EAX.
    fn call(&mut self, target: u32, args: &[u32]) -> u32 {
        {
            let mut m = self.runtime.memory();
            m.set_u32(ESP, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(ESP + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu.esp = ESP;
        self.cpu.eax = 0xdead_beef;
        let t = Runner::default().run(&mut self.cpu, &mut self.ctx, target);
        assert_eq!(t.kind, abi::STOP, "{target:#x}: {t:?}");
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x}: {:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(
            self.cpu.esp,
            ESP + 4 + 4 * args.len() as u32,
            "{target:#x} cleanup"
        );
        self.cpu.eax
    }
    fn u32(&self, a: u32) -> u32 {
        self.runtime.memory().u32(a).unwrap()
    }
    /// The container's create call (`0x388ac1..0x388b01`) for a profile.
    fn create(&mut self, bytes_per_second: u32, block_align: u32) -> (u32, u32) {
        let out = SCRATCH;
        let r = self.call(
            CREATE,
            &[
                out,
                CONTEXT,
                INPUT,
                2,
                0x8800,
                44_100,
                2,
                bytes_per_second,
                block_align,
                15,
                0,
            ],
        );
        (r, self.u32(out))
    }
    /// One PCM retrieval of up to 2048 frames, as `0x3886b0` asks.
    fn pcm(&mut self, handle: u32) -> Vec<i16> {
        let count = SCRATCH + 0x10;
        let buf = SCRATCH + 0x100;
        self.runtime
            .memory()
            .set_u32(count, 0xabcd_0000 | 2048)
            .unwrap();
        assert_eq!(self.call(GET_PCM, &[handle, count, buf, 8192]), OK);
        let n = self.u32(count);
        assert_eq!(n >> 16, 0xabcd, "only the u16 count is written");
        let n = (n & 0xffff) as usize;
        assert!(n <= 2048);
        let mut b = vec![0; n * 4];
        self.runtime.memory().read_bytes(buf, &mut b, 1).unwrap();
        b.as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect()
    }
    /// The oracle's model of the container loop: begin, decode, drain; begin
    /// again after PACKET_DONE; stop on NO_DATA with the input empty.
    fn decode_all(&mut self, handle: u32) -> Vec<i16> {
        let mut pcm = Vec::new();
        let mut begin = true;
        for _ in 0..10_000 {
            if begin {
                let s = self.call(BEGIN, &[handle]);
                if s == NO_DATA && self.input.lock().unwrap().packets.is_empty() {
                    return pcm;
                }
                assert_eq!(s, OK);
            }
            let s = self.call(DECODE, &[handle]);
            assert!(s == OK || s == PACKET_DONE, "{s:#x}");
            loop {
                let p = self.pcm(handle);
                if p.is_empty() {
                    break;
                }
                pcm.extend(p);
            }
            begin = s == PACKET_DONE;
        }
        panic!("decode loop did not end");
    }
}

/// A silent synthetic packet: `sequence`, one full-size uncoded frame.
fn silent(sequence: u8, block_align: usize) -> Vec<u8> {
    let mut p = vec![0; block_align];
    p[0] = (sequence << 4) | 1;
    p
}

fn direct(packets: &[Vec<u8>], bytes_per_second: u32, block_align: u16) -> Vec<i16> {
    let tables = wma::Tables::synthetic();
    let format = wma::Format {
        bytes_per_second,
        block_align,
    };
    let mut d = wma::Decoder::new(&tables, format).unwrap();
    let mut pcm = Vec::new();
    for p in packets {
        pcm.extend(d.push(p).unwrap());
    }
    pcm.extend(d.finish().unwrap());
    pcm
}

#[test]
fn registration_installs_the_six_codec_entries_or_nothing() {
    let mut d = Dispatch::default();
    let hle = WmaHle::new(Arc::new(wma::Tables::synthetic()));
    let r = xwma::register(&mut d, &hle).unwrap();
    assert_eq!(
        r.addresses,
        [
            0x0038_a0c5,
            0x0038_a13f,
            0x0038_a16b,
            0x0038_a189,
            0x0038_a1a7,
            0x0038_a1cc
        ]
    );
    for a in &r.addresses {
        assert!(d.override_name(*a).is_some_and(|n| n.starts_with("wma::")));
    }
    let Err(RegisterError::Occupied(c)) = xwma::register(&mut d, &hle) else {
        panic!("second registration must fail closed");
    };
    assert_eq!(c.len(), 6);
    xwma::unregister(&mut d, &r);
}

#[test]
fn both_profiles_decode_through_the_guest_callback_like_the_direct_decoder() {
    for (bps, align) in [(12003u32, 4459u16), (16002, 5945)] {
        let mut w = world();
        let packets: Vec<Vec<u8>> = (0..6).map(|i| silent(i, align.into())).collect();
        let want = direct(&packets, bps, align);
        assert_eq!(want.len(), 5 * 4096);
        w.input.lock().unwrap().packets = packets.iter().cloned().collect();
        let (r, handle) = w.create(bps, u32::from(align));
        assert_eq!(r, OK);
        assert_ne!(handle, 0);
        assert_eq!(w.hle.streams(), 1);
        let got = w.decode_all(handle);
        assert_eq!(got, want, "{bps}");
        // Every callback got the container's context, in <= 128-byte chunks.
        let calls = w.input.lock().unwrap().calls.clone();
        assert!(calls.iter().all(|&c| c == CONTEXT));
        let chunks = usize::from(align).div_ceil(128);
        assert_eq!(
            calls.len(),
            6 * chunks + 2,
            "a dry call per end-of-input begin"
        );
        let stats = w.hle.stats();
        assert_eq!(
            (stats.packets, stats.pcm_frames, stats.errors),
            (6, 5 * 2048, 0)
        );
        // Destroy clears the container's handle word.
        w.runtime.memory().set_u32(SCRATCH + 0x20, handle).unwrap();
        assert_eq!(w.call(DESTROY, &[SCRATCH + 0x20]), OK);
        assert_eq!(w.u32(SCRATCH + 0x20), 0);
        assert_eq!(w.hle.streams(), 0);
        assert_eq!(w.call(DESTROY, &[SCRATCH + 0x20]), OK, "a cleared word");
        assert_eq!(w.hle.stats().packets, 6, "totals survive destroy");
    }
}

#[test]
fn input_that_stalls_mid_packet_resumes_on_the_next_begin() {
    let mut w = world();
    let packets: Vec<Vec<u8>> = (0..3).map(|i| silent(i, 4459)).collect();
    let want = direct(&packets, 12003, 4459);
    let (_, handle) = w.create(12003, 4459);
    // Only 2000 bytes of packet 0 have been read from the disc so far.
    {
        let mut i = w.input.lock().unwrap();
        i.packets = packets.iter().cloned().collect();
        i.stall_at = Some(2000);
    }
    assert_eq!(w.call(BEGIN, &[handle]), NO_DATA);
    assert_eq!(w.call(BEGIN, &[handle]), NO_DATA, "still waiting");
    assert_eq!(w.hle.stats().packets, 0);
    // The read lands; the same packet continues without a start flag.
    w.input.lock().unwrap().stall_at = None;
    let got = w.decode_all(handle);
    assert_eq!(got, want);
    let stats = w.hle.stats();
    assert_eq!(
        (stats.packets, stats.dropped_partials, stats.restarts),
        (3, 0, 0)
    );
}

#[test]
fn reset_and_unknown_handles() {
    let mut w = world();
    let (_, handle) = w.create(12003, 4459);
    assert_eq!(w.call(RESET, &[handle]), OK);
    assert_eq!(w.call(RESET, &[0x1234]), 0x8007_0057);
    assert_eq!(w.call(DECODE, &[0x1234]), 0x8007_0057);
    assert_eq!(w.call(BEGIN, &[0x1234]), 0x8007_0057);
    w.runtime.memory().set_u32(SCRATCH + 0x10, 16).unwrap();
    assert_eq!(
        w.call(GET_PCM, &[0x1234, SCRATCH + 0x10, SCRATCH + 0x100, 64]),
        0x8007_0057
    );
    // A drained stream: begin is NO_DATA after the end, decode still done.
    assert_eq!(w.call(BEGIN, &[handle]), OK, "end of an empty input");
    assert_eq!(w.call(BEGIN, &[handle]), NO_DATA);
    assert_eq!(w.call(DECODE, &[handle]), PACKET_DONE);
}

#[test]
fn formats_outside_the_retail_profiles_are_refused() {
    let mut w = world();
    let r = w.call(
        CREATE,
        &[
            SCRATCH, CONTEXT, INPUT, 2, 0x8800, 22_050, 1, 4000, 1000, 15, 0,
        ],
    );
    assert_eq!(r, UNSUPPORTED);
    assert_eq!(w.u32(SCRATCH), 0, "no handle written");
    assert_eq!(w.hle.streams(), 0);
    let refused = w.hle.take_refusals();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].params.sample_rate, 22_050);
}

/// A real permutation from the owner's disc through the boundary, against
/// `wma::decode_asf` on the same bytes (which the oracle checked against the
/// game's own decoder). Reads `$H2_ISO`, else `halo2.iso` beside `$H2_XBE`;
/// nothing is written or kept.
#[test]
#[ignore = "needs the retail disc image"]
fn a_disc_stream_decodes_natively_through_the_boundary() {
    let xbe_path = std::env::var_os("H2_XBE").map(std::path::PathBuf::from);
    let iso = std::env::var_os("H2_ISO")
        .map(std::path::PathBuf::from)
        .or_else(|| xbe_path.as_ref().map(|x| x.with_file_name("halo2.iso")))
        .expect("set $H2_ISO (or $H2_XBE next to halo2.iso)");
    let disc = asf::disc::Disc::open(&iso).expect("open the disc image");
    let xbe = match &xbe_path {
        Some(p) => std::fs::read(p).expect("read $H2_XBE"),
        None => disc.read_xbe().expect("read default.xbe"),
    };
    let image = wma::VerifiedImage::load(&xbe).expect("retail default.xbe");
    let tables = wma::Tables::load(&image).unwrap();
    // 0: 128 kbps menu music; 38 has a zero-frame reservoir packet.
    for index in [0usize, 38] {
        let bytes = disc.read_stream(index).unwrap();
        let reference = wma::decode_asf(&tables, &bytes, 128 << 20).unwrap();
        let file = asf::Asf::parse(&bytes).unwrap();
        let stream = &file.streams[0];
        let format = stream.audio.as_ref().unwrap();
        let mut reassembly = asf::Reassembler::new(stream.number, 1 << 20).unwrap();
        let mut packets = Vec::new();
        for packet in file.packets() {
            for payload in packet.unwrap().payloads {
                if let Some(o) = reassembly.push(&payload).unwrap() {
                    packets.extend(
                        o.data
                            .chunks_exact(format.block_align.into())
                            .map(<[u8]>::to_vec),
                    );
                }
            }
        }
        let mut w = world_with_tables(wma::Tables::load(&image).unwrap());
        w.input.lock().unwrap().packets = packets.into();
        let (r, handle) = w.create(format.bytes_per_second, format.block_align.into());
        assert_eq!(r, OK, "stream {index}");
        let pcm = w.decode_all(handle);
        assert_eq!(pcm.len(), reference.pcm.len(), "stream {index}");
        assert!(pcm == reference.pcm, "stream {index}: PCM differs");
        assert_eq!(w.hle.stats().errors, 0);
        eprintln!(
            "stream {index}: {} stereo frames, {} packets",
            pcm.len() / 2,
            w.hle.stats().packets
        );
    }
}
