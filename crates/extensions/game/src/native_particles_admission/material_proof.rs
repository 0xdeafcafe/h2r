//! The Rust particle and contrail materials against the draws the native
//! admission takes (docs/graphics/retire-d3d-r2.md, "R2's materials"):
//! one admitted draw in 64 of each kind is compared, field by field, with
//! the material made from the setup's inputs (`render_bridge::
//! effect_materials`) and the tags. The particle material needs the map's
//! shader library, read off the game thread once a map.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use d3d::shader::VertexProcessing;
use engine::memory::GuestMemory;
use native_render::effect_material::{self, Material, Observed, Resolved};
use native_render::{contrail_material, particle_material};
use render_bridge::effect_materials::{self as proof, Kind};
use tags::material::ShaderLibrary;

/// A sampled draw's material and its textures and program, to compare
/// with the draw the device admits.
pub(super) struct Expected {
    kind: Kind,
    material: Material,
    resolved: Resolved,
}

fn made(kind: Kind, outcome: Result<(Material, Resolved), &'static str>) -> Option<Expected> {
    match outcome {
        Ok((material, resolved)) => Some(Expected { kind, material, resolved }),
        Err(why) => {
            proof::record(kind, Err(why));
            None
        }
    }
}

fn resolved(mem: &dyn GuestMemory, material: Material) -> Result<(Material, Resolved), &'static str> {
    let r = effect_material::resolve(&material, &|a| mem.u32(a).ok(), &|a, n| mem.bytes(a, n).ok())?;
    Ok((material, r))
}

/// The particle emission `call`'s material when it is sampled: only an
/// emission of the particle walk (its return to `0x00224cd7`), which the
/// particle draw's setup precedes.
pub(super) fn particle(mem: &dyn GuestMemory, return_address: u32) -> Option<Expected> {
    if return_address != engine::particle_emission::EMIT_RETURN || !proof::sample(Kind::Particle) {
        return None;
    }
    let outcome = (|| {
        let inputs = proof::particle_inputs().ok_or("no_setup")?;
        particle_material::check(&inputs).map_err(particle_material::Refused::name)?;
        resolved(mem, library_material(mem, &inputs)?)
    })();
    made(Kind::Particle, outcome)
}

/// The contrail draw's material when it is sampled; `context` is the
/// dynamic draw's builder context (the setup's entry data). The setup's
/// inputs are taken whether or not the draw is sampled.
pub(super) fn contrail(mem: &dyn GuestMemory, context: u32) -> Option<Expected> {
    let inputs = proof::take_contrail_inputs(context);
    if !proof::sample(Kind::Contrail) {
        return None;
    }
    let outcome = (|| {
        let inputs = inputs.ok_or("no_setup")?;
        let word = |a: u32| mem.u32(a).ok();
        let shader = effect_material::engine_vertex_shader(&word, contrail_material::VERTEX_SLOT)
            .ok_or("vertex_program_unread")?;
        let material = contrail_material::material(&inputs, shader).map_err(contrail_material::Refused::name)?;
        resolved(mem, material)
    })();
    made(Kind::Contrail, outcome)
}

/// Whether the setup of `kind` running now may be cut: its material,
/// textures and program are made (or the refusal is named), and then
/// `device_state`: the device resolves a draw's program, vertex format,
/// combiner, textures and constants from its own state, and no admission
/// draws from a supplied one, so a cut setup's draw would take the state
/// the draw before it left. Must not record a proof (the caller holds it).
pub(super) fn cut(mem: &dyn GuestMemory, kind: Kind) -> Result<(), &'static str> {
    let material = match kind {
        Kind::Particle => {
            let inputs = proof::particle_inputs().ok_or("no_setup")?;
            particle_material::check(&inputs).map_err(particle_material::Refused::name)?;
            library_material(mem, &inputs)?
        }
        Kind::Contrail => {
            let inputs = proof::contrail_inputs().ok_or("no_setup")?;
            let word = |a: u32| mem.u32(a).ok();
            let shader = effect_material::engine_vertex_shader(&word, contrail_material::VERTEX_SLOT)
                .ok_or("vertex_program_unread")?;
            contrail_material::material(&inputs, shader).map_err(contrail_material::Refused::name)?
        }
    };
    resolved(mem, material)?;
    Err("device_state")
}

/// Compares `draw` with `e` and records the differing fields.
pub(super) fn record(e: &Expected, draw: &d3d::ResolvedDraw) {
    let program = match &draw.shader.vertex {
        VertexProcessing::Program { microcode, .. } => {
            let mut code = vec![0x78, 0x20];
            code.extend_from_slice(&((microcode.len() / 4) as u16).to_le_bytes());
            code.extend(microcode.iter().flat_map(|w| w.to_le_bytes()));
            Some(code)
        }
        VertexProcessing::FixedFunction { .. } => None,
    };
    let s = &draw.raw.stages;
    let observed = Observed {
        render: &draw.raw.render,
        stages: [&s[0], &s[1], &s[2], &s[3]],
        combiner: draw.shader.pixel_shader.as_deref(),
        textures: draw.textures,
        constants: &draw.shader.constants[..],
        program,
    };
    let fields = effect_material::compare(&e.material, &e.resolved, &observed);
    proof::record(e.kind, Ok(&fields));
}

/// The map's shader library and the particle materials made from it.
#[derive(Default)]
struct Library {
    generation: Option<u64>,
    library: Option<Arc<Mutex<ShaderLibrary>>>,
    failed: bool,
    loading: bool,
    /// By (shader, layer, pass): the material with no matrix and class 0.
    materials: HashMap<(u32, u32, u32), Result<Material, particle_material::Refused>>,
}

static LIBRARY: LazyLock<Mutex<Library>> = LazyLock::new(Mutex::default);

/// The particle material for `inputs`, its view matrix and program index
/// the draw's own.
fn library_material(mem: &dyn GuestMemory, inputs: &particle_material::Inputs) -> Result<Material, &'static str> {
    let generation = render_bridge::native_map_work::generation();
    let mut s = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    if s.generation != Some(generation) {
        *s = Library { generation: Some(generation), ..Library::default() };
    }
    if s.failed {
        return Err("library_unread");
    }
    let Some(library) = s.library.clone() else {
        if !s.loading {
            let name = crate::health::loaded_map_name(mem).ok().filter(|n| !n.is_empty()).ok_or("library_loading")?;
            s.loading = true;
            load(name, generation);
        }
        return Err("library_loading");
    };
    let key = (inputs.shader, inputs.layer, inputs.pass);
    let base = s.materials.entry(key).or_insert_with(|| {
        let mut lib = library.lock().unwrap_or_else(|e| e.into_inner());
        let base = particle_material::Inputs { matrix: [[0.0; 4]; 3], class: 0, ..*inputs };
        particle_material::material(&mut lib, &base)
    });
    let mut m = base.clone().map_err(particle_material::Refused::name)?;
    m.constants = inputs.matrix.iter().enumerate().map(|(n, row)| (effect_material::MATRIX_REGISTER + n, *row)).collect();
    m.program.1 = u32::from(inputs.class) + particle_material::PROGRAM_BASE;
    Ok(m)
}

/// Reads map `name`'s shader library off the game thread.
fn load(name: String, generation: u64) {
    let spawned = std::thread::Builder::new().name("particle material library".into()).spawn(move || {
        let read = crate::native_map::open(&name)
            .map_err(|e| e.to_string())
            .and_then(|(mut map, _)| ShaderLibrary::read(&mut map).map_err(|e| e.to_string()));
        let mut s = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
        if s.generation != Some(generation) {
            return;
        }
        s.loading = false;
        match read {
            Ok(library) => s.library = Some(Arc::new(Mutex::new(library))),
            Err(e) => {
                eprintln!("particle material: {name}: the shader library did not read ({e})");
                s.failed = true;
            }
        }
    });
    if spawned.is_err() {
        LIBRARY.lock().unwrap_or_else(|e| e.into_inner()).failed = true;
    }
}
