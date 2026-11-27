//! A native inline draw whose material its producer supplies
//! ([`SuppliedState`]) instead of the device's bound state: the draw a
//! cut material setup leaves to Rust (docs/graphics/retire-d3d-r2.md,
//! "The cut"). The supplied words, program, combiner, constants and
//! textures stand over the device's for this draw only; the device's own
//! state (the guest arrays, constants, bindings) is as before after it,
//! which is what the guest's later draws expect when the setup did not
//! run. What the material does not set is the device's, as the guest's
//! setup leaves it.
use super::*;
use crate::layout::{STAGE_SLOTS, STAGES};

/// A draw's material, from its producer.
#[derive(Clone, Debug, PartialEq)]
pub struct SuppliedState {
    /// Render words `(slot, value)` over the device's, in order.
    pub render: Vec<(u32, u32)>,
    /// Stage words `(stage, slot, value)` over the device's.
    pub stages: Vec<(u32, u32, u32)>,
    /// The combiner definition; its first 57 words are also the draw's
    /// render slots 0..56, as SetPixelShader copies them.
    pub pixel_shader: [u32; 60],
    /// Constant registers `(hardware slot, value, component mask)`.
    pub constants: Vec<(usize, [f32; 4], u8)>,
    /// The vertex program's microcode (16-byte instructions, no header),
    /// at slot 0: the rasterizer loads its programs there
    /// (`LoadVertexShader(program, 0)` at 0x1C5C7).
    pub microcode: Vec<u32>,
    /// Texture headers per stage (0 unbinds); `None` keeps the device's
    /// binding.
    pub textures: [Option<u32>; 4],
    /// The inline array formats (`NV097_SET_VERTEX_DATA_ARRAY_FORMAT`
    /// per register); `None` takes the device's.
    pub vertex_format: Option<[u32; 16]>,
}

impl Default for SuppliedState {
    fn default() -> Self {
        Self {
            render: Vec::new(),
            stages: Vec::new(),
            pixel_shader: [0; 60],
            constants: Vec::new(),
            microcode: Vec::new(),
            textures: [None; 4],
            vertex_format: None,
        }
    }
}

/// Why a supplied draw is not drawn: `<kind>_material_<name>`.
pub fn refusal_name(r: NativeRefusal) -> &'static str {
    match r {
        NativeRefusal::Boundary => "boundary",
        NativeRefusal::Geometry => "geometry",
        NativeRefusal::Admission => "admission",
        NativeRefusal::Binding => "binding",
        NativeRefusal::Resources => "resources",
        NativeRefusal::Sink => "sink",
        NativeRefusal::Backend => "backend",
    }
}

impl<B: Backend> Device<B> {
    /// Whether a draw from `s` alone can be made, whatever its geometry:
    /// its program, its slots and its textures (none palettized, which
    /// would take the device's palette). `Err` names the first that
    /// cannot. The game asks this before it cuts a setup, so a refused
    /// material keeps the guest's.
    pub fn check_supplied(&self, mem: &dyn GuestMemory, s: &SuppliedState) -> Result<(), &'static str> {
        if s.microcode.is_empty() || !s.microcode.len().is_multiple_of(4) || s.microcode.len() > 4 * PROGRAM_SLOTS {
            return Err("program");
        }
        if s.render.iter().any(|&(slot, _)| slot as usize >= RENDER_STATE_SLOTS) {
            return Err("render_slot");
        }
        if s.stages.iter().any(|&(stage, slot, _)| stage as usize >= STAGES || slot as usize >= STAGE_SLOTS) {
            return Err("stage_slot");
        }
        if s.constants.iter().any(|&(slot, ..)| slot >= CONSTANTS) {
            return Err("constant_slot");
        }
        if let Some(formats) = &s.vertex_format
            && inline_array_layout(formats).is_err()
        {
            return Err("vertex_format");
        }
        for &header in s.textures.iter().flatten().filter(|&&h| h != 0) {
            let Ok(pc) = PixelContainer::read(mem, header) else { return Err("texture_unread") };
            match crate::texdesc::from_header(pc.format, pc.size) {
                Ok(desc) if desc.format == D3DFormat::P8 => return Err("palette"),
                Ok(_) => {}
                Err(_) => return Err("texture_format"),
            }
        }
        Ok(())
    }

    /// Records `data` (inline vertices) with `s` as the draw's material.
    /// `Ok(Err(name))` draws nothing and leaves the device as it was;
    /// `Ok(Ok(()))` leaves it as it was but for the carried attributes,
    /// which the guest's emitter would have written too.
    pub fn try_native_supplied_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        data: Vec<u8>,
        carried: &[Option<[f32; 4]>; 16],
        s: &SuppliedState,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<Result<(), &'static str>, D3dError> {
        const ENTRY: &str = "supplied Begin/End";
        if !self.native_producer_ready(mem)? {
            return Ok(Err("boundary"));
        }
        let formats = s.vertex_format.unwrap_or(self.hw.vertex_format);
        let Ok((attributes, size)) = inline_array_layout(&formats) else { return Ok(Err("vertex_format")) };
        if size == 0 || data.is_empty() || !data.len().is_multiple_of(size as usize) {
            return Ok(Err("geometry"));
        }
        if let Err(cause) = self.check_supplied(mem, s) {
            return Ok(Err(cause));
        }
        let vertex_count = u32::try_from(data.len() / size as usize)
            .map_err(|_| D3dError::invalid(ENTRY, "too many inline vertices"))?;
        vertex_limit(ENTRY, vertex_count)?;
        let mut words = (*self.pushed_state(mem)?).clone();
        // SetPixelShader (0x3FA3D0) copies the definition's first 57 words
        // into D3D__RenderState 0..56 (`rep movsd` at 0x3FA4D5); the draw's
        // combiner state is those slots.
        words.render[..57].copy_from_slice(&s.pixel_shader[..57]);
        for &(slot, value) in &s.render {
            words.render[slot as usize] = value;
        }
        for &(stage, slot, value) in &s.stages {
            words.stages[stage as usize][slot as usize] = value;
        }

        // The carried attributes, as the inline draw applies them.
        let initial = *self.immediate.registers();
        for (n, value) in carried.iter().enumerate() {
            if let Some(value) = value {
                self.immediate.carry(n, *value);
            }
        }
        let last = &data[data.len() - size as usize..];
        for a in &attributes {
            let o = a.offset as usize;
            let value = attribute_value(a.data_type, &last[o..o + a.data_type.byte_size() as usize]);
            self.immediate.carry(a.register.0 as usize, value);
        }

        // The material over the device's bindings, for this draw.
        let textures = self.textures;
        for (bound, t) in self.textures.iter_mut().zip(&s.textures) {
            if let Some(header) = t {
                *bound = *header;
            }
        }
        let mut constants = Vec::with_capacity(s.constants.len());
        for &(slot, value, mask) in &s.constants {
            let row = &mut self.constants[4 * slot..4 * slot + 4];
            constants.push((slot, [row[0], row[1], row[2], row[3]]));
            for (c, w) in row.iter_mut().enumerate() {
                if mask & (1 << c) != 0 {
                    *w = value[c].to_bits();
                }
            }
        }
        self.constants_generation += 1;
        self.supplied_pixel = Some(s.pixel_shader);

        let parts = DrawParts {
            entry: ENTRY,
            indexed_esp: None,
            primitive,
            vertex_count,
            layout: VertexLayout {
                streams: vec![StreamLayout { stream: 0, attributes, stride: size }],
                pretransformed: false,
            },
            vertex: VertexProcessing::Program { start: 0, microcode: Microcode::from(s.microcode.clone()) },
            streams: vec![StreamBytes { data: data.into(), stride: size }],
            indices: None,
            words: Arc::new(words),
        };
        // No inline run: the next inline draw is resolved from the device.
        self.inline_capture = false;
        let result = self.try_native_producer(mem, parts, admission);
        self.inline_run = None;

        self.supplied_pixel = None;
        for (slot, row) in constants.into_iter().rev() {
            self.constants[4 * slot..4 * slot + 4].copy_from_slice(&row);
        }
        self.constants_generation += 1;
        self.textures = textures;
        if !matches!(result, Ok(true)) {
            self.immediate = Immediate::with_registers(initial);
        }
        match result? {
            true => Ok(Ok(())),
            false => Ok(Err(self.native_refusal.take().map_or("refused", refusal_name))),
        }
    }
}
