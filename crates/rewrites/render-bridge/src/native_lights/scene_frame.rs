//! A light's passes under the cut (docs/graphics/native-lights.md, "The
//! light frame under the cut"). When the host draws the scene's light
//! layers ([`super::scene_path_ready`]) the guest's light frame body never
//! runs: the Rust frame body gathers every drawn light as the guest does,
//! and each light's passes (`0x00031170`) stop after their first step, the
//! item lists `0x00040e90`. What the guest's remaining steps would draw
//! goes into the scene instead: the light and its steps (from the
//! hand-off, [`super::handoff`]) and, here, one group per drawn layer,
//! made from the lists the item body just built as the guest's layer
//! builds would sort them ([`super::typed_groups::plan`]). No constant,
//! target, stencil or draw call is made.
//!
//! The light's pass flags and shadow projection are computed, not read:
//! the constants `0x0004a1e0` and the shadow setup `0x00048ee0` do not
//! run. The setup's shadow-target resize is laid over guest memory for the
//! groups' read only ([`super::typed::Mirror`]); nothing is written.
//!
//! A light this cannot make whole is a named drop
//! (`scene_dropped_light_<cause>` in `render.scene`), never a guest draw.

use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_constants as lc;
use engine::memory::GuestMemory;
use engine::render::light_gather::{Writes, passes};
use engine::shadow_constants as sc;
use frame_scene::{GroupSite, PROJECTION_ROWS, SceneLight};
use native_render::light_passes::{self, SHADOW, Steps};

use super::typed::Mirror;
use super::{Entry, SceneGroup, Source, source_constants, typed_groups};

/// Why a light's passes put nothing, or not everything, into the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The passes' arguments, the light's lists or its tags did not read.
    Unread,
    /// The light's constants or flags did not compute.
    Constants,
    /// The shadow projection did not compute.
    Projection,
    /// A layer's entries are not all of a kind the scene holds whole.
    Group,
    /// The scene's open light is not this call's (the hand-off refused).
    Light,
    /// The additional casters' layers (`0x00030ed0`), whose record walk
    /// the cut does not make. The light's own layers still go in.
    AdditionalCasters,
}

impl Dropped {
    pub const ALL: [Dropped; 6] = [
        Dropped::Unread,
        Dropped::Constants,
        Dropped::Projection,
        Dropped::Group,
        Dropped::Light,
        Dropped::AdditionalCasters,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Dropped::Unread => "scene_dropped_light_unread",
            Dropped::Constants => "scene_dropped_light_constants",
            Dropped::Projection => "scene_dropped_light_projection",
            Dropped::Group => "scene_dropped_light_group",
            Dropped::Light => "scene_dropped_light_light",
            Dropped::AdditionalCasters => "scene_dropped_light_additional_casters",
        }
    }
}

static DROPPED: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static LIGHTS: AtomicU64 = AtomicU64::new(0);
static GROUPS: AtomicU64 = AtomicU64::new(0);

fn drop_light(why: Dropped) {
    let n = Dropped::ALL.iter().position(|&d| d == why).unwrap_or(0);
    DROPPED[n].fetch_add(1, Ordering::Relaxed);
}

/// `render.scene`'s columns for the cut light frame: lights and groups
/// gathered, and drops by cause.
pub fn counters() -> Vec<(&'static str, u64)> {
    [
        ("scene_cut_lights", LIGHTS.load(Ordering::Relaxed)),
        ("scene_cut_light_groups", GROUPS.load(Ordering::Relaxed)),
    ]
    .into_iter()
    .chain(
        Dropped::ALL
            .iter()
            .zip(&DROPPED)
            .map(|(d, c)| (d.name(), c.load(Ordering::Relaxed))),
    )
    .collect()
}

/// What one light's passes put into the scene.
#[derive(Clone, Debug)]
pub struct Planned {
    /// The scene's light as the passes would set it.
    pub light: SceneLight,
    pub steps: Steps,
    /// One group per drawn layer, in step order.
    pub groups: Vec<SceneGroup>,
    /// The additional casters would run: their groups are not made.
    pub additional_casters: bool,
}

/// The passes' registers at their entry, as the hand-off reads them.
pub fn entry(p: &passes::Plan) -> Entry {
    Entry {
        eax: p.definition,
        ecx: p.context,
        esp: p.entry_esp,
        mxcsr: p.fpu.mxcsr,
        x87_control: p.fpu.x87_control,
    }
}

/// The scene's light and groups for the passes entered with `e`, once the
/// item lists are made. Reads only.
pub fn plan(mem: &dyn GuestMemory, e: &Entry) -> Result<Planned, Dropped> {
    let source = Source::observed(mem, e).ok_or(Dropped::Unread)?;
    let fpu = e.fpu();
    let c = source_constants(mem, &source, fpu).ok_or(Dropped::Constants)?;
    let inputs = source
        .inputs(mem, c.flags, c.definition_flags)
        .ok_or(Dropped::Unread)?;
    let steps = light_passes::steps(&inputs);
    let shadow = if c.flags & SHADOW != 0 {
        let g = lc::light_globals(mem, &source.call, fpu, &mut |_| None)
            .map_err(|_| Dropped::Constants)?;
        let call = sc::Call {
            context: source.call.context,
            frame: source.call.frame,
            definition: source.call.definition,
        };
        Some(
            sc::shadow_globals_for_light(mem, &call, fpu, &g, c.flags)
                .map_err(|_| Dropped::Projection)?,
        )
    } else {
        None
    };
    let groups = groups(mem, &steps, c.flags, c.definition_flags, shadow.as_ref())?;
    Ok(Planned {
        light: SceneLight {
            handle: source.call.light,
            definition: source.definition,
            flags: inputs.flags,
            definition_flags: inputs.definition_flags,
            additional_casters: inputs.additional_casters,
            guest_weight: Some(f32::from_bits(source.call.weight)),
            ..Default::default()
        },
        steps,
        groups,
        additional_casters: inputs.additional_casters,
    })
}

/// The groups of a light's drawn `steps` from the item lists in `mem`, as
/// the guest's layer builds would sort them with the passes' `flags`.
/// `shadow` is what the shadow setup would store: its block is the
/// casters' and receivers' projection, and its target words are laid over
/// `mem` for the read (the setup's resize), not written.
pub fn groups(
    mem: &dyn GuestMemory,
    steps: &Steps,
    flags: u32,
    definition_flags: u32,
    shadow: Option<&sc::ShadowGlobals>,
) -> Result<Vec<SceneGroup>, Dropped> {
    let mut mirror = Mirror {
        mem,
        writes: Writes::new(),
    };
    let projection = match shadow {
        Some(shadow) => {
            shadow.write(&mut mirror).map_err(|_| Dropped::Unread)?;
            Some(projection_rows(&shadow.block))
        }
        None => None,
    };
    typed_groups::plan(
        &mirror,
        GroupSite::Light,
        steps.as_slice(),
        flags,
        definition_flags,
        projection,
    )
    .ok_or(Dropped::Group)
}

/// The projection block's rows as the scene's groups hold them.
fn projection_rows(block: &[u32]) -> [[u32; 4]; PROJECTION_ROWS] {
    core::array::from_fn(|row| core::array::from_fn(|col| block.get(4 * row + col).copied().unwrap_or(0)))
}

/// A passes call whose plan refused: its light is a drop.
pub(super) fn refused() {
    drop_light(Dropped::Unread);
}

/// After the item lists of the passes entered with `p`: the light's
/// groups into the scene, behind the light the hand-off opened. Returns
/// the groups put in.
pub(super) fn light(mem: &dyn GuestMemory, p: &passes::Plan) -> usize {
    let e = entry(p);
    // A light whose lists the guest made was left for the constants call,
    // which does not run here.
    if let Some((deferred, serial)) = super::handoff::take_deferred(e.esp) {
        super::observe(mem, deferred, serial);
    }
    // The passes draw nothing: no layer of this light is a candidate of the
    // resolved-draw path.
    super::CANDIDATE.take();
    let planned = match plan(mem, &e) {
        Ok(planned) => planned,
        Err(why) => {
            drop_light(why);
            return 0;
        }
    };
    if !crate::scene_step::current_light_matches(&planned.light, planned.steps.as_slice()) {
        drop_light(Dropped::Light);
        return 0;
    }
    if planned.additional_casters {
        drop_light(Dropped::AdditionalCasters);
    }
    let n = planned.groups.len();
    crate::scene_step::typed_light_groups(planned.groups);
    LIGHTS.fetch_add(1, Ordering::Relaxed);
    GROUPS.fetch_add(n as u64, Ordering::Relaxed);
    n
}
