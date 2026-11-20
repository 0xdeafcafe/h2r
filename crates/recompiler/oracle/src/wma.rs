//! Private-disc WMAv2 oracle. Executes the authenticated XBE's decoder under
//! Unicorn. This is a validation tool, never the engine's playback backend.
use anyhow::{Result, bail, ensure};
use std::collections::VecDeque;
use unicorn_engine::{
    RegisterX86 as R, Unicorn,
    unicorn_const::{Arch, Mode, Prot, X86CpuModel, uc_error},
};

const RETURN: u32 = 0x0100_0000;
const INPUT_CALLBACK: u32 = RETURN + 0x100;
const STACK: u32 = 0x0110_0000;
const SCRATCH: u32 = 0x0120_0000;
const PCM: u32 = SCRATCH + 0x1000;
const HEAP: u32 = 0x0200_0000;
const HEAP_END: u32 = 0x0300_0000;
const CREATE: u32 = 0x0038_a0c5;
const BEGIN_PACKET: u32 = 0x0038_a189;
const DECODE: u32 = 0x0038_a1a7;
const GET_PCM: u32 = 0x0038_a1cc;
const MALLOC: u32 = 0x0032_2e16;
const FREE: u32 = 0x0032_4c91;

#[derive(Default)]
struct Host {
    heap: u32,
    input: VecDeque<Vec<u8>>,
    offset: usize,
    callbacks: usize,
    error: Option<String>,
}
pub struct Decoder {
    uc: Unicorn<'static, Host>,
    handle: u32,
}
fn read32(uc: &Unicorn<Host>, address: u32) -> Result<u32, uc_error> {
    let mut b = [0; 4];
    uc.mem_read(address.into(), &mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn write32(uc: &mut Unicorn<Host>, address: u32, value: u32) -> Result<(), uc_error> {
    uc.mem_write(address.into(), &value.to_le_bytes())
}
fn return_from(uc: &mut Unicorn<Host>, result: u32, cleanup: u32) -> Result<(), uc_error> {
    let esp = uc.reg_read(R::ESP)? as u32;
    let ret = read32(uc, esp)?;
    uc.reg_write(R::EAX, result.into())?;
    uc.reg_write(R::ESP, u64::from(esp + 4 + cleanup))?;
    uc.reg_write(R::EIP, ret.into())
}
fn host_call(uc: &mut Unicorn<Host>, address: u64) -> Result<(), uc_error> {
    let esp = uc.reg_read(R::ESP)? as u32;
    match address as u32 {
        MALLOC => {
            let n = read32(uc, esp + 4)?;
            let heap = uc.get_data().heap;
            let next = heap
                .checked_add(n.max(1))
                .and_then(|n| n.checked_add(15))
                .map(|n| n & !15);
            if let Some(next) = next.filter(|&n| n <= HEAP_END) {
                uc.get_data_mut().heap = next;
                return_from(uc, heap, 0)
            } else {
                uc.get_data_mut().error = Some("oracle heap exhausted".into());
                uc.emu_stop()
            }
        }
        FREE => return_from(uc, 0, 0),
        INPUT_CALLBACK => {
            // Callback ABI verified at 0x38ce7d..0x38ce8c: (data**, size*,
            // context, 128-byte staging buffer). Return 3 marks a packet start.
            let out_ptr = read32(uc, esp + 4)?;
            let out_size = read32(uc, esp + 8)?;
            let buffer = read32(uc, esp + 16)?;
            let host = uc.get_data_mut();
            host.callbacks += 1;
            let mut result = 0;
            let chunk = if let Some(packet) = host.input.front() {
                if host.offset == 0 {
                    result = 3;
                }
                let end = (host.offset + 128).min(packet.len());
                let chunk = packet[host.offset..end].to_vec();
                host.offset = end;
                if end == packet.len() {
                    host.input.pop_front();
                    host.offset = 0;
                }
                chunk
            } else {
                Vec::new()
            };
            if !chunk.is_empty() {
                uc.mem_write(buffer.into(), &chunk)?;
            }
            write32(uc, out_ptr, buffer)?;
            write32(uc, out_size, chunk.len() as u32)?;
            return_from(uc, result, 16)
        }
        _ => unreachable!(),
    }
}
impl Decoder {
    pub fn new(
        image: &::wma::VerifiedImage<'_>,
        format: &asf::WaveFormat<'_>,
        packets: Vec<Vec<u8>>,
    ) -> Result<Self> {
        ensure!(
            format.format_tag == 0x161 && format.channels == 2 && format.sample_rate == 44100,
            "oracle supports retail WMAv2 stereo only"
        );
        ensure!(format.extra.len() == 10, "WMAv2 extra size");
        let mut uc = Unicorn::new_with_data(
            Arch::X86,
            Mode::MODE_32,
            Host {
                heap: HEAP,
                input: packets.into(),
                ..Host::default()
            },
        )?;
        uc.ctl_set_cpu_model(X86CpuModel::PENTIUM3 as i32)?;
        // A bounded guest sandbox. The image's initialized data is copied in;
        // all BSS and scratch memory starts at zero.
        uc.mem_map(0x10000, 0x600000, Prot::ALL)?;
        for s in &image.xbe().sections {
            let n = s.raw_size.min(s.vsize) as usize;
            uc.mem_write(s.va.into(), image.bytes_at(s.va, n)?)?;
        }
        uc.mem_map(RETURN.into(), 0x1000, Prot::ALL)?;
        uc.mem_map(STACK.into(), 0x100000, Prot::READ | Prot::WRITE)?;
        uc.mem_map(SCRATCH.into(), 0x10000, Prot::READ | Prot::WRITE)?;
        uc.mem_map(
            HEAP.into(),
            (HEAP_END - HEAP).into(),
            Prot::READ | Prot::WRITE,
        )?;
        uc.mem_write(RETURN.into(), &[0xcc])?;
        uc.mem_write(INPUT_CALLBACK.into(), &[0xcc])?;
        for address in [MALLOC, FREE, INPUT_CALLBACK] {
            uc.add_code_hook(address.into(), address.into(), |uc, address, _| {
                if let Err(e) = host_call(uc, address) {
                    uc.get_data_mut().error = Some(format!("host boundary {address:#x}: {e:?}"));
                    let _ = uc.emu_stop();
                }
            })?;
        }
        uc.reg_write(R::CR0, 0x21)?;
        uc.reg_write(R::CR4, 0x600)?;
        uc.reg_write(R::FPCW, 0x027f)?;
        uc.reg_write(R::FPTAG, 0xffff)?;
        uc.reg_write(R::MXCSR, 0x1f80)?;
        let mut decoder = Self { uc, handle: 0 };
        let samples = u32::from_le_bytes(format.extra[..4].try_into()?);
        let flags = u16::from_le_bytes(format.extra[4..6].try_into()?);
        let result = decoder.call(
            CREATE,
            &[
                SCRATCH,
                0,
                INPUT_CALLBACK,
                2,
                samples,
                format.sample_rate,
                format.channels.into(),
                format.bytes_per_second,
                format.block_align.into(),
                flags.into(),
                0,
            ],
        )?;
        ensure!(result == 0, "codec initialization: {result:#010x}");
        decoder.handle = read32(&decoder.uc, SCRATCH)?;
        Ok(decoder)
    }
    fn call(&mut self, address: u32, args: &[u32]) -> Result<u32> {
        let esp = STACK + 0xff000;
        let mut stack = RETURN.to_le_bytes().to_vec();
        for a in args {
            stack.extend(a.to_le_bytes());
        }
        self.uc.mem_write(esp.into(), &stack)?;
        self.uc.reg_write(R::ESP, esp.into())?;
        let result = self
            .uc
            .emu_start(address.into(), RETURN.into(), 10_000_000, 50_000_000);
        let eip = self.uc.reg_read(R::EIP)?;
        if let Some(e) = self.uc.get_data_mut().error.take() {
            bail!("{e}");
        }
        result.map_err(|e| anyhow::anyhow!("oracle {address:#x} stopped at {eip:#x}: {e:?}"))?;
        ensure!(
            eip == u64::from(RETURN),
            "oracle budget exceeded at {eip:#x}"
        );
        Ok(self.uc.reg_read(R::EAX)? as u32)
    }
    /// Packet driver modelled on the container loop at 0x388533. It is not
    /// XWMAXMO's full state machine. Its output length matches the native
    /// decoder on the 48 streams compared. Raw output includes codec
    /// priming/tail samples.
    pub fn decode(&mut self, max_calls: usize) -> Result<Vec<i16>> {
        let mut pcm = Vec::new();
        let mut begin = true;
        for _ in 0..max_calls {
            if begin {
                let status = self.call(BEGIN_PACKET, &[self.handle])?;
                if status == 0x80040004 && self.uc.get_data().input.is_empty() {
                    return Ok(pcm);
                }
                ensure!(
                    status == 0 || status == 0x80040001,
                    "packet begin: {status:#010x}"
                );
            }
            let status = self.call(DECODE, &[self.handle])?;
            ensure!(
                status == 0 || status == 4 || status == 0x80040004,
                "frame decode: {status:#010x}"
            );
            loop {
                self.uc
                    .mem_write((SCRATCH + 8).into(), &2048u16.to_le_bytes())?;
                let result = self.call(GET_PCM, &[self.handle, SCRATCH + 8, PCM, 8192])?;
                ensure!(result == 0, "PCM retrieval: {result:#010x}");
                let n = read32(&self.uc, SCRATCH + 8)? & 0xffff;
                if n == 0 {
                    break;
                }
                ensure!(n <= 2048, "PCM output size");
                let bytes = self.uc.mem_read_as_vec(PCM.into(), n as usize * 4)?;
                pcm.extend(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes([b[0], b[1]])),
                );
                ensure!(pcm.len() <= 44100 * 2 * 600, "PCM output limit");
            }
            if status == 0x80040004 && self.uc.get_data().input.is_empty() {
                return Ok(pcm);
            }
            begin = status == 4;
        }
        bail!("oracle decode call limit");
    }
}
