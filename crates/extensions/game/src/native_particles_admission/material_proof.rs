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
    /// The state arrays at the call of the draw's setup.
    before: Option<effect_material::State>,
}

fn made(kind: Kind, outcome: Result<(Material, Resolved), &'static str>) -> Option<Expected> {
    let before = proof::take_state(kind);
    match outcome {
        Ok((material, resolved)) => Some(Expected { kind, material, resolved, before }),
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

/// The inline array formats of the particle walk's vertices: `v0` four
/// `S32K` shorts in an 8-byte vertex, every other register off (what the
/// admission takes, `Refused::Geometry`); the cut setup's declaration
/// does not run, so the supplied draw names it.
pub(super) const PARTICLE_FORMATS: [u32; 16] = {
    let mut f = [0x02; 16];
    f[0] = 0x0845;
    f
};

/// The material of the setup of `kind` running now as the device's
/// supplied draw takes it, for [`Host::material_cut`]: its material,
/// textures and program made, or the refusal named, which keeps the
/// guest's setup. Must not record a proof (the caller holds it).
///
/// [`Host::material_cut`]: render_bridge::native_particles::Host::material_cut
pub(super) fn cut(mem: &dyn GuestMemory, kind: Kind) -> Result<d3d::SuppliedState, &'static str> {
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
    let (material, resolved) = resolved(mem, material)?;
    let mut s = supplied(&material, &resolved)?;
    if kind == Kind::Particle {
        s.vertex_format = Some(PARTICLE_FORMATS);
    }
    Ok(s)
}

/// `m` and its textures and program as a supplied draw's state. Refused:
/// a combiner word the draw drives (`combiner_driven`), which the
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

/// The state a draw of `kind` draws from: `None` unless its setup was
/// cut ([`proof::under_cut`]) with a material held.
pub(super) fn held(kind: Kind) -> Option<Arc<d3d::SuppliedState>> {
    if !proof::under_cut(kind) {
        return None;
    }
    HELD.lock().unwrap_or_else(|e| e.into_inner())[kind as usize].clone()
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
    let mut fields = effect_material::compare(&e.material, &e.resolved, &observed);
    // The rest of the draw's state is what the arrays held at its setup's
    // call: what a cut draw takes from the device.
    match &e.before {
        Some(before) => fields.extend(effect_material::compare_state(&e.material, before, observed.render, observed.stages)),
        None => {
            proof::record(e.kind, Err("state_unread"));
            return;
        }
    }
    if let VertexProcessing::Program { start, .. } = &draw.shader.vertex
        && *start != 0
    {
        fields.push("vertex_program_start".to_owned());
    }
    if e.kind == Kind::Particle && draw.layout.streams.first().map(|st| st.stride) != Some(8) {
        fields.push("vertex_format".to_owned());
    }
    if supplied(&e.material, &e.resolved).is_err() {
        fields.push("supplied".to_owned());
    }
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
    let matrix = effect_material::MATRIX_SLOT..effect_material::MATRIX_SLOT + 3;
    m.constants.retain(|(slot, _)| !matrix.contains(slot));
    m.constants.extend(inputs.matrix.iter().enumerate().map(|(n, row)| (effect_material::MATRIX_SLOT + n, *row)));
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
