//! The Rust particle and contrail materials a cut setup's draws take
//! (docs/graphics/retire-d3d-r2.md, "R2's materials"): made from the
//! setup's inputs (`render_bridge::effect_materials`) and the tags, as the
//! device's supplied draw state. The particle material needs the map's
//! shader library, read once a map.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use native_render::effect_material::{self, Material, Resolved};
use native_render::object_streams::StreamFormats;
use native_render::{contrail_material, particle_material};
use render_bridge::effect_materials::{self as proof, Kind};
use tags::material::ShaderLibrary;

fn resolved(mem: &dyn GuestMemory, material: Material) -> Result<(Material, Resolved), &'static str> {
    let r = effect_material::resolve(&material, &|a| mem.u32(a).ok(), &|a, n| mem.bytes(a, n).ok())?;
    Ok((material, r))
}

/// The vertex array formats the cut particle setup's declaration gives
/// its draws: the stream type it declares
/// ([`particle_material::stream_type`]) over the program's inputs, the
/// stream format read from the table the guest reads (`0x0043f408`).
fn particle_formats(mem: &dyn GuestMemory, inputs: &particle_material::Inputs, m: &Material) -> Result<[u32; 16], &'static str> {
    struct Ro<'a>(&'a dyn GuestMemory);
    impl native_render::camera::GuestRead for Ro<'_> {
        fn read_u32(&self, address: u32) -> Option<u32> {
            self.0.u32(address).ok()
        }
    }
    let formats = StreamFormats::read(&Ro(mem)).ok_or("stream_formats_unread")?;
    let elements = formats.elements(particle_material::stream_type(inputs)).map_err(|_| "stream_format")?;
    let semantics =
        effect_material::program_semantics(&|a| mem.u32(a).ok(), &|a, n| mem.bytes(a, n).ok(), m.program.0, m.program.1)?;
    Ok(particle_material::vertex_formats(&semantics, &elements))
}

/// The material of the setup of `kind` running now as the device's
/// supplied draw takes it, for [`Host::material_cut`]: its material,
/// textures and program made, or the cause it cannot be (which panics).
///
/// [`Host::material_cut`]: render_bridge::native_particles::Host::material_cut
pub(super) fn cut(mem: &dyn GuestMemory, kind: Kind) -> Result<d3d::SuppliedState, &'static str> {
    let mut formats = None;
    let material = match kind {
        Kind::Particle => {
            let inputs = proof::particle_inputs().ok_or("no_setup")?;
            particle_material::check(&inputs).map_err(particle_material::Refused::name)?;
            let m = library_material(mem, &inputs)?;
            formats = Some(particle_formats(mem, &inputs, &m)?);
            m
        }
        Kind::Contrail => {
            let inputs = proof::contrail_inputs().ok_or("no_setup")?;
            let word = |a: u32| mem.u32(a).ok();
            let shader = effect_material::engine_vertex_shader(&word, contrail_material::VERTEX_SLOT)
                .ok_or("vertex_program_unread")?;
            contrail_material::material(&inputs, shader).map_err(contrail_material::Refused::name)?
        }
    };
    let (material, resolved) = resolved(mem, material)?;
    let mut s = supplied(&material, &resolved)?;
    s.vertex_format = formats;
    Ok(s)
}

/// `m` and its textures and program as a supplied draw's state. Not
/// made: a combiner word the draw drives (`combiner_driven`), which the
/// material does not hold, and a program whose length is not its
/// header's (`vertex_program_size`).
pub(super) fn supplied(m: &Material, r: &Resolved) -> Result<d3d::SuppliedState, &'static str> {
    if m.combiner_driven != 0 {
        return Err("combiner_driven");
    }
    let code = r.program.get(4..).ok_or("vertex_program_size")?;
    let count = r.program.get(2..4).map_or(0, |c| usize::from(u16::from_le_bytes([c[0], c[1]])));
    if count == 0 || code.len() != 16 * count {
        return Err("vertex_program_size");
    }
    Ok(d3d::SuppliedState {
        render: m.states.iter().map(|&(slot, value)| (u32::from(slot), value)).collect(),
        stages: m.stages.iter().map(|&(stage, slot, value)| (u32::from(stage), u32::from(slot), value)).collect(),
        pixel_shader: m.combiner,
        constants: m.constants.iter().map(|&(slot, value)| (slot, value, 0xf)).collect(),
        microcode: code.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect(),
        textures: r.textures.map(|t| t.map(|header| header.unwrap_or(0))),
        vertex_format: None,
    })
}

/// The supplied state of the cut setup of each kind, for its draws.
static HELD: Mutex<[Option<Arc<d3d::SuppliedState>>; 2]> = Mutex::new([None, None]);

/// Holds `s` for the draws of the cut setup of `kind`.
pub(super) fn hold(kind: Kind, s: d3d::SuppliedState) {
    HELD.lock().unwrap_or_else(|e| e.into_inner())[kind as usize] = Some(Arc::new(s));
}

/// The state a draw of `kind` draws from: the material its setup's cut
/// held, `None` before the first setup of `kind`.
pub(super) fn held(kind: Kind) -> Option<Arc<d3d::SuppliedState>> {
    HELD.lock().unwrap_or_else(|e| e.into_inner())[kind as usize].clone()
}

/// The map's shader library and the particle materials made from it.
#[derive(Default)]
struct Library {
    generation: Option<u64>,
    library: Option<Arc<Mutex<ShaderLibrary>>>,
    /// By (shader, layer, pass): the material with no matrix and class 0.
    materials: HashMap<(u32, u32, u32), Result<Material, particle_material::Refused>>,
}

static LIBRARY: LazyLock<Mutex<Library>> = LazyLock::new(Mutex::default);

/// The particle material for `inputs`, its view matrix and program index
/// the draw's own. The map's shader library is read at the map's first
/// particle setup, on the game thread: the setup waits for it.
fn library_material(mem: &dyn GuestMemory, inputs: &particle_material::Inputs) -> Result<Material, &'static str> {
    let generation = render_bridge::native_map_work::generation();
    let mut s = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    if s.generation != Some(generation) {
        *s = Library { generation: Some(generation), ..Library::default() };
    }
    let library = match s.library.clone() {
        Some(library) => library,
        None => {
            let name = crate::health::loaded_map_name(mem).ok().filter(|n| !n.is_empty()).ok_or("map_name")?;
            let read = crate::native_map::open(&name)
                .map_err(|e| e.to_string())
                .and_then(|(mut map, _)| ShaderLibrary::read(&mut map).map_err(|e| e.to_string()));
            let library = match read {
                Ok(library) => Arc::new(Mutex::new(library)),
                Err(e) => unimplemented!("particle_material: {name}: the shader library did not read ({e})"),
            };
            s.library = Some(Arc::clone(&library));
            library
        }
    };
    let key = (inputs.shader, inputs.layer, inputs.pass);
    let base = s.materials.entry(key).or_insert_with(|| {
        let mut lib = library.lock().unwrap_or_else(|e| e.into_inner());
        let base = particle_material::Inputs { matrix: [[0.0; 4]; 3], class: 0, ..*inputs };
        particle_material::material(&mut lib, &base)
    });
    let mut m = base.clone().map_err(particle_material::Refused::name)?;
    let matrix = effect_material::MATRIX_SLOT..effect_material::MATRIX_SLOT + 3;
    m.constants.retain(|(slot, _)| !matrix.contains(slot));
    m.constants.extend(inputs.matrix.iter().enumerate().map(|(n, row)| (effect_material::MATRIX_SLOT + n, *row)));
    m.program.1 = u32::from(inputs.class) + particle_material::PROGRAM_BASE;
    Ok(m)
}
