//! Bit-preserving SSE1/MMX queries to an isolated x86-64 hardware runner.
use anyhow::{Context, Result, bail};
use iced_x86::{CpuidFeature, Decoder, DecoderOptions, Encoder, Instruction, OpKind, Register};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Form {
    pub mnemonic: String,
    pub code: String,
    pub bytes: Vec<u8>,
    pub sites: usize,
    pub immediate: Option<u8>,
}

/// Analyze claimed instruction starts, never linear-sweep embedded data.
pub fn inventory(data: &[u8]) -> Result<Vec<Form>> {
    let xbe = xbe::Xbe::parse(data)?;
    let analysis = analyze::analyze(&xbe)?;
    if !analysis.converged {
        bail!("XBE analysis did not converge");
    }
    let mut forms = BTreeMap::<String, Form>::new();
    for (base, size, _) in &analysis.ranges {
        for va in *base..base + size {
            if analysis.claim(va).map(|c| c.0) != Some('I') {
                continue;
            }
            let bytes = xbe.bytes_at(va, 15.min((base + size - va) as usize))?;
            let mut insn = Decoder::with_ip(32, bytes, va as u64, DecoderOptions::NONE).decode();
            if !insn
                .cpuid_features()
                .iter()
                .any(|f| matches!(f, CpuidFeature::SSE | CpuidFeature::MMX))
            {
                continue;
            }
            normalize(&mut insn)?;
            let mut encoder = Encoder::new(64);
            encoder
                .encode(&insn, 0)
                .with_context(|| format!("encoding {:?}", insn.code()))?;
            let bytes = encoder.take_buffer();
            let key = format!("{:?}:{bytes:02x?}", insn.code());
            let immediate = (0..insn.op_count())
                .find(|i| insn.op_kind(*i) == OpKind::Immediate8)
                .map(|_| insn.immediate8());
            forms
                .entry(key)
                .and_modify(|f| f.sites += 1)
                .or_insert(Form {
                    mnemonic: format!("{:?}", insn.mnemonic()).to_lowercase(),
                    code: format!("{:?}", insn.code()),
                    bytes,
                    sites: 1,
                    immediate,
                });
        }
    }
    Ok(forms.into_values().collect())
}

fn normalize(insn: &mut Instruction) -> Result<()> {
    insn.set_has_lock_prefix(false);
    insn.set_segment_prefix(Register::None);
    // Preserve aliases within a register class, using canonical slots 0..7.
    let mut registers = BTreeMap::new();
    let (mut xmm, mut mm, mut gpr) = (0, 0, 0);
    for i in 0..insn.op_count() {
        match insn.op_kind(i) {
            OpKind::Register => {
                let r = insn.op_register(i);
                let replacement = *registers.entry(r).or_insert_with(|| {
                    if r.is_xmm() {
                        let n = Register::XMM0 as u32 + xmm;
                        xmm += 1;
                        Register::try_from(n as usize).unwrap()
                    } else if r.is_mm() {
                        let n = Register::MM0 as u32 + mm;
                        mm += 1;
                        Register::try_from(n as usize).unwrap()
                    } else if r.is_gpr32() {
                        let n = [Register::EAX, Register::ECX][gpr];
                        gpr += 1;
                        n
                    } else {
                        r
                    }
                });
                insn.set_op_register(i, replacement);
            }
            OpKind::Memory => {
                insn.set_memory_base(Register::RDI);
                insn.set_memory_index(Register::None);
                insn.set_memory_index_scale(1);
                insn.set_memory_displacement64(0);
                insn.set_memory_displ_size(0);
            }
            OpKind::Immediate8 => (),
            other => bail!("unsupported SSE operand: {other:?}"),
        }
    }
    Ok(())
}

/// Little-endian lanes. No floating-point JSON values, even for NaNs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub xmm: [[u32; 4]; 8],
    pub mm: [[u32; 2]; 8],
    pub mxcsr: u32,
    pub eflags: u32,
    pub eax: u32,
    pub ecx: u32,
    pub memory: [u32; 4],
}
impl Default for State {
    fn default() -> Self {
        Self {
            xmm: [[0; 4]; 8],
            mm: [[0; 2]; 8],
            mxcsr: 0x1f80,
            eflags: 0x202,
            eax: 0,
            ecx: 0,
            memory: [0; 4],
        }
    }
}
impl State {
    pub(crate) fn words(&self) -> Vec<u32> {
        self.xmm
            .iter()
            .flatten()
            .chain(self.mm.iter().flatten())
            .copied()
            .chain([self.mxcsr, self.eflags, self.eax, self.ecx])
            .chain(self.memory)
            .collect()
    }
    pub(crate) fn from_words(words: &[u32]) -> Self {
        let mut s = Self::default();
        for (dst, src) in s.xmm.iter_mut().flatten().zip(&words[..32]) {
            *dst = *src;
        }
        for (dst, src) in s.mm.iter_mut().flatten().zip(&words[32..48]) {
            *dst = *src;
        }
        s.mxcsr = words[48];
        s.eflags = words[49];
        s.eax = words[50];
        s.ecx = words[51];
        s.memory.copy_from_slice(&words[52..56]);
        s
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    /// Absent on a signal: partial state is deliberately not presented as an output.
    pub state: Option<State>,
    /// Host POSIX signal number, or null. MXCSR flags are in state.mxcsr & 63.
    pub exception: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fixture {
    pub schema: u32,
    pub backend: String,
    pub mnemonic: String,
    pub code: String,
    pub instruction: Vec<u8>,
    pub case: String,
    pub input: State,
    pub output: Observation,
}

const NATIVE_LINUX: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

/// Whether the supported host has the tools needed for live x86-64 queries.
pub fn runner_available() -> bool {
    if NATIVE_LINUX {
        Command::new("clang")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    } else {
        rosetta_available()
    }
}

/// Build separately from Cargo so native arm64 clients need no x86 Rust target.
pub fn build_runner(directory: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(directory)?;
    let c = directory.join("runner.c");
    let asm = directory.join("execute.S");
    std::fs::write(&c, include_str!("../native/runner.c"))?;
    std::fs::write(&asm, include_str!("../native/execute.S"))?;
    let binary = directory.join("sse-runner");
    let mut clang = Command::new("clang");
    if !NATIVE_LINUX {
        clang.args(["-arch", "x86_64"]);
    }
    let output = clang
        .args(["-O2", "-Wall", "-Wextra", "-Werror", "-mno-sse2"])
        .arg(&c)
        .arg(&asm)
        .arg("-o")
        .arg(&binary)
        .output()?;
    if !output.status.success() {
        bail!("clang: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(binary)
}

pub struct Oracle {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}
impl Oracle {
    pub fn spawn(binary: &Path) -> Result<Self> {
        let mut cmd = if NATIVE_LINUX {
            Command::new(binary)
        } else {
            let mut arch = Command::new("arch");
            arch.arg("-x86_64").arg(binary);
            arch
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child.stdin.take().context("runner stdin")?;
        let output = child.stdout.take().context("runner stdout")?;
        Ok(Self {
            child,
            input,
            output,
        })
    }
    /// One legacy SSE/MMX instruction; explicit registers are limited to the
    /// captured bank, EAX/ECX, and memory [RDI]. Use inventory forms or explicit x86 bytes.
    pub fn query(&mut self, bytes: &[u8], state: &State) -> Result<Observation> {
        validate_instruction(bytes)?;
        if state.mxcsr & !0xffbf != 0 || state.mxcsr & 0x1f80 != 0x1f80 {
            bail!("MXCSR must mask all exceptions, clear DAZ and reserved bits");
        }
        let mut request = Vec::with_capacity(244);
        request.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        request.extend_from_slice(bytes);
        request.resize(20, 0);
        for w in state.words() {
            request.extend_from_slice(&w.to_le_bytes());
        }
        self.input.write_all(&request)?;
        self.input.flush()?;
        let mut response = [0u8; 228];
        self.output
            .read_exact(&mut response)
            .context("runner exited or truncated response")?;
        let words: Vec<_> = response
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let signal = words[56];
        Ok(Observation {
            state: (signal == 0).then(|| State::from_words(&words)),
            exception: (signal != 0).then_some(signal),
        })
    }
}
impl Drop for Oracle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn validate_instruction(bytes: &[u8]) -> Result<()> {
    let mut decoder = Decoder::new(64, bytes, DecoderOptions::NONE);
    let i = decoder.decode();
    if i.is_invalid()
        || i.len() != bytes.len()
        || i.has_lock_prefix()
        || !i
            .cpuid_features()
            .iter()
            .any(|f| matches!(f, CpuidFeature::SSE | CpuidFeature::MMX))
    {
        bail!("expected exactly one SSE1/MMX instruction");
    }
    for operand in 0..i.op_count() {
        match i.op_kind(operand) {
            OpKind::Register => {
                let r = i.op_register(operand);
                if !((r >= Register::XMM0 && r <= Register::XMM7)
                    || r.is_mm()
                    || matches!(r, Register::EAX | Register::ECX))
                {
                    bail!("uncaptured register {r:?}");
                }
            }
            OpKind::Memory
                if i.memory_base() == Register::RDI
                    && i.memory_index() == Register::None
                    && i.memory_displacement64() == 0
                    && i.segment_prefix() == Register::None => {}
            OpKind::Immediate8 => (),
            kind => bail!("unsupported operand {kind:?}; memory must be [rdi]"),
        }
    }
    Ok(())
}

pub mod corpus;
pub mod fixtures;
pub use fixtures::{fixtures, rosetta_available};
pub mod fidelity;
