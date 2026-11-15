//! `d3d/pushbuf-frame`: the D3D device draining one synthetic frame of
//! inline push-buffer draws, the way Halo's inlined Begin/SetVertexData/End
//! sites write them, through `Device::kick_off` with a recording backend
//! (no GPU). Measures the push-buffer parse, immediate-mode assembly, state
//! resolution and draw submission on the CPU side.
//!
//! The guest memory layout follows crates/d3d's own tests: synthetic, with
//! a hand-made stand-in for the simple-render-state encode table.

use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use d3d::abi::{self, Regs};
use d3d::device::{DeviceConfig, GuestRange, PresentParameters};
use d3d::pushbuf::method;
use d3d::{Backend, Device, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};

use crate::bench::{BenchCtx, Better, Measurement, Prepared, Scenario};

const PB: GuestRange = GuestRange {
    addr: 0x0100_0000,
    len: 0x4_0000,
};
const SCRATCH: GuestRange = GuestRange {
    addr: 0x0200_0000,
    len: 0x1000,
};
const MISC: u32 = 0x0300_0000;
const PARAMS: u32 = MISC;
const PP_DEVICE: u32 = MISC + 0x100;
const STACK_TOP: u32 = MISC + 0xF000;
const QUADLIST: u32 = 8;

/// Halo's LTCG entry addresses used here (CreateDevice, two render
/// states); the adapters are crates/d3d's.
const CREATE_DEVICE: u32 = 0x3f5240;
const SET_ZENABLE: u32 = 0x3f54f0;
const SET_CULLMODE: u32 = 0x3f53d0;

fn memory() -> Result<SparseMemory> {
    let mut m = SparseMemory::new();
    m.map(0x0040_4000, 0x5000);
    let encode = d3d::GuestLayout::HALO2.simple_encode;
    m.map(encode & !0xFFF, 0x1000);
    // As crates/d3d's tests: a few slots carry their NV2A method (ZFUNC,
    // ALPHABLENDENABLE, ZWRITEENABLE, one more); every other slot shares
    // the non-invertible 0x1D90.
    let mut slots = [(1u32 << 18) | 0x1D90; d3d::pushbuf::SIMPLE_ENCODE_SLOTS];
    for (slot, m) in [(57, 0x0354), (59, 0x0304), (64, 0x035C), (82, 0x1D78)] {
        slots[slot] = (1 << 18) | m;
    }
    let table: Vec<u8> = slots.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.write(encode, &table)?;
    m.map(PB.addr, PB.len);
    m.map(SCRATCH.addr, SCRATCH.len);
    m.map(MISC, 0x1_0000);
    PresentParameters {
        width: 64,
        height: 64,
        back_buffer_format: 0x06,
        back_buffer_count: 1,
        multisample_type: 0,
        swap_effect: 0,
        enable_auto_depth_stencil: true,
        auto_depth_stencil_format: 0x2A,
        flags: 0,
        refresh_rate: 0,
        presentation_interval: 0x8000_0000,
        buffer_surface: 0,
        depth_stencil_surface: 0,
    }
    .write(&mut m, PARAMS)?;
    Ok(m)
}

fn call<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    addr: u32,
    regs: Regs,
    stack: &[u32],
) -> Result<()> {
    let entries = abi::entries::<B>();
    let e = entries
        .iter()
        .find(|e| e.addr == addr)
        .with_context(|| format!("no d3d entry {addr:#x}"))?;
    let esp = STACK_TOP - 4 - 4 * stack.len() as u32;
    m.write_u32(esp, 0xDEAD_BEEF)?;
    for (i, v) in stack.iter().enumerate() {
        m.write_u32(esp + 4 + 4 * i as u32, *v)?;
    }
    abi::call(e, d, m, &Regs { esp, ..regs }).map_err(|e| anyhow!("{e}"))?;
    Ok(())
}

/// The frame's words: `quads` inline QUADLIST draws with a colour each.
pub fn frame_words(quads: u32) -> Vec<u32> {
    let header = |m: u32, count: u32| (count << 18) | m;
    let mut w = Vec::with_capacity(quads as usize * 26);
    for q in 0..quads {
        let (x0, y0) = ((q % 8) as f32 * 8.0, ((q / 8) % 8) as f32 * 8.0);
        let (x1, y1) = (x0 + 8.0, y0 + 8.0);
        w.push(header(method::BEGIN_END, 1));
        w.push(QUADLIST);
        w.push(header(method::VERTEX_DATA4UB + 4 * 3, 1));
        w.push(0xFF00_0000 | q.wrapping_mul(0x9E37_79B9) >> 8);
        for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            w.push(header(method::VERTEX4F, 4));
            for f in [x, y, 0.5f32, 1.0] {
                w.push(f.to_bits());
            }
        }
        w.push(header(method::BEGIN_END, 1));
        w.push(0);
    }
    w
}

pub struct PushbufFrame {
    quads: u32,
    state: Option<(Device<RecordingBackend>, SparseMemory, Vec<u8>)>,
}

impl Default for PushbufFrame {
    fn default() -> Self {
        PushbufFrame {
            quads: 1000,
            state: None,
        }
    }
}

impl Scenario for PushbufFrame {
    fn name(&self) -> &str {
        "d3d/pushbuf-frame"
    }
    fn description(&self) -> &str {
        "D3D drains a synthetic frame of inline push-buffer quads (recording backend)"
    }
    fn unit(&self) -> &str {
        "us/frame"
    }
    fn better(&self) -> Better {
        Better::Lower
    }
    fn prepare(&mut self, ctx: &BenchCtx) -> Result<Prepared> {
        if ctx.quick {
            self.quads = 50;
        }
        let mut m = memory()?;
        let mut d = Device::new(
            DeviceConfig {
                upload: UploadPolicy::Tracked,
                ..DeviceConfig::halo2(PB, SCRATCH)
            },
            RecordingBackend::new(),
        );
        call(
            &mut d,
            &mut m,
            CREATE_DEVICE,
            Regs {
                eax: 0x40,
                ecx: PP_DEVICE,
                ..Regs::default()
            },
            &[PARAMS],
        )?;
        call(&mut d, &mut m, SET_ZENABLE, Regs::default(), &[0])?;
        call(&mut d, &mut m, SET_CULLMODE, Regs::default(), &[0])?;
        d.set_vertex_shader(&mut m, gfx::vertex::fvf::XYZRHW | gfx::vertex::fvf::DIFFUSE)
            .map_err(|e| anyhow!("{e}"))?;
        let bytes: Vec<u8> = frame_words(self.quads)
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        self.state = Some((d, m, bytes));
        Ok(Prepared::Ready)
    }
    fn measure(&mut self) -> Result<Measurement> {
        let (d, m, bytes) = self.state.as_mut().context("not prepared")?;
        let dev = d3d::GuestLayout::HALO2.device;
        // Rewind, as D3D_MakeRequestedSpace does, then write the frame
        // where game code would and advance put. Not timed.
        let start = d
            .make_requested_space(m, 0, 0)
            .map_err(|e| anyhow!("{e}"))?;
        m.write(start, bytes)?;
        m.write_u32(dev, start + bytes.len() as u32)?;
        let before = d.stats().inline_draws;
        let t = Instant::now();
        d.kick_off(m).map_err(|e| anyhow!("{e}"))?;
        let us = t.elapsed().as_nanos() as f64 / 1000.0;
        let draws = d.stats().inline_draws - before;
        if draws != self.quads as u64 {
            anyhow::bail!(
                "drew {draws} of {} quads; events {:?}",
                self.quads,
                d.take_events()
            );
        }
        d.backend.calls.clear();
        d.take_events();
        Ok(Measurement::new(us)
            .with("quads", self.quads as f64)
            .with("ns_per_quad", us * 1000.0 / self.quads as f64)
            .with("words", (bytes.len() / 4) as f64))
    }
}
