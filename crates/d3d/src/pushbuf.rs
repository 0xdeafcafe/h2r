//! The guest push buffer, and the small slice of the NV2A method stream that
//! game code writes into it directly.
//!
//! Halo inlines `D3DDevice_Begin`/`SetVertexData*`/`End` and a few state
//! pokes: game code reserves space by comparing the device's put pointer
//! (`device+0`) with its threshold (`device+4`), calling
//! `D3D_MakeRequestedSpace` when full, then stores NV2A method headers and
//! data words and advances put (36 `BEGIN_END` stores in 22 functions; see
//! `docs/d3d.md`). The device gives those writers a real buffer in guest
//! memory and *reads back* what they wrote at every D3D entry, so the inline
//! draws are executed in order with the calls around them.
//!
//! This is deliberately not an NV2A emulator: only the methods found in game
//! code are understood, and anything else is reported as
//! [`PushError::UnknownMethod`] so the caller can stop with a clear event.

use gfx::vertex::PrimitiveType;

use crate::guest::{GuestError, GuestMemory};

/// One method header plus its data words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodCall {
    /// Guest address of the header word.
    pub at: u32,
    /// Method offset of the first data word (`0x17FC` = `BEGIN_END`).
    pub method: u32,
    /// `true` for non-incrementing headers (all words go to `method`).
    pub non_incrementing: bool,
    /// Data words.
    pub data: Vec<u32>,
}

impl MethodCall {
    /// `(method, word)` pairs, honouring the increment mode.
    pub fn writes(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.data.iter().enumerate().map(|(i, &w)| {
            (
                if self.non_incrementing {
                    self.method
                } else {
                    self.method + 4 * i as u32
                },
                w,
            )
        })
    }
}

/// Why decoding stopped.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PushError {
    /// Guest memory fault.
    #[error(transparent)]
    Guest(#[from] GuestError),
    /// A jump, call or return word. Halo's inline sites never emit these;
    /// they would come from `RunPushBuffer`, which is not implemented.
    #[error("push-buffer control word {word:#010x} at {at:#010x} (jump/call/return)")]
    Control {
        /// Address of the word.
        at: u32,
        /// The word.
        word: u32,
    },
    /// A header with reserved bits set (bit 31, bits 17:16).
    #[error("push-buffer word {word:#010x} at {at:#010x} has reserved bits set")]
    Reserved {
        /// Address of the word.
        at: u32,
        /// The word.
        word: u32,
    },
    /// A method on a subchannel other than 0 (the 3D object).
    #[error("push-buffer method {word:#010x} at {at:#010x} targets subchannel {subchannel}")]
    Subchannel {
        /// Address of the header.
        at: u32,
        /// The header word.
        word: u32,
        /// Its subchannel.
        subchannel: u32,
    },
    /// The data words run past the put pointer.
    #[error(
        "push-buffer method at {at:#010x} needs {count} words but only {available} were written"
    )]
    Truncated {
        /// Address of the header.
        at: u32,
        /// Words announced.
        count: u32,
        /// Words before put.
        available: u32,
    },
    /// A method the device does not interpret.
    #[error("unimplemented NV2A method {method:#06x} (header at {at:#010x})")]
    UnknownMethod {
        /// Method offset.
        method: u32,
        /// Address of the header.
        at: u32,
    },
    /// A malformed immediate-mode sequence.
    #[error("immediate-mode error at {at:#010x}: {what}")]
    Immediate {
        /// Address of the header.
        at: u32,
        /// What went wrong.
        what: &'static str,
    },
}

/// Decodes the command at `at` (which must be below `to`). Returns the
/// address after it and the method call, or `None` for a zero-count header
/// (a hardware no-op).
pub fn decode_one(
    mem: &dyn GuestMemory,
    at: u32,
    to: u32,
) -> Result<(u32, Option<MethodCall>), PushError> {
    decode_one_into(mem, at, to, &mut Vec::new())
}

/// [`decode_one`], reading the data words into `words`' storage (taken
/// for the returned call's `data`; give it back for the next command), so
/// the per-entry drain does not allocate.
pub fn decode_one_into(
    mem: &dyn GuestMemory,
    at: u32,
    to: u32,
    words: &mut Vec<u32>,
) -> Result<(u32, Option<MethodCall>), PushError> {
    if to < at || to - at < 4 {
        return Err(PushError::Truncated {
            at,
            count: 1,
            available: 0,
        });
    }
    let word = mem.read_u32(at)?;
    // Old-style jump (bit 29), new-style jump (bits 1:0 = 1), call (2),
    // and return (0x00020000) are control flow.
    if word & 0x2000_0000 != 0 || word & 3 != 0 || word == 0x0002_0000 {
        return Err(PushError::Control { at, word });
    }
    // Bit 31 and bits 17:16 have no meaning in a method header.
    if word & 0x8003_0000 != 0 {
        return Err(PushError::Reserved { at, word });
    }
    let count = (word >> 18) & 0x7FF;
    let subchannel = (word >> 13) & 7;
    let method = word & 0x1FFC;
    let non_incrementing = word & 0x4000_0000 != 0;
    if count == 0 {
        return Ok((at + 4, None));
    }
    if subchannel != 0 {
        return Err(PushError::Subchannel {
            at,
            word,
            subchannel,
        });
    }
    let available = (to - at - 4) / 4;
    if count > available {
        return Err(PushError::Truncated {
            at,
            count,
            available,
        });
    }
    read_words_into(mem, at + 4, count as usize, words)?;
    let data = std::mem::take(words);
    let next = at + 4 + 4 * count;
    Ok((
        next,
        Some(MethodCall {
            at,
            method,
            non_incrementing,
            data,
        }),
    ))
}

/// [`GuestMemory::read_words`] into `out` (replacing what it held): one
/// guest read, with the same limits and errors, through a stack buffer.
fn read_words_into(
    mem: &dyn GuestMemory,
    addr: u32,
    n: usize,
    out: &mut Vec<u32>,
) -> Result<(), GuestError> {
    const WORDS: usize = 64;
    out.clear();
    if n > WORDS {
        out.extend(mem.read_words(addr, n)?);
        return Ok(());
    }
    crate::guest::check_range(addr, 4 * n)?;
    let mut bytes = [0u8; 4 * WORDS];
    let bytes = &mut bytes[..4 * n];
    mem.read(addr, bytes)?;
    out.extend(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c)),
    );
    Ok(())
}

/// Splits `[from, to)` into method calls.
pub fn decode(mem: &dyn GuestMemory, from: u32, to: u32) -> Result<Vec<MethodCall>, PushError> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let (next, call) = decode_one(mem, at, to)?;
        out.extend(call);
        at = next;
    }
    Ok(out)
}

/// NV2A (Kelvin) method offsets used here.
pub mod method {
    /// `NV097_NO_OPERATION`.
    pub const NOP: u32 = 0x0100;
    /// `NV097_WAIT_FOR_IDLE`.
    pub const WAIT_FOR_IDLE: u32 = 0x0110;
    /// `NV097_SET_BEGIN_END`: primitive type to begin, 0 to end.
    pub const BEGIN_END: u32 = 0x17FC;
    /// `NV097_INLINE_ARRAY`: vertex data inline in the push buffer, packed
    /// per the `SET_VERTEX_DATA_ARRAY_FORMAT` registers. Halo pushes it
    /// non-incrementing between `BEGIN_END` words (`0x4b220`, after
    /// BeginPush `0x3fae10`).
    pub const INLINE_ARRAY: u32 = 0x1818;
    /// `NV097_SET_VERTEX4F`: position, triggers a vertex.
    pub const VERTEX4F: u32 = 0x1518;
    /// `NV097_SET_VERTEX_DATA2F_M`: 16 registers x 2 floats.
    pub const VERTEX_DATA2F: u32 = 0x1880;
    /// `NV097_SET_VERTEX_DATA2S`: 16 registers x 1 word (two i16).
    pub const VERTEX_DATA2S: u32 = 0x1900;
    /// `NV097_SET_VERTEX_DATA4UB`: 16 registers x 1 word (four u8).
    pub const VERTEX_DATA4UB: u32 = 0x1940;
    /// `NV097_SET_VERTEX_DATA4S_M`: 16 registers x 2 words.
    pub const VERTEX_DATA4S: u32 = 0x1980;
    /// `NV097_SET_VERTEX_DATA4F_M`: 16 registers x 4 floats.
    pub const VERTEX_DATA4F: u32 = 0x1A00;
}

/// Vertices one immediate-mode primitive may hold (each is 256 bytes).
pub const MAX_IMMEDIATE_VERTICES: usize = crate::device::MAX_VERTICES as usize;

/// Words of inline-array data one primitive may hold: as many vertices as
/// immediate mode allows, at the largest vertex (16 float4 registers).
pub const MAX_INLINE_WORDS: usize = MAX_IMMEDIATE_VERTICES * 64;

/// Number of vertex attribute registers.
pub const REGISTERS: usize = 16;

/// One assembled immediate-mode vertex: all 16 registers.
pub type ImmediateVertex = [[f32; 4]; REGISTERS];

/// The result of an `End`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImmediateDraw {
    /// Primitive from the `Begin`.
    pub primitive: PrimitiveType,
    /// Assembled vertices.
    pub vertices: Vec<ImmediateVertex>,
    /// Registers written at least once between `Begin` and `End` (bit n =
    /// register n). Others hold their persistent values.
    pub written: u16,
    /// `INLINE_ARRAY` words pushed between `Begin` and `End`. A primitive
    /// has either these or assembled `vertices`, never both.
    pub inline: Vec<u32>,
}

/// Immediate-mode vertex assembly with NV2A semantics: attribute registers
/// persist across vertices (and across Begin/End), and writing the last
/// component of register 0 (position) emits a vertex.
#[derive(Clone, Debug)]
pub struct Immediate {
    regs: ImmediateVertex,
    open: Option<(PrimitiveType, u32)>,
    vertices: Vec<ImmediateVertex>,
    written: u16,
    inline: Vec<u32>,
}

impl Default for Immediate {
    fn default() -> Self {
        // Register defaults: (0, 0, 0, 1), diffuse and specular white-ish are
        // not set by the hardware; D3D's SetVertexData defaults are zeros
        // with w = 1.
        Self {
            regs: [[0.0, 0.0, 0.0, 1.0]; REGISTERS],
            open: None,
            vertices: Vec::new(),
            written: 0,
            inline: Vec::new(),
        }
    }
}

/// What a method write did.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Nothing observable.
    None,
    /// An `End` completed a primitive.
    Draw(ImmediateDraw),
}

impl Immediate {
    /// Whether a `Begin` is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Vertices assembled since the open `Begin`.
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Current (persistent) register values.
    pub fn registers(&self) -> &ImmediateVertex {
        &self.regs
    }

    /// `D3DDevice_Begin` (also `BEGIN_END` with a primitive).
    pub fn begin(&mut self, primitive: PrimitiveType, at: u32) -> Result<(), PushError> {
        if self.open.is_some() {
            return Err(PushError::Immediate {
                at,
                what: "Begin inside Begin",
            });
        }
        self.open = Some((primitive, at));
        self.vertices.clear();
        self.inline.clear();
        self.written = 0;
        Ok(())
    }

    /// `D3DDevice_End`.
    pub fn end(&mut self, at: u32) -> Result<ImmediateDraw, PushError> {
        let (primitive, _) = self.open.take().ok_or(PushError::Immediate {
            at,
            what: "End without Begin",
        })?;
        if !self.inline.is_empty() && !self.vertices.is_empty() {
            self.vertices.clear();
            self.inline.clear();
            return Err(PushError::Immediate {
                at,
                what: "INLINE_ARRAY and vertex-data writes in one Begin/End",
            });
        }
        Ok(ImmediateDraw {
            primitive,
            vertices: std::mem::take(&mut self.vertices),
            written: self.written,
            inline: std::mem::take(&mut self.inline),
        })
    }

    /// Sets the persistent value of register `reg` (an inline-array
    /// draw leaves its last vertex's attributes there).
    pub fn carry(&mut self, reg: usize, value: [f32; 4]) {
        self.regs[reg] = value;
    }

    /// One `INLINE_ARRAY` word. Only inside `Begin`/`End`; held to
    /// [`MAX_INLINE_WORDS`] per primitive for the same reason as
    /// [`Immediate::set`].
    pub fn inline_word(&mut self, at: u32, w: u32) -> Result<(), PushError> {
        if self.open.is_none() {
            return Err(PushError::Immediate {
                at,
                what: "INLINE_ARRAY outside Begin/End",
            });
        }
        if self.inline.len() >= MAX_INLINE_WORDS {
            return Err(PushError::Immediate {
                at,
                what: "more than MAX_INLINE_WORDS inline-array words in one Begin/End",
            });
        }
        self.inline.push(w);
        Ok(())
    }

    /// Sets register `reg` component-wise (the `SetVertexData*` family).
    /// `emit` says whether this write is the one that triggers a vertex.
    ///
    /// Every emission path (inline words, split across any number of drains
    /// and `MakeRequestedSpace` rewinds, and the API) ends here, so this is
    /// where one primitive is held to [`MAX_IMMEDIATE_VERTICES`]: the guest
    /// buffer's size does not bound it, since a rewind reuses the buffer
    /// while the `Begin` stays open. Nothing changes on failure.
    pub fn set(
        &mut self,
        at: u32,
        reg: usize,
        values: &[f32],
        first_component: usize,
        emit: bool,
    ) -> Result<(), PushError> {
        let emits = emit && reg == 0 && self.open.is_some();
        if emits && self.vertices.len() >= MAX_IMMEDIATE_VERTICES {
            return Err(PushError::Immediate {
                at,
                what: "more than MAX_IMMEDIATE_VERTICES vertices in one Begin/End",
            });
        }
        for (i, v) in values.iter().enumerate() {
            if first_component + i < 4 {
                self.regs[reg][first_component + i] = *v;
            }
        }
        self.written |= 1 << reg;
        if emits {
            self.vertices.push(self.regs);
        }
        Ok(())
    }

    /// Applies one method write. Methods outside the immediate-mode set
    /// return `Ok(None)` so the caller can decide (state mirrors, unknowns).
    pub fn apply(&mut self, at: u32, m: u32, w: u32) -> Result<Option<Effect>, PushError> {
        let f = f32::from_bits(w);
        Ok(Some(match m {
            method::NOP | method::WAIT_FOR_IDLE => Effect::None,
            method::BEGIN_END => {
                if w == 0 {
                    Effect::Draw(self.end(at)?)
                } else {
                    let p = PrimitiveType::from_xbox(w).ok_or(PushError::Immediate {
                        at,
                        what: "bad primitive type",
                    })?;
                    self.begin(p, at)?;
                    Effect::None
                }
            }
            method::INLINE_ARRAY => {
                self.inline_word(at, w)?;
                Effect::None
            }
            m if (method::VERTEX4F..method::VERTEX4F + 16).contains(&m) => {
                let c = ((m - method::VERTEX4F) / 4) as usize;
                self.set(at, 0, &[f], c, c == 3)?;
                Effect::None
            }
            m if (method::VERTEX_DATA2F..method::VERTEX_DATA2F + 0x80).contains(&m) => {
                let reg = ((m - method::VERTEX_DATA2F) / 8) as usize;
                let c = (((m - method::VERTEX_DATA2F) / 4) % 2) as usize;
                if c == 0 {
                    // A 2F write resets z and w.
                    self.regs[reg][2] = 0.0;
                    self.regs[reg][3] = 1.0;
                }
                self.set(at, reg, &[f], c, c == 1)?;
                Effect::None
            }
            m if (method::VERTEX_DATA2S..method::VERTEX_DATA2S + 0x40).contains(&m) => {
                let reg = ((m - method::VERTEX_DATA2S) / 4) as usize;
                let (x, y) = (w as u16 as i16 as f32, (w >> 16) as u16 as i16 as f32);
                self.set(at, reg, &[x, y, 0.0, 1.0], 0, true)?;
                Effect::None
            }
            m if (method::VERTEX_DATA4UB..method::VERTEX_DATA4UB + 0x40).contains(&m) => {
                let reg = ((m - method::VERTEX_DATA4UB) / 4) as usize;
                let b = w.to_le_bytes().map(|b| b as f32 / 255.0);
                self.set(at, reg, &b, 0, true)?;
                Effect::None
            }
            m if (method::VERTEX_DATA4S..method::VERTEX_DATA4S + 0x80).contains(&m) => {
                let reg = ((m - method::VERTEX_DATA4S) / 8) as usize;
                let c = (((m - method::VERTEX_DATA4S) / 4) % 2) as usize * 2;
                let (x, y) = (w as u16 as i16 as f32, (w >> 16) as u16 as i16 as f32);
                self.set(at, reg, &[x, y], c, c == 2)?;
                Effect::None
            }
            m if (method::VERTEX_DATA4F..method::VERTEX_DATA4F + 0x100).contains(&m) => {
                let reg = ((m - method::VERTEX_DATA4F) / 16) as usize;
                let c = (((m - method::VERTEX_DATA4F) / 4) % 4) as usize;
                self.set(at, reg, &[f], c, c == 3)?;
                Effect::None
            }
            _ => return Ok(None),
        }))
    }
}

/// Entries in `D3DSIMPLERENDERSTATEENCODE` (slots below `0x5C`).
pub const SIMPLE_ENCODE_SLOTS: usize = 92;

/// `D3DSIMPLERENDERSTATEENCODE`: the push header (`1 << 18 | method`) D3D
/// uses for guest render-state slot `i`. The game indexes it by slot below
/// `0x5C` in the inlined generic setter at `0x1828e`, and passes its
/// entries as the `ECX` of `SetRenderState_Simple`. Slots 0..56 are the
/// pixel-shader combiner registers, 57..83 the simple states.
///
/// The table is part of the XBE, so it is not copied into this crate: the
/// device reads it from the loaded image ([`GuestLayout::simple_encode`])
/// at `Direct3D_CreateDevice` and validates it.
///
/// [`GuestLayout::simple_encode`]: crate::GuestLayout::simple_encode
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimpleEncode {
    methods: [u32; SIMPLE_ENCODE_SLOTS],
    /// [`slot`](Self::slot) by `method / 4`, for every 4-aligned method
    /// below `0x2000` (the range `from_headers` accepts).
    by_method: Box<[Option<u8>]>,
}

impl SimpleEncode {
    /// Reads and validates the table at `addr`.
    pub fn read(mem: &dyn GuestMemory, addr: u32) -> Result<Self, SimpleEncodeError> {
        let mut headers = [0; SIMPLE_ENCODE_SLOTS];
        for (i, h) in headers.iter_mut().enumerate() {
            *h = mem
                .read_u32(addr.wrapping_add(4 * i as u32))
                .map_err(|e| SimpleEncodeError::Unreadable { addr, source: e })?;
        }
        Self::from_headers(&headers)
    }

    /// Validates `headers`: every entry must be a one-word, incrementing
    /// header for a 4-aligned method below `0x2000`, and none may be
    /// `0x030C`, which the device maps to slot 143 itself.
    pub fn from_headers(headers: &[u32; SIMPLE_ENCODE_SLOTS]) -> Result<Self, SimpleEncodeError> {
        let mut methods = [0; SIMPLE_ENCODE_SLOTS];
        for (slot, (&h, m)) in headers.iter().zip(methods.iter_mut()).enumerate() {
            let method = h & 0x3_FFFF;
            if h >> 18 != 1 || method == 0 || method & 3 != 0 || method >= 0x2000 {
                return Err(SimpleEncodeError::BadHeader { slot, header: h });
            }
            if method == DEPTH_TEST_ENABLE {
                return Err(SimpleEncodeError::BadHeader { slot, header: h });
            }
            *m = method;
        }
        if methods.iter().all(|&m| m == methods[0]) {
            return Err(SimpleEncodeError::Uniform { method: methods[0] });
        }
        let mut count = vec![0u8; 0x800];
        let mut by_method = vec![None; 0x800];
        for (slot, &m) in methods.iter().enumerate() {
            let i = (m / 4) as usize;
            count[i] = count[i].saturating_add(1);
            by_method[i] = Some(slot as u8);
        }
        for (b, c) in by_method.iter_mut().zip(&count) {
            if *c != 1 {
                *b = None;
            }
        }
        Ok(Self {
            methods,
            by_method: by_method.into_boxed_slice(),
        })
    }

    /// The method of slot `slot`.
    pub fn method(&self, slot: usize) -> Option<u32> {
        self.methods.get(slot).copied()
    }

    /// The only slot that encodes `m`, if exactly one does (a method
    /// several slots share, such as the `0x1D90` placeholder, is not
    /// invertible).
    pub fn slot(&self, m: u32) -> Option<u32> {
        if m & 3 != 0 {
            return None;
        }
        self.by_method
            .get((m / 4) as usize)
            .copied()
            .flatten()
            .map(u32::from)
    }

    /// The guest render-state slot whose value an inline write of method
    /// `m` carries, for the methods game code pushes itself or through
    /// `SetRenderState_Simple`: the payload *is* the slot value from that
    /// point in the push buffer on, whatever the array holds by the time it
    /// drains. `0x030C` (depth-test enable) maps to slot 143.
    pub fn mirror_slot(&self, m: u32) -> Option<u32> {
        if m == DEPTH_TEST_ENABLE {
            return Some(ZENABLE_SLOT);
        }
        self.slot(m)
    }
}

/// Why the encode table in guest memory was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SimpleEncodeError {
    /// The table could not be read.
    #[error("D3DSIMPLERENDERSTATEENCODE at {addr:#010x} is not readable: {source}")]
    Unreadable {
        /// Table address.
        addr: u32,
        /// The failed access.
        source: GuestError,
    },
    /// An entry is not a one-word state header.
    #[error(
        "D3DSIMPLERENDERSTATEENCODE[{slot}] = {header:#x} is not a one-word state method header"
    )]
    BadHeader {
        /// Slot.
        slot: usize,
        /// The word found.
        header: u32,
    },
    /// Every entry is the same method (not the table).
    #[error("D3DSIMPLERENDERSTATEENCODE holds only method {method:#x}")]
    Uniform {
        /// The method.
        method: u32,
    },
}

/// `NV097_SET_DEPTH_TEST_ENABLE`, which Halo's inlined ZEnable pushes with
/// the new `D3DRS_ZENABLE` value before storing it in slot 143 (`0x1eb66`
/// / `0x1eb8c` and 17 more sites, all the same shape).
pub const DEPTH_TEST_ENABLE: u32 = 0x030C;
/// Guest render-state slot of `D3DRS_ZENABLE`.
pub const ZENABLE_SLOT: u32 = 143;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::SparseMemory;

    fn header(method: u32, count: u32) -> u32 {
        (count << 18) | method
    }

    /// Reusing the word storage gives `read_words`' words and faults, for
    /// commands below and above the stack buffer's 64 words.
    #[test]
    fn decode_one_into_matches_read_words() {
        let mut m = SparseMemory::new();
        m.map(0x1000, 0x1000);
        let mut words = vec![0xAAAA_AAAA; 3];
        for count in [1u32, 5, 64, 65, 300] {
            m.write_u32(0x1000, header(method::INLINE_ARRAY, count) | 0x4000_0000)
                .unwrap();
            for i in 0..count {
                m.write_u32(0x1004 + 4 * i, i * 7 + count).unwrap();
            }
            let end = 0x1004 + 4 * count;
            let (next, call) = decode_one_into(&m, 0x1000, end, &mut words).unwrap();
            let call = call.unwrap();
            assert_eq!(next, end);
            assert_eq!(call.data, m.read_words(0x1004, count as usize).unwrap());
            words = call.data;
        }
        // A command running off mapped memory faults as `read_words` does.
        for count in [4u32, 100] {
            let at = 0x2000 - 8;
            m.write_u32(at, header(method::INLINE_ARRAY, count))
                .unwrap();
            let e = decode_one_into(&m, at, at + 4 + 4 * count, &mut words).unwrap_err();
            let PushError::Guest(e) = e else {
                panic!("{e:?}")
            };
            assert_eq!(e, m.read_words(at + 4, count as usize).unwrap_err());
        }
    }

    #[test]
    fn decode_splits_methods_and_rejects_control_words() {
        let mut m = SparseMemory::new();
        m.map(0x1000, 0x100);
        let words = [
            header(method::BEGIN_END, 1),
            6,
            header(method::VERTEX4F, 4),
            1,
            2,
            3,
            4,
            0,
            header(method::BEGIN_END, 1),
            0,
        ];
        for (i, w) in words.iter().enumerate() {
            m.write_u32(0x1000 + 4 * i as u32, *w).unwrap();
        }
        let calls = decode(&m, 0x1000, 0x1000 + 4 * words.len() as u32).unwrap();
        assert_eq!(calls.len(), 3); // the zero word is skipped
        assert_eq!(
            calls[1].writes().collect::<Vec<_>>(),
            vec![(0x1518, 1), (0x151C, 2), (0x1520, 3), (0x1524, 4)]
        );
        m.write_u32(0x1000, 0x2000_1000).unwrap();
        assert!(matches!(
            decode(&m, 0x1000, 0x1008),
            Err(PushError::Control { .. })
        ));
        m.write_u32(0x1000, header(method::VERTEX4F, 4)).unwrap();
        assert!(matches!(
            decode(&m, 0x1000, 0x1008),
            Err(PushError::Truncated { .. })
        ));
    }

    #[test]
    fn reserved_bits_and_short_ranges_are_rejected() {
        let mut m = SparseMemory::new();
        m.map(0x1000, 0x100);
        // Count 1, NOP, but bit 31 set: not a NOP.
        m.write_u32(0x1000, 0x8004_0100).unwrap();
        assert!(matches!(
            decode(&m, 0x1000, 0x1008),
            Err(PushError::Reserved { .. })
        ));
        m.write_u32(0x1000, header(method::NOP, 1) | 0x0001_0000)
            .unwrap();
        assert!(matches!(
            decode(&m, 0x1000, 0x1008),
            Err(PushError::Reserved { .. })
        ));
        // A range shorter than one word cannot underflow.
        m.write_u32(0x1000, header(method::NOP, 1)).unwrap();
        assert!(matches!(
            decode_one(&m, 0x1000, 0x1001),
            Err(PushError::Truncated { .. })
        ));
        assert!(matches!(
            decode_one(&m, 0x1000, 0x1004),
            Err(PushError::Truncated { .. })
        ));
    }

    #[test]
    fn immediate_mode_emits_on_position_and_keeps_registers() {
        let mut im = Immediate::default();
        let ap = |im: &mut Immediate, m, w: u32| im.apply(0, m, w).unwrap().unwrap();
        ap(&mut im, method::BEGIN_END, 5);
        // Diffuse via 4UB on register 3, then positions via VERTEX_DATA4F[0].
        ap(&mut im, method::VERTEX_DATA4UB + 3 * 4, 0xFF00_00FF); // r=1, a=1
        for v in [
            [0.0f32, 0.0, 0.5, 1.0],
            [1.0, 0.0, 0.5, 1.0],
            [0.0, 1.0, 0.5, 1.0],
        ] {
            for (c, x) in v.iter().enumerate() {
                ap(&mut im, method::VERTEX_DATA4F + 4 * c as u32, x.to_bits());
            }
        }
        let Effect::Draw(d) = ap(&mut im, method::BEGIN_END, 0) else {
            panic!("no draw")
        };
        assert_eq!(d.primitive, PrimitiveType::TriangleList);
        assert_eq!(d.vertices.len(), 3);
        assert_eq!(d.vertices[2][0], [0.0, 1.0, 0.5, 1.0]);
        assert_eq!(d.vertices[0][3], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(d.written, 0b1001);
        assert!(d.inline.is_empty());
        assert!(im.apply(0, 0x1234, 0).unwrap().is_none());
        assert!(
            im.apply(0, method::BEGIN_END, 0).is_err(),
            "End without Begin"
        );
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn inline_array_words_collect_between_begin_and_end() {
        let mut im = Immediate::default();
        assert!(matches!(
            im.apply(0x10, method::INLINE_ARRAY, 1),
            Err(PushError::Immediate { at: 0x10, .. })
        ));
        im.apply(0, method::BEGIN_END, 6).unwrap();
        for w in 1..=6 {
            im.apply(0, method::INLINE_ARRAY, w).unwrap();
        }
        let Some(Effect::Draw(d)) = im.apply(0, method::BEGIN_END, 0).unwrap() else {
            panic!("no draw")
        };
        assert_eq!(d.primitive, PrimitiveType::TriangleStrip);
        assert_eq!(d.inline, vec![1, 2, 3, 4, 5, 6]);
        assert!(d.vertices.is_empty());
        // The next primitive starts empty.
        im.apply(0, method::BEGIN_END, 5).unwrap();
        let Some(Effect::Draw(d)) = im.apply(0, method::BEGIN_END, 0).unwrap() else {
            panic!("no draw")
        };
        assert!(d.inline.is_empty());
    }

    #[test]
    fn inline_array_and_vertex_data_do_not_mix() {
        let mut im = Immediate::default();
        im.apply(0, method::BEGIN_END, 5).unwrap();
        im.apply(0, method::INLINE_ARRAY, 0).unwrap();
        im.apply(0, method::VERTEX4F + 12, 1.0f32.to_bits())
            .unwrap();
        assert!(matches!(
            im.apply(0x20, method::BEGIN_END, 0),
            Err(PushError::Immediate { at: 0x20, .. })
        ));
        assert!(!im.is_open());
    }
}

#[cfg(test)]
mod encode_tests {
    use super::*;
    use crate::guest::SparseMemory;

    /// A synthetic table: slot 1 encodes 0x304, slot 2 0x35C, the rest
    /// share 0x1D90.
    fn table() -> [u32; SIMPLE_ENCODE_SLOTS] {
        let mut t = [(1 << 18) | 0x1D90; SIMPLE_ENCODE_SLOTS];
        t[1] = (1 << 18) | 0x304;
        t[2] = (1 << 18) | 0x35C;
        t
    }

    #[test]
    fn a_valid_table_inverts_unique_methods_only() {
        let e = SimpleEncode::from_headers(&table()).unwrap();
        assert_eq!(e.method(1), Some(0x304));
        assert_eq!(e.mirror_slot(0x304), Some(1));
        assert_eq!(e.mirror_slot(0x35C), Some(2));
        assert_eq!(e.mirror_slot(0x1D90), None, "shared by many slots");
        assert_eq!(e.mirror_slot(0x0B00), None, "not in the table");
        assert_eq!(e.mirror_slot(DEPTH_TEST_ENABLE), Some(ZENABLE_SLOT));
    }

    #[test]
    fn malformed_tables_are_rejected() {
        let bad = |slot: usize, word: u32| {
            let mut t = table();
            t[slot] = word;
            SimpleEncode::from_headers(&t).unwrap_err()
        };
        for word in [
            (2 << 18) | 0x304,         // two words
            0x304,                     // no count
            (1 << 18) | 0x306,         // unaligned
            (1 << 18) | 0x2000,        // out of range
            (1 << 18) | (1 << 30) | 4, // non-incrementing / control bits
            1 << 18,                   // method 0
            (1 << 18) | DEPTH_TEST_ENABLE,
        ] {
            assert_eq!(
                bad(5, word),
                SimpleEncodeError::BadHeader {
                    slot: 5,
                    header: word
                }
            );
        }
        let uniform = [(1 << 18) | 0x1D90; SIMPLE_ENCODE_SLOTS];
        assert!(matches!(
            SimpleEncode::from_headers(&uniform),
            Err(SimpleEncodeError::Uniform { method: 0x1D90 })
        ));
    }

    #[test]
    fn read_reports_unmapped_tables() {
        let mut m = SparseMemory::new();
        m.map(0x1000, 0x1000);
        for (i, w) in table().iter().enumerate() {
            m.write_u32(0x1000 + 4 * i as u32, *w).unwrap();
        }
        assert_eq!(
            SimpleEncode::read(&m, 0x1000).unwrap().mirror_slot(0x35C),
            Some(2)
        );
        // Runs off the end of the mapping.
        assert!(matches!(
            SimpleEncode::read(&m, 0x1F00),
            Err(SimpleEncodeError::Unreadable { addr: 0x1F00, .. })
        ));
    }

    #[test]
    fn slot_lookup_matches_a_linear_search() {
        let mut headers = [0u32; SIMPLE_ENCODE_SLOTS];
        for (i, h) in headers.iter_mut().enumerate() {
            // Several slots share 0x0400; the rest are distinct.
            *h = (1 << 18)
                | if i % 7 == 0 {
                    0x400
                } else {
                    0x500 + 4 * i as u32
                };
        }
        let e = SimpleEncode::from_headers(&headers).unwrap();
        for m in 0..0x2100u32 {
            let hits: Vec<u32> = (0..SIMPLE_ENCODE_SLOTS as u32)
                .filter(|&s| e.method(s as usize) == Some(m))
                .collect();
            let want = (hits.len() == 1).then(|| hits[0]);
            assert_eq!(e.slot(m), want, "method {m:#x}");
        }
    }
}
