//! The HUD's and the UI's screen draws as the native renderer sees them
//! (docs/graphics/native-renderer.md, "Stage 5: post and HUD", "The UI and
//! HUD"). Pure: what the hooks read out of guest memory, the plan a
//! native draw would take, and how the plan pairs with what the D3D path
//! drew. The GPU side is not here.
//!
//! Two primitives carry nearly everything the HUD and the menus draw
//! (Code, `h2 disasm`):
//!
//! - **Screen quads** (`0x00035b90` begin, `0x000363a0` one quad,
//!   `0x00036560` end). The begin takes one record ([`Record`],
//!   `RECORD_SIZE` bytes) and sets the state every quad of the batch draws
//!   with: up to three bitmaps, each stage's address mode and the filter,
//!   the blend by mode (`0x000142f0`), colour writes without alpha, depth
//!   off, and a fixed combiner it writes into the PSDEF block at
//!   [`COMBINER`] with the record's colours packed in. Each quad then pushes
//!   one inline `Begin(7)`/`End` of four vertices straight into the push
//!   buffer (no D3D entry), each vertex a diffuse colour, a coordinate and
//!   a position ([`Vertex`]). Text is drawn this way: `0x0001f070` (the
//!   string draw, from `0x0001fa50` and `0x0001fb10`) begins a batch over
//!   the glyph cache ([`GLYPH_BITMAP`]) and its glyph callback `0x0001f490`
//!   adds one quad per glyph (`0x0001f3a0`). The only other caller is a
//!   full-screen bitmap at `0x00156960`.
//! - **HUD shader quads** (`0x0022a664`). One shader draw (`0x0001bbf0`)
//!   with the HUD's bitmap named in two globals ([`HUD_BITMAP`]), then one
//!   `D3DDevice_Begin(7)`, four corners through `SetVertexData2f` (the
//!   coordinate) and `SetVertexData4f` (the position), and
//!   `D3DDevice_End` ([`HudQuad`]). The HUD widgets (`0x0022b...`), the UI's
//!   shader bitmaps (`0x002b12ae`) and others call it.

use crate::camera::GuestRead;

/// `0x00035b90`'s record: its size in bytes.
pub const RECORD_SIZE: u32 = 0x98;
/// The glyph cache's bitmap (a pointer; `0x0001eff0` makes it).
pub const GLYPH_BITMAP: u32 = 0x004b_62ac;
/// A word that, when not 0, makes the begin and each quad draw nothing.
pub const QUADS_OFF: u32 = 0x0048_5602;
/// The rectangle the begin's screen transform divides by (shorts: top,
/// left, bottom, right).
pub const SCREEN_RECT: u32 = 0x0048_5648;
/// The begin's scale constants for x and y (floats in the XBE's data),
/// and the depth it puts every quad at.
pub const SCALE_X: u32 = 0x0045_dc0c;
pub const SCALE_Y: u32 = 0x0045_df10;
pub const DEPTH: u32 = 0x0045_dbbc;
/// The combiner block the begin writes (`D3DPIXELSHADERDEF`, 60 words).
pub const COMBINER: u32 = 0x0048_4f68;
pub const COMBINER_WORDS: usize = 60;
/// `0x000142f0`'s tables, indexed by blend mode: source factor,
/// destination factor and equation (the NV2A's values). Mode
/// [`BLEND_OFF`] turns blending off and reads none.
pub const BLEND_SOURCE: u32 = 0x0045_0718;
pub const BLEND_DEST: u32 = 0x0045_06e8;
pub const BLEND_EQUATION: u32 = 0x0045_06b8;
pub const BLEND_OFF: u16 = 10;
/// Modes the tables hold (the next table starts 12 words on).
pub const BLEND_MODES: u16 = 12;
/// The push buffer's put pointer: `[[PUSH]]` is where the next word goes.
pub const PUSH: u32 = 0x0040_7488;
/// The HUD's bitmap for the shader draw: its datum and its index.
pub const HUD_BITMAP: u32 = 0x004e_6978;
/// Where `0x0022a664`'s `D3DDevice_End` returns to: a traced draw from
/// that entry with this return address is a HUD quad's.
pub const HUD_QUAD_END_RET: u32 = 0x0022_a854;

/// Reads a short of a word-addressed reader: `addr` even.
fn short(mem: &dyn GuestRead, addr: u32) -> Option<i16> {
    let w = mem.read_u32(addr & !3)?;
    Some((if addr & 2 == 0 { w } else { w >> 16 }) as u16 as i16)
}

fn byte(mem: &dyn GuestRead, addr: u32) -> Option<u8> {
    let w = mem.read_u32(addr & !3)?;
    Some((w >> (8 * (addr & 3))) as u8)
}

fn finite(mem: &dyn GuestRead, addr: u32) -> Option<f32> {
    mem.read_f32(addr).filter(|v| v.is_finite())
}

/// The fields of `0x00035b90`'s record this module reads (Code,
/// `0x00035b90..0x00036395`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Record {
    /// `+0x04`: a pointer to a pixel offset `(x, y)`, or 0.
    pub offset: u32,
    /// `+0x08..+0x0a`: three bytes that pick the begin's second constant
    /// block (S: per-stage coordinate flips).
    pub flips: [u8; 3],
    /// `+0x0c`, `+0x10`, `+0x14`: the bitmap at each stage, or 0. The
    /// begin binds stages up to the first 0; a batch with stage 0 empty
    /// keeps the caller's combiner.
    pub bitmaps: [u32; 3],
    /// `+0x18..+0x1a`: per stage, wrap (not 0) or clamp (0).
    pub wrap: [bool; 3],
    /// `+0x1c`, `+0x20`, `+0x24`: pointers to coordinate pairs for the
    /// second constant block, or 0.
    pub pairs: [u32; 3],
    /// `+0x40`, `+0x44`: the coordinate scale (`0x0001f070`: one over the
    /// bitmap's size, or 1 for a linear bitmap, which takes texels).
    pub coordinate_scale: [f32; 2],
    /// `+0x48..+0x54`: the second block's first row.
    pub second: [f32; 4],
    /// `+0x94`: the blend mode (`0x000142f0`).
    pub blend_mode: u16,
    /// `+0x96`: point filtering (not 0) or linear (0), every stage.
    pub point: bool,
    /// `+0x97`: keep the caller's state (the begin then sets none of it).
    pub keep_state: bool,
}

impl Record {
    pub fn read(mem: &dyn GuestRead, at: u32) -> Option<Record> {
        let word = |o: u32| mem.read_u32(at + o);
        let flips = word(0x08)?.to_le_bytes();
        let wrap = word(0x18)?.to_le_bytes();
        let tail = word(0x94)?;
        Some(Record {
            offset: word(0x04)?,
            flips: [flips[0], flips[1], flips[2]],
            bitmaps: [word(0x0c)?, word(0x10)?, word(0x14)?],
            wrap: [wrap[0] != 0, wrap[1] != 0, wrap[2] != 0],
            pairs: [word(0x1c)?, word(0x20)?, word(0x24)?],
            coordinate_scale: [mem.read_f32(at + 0x40)?, mem.read_f32(at + 0x44)?],
            second: [
                mem.read_f32(at + 0x48)?,
                mem.read_f32(at + 0x4c)?,
                mem.read_f32(at + 0x50)?,
                mem.read_f32(at + 0x54)?,
            ],
            blend_mode: tail as u16,
            point: (tail >> 16) as u8 != 0,
            keep_state: (tail >> 24) as u8 != 0,
        })
    }
}

/// Why a batch keeps its D3D draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refused {
    /// The record or a word it points at does not read.
    Unreadable,
    /// The begin keeps the caller's state (`+0x97`): what it draws with
    /// is not the record's.
    KeepsState,
    /// Stage 0 has no bitmap: the begin writes no combiner.
    NoBitmap,
    /// A second or third stage is bound: only one-stage batches so far.
    Stages,
    /// The second constant block is not the text's (flips, pairs or its
    /// first row set): what the vertex program does with it is not read.
    SecondBlock,
    /// A blend mode past the tables.
    BlendMode,
    /// The screen rectangle is empty.
    Rect,
    /// Not the glyph cache: other bitmaps are not uploaded yet.
    NotText,
}

impl Refused {
    pub const ALL: [Refused; 8] = [
        Refused::Unreadable,
        Refused::KeepsState,
        Refused::NoBitmap,
        Refused::Stages,
        Refused::SecondBlock,
        Refused::BlendMode,
        Refused::Rect,
        Refused::NotText,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&r| r == self).unwrap_or(0)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Refused::Unreadable => "the record does not read",
            Refused::KeepsState => "the batch keeps the caller's state",
            Refused::NoBitmap => "stage 0 has no bitmap",
            Refused::Stages => "more than one stage is bound",
            Refused::SecondBlock => "the second constant block is not the text's",
            Refused::BlendMode => "a blend mode past the tables",
            Refused::Rect => "the screen rectangle is empty",
            Refused::NotText => "stage 0 is not the glyph cache",
        }
    }
}

/// A blend as the NV2A takes it: off, or the equation and factors
/// (GL values, the same numbers as Xbox D3D's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Off,
    On {
        source: u32,
        dest: u32,
        equation: u32,
    },
}

impl Blend {
    /// Mode `mode` through `0x000142f0`'s tables; `None` past them.
    pub fn read(mem: &dyn GuestRead, mode: u16) -> Option<Blend> {
        if mode == BLEND_OFF {
            return Some(Blend::Off);
        }
        if mode >= BLEND_MODES {
            return None;
        }
        let at = 4 * u32::from(mode);
        Some(Blend::On {
            source: mem.read_u32(BLEND_SOURCE + at)?,
            dest: mem.read_u32(BLEND_DEST + at)?,
            equation: mem.read_u32(BLEND_EQUATION + at)?,
        })
    }
}

/// The begin's screen transform (Code, `0x00035cf0..0x00035e92`): rows of
/// a 4x4 the begin uploads as vertex constants, position `(x, y, 0, 1)` to
/// clip space. With the scales at -2 and 2 it is the pixel-centre mapping
/// `x' = (2x - 1) / w - 1`. S: that the batch's vertex program takes
/// `oPos` from these rows (the program is bound elsewhere and not read).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenTransform {
    pub rows: [[f32; 4]; 4],
}

impl ScreenTransform {
    /// From the screen rectangle, the record's offset pointer and the
    /// begin's constants, with the begin's own float operations.
    pub fn read(mem: &dyn GuestRead, record: &Record) -> Option<ScreenTransform> {
        let (top, left, bottom, right) = (
            short(mem, SCREEN_RECT)?,
            short(mem, SCREEN_RECT + 2)?,
            short(mem, SCREEN_RECT + 4)?,
            short(mem, SCREEN_RECT + 6)?,
        );
        let w = right.wrapping_sub(left);
        let h = bottom.wrapping_sub(top);
        if w <= 0 || h <= 0 {
            return None;
        }
        let (sx, sy, z) = (
            finite(mem, SCALE_X)?,
            finite(mem, SCALE_Y)?,
            finite(mem, DEPTH)?,
        );
        let offset = if record.offset == 0 {
            None
        } else {
            Some((finite(mem, record.offset)?, finite(mem, record.offset + 4)?))
        };
        Some(Self::of((f32::from(w), f32::from(h)), (sx, sy, z), offset))
    }

    /// The rows for a rectangle `w` x `h`, scales and depth `k`, and the
    /// pixel offset.
    pub fn of(
        (w, h): (f32, f32),
        (sx, sy, z): (f32, f32, f32),
        offset: Option<(f32, f32)>,
    ) -> ScreenTransform {
        let (ox, oy) = match offset {
            Some((x, y)) => (x * sx / w, y * sy / h),
            None => (0.0, 0.0),
        };
        let (iw, ih) = (1.0 / w, 1.0 / h);
        ScreenTransform {
            rows: [
                [iw * sx, 0.0, 0.0, ox - (iw + 1.0)],
                [0.0, ih * sy, 0.0, ih + oy + 1.0],
                [0.0, 0.0, 0.0, z],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// `(x, y)` in clip space.
    pub fn apply(&self, x: f32, y: f32) -> [f32; 4] {
        let r = &self.rows;
        std::array::from_fn(|i| r[i][0] * x + r[i][1] * y + r[i][3])
    }
}

/// A bitmap the batch samples: its size and whether it is linear (its
/// coordinates are texels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub at: u32,
    pub width: u16,
    pub height: u16,
    pub linear: bool,
    /// `+0x50`: its D3D texture header.
    pub header: u32,
}

impl Bitmap {
    /// The bitmap at `at` (`+0x04`/`+0x06` its size, bit 4 of the flags at
    /// `+0x0e` linear, `+0x50` the header); `None` unreadable or empty.
    pub fn read(mem: &dyn GuestRead, at: u32) -> Option<Bitmap> {
        let (w, h) = (short(mem, at + 4)?, short(mem, at + 6)?);
        let flags = short(mem, at + 0xe)? as u16;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(Bitmap {
            at,
            width: w as u16,
            height: h as u16,
            linear: flags & 0x10 != 0,
            header: mem.read_u32(at + 0x50)?,
        })
    }
}

/// A batch of screen quads the native renderer can draw: the state its
/// begin sets from the record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Batch {
    pub record: u32,
    pub bitmap: Bitmap,
    pub wrap: bool,
    pub point: bool,
    pub blend: Blend,
    pub transform: ScreenTransform,
    pub coordinate_scale: [f32; 2],
}

impl Batch {
    /// The batch the record at `record` begins, or why it stays on D3D.
    pub fn read(mem: &dyn GuestRead, record: u32) -> Result<Batch, Refused> {
        let r = Record::read(mem, record).ok_or(Refused::Unreadable)?;
        if r.keep_state {
            return Err(Refused::KeepsState);
        }
        if r.bitmaps[0] == 0 {
            return Err(Refused::NoBitmap);
        }
        if r.bitmaps[1] != 0 || r.bitmaps[2] != 0 {
            return Err(Refused::Stages);
        }
        if r.flips != [0; 3] || r.pairs != [0; 3] || r.second != [0.0; 4] {
            return Err(Refused::SecondBlock);
        }
        let blend = Blend::read(mem, r.blend_mode).ok_or(Refused::BlendMode)?;
        let transform = ScreenTransform::read(mem, &r).ok_or(Refused::Rect)?;
        let glyphs = mem.read_u32(GLYPH_BITMAP).ok_or(Refused::Unreadable)?;
        if r.bitmaps[0] != glyphs {
            return Err(Refused::NotText);
        }
        let bitmap = Bitmap::read(mem, r.bitmaps[0]).ok_or(Refused::Unreadable)?;
        Ok(Batch {
            record,
            bitmap,
            wrap: r.wrap[0],
            point: r.point,
            blend,
            transform,
            coordinate_scale: r.coordinate_scale,
        })
    }
}

/// One corner of a screen quad (`0x000363a0`'s argument: four of these,
/// `0x14` bytes each).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    /// `0xAARRGGBB`; `0x000363a0` pushes it with red and blue swapped
    /// (`SET_VERTEX_DATA4UB`, bytes R, G, B, A).
    pub argb: u32,
}

/// The four corners at `at`, in the order they are pushed.
pub fn read_quad(mem: &dyn GuestRead, at: u32) -> Option<[Vertex; 4]> {
    let mut out = [Vertex {
        x: 0.0,
        y: 0.0,
        u: 0.0,
        v: 0.0,
        argb: 0,
    }; 4];
    for (i, v) in out.iter_mut().enumerate() {
        let p = at + 0x14 * i as u32;
        *v = Vertex {
            x: mem.read_f32(p)?,
            y: mem.read_f32(p + 4)?,
            u: mem.read_f32(p + 8)?,
            v: mem.read_f32(p + 0xc)?,
            argb: mem.read_u32(p + 0x10)?,
        };
    }
    Some(out)
}

/// Where the next push-buffer word goes (`[[PUSH]]`).
pub fn push_put(mem: &dyn GuestRead) -> Option<u32> {
    mem.read_u32(mem.read_u32(PUSH)?)
}

/// One call of `0x0022a664` (Code, `0x0022a664..0x0022a858`): ECX the
/// four corners `(x, y)`, EAX the coordinate rectangle, stack `(bitmap
/// datum, bitmap index, shader)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudQuad {
    pub corners: [[f32; 2]; 4],
    /// `(u0, u1, v0, v1)`: corner 0 takes `(u0, v0)`, 1 `(u1, v0)`, 2
    /// `(u1, v1)`, 3 `(u0, v1)`, each times the bitmap's size when it is
    /// linear.
    pub rect: [f32; 4],
    pub bitmap: u32,
    pub index: u32,
    /// S: the shader the draw (`0x0001bbf0`) takes in EDI.
    pub shader: u32,
}

impl HudQuad {
    pub fn read(mem: &dyn GuestRead, corners: u32, rect: u32, args: [u32; 3]) -> Option<HudQuad> {
        let f = |a: u32| mem.read_f32(a);
        Some(HudQuad {
            corners: [
                [f(corners)?, f(corners + 4)?],
                [f(corners + 8)?, f(corners + 0xc)?],
                [f(corners + 0x10)?, f(corners + 0x14)?],
                [f(corners + 0x18)?, f(corners + 0x1c)?],
            ],
            rect: [f(rect)?, f(rect + 4)?, f(rect + 8)?, f(rect + 0xc)?],
            bitmap: args[0],
            index: args[1],
            shader: args[2],
        })
    }

    /// Each corner's coordinate before the bitmap's size.
    pub fn coordinates(&self) -> [[f32; 2]; 4] {
        let [u0, u1, v0, v1] = self.rect;
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
    }
}

/// The glyph cache's `LIN_A4R4G4B4` texels as RGBA8 (`pitch` bytes a row).
pub fn a4r4g4b4_to_rgba(
    texels: &[u8],
    width: usize,
    height: usize,
    pitch: usize,
) -> Option<Vec<u8>> {
    if pitch < 2 * width || texels.len() < pitch * height.saturating_sub(1) + 2 * width {
        return None;
    }
    let e = |n: u16| ((n & 0xf) * 17) as u8;
    let mut out = Vec::with_capacity(4 * width * height);
    for y in 0..height {
        let row = &texels[y * pitch..y * pitch + 2 * width];
        for t in row.chunks_exact(2) {
            let v = u16::from_le_bytes([t[0], t[1]]);
            out.extend_from_slice(&[e(v >> 8), e(v >> 4), e(v), e(v >> 12)]);
        }
    }
    Some(out)
}

/// What the plan says a hooked call draws, in the order the game asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Planned {
    /// A screen quad, its words pushed from `put` on; `native`: the hook
    /// answered in its place, so the D3D path must draw nothing there.
    Screen { put: u32, native: bool },
    /// A HUD shader quad (`0x0022a664`): one draw through `D3DDevice_End`.
    Hud,
}

/// A draw the D3D path recorded, as the pairing needs it
/// (`d3d::TracedDraw`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Pushed (the push-buffer address of its `End`), or through an
    /// entry (`None`).
    pub push: Option<u32>,
    /// The entry's return address, for a draw through an entry.
    pub ret: Option<u32>,
    /// `Debug` of its primitive.
    pub primitive: String,
    pub vertices: usize,
    pub inline: usize,
    pub target: u32,
}

impl Seen {
    fn is_quad(&self) -> bool {
        self.primitive == "TriangleFan" && self.vertices == 4 && self.inline == 0
    }
}

/// How a window's plan paired with the draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pairing {
    /// Screen quads planned for D3D and drawn there as one fan of four.
    pub screen_paired: usize,
    /// Screen quads answered natively and absent from the D3D path.
    pub screen_native: usize,
    pub hud_paired: usize,
    /// Lines for what did not pair, the first few.
    pub mismatches: Vec<String>,
    /// Planned quads whose draw was not seen in the window (not drained
    /// yet when it was taken).
    pub unseen: usize,
}

/// Lines a pairing keeps.
pub const MISMATCH_LINES: usize = 8;

/// Pairs `planned` with `seen`. A screen quad's draws are those whose
/// `End` lies from its `put` up to the next screen quad's (`end` after the
/// last); a HUD quad's is the next draw through `D3DDevice_End` returning
/// to [`HUD_QUAD_END_RET`].
pub fn pair(planned: &[Planned], seen: &[Seen], end: Option<u32>) -> Pairing {
    fn miss(out: &mut Pairing, line: String) {
        if out.mismatches.len() < MISMATCH_LINES {
            out.mismatches.push(line);
        }
    }
    let mut out = Pairing::default();
    let puts: Vec<u32> = planned
        .iter()
        .filter_map(|p| match p {
            Planned::Screen { put, .. } => Some(*put),
            Planned::Hud => None,
        })
        .collect();
    let mut screen = 0;
    let mut huds = seen
        .iter()
        .filter(|s| s.push.is_none() && s.ret == Some(HUD_QUAD_END_RET));
    for (i, p) in planned.iter().enumerate() {
        match *p {
            Planned::Screen { put, native } => {
                let next = puts.get(screen + 1).copied().or(end);
                screen += 1;
                let mine: Vec<&Seen> = seen
                    .iter()
                    .filter(|s| {
                        s.push
                            .is_some_and(|e| e >= put && next.is_none_or(|n| e < n))
                    })
                    .collect();
                match (native, mine.as_slice()) {
                    (true, []) => out.screen_native += 1,
                    (false, []) => out.unseen += 1,
                    (false, [d]) if d.is_quad() => out.screen_paired += 1,
                    (_, ds) => miss(
                        &mut out,
                        format!(
                            "#{i} screen quad at {put:#010x}{}: {}",
                            if native { " (native)" } else { "" },
                            ds.iter()
                                .map(|&d| describe(d))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ),
                }
            }
            Planned::Hud => match huds.next() {
                None => out.unseen += 1,
                Some(d) if d.is_quad() => out.hud_paired += 1,
                Some(d) => miss(&mut out, format!("#{i} HUD quad: {}", describe(d))),
            },
        }
    }
    out
}

fn describe(d: &Seen) -> String {
    let inline = if d.inline > 0 {
        format!(", an inline array of {} words", d.inline)
    } else {
        String::new()
    };
    format!(
        "{} x{}{inline} into {:#010x}",
        d.primitive, d.vertices, d.target
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const RECORD: u32 = 0x0070_0000;
    const GLYPHS: u32 = 0x0071_0000;

    fn text(m: &mut Mem) {
        for o in (0..RECORD_SIZE).step_by(4) {
            m.w(RECORD + o, 0);
        }
        m.w(RECORD + 0x0c, GLYPHS);
        m.f(RECORD + 0x28, 1.0);
        m.f(RECORD + 0x2c, 1.0);
        m.f(RECORD + 0x40, 1.0);
        m.f(RECORD + 0x44, 1.0);
        m.w(GLYPH_BITMAP, GLYPHS);
        m.w(GLYPHS + 4, 256 | 256 << 16);
        m.w(GLYPHS + 0xc, 0x0010 << 16);
        m.w(GLYPHS + 0x50, 0x0072_0000);
        m.w(SCREEN_RECT, 0); // top 0, left 0
        m.w(SCREEN_RECT + 4, 480 | 640 << 16); // bottom 480, right 640
        m.f(SCALE_X, 2.0);
        m.f(SCALE_Y, -2.0);
        m.f(DEPTH, 0.5);
        for mode in 0..u32::from(BLEND_MODES) {
            m.w(BLEND_SOURCE + 4 * mode, 0x302);
            m.w(BLEND_DEST + 4 * mode, 0x303);
            m.w(BLEND_EQUATION + 4 * mode, 0x8006);
        }
    }

    #[test]
    fn a_text_record_is_a_batch_over_the_glyph_cache() {
        let mut m = Mem::default();
        text(&mut m);
        let b = Batch::read(&m, RECORD).unwrap();
        assert_eq!(
            b.bitmap,
            Bitmap {
                at: GLYPHS,
                width: 256,
                height: 256,
                linear: true,
                header: 0x0072_0000
            }
        );
        assert_eq!(
            b.blend,
            Blend::On {
                source: 0x302,
                dest: 0x303,
                equation: 0x8006
            }
        );
        assert!(!b.wrap && !b.point);
        assert_eq!(b.coordinate_scale, [1.0, 1.0]);
    }

    #[test]
    fn each_record_that_is_not_text_is_refused_for_its_reason() {
        let mut m = Mem::default();
        let refused = |m: &Mem| Batch::read(m, RECORD).unwrap_err();
        text(&mut m);
        m.w(RECORD + 0x94, 1 << 24);
        assert_eq!(refused(&m), Refused::KeepsState);
        text(&mut m);
        m.w(RECORD + 0x0c, 0);
        assert_eq!(refused(&m), Refused::NoBitmap);
        text(&mut m);
        m.w(RECORD + 0x10, GLYPHS);
        assert_eq!(refused(&m), Refused::Stages);
        text(&mut m);
        m.w(RECORD + 0x08, 1);
        assert_eq!(refused(&m), Refused::SecondBlock);
        text(&mut m);
        m.f(RECORD + 0x50, 1.0);
        assert_eq!(refused(&m), Refused::SecondBlock);
        text(&mut m);
        m.w(RECORD + 0x94, u32::from(BLEND_MODES));
        assert_eq!(refused(&m), Refused::BlendMode);
        text(&mut m);
        m.w(SCREEN_RECT + 4, 0);
        assert_eq!(refused(&m), Refused::Rect);
        text(&mut m);
        m.w(GLYPH_BITMAP, 0x0099_0000);
        assert_eq!(refused(&m), Refused::NotText);
        text(&mut m);
        m.0.remove(&(RECORD + 0x40));
        assert_eq!(refused(&m), Refused::Unreadable);
    }

    #[test]
    fn blend_mode_ten_is_off_and_reads_no_table() {
        let m = Mem::default();
        assert_eq!(Blend::read(&m, BLEND_OFF), Some(Blend::Off));
        assert_eq!(Blend::read(&m, 0), None, "the tables do not read");
    }

    #[test]
    fn the_record_reads_its_bytes_by_offset() {
        let mut m = Mem::default();
        text(&mut m);
        m.w(RECORD + 0x18, 0x0000_0100);
        m.w(RECORD + 0x94, 0x0001_0003);
        let r = Record::read(&m, RECORD).unwrap();
        assert_eq!(r.wrap, [false, true, false]);
        assert_eq!(r.blend_mode, 3);
        assert!(r.point && !r.keep_state);
    }

    #[test]
    fn the_transform_maps_pixel_edges_to_clip_space_by_the_begins_arithmetic() {
        let t = ScreenTransform::of((640.0, 480.0), (2.0, -2.0, 0.5), None);
        // A pixel coordinate x lands at (2x - 1) / w - 1.
        let p = t.apply(0.5, 0.5);
        assert!(
            (p[0] + 1.0).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6,
            "{p:?}"
        );
        let q = t.apply(640.5, 480.5);
        assert!(
            (q[0] - 1.0).abs() < 1e-6 && (q[1] + 1.0).abs() < 1e-6,
            "{q:?}"
        );
        assert_eq!((p[2], p[3]), (0.5, 1.0));
        let o = ScreenTransform::of((640.0, 480.0), (2.0, -2.0, 0.5), Some((10.0, 20.0)));
        let a = o.apply(0.5, 0.5);
        assert!((a[0] - (-1.0 + 20.0 / 640.0)).abs() < 1e-6, "{a:?}");
        assert!((a[1] - (1.0 - 40.0 / 480.0)).abs() < 1e-6, "{a:?}");
    }

    #[test]
    fn a_quad_is_four_corners_of_five_words() {
        let mut m = Mem::default();
        for i in 0..4u32 {
            let p = 0x0080_0000 + 0x14 * i;
            m.f(p, i as f32);
            m.f(p + 4, 10.0 + i as f32);
            m.f(p + 8, 20.0 + i as f32);
            m.f(p + 0xc, 30.0 + i as f32);
            m.w(p + 0x10, 0xff00_0000 | i);
        }
        let q = read_quad(&m, 0x0080_0000).unwrap();
        assert_eq!(
            q[3],
            Vertex {
                x: 3.0,
                y: 13.0,
                u: 23.0,
                v: 33.0,
                argb: 0xff00_0003
            }
        );
        m.0.remove(&(0x0080_0000 + 0x14 * 3 + 0x10));
        assert_eq!(read_quad(&m, 0x0080_0000), None);
    }

    #[test]
    fn a_hud_quad_takes_its_rectangle_corner_by_corner() {
        let mut m = Mem::default();
        for i in 0..8u32 {
            m.f(0x0081_0000 + 4 * i, i as f32);
        }
        for (i, v) in [0.0f32, 1.0, 0.25, 0.75].into_iter().enumerate() {
            m.f(0x0082_0000 + 4 * i as u32, v);
        }
        let q = HudQuad::read(&m, 0x0081_0000, 0x0082_0000, [0xe001_0002, 3, 0xe003_0004]).unwrap();
        assert_eq!(q.corners[2], [4.0, 5.0]);
        assert_eq!(
            q.coordinates(),
            [[0.0, 0.25], [1.0, 0.25], [1.0, 0.75], [0.0, 0.75]]
        );
        assert_eq!((q.bitmap, q.index, q.shader), (0xe001_0002, 3, 0xe003_0004));
    }

    #[test]
    fn glyph_texels_widen_by_nibble() {
        // One row of two texels, a pitch of six bytes, then the second row.
        let texels = [0x21, 0x84, 0xf0, 0xf0, 0, 0, 0xff, 0xff, 0x00, 0x00];
        let rgba = a4r4g4b4_to_rgba(&texels, 2, 2, 6).unwrap();
        assert_eq!(
            &rgba[..8],
            &[0x44, 0x22, 0x11, 0x88, 0x00, 0xff, 0x00, 0xff]
        );
        assert_eq!(&rgba[8..], &[0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]);
        assert_eq!(a4r4g4b4_to_rgba(&texels, 2, 3, 6), None, "short");
        assert_eq!(
            a4r4g4b4_to_rgba(&texels, 4, 1, 6),
            None,
            "pitch below the row"
        );
    }

    fn pushed(end: u32) -> Seen {
        Seen {
            push: Some(end),
            ret: None,
            primitive: "TriangleFan".into(),
            vertices: 4,
            inline: 0,
            target: 1,
        }
    }

    #[test]
    fn screen_quads_pair_with_the_draw_whose_end_follows_their_put() {
        let planned = [
            Planned::Screen {
                put: 0x100,
                native: false,
            },
            Planned::Screen {
                put: 0x180,
                native: false,
            },
            Planned::Screen {
                put: 0x200,
                native: true,
            },
        ];
        let seen = [pushed(0x170), pushed(0x1f0)];
        let p = pair(&planned, &seen, Some(0x280));
        assert_eq!(
            (p.screen_paired, p.screen_native, p.unseen),
            (2, 1, 0),
            "{p:?}"
        );
        assert!(p.mismatches.is_empty());
        // A draw where a native quad was: it was not answered after all.
        let p = pair(
            &planned,
            &[pushed(0x170), pushed(0x1f0), pushed(0x240)],
            Some(0x280),
        );
        assert_eq!(p.screen_native, 0);
        assert_eq!(p.mismatches.len(), 1, "{p:?}");
        // Two draws in one quad's range, and one not drained.
        let p = pair(&planned[..2], &[pushed(0x150), pushed(0x170)], Some(0x200));
        assert_eq!(
            (p.screen_paired, p.unseen, p.mismatches.len()),
            (0, 1, 1),
            "{p:?}"
        );
    }

    #[test]
    fn a_hud_quad_pairs_with_the_next_end_from_its_return() {
        let hud = |ret: u32, vertices: usize| Seen {
            push: None,
            ret: Some(ret),
            primitive: "TriangleFan".into(),
            vertices,
            inline: 0,
            target: 1,
        };
        let planned = [Planned::Hud, Planned::Hud, Planned::Hud];
        let seen = [
            hud(0x0002_0000, 4),
            hud(HUD_QUAD_END_RET, 4),
            hud(HUD_QUAD_END_RET, 3),
        ];
        let p = pair(&planned, &seen, None);
        assert_eq!(
            (p.hud_paired, p.unseen, p.mismatches.len()),
            (1, 1, 1),
            "{p:?}"
        );
    }
}
