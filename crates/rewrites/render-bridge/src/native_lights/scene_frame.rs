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
//! A light it takes draws at once, at its passes, from its groups
//! ([`super::draw_cut_light`]); only then do the groups go into the
//! scene. A light this cannot make whole, or the host does not draw,
//! panics by cause (`lights: <cause>`, [`fail`]). The guest's remaining
//! passes never run.

use engine::light_constants as lc;
use guest_layout::memory::GuestMemory;
use engine::render::light_gather::{Refusal, Writes, passes};
use engine::shadow_constants as sc;
use frame_scene::{GroupSite, PROJECTION_ROWS, SceneLight};
use native_render::light_passes::{self, SHADOW, Steps};

use super::typed::Mirror;
use super::typed_groups::GroupRefusal;
use super::{Entry, SceneGroup, Source, source_constants, typed_groups};

/// Why a light's passes cannot put it into the scene ([`fail`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The passes' arguments, the light's lists or its tags did not read.
    Unread,
    /// The light's constants or flags did not compute, with the port's
    /// error where one stopped it.
    Constants(Option<lc::Error>),
    /// The shadow projection did not compute.
    Projection(lc::Error),
    /// A layer's entries are not all of a kind the scene holds whole.
    Group(GroupRefusal),
    /// The scene's open light is not this call's (the hand-off refused).
    Light,
    /// The additional casters' pair (`0x00030ed0`) would make a suffix:
    /// a candidate's first match in the current visibility list is marked
    /// `0x1000` (`Unsupported`), or the candidates do not read (the
    /// lookup's other refusals). The marker lookup `0x000b8d30`, the
    /// object's records `0x00133790` and the suffix's item lists
    /// `0x00044ac0` it would run are not built.
    AdditionalCasters(Refusal),
    /// The host did not draw the light's groups (`Host::draw_scene_light`).
    Host,
}

impl Dropped {
    /// The cause's name in the panic ([`fail`]).
    pub fn name(self) -> &'static str {
        match self {
            Dropped::Unread => "unread",
            Dropped::Constants(_) => "constants",
            Dropped::Projection(_) => "projection",
            Dropped::Group(_) => "group",
            Dropped::Light => "light",
            Dropped::AdditionalCasters(_) => "additional_casters",
            Dropped::Host => "host",
        }
    }
}

/// A light the native passes cannot take: `lights: <cause>` (a group's
/// with its refusal, `lights: group <refusal>`). Nothing falls back.
pub fn fail(why: Dropped) -> ! {
    unimplemented!("lights: {}", why.message())
}

impl Dropped {
    /// The cause as the panic names it: its name, then what stopped it
    /// where the port says (`constants Simd`, `group entry_state`,
    /// `additional_casters Unsupported`).
    pub fn message(self) -> String {
        match self {
            Dropped::Group(refusal) => format!("group {}", refusal.name()),
            Dropped::Constants(Some(e)) | Dropped::Projection(e) => format!("{} {e:?}", self.name()),
            Dropped::AdditionalCasters(r) => format!("{} {r:?}", self.name()),
            why => why.name().to_owned(),
        }
    }
}

/// Why a whole light frame (`0x00031c80`) or a light's passes cannot be
/// planned: `lights: frame <cause>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDropped {
    /// A host is set and does not draw the scene's light layers
    /// ([`super::scene_path_ready`]).
    Host,
    /// The frame's plan refused (`frame::plan`), by its refusal.
    Plan(Refusal),
    /// A light's passes' entry plan refused (`passes::plan`), by its
    /// refusal.
    Passes(Refusal),
}

impl FrameDropped {
    pub fn name(self) -> &'static str {
        match self {
            FrameDropped::Host => "host",
            FrameDropped::Plan(_) => "plan",
            FrameDropped::Passes(_) => "passes",
        }
    }

    /// The cause with its refusal: `plan Residency`.
    pub fn message(self) -> String {
        match self {
            FrameDropped::Plan(r) | FrameDropped::Passes(r) => format!("{} {r:?}", self.name()),
            why => why.name().to_owned(),
        }
    }
}

/// A light frame or passes call the native code cannot plan:
/// `lights: frame <cause>`.
pub fn fail_frame(why: FrameDropped) -> ! {
    unimplemented!("lights: frame {}", why.message())
}

/// What one light's passes put into the scene.
#[derive(Clone, Debug)]
pub struct Planned {
    /// The scene's light as the passes would set it.
    pub light: SceneLight,
    pub steps: Steps,
    /// One group per drawn layer, in step order.
    pub groups: Vec<SceneGroup>,
    /// The additional casters would run.
    pub additional_casters: bool,
    /// The light's record, constants and gels as the host draws it, put
    /// in the scene's light for the draw at the passes (the view's
    /// candidates are read only once its light pass is over).
    pub candidate: frame_scene::LightCandidate,
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
    let c = source_constants(mem, &source, fpu).ok_or(Dropped::Constants(None))?;
    let inputs = source
        .inputs(mem, c.flags, c.definition_flags)
        .ok_or(Dropped::Unread)?;
    let steps = light_passes::steps(&inputs);
    let shadow = if c.flags & SHADOW != 0 {
        let g = lc::light_globals(mem, &source.call, fpu, &mut |_| None)
            .map_err(|e| Dropped::Constants(Some(e)))?;
        let call = sc::Call {
            context: source.call.context,
            frame: source.call.frame,
            definition: source.call.definition,
        };
        Some(
            sc::shadow_globals_for_light(mem, &call, fpu, &g, c.flags)
                .map_err(Dropped::Projection)?,
        )
    } else {
        None
    };
    let groups = groups(mem, &steps, c.flags, c.definition_flags, shadow.as_ref())?;
    let record = super::read_light(
        mem,
        source.call.light,
        source.call.shadow & 0xff != 0,
        f32::from_bits(source.call.weight),
    )
    .ok_or(Dropped::Unread)?;
    let candidate = super::first_person::candidate(mem, &record, source.call.definition, c);
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
        candidate,
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
    typed_groups::plan_or_refusal(
        &mirror,
        GroupSite::Light,
        steps.as_slice(),
        flags,
        definition_flags,
        projection,
    )
    .map_err(Dropped::Group)
}

/// A cut light's layers as the host draws them: one pass per group, in
/// step order, each with the light.
pub(super) fn cut_passes(
    slot: frame_scene::ViewSlot,
    light: &frame_scene::Light,
    groups: &[SceneGroup],
) -> Vec<super::ScenePass> {
    groups
        .iter()
        .map(|group| super::ScenePass { slot, light: light.clone(), group: group.clone() })
        .collect()
}

/// The projection block's rows as the scene's groups hold them.
fn projection_rows(block: &[u32]) -> [[u32; 4]; PROJECTION_ROWS] {
    core::array::from_fn(|row| core::array::from_fn(|col| block.get(4 * row + col).copied().unwrap_or(0)))
}

/// A passes call whose plan refused, by its refusal.
pub(super) fn refused(why: Refusal) -> ! {
    fail_frame(FrameDropped::Passes(why))
}

/// After the item lists of the passes entered with `p`: the light's
/// groups into the scene, behind the light the hand-off opened. Returns
/// the groups put in; a light it cannot take panics by cause ([`fail`]).
pub(super) fn light(mem: &mut dyn GuestMemory, p: &passes::Plan) -> usize {
    let e = entry(p);
    // A light whose lists the guest made was left for the constants call,
    // which does not run here.
    if let Some((deferred, serial)) = super::handoff::take_deferred(e.esp) {
        super::observe(&*mem, deferred, serial);
    }
    // The scene draws the light or nothing does: no layer of this light is
    // a candidate of the resolved-draw path.
    super::CANDIDATE.take();
    take(mem, &e).unwrap_or_else(|why| fail(why))
}

/// The light drawn and its groups into the scene, or why not. Nothing is
/// drawn and nothing goes in on a refusal.
fn take(mem: &mut dyn GuestMemory, e: &Entry) -> Result<usize, Dropped> {
    let planned = plan(&*mem, e)?;
    if !crate::scene_step::current_light_matches(&planned.light, planned.steps.as_slice()) {
        return Err(Dropped::Light);
    }
    // The additional casters' pair (`0x00030ed0`) makes no suffix when no
    // candidate's first match in the current visibility list is marked
    // `0x1000` (`passes::additional::empty`): it draws nothing, and its one
    // store puts back the cursor it saved, so the light draws its own
    // steps. A pair that would make a suffix needs the marker lookup and
    // the suffix's record walk, which are not built.
    if planned.additional_casters {
        passes::additional::empty(&*mem).map_err(Dropped::AdditionalCasters)?;
    }
    let mut light = crate::scene_step::current_light().ok_or(Dropped::Light)?;
    light.candidate = Some(planned.candidate.clone());
    let passes = cut_passes(super::LIGHT_SLOT, &light, &planned.groups);
    if !super::draw_cut_light(mem, &passes, super::view_target()) {
        return Err(Dropped::Host);
    }
    let n = planned.groups.len();
    crate::scene_step::typed_light_groups(planned.groups);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scene_extract::light_groups::Cause;

    #[test]
    fn a_dropped_light_names_its_cause_and_what_stopped_it() {
        assert_eq!(Dropped::Unread.message(), "unread");
        assert_eq!(Dropped::Constants(None).message(), "constants");
        assert_eq!(Dropped::Constants(Some(lc::Error::Simd)).message(), "constants Simd");
        assert_eq!(Dropped::Projection(lc::Error::Data).message(), "projection Data");
        assert_eq!(Dropped::Group(GroupRefusal::Entry(Cause::State)).message(), "group entry_state");
        assert_eq!(Dropped::AdditionalCasters(Refusal::Unsupported).message(), "additional_casters Unsupported");
        assert_eq!(Dropped::Host.message(), "host");
    }

    #[test]
    fn a_refused_frame_names_its_refusal() {
        assert_eq!(FrameDropped::Host.message(), "host");
        assert_eq!(FrameDropped::Plan(Refusal::Residency).message(), "plan Residency");
        assert_eq!(FrameDropped::Passes(Refusal::Layout).message(), "passes Layout");
    }
}
