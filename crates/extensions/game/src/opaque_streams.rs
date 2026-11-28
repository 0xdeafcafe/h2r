//! State left by the BSP vertex program and stream binder (0x1caa0).
//! The declaration belongs to Halo's cache in guest RAM. The D3D calls also
//! own device state, including references to the old and new vertex buffers.

use std::cell::RefCell;
use std::sync::Arc;

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::GuestMemory;
use engine::render_loop::world_lightmap::{self, Binding};
use native_render::object_streams::{Attribute, STREAM_FORMAT_SIZE, STREAM_FORMATS, STREAM_TYPES};


const CACHE: u32 = 0x0051_f0f0;
const SLOT: u32 = CACHE + 0x80;
const PROGRAM_START: u32 = CACHE + 0x84;
const PROGRAM_DIRTY: u32 = CACHE + 0x88;
const FORMATS: u32 = CACHE + 0x8c;
const LAST_FORMATS: u32 = CACHE + 0xcc;
const VAF: u32 = CACHE + 0x10c;
const DECL_DIRTY: u32 = CACHE + 0x20c;
const INPUTS: u32 = CACHE + 0x210;
const COUNT: u32 = CACHE + 0x2d0;
const INPUTS_DIRTY: u32 = CACHE + 0x2d4;
const LIGHTING_TYPE: u8 = 0x30;
const STREAMS: usize = 16;
const VAF_BYTES: usize = STREAMS * 16;

#[derive(Clone, Copy)]
struct Stream {
    /// The descriptor is mutable (especially placement lightmap buckets).
    descriptor: u32,
    format: u32,
    offset_field: u32,
}

#[derive(Clone)]
pub struct Plan {
    lightmap: Option<Binding>,
    vertex_tag: u32,
    class: u32,
    function: u32,
    streams: Vec<Stream>,
    declaration: Arc<Declaration>,
    /// 0x1c8c0 still binds all inputs if a semantic is missing, then
    /// tells its caller to return before binding the material or drawing.
    pub drawable: bool,
}

fn word(mem: &dyn GuestMemory, at: u32, offset: u32) -> Option<u32> {
    mem.u32(at.checked_add(offset)?).ok()
}

fn pointer(mem: &dyn GuestMemory, at: u32, offset: u32) -> Option<u32> {
    word(mem, at, offset).filter(|&p| p != 0)
}

fn encode_word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn empty_declaration() -> [u8; VAF_BYTES] {
    let mut bytes = [0; VAF_BYTES];
    for record in bytes.chunks_exact_mut(16) {
        record[8] = 2; // D3DVSDT_NONE
    }
    bytes
}

/// Write into the VAF in stream order. Later attributes with the same
/// register win. The mask is in tag semantic space, before remapping to vN.
fn add_format(bytes: &[u8; STREAM_FORMAT_SIZE], slot: usize, registers: &[i8; 21],
    vaf: &mut [u8; VAF_BYTES]) -> Option<u32>
{
    let mut offset = 0_u32;
    let mut mask = 0;
    for p in bytes[1..].chunks_exact(2) {
        match p[0] as i8 {
            -2 => offset = offset.wrapping_add(i32::from(p[1] as i8) as u32),
            s if s < 0 => break,
            semantic => {
                let semantic = semantic as usize;
                let register = *registers.get(semantic)?;
                mask |= 1 << semantic;
                let attr = Attribute::from_code(p[1])?;
                if register >= 0 {
                    let base = register as usize * 16;
                    encode_word(vaf, base, slot as u32);
                    encode_word(vaf, base + 4, offset);
                    encode_word(vaf, base + 8, attr.xbox_type());
                }
                offset = offset.wrapping_add(attr.size() as u32);
            }
        }
    }
    Some(mask)
}

#[derive(PartialEq, Eq, Hash)]
struct DeclarationKey {
    registers: [i8; 21],
    formats: Vec<[u8; STREAM_FORMAT_SIZE]>,
}

struct Declaration {
    registers: [i8; 21],
    bytes: [u8; VAF_BYTES],
    streams: Vec<gfx::vertex::StreamLayout>,
    supplied: u32,
}

/// A class and its descriptor formats are reused by every instance and
/// frame. Key by format/semantic contents so address reuse after a map
/// change, snapshot load, or an editor change cannot restore stale inputs.
fn declaration(key: DeclarationKey) -> Result<Arc<Declaration>, &'static str> {
    thread_local! {
        static CACHE: RefCell<rustc_hash::FxHashMap<DeclarationKey, Arc<Declaration>>> = RefCell::default();
    }
    CACHE.with_borrow_mut(|cache| {
        if let Some(ready) = cache.get(&key) { return Ok(Arc::clone(ready)); }
        let mut bytes = empty_declaration();
        let mut supplied = 0;
        for (slot, format) in key.formats.iter().enumerate() {
            supplied |= add_format(format, slot, &key.registers, &mut bytes).ok_or("unsupported vertex declaration")?;
        }
        let vaf = to_words(&bytes);
        let streams = d3d::device::declaration_streams(&vaf, |_| 0, "native BSP declaration")
            .map_err(|_| "unsupported vertex declaration")?;
        let ready = Arc::new(Declaration { registers: key.registers, bytes, streams, supplied });
        if cache.len() >= 512 { cache.clear(); }
        cache.insert(key, Arc::clone(&ready));
        Ok(ready)
    })
}

fn to_words(bytes: &[u8; VAF_BYTES]) -> [u32; 64] {
    let words = bytes.as_chunks::<4>().0;
    std::array::from_fn(|i| u32::from_le_bytes(words[i]))
}

impl Plan {
    pub fn read(mem: &dyn GuestMemory, section: u32, geometry: u32, part: u32, implementation: u32,
        record: u32, layer: u32, shader_flags: u16) -> Result<Self, &'static str>
    {
        let vertex_tag = word(mem, implementation, 0x100).filter(|&v| v != u32::MAX)
            .ok_or("missing vertex program")?;
        let table = pointer(mem, engine::tags::TAG_ENTRIES_GLOBAL, 0).ok_or("tag table")?;
        let vertex = pointer(mem, table, (vertex_tag & 0xffff) * 16 + 8).ok_or("vertex tag")?;
        let kind = mem.u16(section.checked_add(0x14).ok_or("geometry section")?)
            .map_err(|_| "geometry classification")?;
        let count = word(mem, vertex, 4).ok_or("vertex classifications")?;
        let class = match kind {
            0..=2 => u32::from(kind),
            3 => {
                let part_kind = mem.u16(part).map_err(|_| "geometry part")?;
                let field = if matches!(part_kind, 4 | 5) { 0x11 } else { 0x10 };
                let influences = mem.u8(section.checked_add(field).ok_or("node influences")?)
                    .map_err(|_| "node influences")? as i8;
                let candidate = (2 + i32::from(influences)) as u32;
                if candidate < count { candidate } else { 0 }
            }
            _ => return Err("unsupported BSP vertex classification"),
        };
        if class >= count { return Err("missing BSP vertex classification"); }
        let descriptor = pointer(mem, vertex, 8).and_then(|p| p.checked_add(class.checked_mul(0x1c)?))
            .ok_or("vertex classification")?;
        let function = pointer(mem, descriptor, 0x10).ok_or("missing vertex program code")?;
        let n = word(mem, descriptor, 4).ok_or("vertex semantics")?;
        if n > STREAMS as u32 { return Err("vertex register count"); }
        let mut required = 0_u32;
        let mut registers = [-1; 21];
        if n != 0 {
            let semantics = pointer(mem, descriptor, 8).ok_or("vertex semantics")?;
            for i in 0..n {
                let semantic = mem.u16(semantics.checked_add(i * 2).ok_or("vertex semantics")?)
                    .map_err(|_| "vertex semantics")? as usize;
                *registers.get_mut(semantic).ok_or("vertex semantic outside declaration")? = i as i8;
                required |= 1 << semantic;
            }
        }
        let lightmap = world_lightmap::binding(mem, record, layer, shader_flags)?;
        let count = word(mem, geometry, 0x38).ok_or("geometry streams")? as i32;
        // 0x1c8c0 enters its section loop only with a positive signed count.
        let count = count.max(0) as usize;
        if count > STREAMS { return Err("too many vertex streams"); }
        let mut streams = Vec::with_capacity(count + 2);
        let mut formats = Vec::with_capacity(count + 2);
        let mut add = |descriptor: u32, kind: Option<u8>, offset_field| {
            descriptor.checked_add(0x13).ok_or("vertex descriptor address")?;
            if streams.len() >= STREAMS { return Err("too many vertex streams"); }
            let kind = match kind {
                Some(kind) => kind,
                None => mem.u8(descriptor).map_err(|_| "vertex descriptor format")?,
            };
            if usize::from(kind) >= STREAM_TYPES { return Err("unknown vertex stream format"); }
            let format = STREAM_FORMATS + u32::from(kind) * STREAM_FORMAT_SIZE as u32;
            let mut bytes = [0; STREAM_FORMAT_SIZE];
            mem.read(format, &mut bytes).map_err(|_| "vertex format table")?;
            formats.push(bytes);
            streams.push(Stream { descriptor, format, offset_field });
            Ok(())
        };
        if count != 0 {
            let base = pointer(mem, geometry, 0x3c).ok_or("geometry stream descriptors")?;
            for i in 0..count {
                add(base.checked_add(i as u32 * 0x20).ok_or("vertex descriptor")?, None, 8)?;
            }
        }
        if let Some(binding) = &lightmap {
            // 0x1c96b: coordinates use their own format. 0x1c9ce: the
            // lighting descriptor always uses the dedicated lighting format.
            if let Some(uv) = binding.streams[1] { add(uv.descriptor, None, 4)?; }
            if let Some(light) = binding.streams[0] { add(light.descriptor, Some(LIGHTING_TYPE), 4)?; }
        }
        // A program change resets an unfinished stream assembly. Otherwise
        // new descriptors append to its current cursor (0x1c620).
        if !streams.is_empty() && mem.u32(CACHE + 4) == Ok(vertex_tag) && mem.u32(CACHE + 8) == Ok(class) {
            let prefix = mem.u32(COUNT).map_err(|_| "vertex declaration cursor")?;
            if prefix > STREAMS as u32 - streams.len() as u32 { return Err("too many vertex streams"); }
        }
        let declaration = declaration(DeclarationKey { registers, formats })?;
        let drawable = required & !declaration.supplied == 0;
        Ok(Self { lightmap, vertex_tag, class, function, streams, declaration, drawable })
    }

    pub fn lightmap(&self) -> Option<Binding> {
        self.lightmap
    }

    /// Predict the same VAF and streams at section prepare. Publishing a
    /// declaration and binding its inputs are independently conditional; the
    /// submitter checks this projection against the live layout and sources.
    pub fn vertex_inputs(&self, mem: &dyn GuestMemory)
        -> Option<([u32; 64], gfx::vertex::VertexLayout, Vec<d3d::device::StreamSource>)>
    {
        let mut inputs = [d3d::device::StreamSource::default(); STREAMS];
        for (i, stream) in self.streams.iter().enumerate() {
            let desc = stream.descriptor;
            let projected = self.lightmap.and_then(|b| b.streams.into_iter().flatten()
                .find(|write| write.descriptor == desc));
            let offset = match projected {
                Some(write) => write.bytes,
                None => word(mem, desc, stream.offset_field)?,
            };
            inputs[i] = d3d::device::StreamSource {
                buffer: word(mem, desc, 0x10)?,
                stride: u32::from(mem.u8(desc.checked_add(1)?).ok()?),
                offset,
            };
        }
        let mut streams = self.declaration.streams.clone();
        for stream in &mut streams { stream.stride = inputs[stream.stream as usize].stride; }
        let sources = streams.iter().map(|s| inputs[s.stream as usize]).collect();
        let vaf = to_words(&self.declaration.bytes);
        Some((vaf, gfx::vertex::VertexLayout { streams, pretransformed: false }, sources))
    }

    /// Drive through the registered Xbox D3D entries so push, destruction
    /// and vblank callbacks complete before the next Halo cache mutation.
    pub async fn handoff(&self, cx: &Cx) -> Result<bool, EntryError> {
        let mut resume = Resume::Lightmap;
        while let Some(setter) = cx.write(|m| self.next(m, &mut resume))? {
            cx.call(setter.call()).await?;
        }
        Ok(self.drawable)
    }

    fn next(&self, mem: &mut dyn GuestMemory, resume: &mut Resume) -> Result<Option<Setter>, EntryError> {
        loop {
            match *resume {
                Resume::Lightmap => {
                    *resume = Resume::Program;
                    let Some(binding) = self.lightmap else { continue };
                    for stream in binding.streams.into_iter().flatten() {
                        mem.set_u32(stream.descriptor + 4, stream.bytes)?;
                        mem.set_u32(stream.descriptor + 8, stream.bytes)?;
                    }
                    let requested = i32::from(binding.palette);
                    if requested >= 0 {
                        let last = mem.i32(world_lightmap::LAST_PALETTE)?;
                        if requested != last {
                            let count = mem.i32(world_lightmap::PALETTE_COUNT)?;
                            if requested < count && let Some(header) = world_lightmap::palette_header(
                                requested, last, mem.i32(STAGE_PALETTE)?, count,
                            ) {
                                *resume = Resume::PaletteBound(binding);
                                return Ok(Some(Setter::Palette(header)));
                            }
                        }
                    }
                    mem.set_u32(world_lightmap::PAGE, binding.page as u32)?;
                }
                Resume::PaletteBound(binding) => {
                    mem.set_u32(STAGE_PALETTE, i32::from(binding.palette) as u32)?;
                    mem.set_u32(world_lightmap::LAST_PALETTE, i32::from(binding.palette) as u32)?;
                    mem.set_u32(world_lightmap::PAGE, binding.page as u32)?;
                    *resume = Resume::Program;
                }
                Resume::Program => {
                    *resume = Resume::Streams;
                    mem.set_u32(SLOT, 0)?;
                    if mem.u32(CACHE + 4)? != self.vertex_tag || mem.u32(CACHE + 8)? != self.class {
                        mem.set_u32(CACHE, 0)?;
                        mem.set_u32(CACHE + 4, self.vertex_tag)?;
                        mem.set_u32(CACHE + 8, self.class)?;
                        mem.set_u32(CACHE + 12, 0)?;
                        *resume = Resume::ProgramLoaded;
                        return Ok(Some(Setter::Program(self.function)));
                    }
                }
                Resume::ProgramLoaded => {
                    mem.set_u32(PROGRAM_START, 0)?;
                    mem.set_u8(PROGRAM_DIRTY, 1)?;
                    mem.write(LAST_FORMATS, &[0; STREAMS * 4])?;
                    mem.set_u8(DECL_DIRTY, 1)?;
                    mem.write(INPUTS, &[0; STREAMS * 12])?;
                    mem.set_u8(INPUTS_DIRTY, 1)?;
                    mem.set_u32(COUNT, 0)?;
                    *resume = Resume::Streams;
                }
                Resume::Streams => {
                    if self.streams.is_empty() { return Ok(None); }
                    if let Some(setter) = self.inputs(mem)? {
                        *resume = Resume::InputsBound;
                        return Ok(Some(setter));
                    }
                    *resume = Resume::Select(false);
                }
                Resume::InputsBound => {
                    mem.set_u8(INPUTS_DIRTY, 0)?;
                    *resume = Resume::Select(true);
                }
                Resume::Select(input_changed) => {
                    if mem.u8(PROGRAM_DIRTY)? != 0 || mem.u8(DECL_DIRTY)? != 0 {
                        *resume = Resume::Selected;
                        return Ok(Some(Setter::Select(mem.u32(PROGRAM_START)?)));
                    }
                    mem.set_u32(COUNT, 0)?;
                    if input_changed { mem.set_u8(DECL_DIRTY, 0)?; }
                    return Ok(None);
                }
                Resume::Selected => {
                    mem.set_u8(PROGRAM_DIRTY, 0)?;
                    mem.set_u32(COUNT, 0)?;
                    mem.set_u8(DECL_DIRTY, 0)?;
                    return Ok(None);
                }
            }
        }
    }

    fn inputs(&self, mem: &mut dyn GuestMemory) -> Result<Option<Setter>, EntryError> {
        let prefix = mem.u32(COUNT)? as usize;
        if prefix > STREAMS - self.streams.len() {
            return Err(EntryError::IsolatedCall("BSP declaration exceeds sixteen streams".into()));
        }
        let mut input_dirty = mem.u8(INPUTS_DIRTY)? != 0;
        let mut decl_dirty = mem.u8(DECL_DIRTY)? != 0;
        for (i, stream) in self.streams.iter().enumerate() {
            let i = prefix + i;
            // Descriptor offsets can alias input from an earlier placement.
            // Read them after applying that placement's lightmap writes.
            let desc = stream.descriptor;
            let input = [
                mem.u32(desc + 0x10)?,
                u32::from(mem.u8(desc + 1)?),
                mem.u32(desc + stream.offset_field)?,
            ];
            let slot = INPUTS + 12 * i as u32;
            let mut old = [0; 12];
            mem.read(slot, &mut old)?;
            let mut bytes = [0; 12];
            for (index, value) in input.into_iter().enumerate() { encode_word(&mut bytes, 4 * index, value); }
            if old != bytes {
                mem.write(slot, &bytes)?;
                input_dirty = true;
                mem.set_u8(INPUTS_DIRTY, 1)?;
            }
            mem.set_u32(FORMATS + 4 * i as u32, stream.format)?;
            decl_dirty |= mem.u32(LAST_FORMATS + 4 * i as u32)? != stream.format;
        }
        let count = (prefix + self.streams.len()) as u32;
        mem.set_u32(COUNT, count)?;
        if decl_dirty {
            if prefix == 0 {
                mem.write(VAF, &self.declaration.bytes)?;
                for (i, stream) in self.streams.iter().enumerate() {
                    mem.set_u32(LAST_FORMATS + 4 * i as u32, stream.format)?;
                    mem.set_u32(FORMATS + 4 * i as u32, 0)?;
                }
            } else {
                // 0x1c330 processes every pending format, including the
                // prefix built by the caller. Null format slots preserve
                // their previous entry in LAST_FORMATS.
                let mut vaf = empty_declaration();
                for i in 0..count {
                    let format = mem.u32(FORMATS + 4 * i)?;
                    if format == 0 { continue; }
                    let mut bytes = [0; STREAM_FORMAT_SIZE];
                    mem.read(format, &mut bytes)?;
                    add_format(&bytes, i as usize, &self.declaration.registers, &mut vaf)
                        .ok_or_else(|| EntryError::IsolatedCall("invalid pending vertex declaration".into()))?;
                    mem.set_u32(LAST_FORMATS + 4 * i, format)?;
                    mem.set_u32(FORMATS + 4 * i, 0)?;
                }
                mem.write(VAF, &vaf)?;
            }
            // 0x1c478 clears this *inside* the declaration builder. Rebuilding
            // the VAF alone does not make 0x1c710 call SetVertexShaderInput.
            mem.set_u8(DECL_DIRTY, 0)?;
        }
        if input_dirty {
            let shader = d3d::GuestLayout::HALO2.static_vertex_shader;
            let mut vaf = [0; VAF_BYTES];
            mem.read(VAF, &mut vaf)?;
            mem.write(shader + 0x14, &vaf)?;
            return Ok(Some(Setter::Inputs(shader | 1, count)));
        }
        Ok(None)
    }
}

const STAGE_PALETTE: u32 = world_lightmap::STAGE_PALETTES + 3 * 4;

#[derive(Clone, Copy)]
enum Resume {
    Lightmap,
    PaletteBound(Binding),
    Program,
    ProgramLoaded,
    Streams,
    InputsBound,
    Select(bool),
    Selected,
}

#[derive(Debug, PartialEq, Eq)]
enum Setter {
    Palette(u32),
    Program(u32),
    Inputs(u32, u32),
    Select(u32),
}

impl Setter {
    fn call(self) -> GuestCall {
        const SET_PALETTE: u32 = 0x003f_84d0;
        const LOAD_VERTEX_PROGRAM: u32 = 0x003f_6c60;
        const SET_VERTEX_INPUT: u32 = 0x003f_7440;
        const SELECT_VERTEX_DIRECT: u32 = 0x003f_6cf0;
        match self {
            Self::Palette(header) => GuestCall::new(SET_PALETTE).eax(3).args([header]),
            Self::Program(function) => GuestCall::new(LOAD_VERTEX_PROGRAM).args([function, 0]),
            Self::Inputs(shader, count) => GuestCall::new(SET_VERTEX_INPUT).args([shader, count, INPUTS]),
            Self::Select(start) => GuestCall::new(SELECT_VERTEX_DIRECT).eax(start).ecx(VAF),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod declaration_tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn callback_sees_original_pre_call_cache_and_can_affect_following_program_select() {
        let mut m = FlatMemory::new(0x0040_0000, 0x14_0000);
        let descriptor = CACHE + 0x800;
        let function = CACHE + 0x900;
        let plan = Plan {
            lightmap: Some(Binding { page: 22, palette: 1, streams: [None; 2] }),
            vertex_tag: 3, class: 1, function,
            streams: vec![Stream { descriptor, format: STREAM_FORMATS, offset_field: 8 }],
            declaration: declaration(DeclarationKey { registers: [-1; 21], formats: vec![] }).unwrap(),
            drawable: true,
        };
        m.set_u32(world_lightmap::PAGE, 11).unwrap();
        m.set_u32(world_lightmap::PALETTE_COUNT, 2).unwrap();
        let mut cursor = Resume::Lightmap;
        assert!(matches!(plan.next(&mut m, &mut cursor).unwrap(), Some(Setter::Palette(_))));
        assert_eq!(m.u32(world_lightmap::PAGE), Ok(11));
        assert_eq!(m.u32(STAGE_PALETTE), Ok(0));

        assert_eq!(plan.next(&mut m, &mut cursor).unwrap(), Some(Setter::Program(function)));
        assert_eq!(m.u32(world_lightmap::PAGE), Ok(22));
        assert_eq!(m.u32(STAGE_PALETTE), Ok(1));
        assert_eq!(m.u32(CACHE + 4), Ok(3));
        assert_eq!(m.u8(PROGRAM_DIRTY), Ok(0));

        // A load callback can update the vertex buffer before binding it.
        m.set_u32(descriptor + 0x10, 123).unwrap();
        assert!(matches!(plan.next(&mut m, &mut cursor).unwrap(), Some(Setter::Inputs(_, 1))));
        assert_eq!(m.u32(INPUTS), Ok(123));
        assert_eq!(m.u8(INPUTS_DIRTY), Ok(1));
        assert_eq!(m.u32(COUNT), Ok(1));

        // SetVertexShaderInput can drain a callback selecting another slot.
        m.set_u32(PROGRAM_START, 17).unwrap();
        assert_eq!(plan.next(&mut m, &mut cursor).unwrap(), Some(Setter::Select(17)));
        assert_eq!(m.u8(INPUTS_DIRTY), Ok(0));
        assert_eq!(m.u8(PROGRAM_DIRTY), Ok(1));
        assert_eq!(plan.next(&mut m, &mut cursor).unwrap(), None);
        assert_eq!(m.u8(PROGRAM_DIRTY), Ok(0));
        assert_eq!(m.u32(COUNT), Ok(0));
    }

    #[test]
    fn a_foreign_prefix_is_consumed_by_the_next_declaration() {
        let mut m = FlatMemory::new(0x0040_0000, 0x14_0000);
        let mut registers = [-1; 21];
        registers[2] = 3;
        registers[5] = 0;
        let mut prefix = [0; STREAM_FORMAT_SIZE];
        prefix[..4].copy_from_slice(&[8, 2, 2, 0xff]);
        let mut next = [0; STREAM_FORMAT_SIZE];
        next[..4].copy_from_slice(&[9, 5, 3, 0xff]);
        let incoming = STREAM_FORMATS + STREAM_FORMAT_SIZE as u32;
        m.write(STREAM_FORMATS, &prefix).unwrap();
        m.write(incoming, &next).unwrap();
        m.set_u32(COUNT, 1).unwrap();
        m.set_u32(FORMATS, STREAM_FORMATS).unwrap();
        let desc = CACHE + 0x700;
        m.set_u32(desc + 0x10, 99).unwrap();
        let plan = Plan {
            lightmap: None, vertex_tag: 0, class: 0, function: 0,
            streams: vec![Stream { descriptor: desc, format: incoming, offset_field: 8 }],
            declaration: declaration(DeclarationKey { registers, formats: vec![next] }).unwrap(),
            drawable: true,
        };
        let mut cursor = Resume::Lightmap;
        assert!(matches!(plan.next(&mut m, &mut cursor).unwrap(), Some(Setter::Inputs(_, 2))));
        assert_eq!(m.u32(COUNT), Ok(2));
        assert_eq!(m.u32(INPUTS + 12), Ok(99));
        assert_eq!(m.u32(VAF), Ok(1));
        assert_eq!(m.u32(VAF + 8), Ok(0x42));
        assert_eq!(m.u32(VAF + 3 * 16), Ok(0));
        assert_eq!(m.u32(VAF + 3 * 16 + 8), Ok(0x32));
        assert_eq!(m.u32(LAST_FORMATS), Ok(STREAM_FORMATS));
        assert_eq!(m.u32(LAST_FORMATS + 4), Ok(incoming));
        assert_eq!(plan.next(&mut m, &mut cursor).unwrap(), None);
        assert_eq!(m.u32(COUNT), Ok(0));
    }

    #[test]
    fn inactive_registers_and_overlapping_semantics_match_the_guest_vaf() {
        // Two synthetic formats. The leading attribute in the second format
        // is unconsumed; both it and the explicit padding advance the offset.
        let mut a = [0; STREAM_FORMAT_SIZE];
        a[..4].copy_from_slice(&[0, 2, 2, 0xff]);
        let mut b = [0; STREAM_FORMAT_SIZE];
        b[..8].copy_from_slice(&[1, 7, 3, 0xfe, 3, 2, 4, 0xff]);
        let mut registers = [-1; 21];
        registers[2] = 5;
        let mut actual = empty_declaration();
        assert_eq!(add_format(&a, 0, &registers, &mut actual), Some(1 << 2));
        assert_eq!(add_format(&b, 1, &registers, &mut actual), Some(1 << 2 | 1 << 7));
        let mut expected = [0; VAF_BYTES];
        for slot in expected.chunks_exact_mut(16) { slot[8] = 2; }
        let v5 = &mut expected[5 * 16..6 * 16];
        v5[0] = 1;
        v5[4] = 19;
        v5[8] = 0x14;
        assert_eq!(actual, expected);
    }
}
