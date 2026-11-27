//! The first-person lights at `FIRST_PERSON_LIGHTS` (`0x000404b0`) and
//! each slot's record replay at `FIRST_PERSON_LIGHT_RECORDS`
//! (`0x000402b0`; docs/graphics/native-light-gather.md, slices L6 and L8).
//!
//! With the light walk on, the entry checks its plan and runs the guest
//! function whole. When a renderer can take typed passes and the float
//! state allows them, the entry keeps the plan, its return address and
//! the caller's saved registers. The guest's own call to
//! `LIGHT_CONSTANTS` (return word `site::CONSTANTS`) then offers the
//! loop's last slot to the typed graph (`first_person::final_slot`). A
//! commit returns from the whole function as its loop exit would; any
//! refusal, and every earlier slot, leaves that call and the rest of the
//! body to the guest untouched. No guest code resumes inside a skipped
//! slot.
//!
//! At the entry, before either, E3's chosen first-person lights that
//! have no slot are queued over the first-person parts
//! (`chosen::first_person::at_pass`).
//!
//! With the walk off, L6 runs the body in Rust: every callee a guest
//! call naming the guest's return site through `passes::caller`, so the
//! layer draws and the walk's builds that tell the first-person layers
//! apart by their caller see the guest's site, and the light goes into
//! the first person's view at its first layer draw
//! (`native_lights::first_person_layer`). A plan that refuses runs the
//! guest function whole.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_constants::{Call, Fpu};
use engine::memory::GuestMemory;
use engine::points;
use engine::render::light_gather::first_person::{self, AtConstants, SlotRecords};
use engine::render::light_gather::items::Made;
use engine::render::light_gather::{Refusal, typed, typed_tail};
use frame_scene::GroupSite;
use hooks::{Builder, Cx, EntryFuture, Flow};
use native_render::light_graph;
use native_render::light_passes::{self, Steps};
use runtime::TransferAbi;
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use super::{Ro, SceneGroup};
use crate::native_map_work::{Stage, bsp_generation};

/// EBX, EBP, ESI and EDI as the guest's caller handed them to
/// `0x000404b0`. Its epilogue restores these four.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Saved {
    ebx: u32,
    ebp: u32,
    esi: u32,
    edi: u32,
}

/// The invocation that runs the whole guest body with typed passes
/// offered: its plan, the return address it was called with, the
/// caller's saved registers, and the map generation it began in (a load
/// or a restored snapshot moves it on).
struct Pending {
    plan: first_person::Plan,
    ret: u32,
    saved: Saved,
    generation: u64,
}

thread_local! {
    /// Set only by the entry that runs the guest body, cleared by every
    /// entry and by [`forget`], and kept across the loop's earlier slots
    /// until the last slot's constants call takes it. The thread that runs
    /// the guest owns it; another thread never sees it.
    static PENDING: Cell<Option<Pending>> = const { Cell::new(None) };
}

/// Drops the kept invocation (`native_lights::forget_runs`).
pub(super) fn forget() {
    PENDING.take();
}

pub(super) fn install(b: &mut Builder) {
    let walk = super::walk::on(b.switches());
    b.raw()
        .implement(
            &points::FIRST_PERSON_LIGHTS,
            "native_lights_first_person",
            move |cpu, ctx, addr| {
                // Any earlier invocation has returned.
                forget();
                if addr != points::FIRST_PERSON_LIGHTS.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
                }
                {
                    // E3's chosen first-person lights that have no slot
                    // here: queued as E3 stages before the slots' passes,
                    // whatever runs the body.
                    let runtime = Arc::clone(ctx.runtime());
                    let mut mem = runtime.memory();
                    super::chosen::first_person::at_pass(&mut *mem);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let (plan, ret) = {
                    let runtime = Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    (
                        first_person::plan(&*mem, cpu.esp, fpu),
                        GuestMemory::u32(&*mem, cpu.esp),
                    )
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::FirstPersonLights, why);
                        return original(cpu, ctx, addr);
                    }
                };
                // Under the scene path each slot stops after its records,
                // draws from its groups at once on the first person's
                // target (bound as the loop begins) and goes into the
                // scene; a slot the host does not draw runs its steps.
                if super::scene_path_ready() {
                    let target = super::bound_target();
                    return hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                        Box::pin(async move {
                            let (ret, slots) = first_person::run_scene(
                                &cx,
                                plan,
                                &mut |m, p, call| scene_slot(m, p, call, target),
                                // Each taken slot drew at its place in the loop.
                                &mut |_, _| {},
                            )
                            .await?;
                            SCENE_SLOTS.fetch_add(slots.cut.len() as u64, Ordering::Relaxed);
                            SceneList::FirstPersonLights.count(SceneListResult::Native);
                            Ok::<_, engine::entry::EntryError>(ret)
                        })
                    });
                }
                if walk {
                    // The guest body whole. Typed slots only when a
                    // renderer takes them under this float state.
                    let typed_fpu =
                        super::typed::fpu_ready(cpu.x87.tag, cpu.x87.status, cpu.x87.control);
                    if typed_fpu && super::typed::host_ready() {
                        if let Ok(ret) = ret {
                            let saved = Saved {
                                ebx: cpu.ebx,
                                ebp: cpu.ebp,
                                esi: cpu.esi,
                                edi: cpu.edi,
                            };
                            PENDING.set(Some(Pending {
                                plan,
                                ret,
                                saved,
                                generation: bsp_generation(),
                            }));
                        } else {
                            refused(SceneList::TypedFirstPerson, Refusal::Memory);
                        }
                    } else {
                        refused(SceneList::TypedFirstPerson, Refusal::Unsupported);
                    }
                    return original(cpu, ctx, addr);
                }
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, _) = first_person::run_with(&cx, plan, &mut |_, _, _| {
                            refused(SceneList::TypedFirstPerson, Refusal::Unsupported);
                            Ok(None)
                        })
                        .await?;
                        SceneList::FirstPersonLights.count(SceneListResult::Native);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::LIGHT_CONSTANTS,
            "native_lights_typed_first_person",
            |c| Ok(at_constants(c).map_or(Flow::Continue, Flow::Return)),
        )
        .context_only();
    b.raw()
        .implement(
            &points::FIRST_PERSON_LIGHT_RECORDS,
            "native_lights_first_person_records",
            |cpu, ctx, addr| {
                if addr != points::FIRST_PERSON_LIGHT_RECORDS.base || Stage::Lights.standing_down()
                {
                    return original(cpu, ctx, addr);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let plan = {
                    let runtime = std::sync::Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    first_person::plan_records(&*mem, cpu.esp, cpu.eax, cpu.edx, fpu)
                };
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(why) => {
                        refused(SceneList::FirstPersonRecords, why);
                        return original(cpu, ctx, addr);
                    }
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let (ret, records) = first_person::run_records(&cx, plan).await?;
                        count(&records);
                        Ok::<_, engine::entry::EntryError>(ret)
                    })
                })
            },
        )
        .context_only();
}

/// `LIGHT_CONSTANTS` entered. Acts only on the guest's own call from the
/// first-person loop (return word `site::CONSTANTS`) of the invocation
/// the entry kept; any other caller, including the L6 body's calls (whose
/// return word is a continuation token), passes through. `None` leaves
/// every register and word as the guest left them, and the constants
/// run.
fn at_constants(c: &mut hooks::Call<'_>) -> Option<TransferAbi> {
    let pending = PENDING.take()?;
    let rt = Arc::clone(c.ctx.runtime());
    let own_call = pending.generation == bsp_generation()
        && first_person::entry_esp_at_constants(c.cpu.esp) == pending.plan.entry_esp
        && GuestMemory::u32(&*rt.memory(), c.cpu.esp).ok() == Some(first_person::site::CONSTANTS);
    if !own_call {
        // Not this invocation's call (or a map since): an earlier slot's
        // call still finds the plan; a stale one is dropped.
        if pending.generation == bsp_generation() {
            PENDING.set(Some(pending));
        }
        return None;
    }
    let out = final_slot(c, &rt, &pending);
    if out.is_none() {
        // The guest makes this slot; a later one may still be the last.
        PENDING.set(Some(pending));
    }
    out
}

/// The kept invocation's constants call: commit the loop's last slot and
/// return from `0x000404b0`, or `None` with nothing written.
fn final_slot(
    c: &mut hooks::Call<'_>,
    rt: &Arc<runtime::Runtime>,
    pending: &Pending,
) -> Option<TransferAbi> {
    let stage = SceneList::TypedFirstPerson;
    let p = &pending.plan;
    if Stage::Lights.standing_down() {
        refused(stage, Refusal::Unsupported);
        return None;
    }
    // The graph and the light frame run under the entry's float modes,
    // which the guest's constants call would also see.
    let cpu = &*c.cpu;
    if (cpu.mxcsr ^ p.fpu.mxcsr) & !0x3f != 0
        || cpu.x87.control != p.fpu.x87_control
        || !super::typed::fpu_ready(cpu.x87.tag, cpu.x87.status, cpu.x87.control)
    {
        refused(stage, Refusal::Layout);
        return None;
    }
    let at = AtConstants {
        esp: cpu.esp,
        eax: cpu.eax,
        ecx: cpu.ecx,
        edx: cpu.edx,
        ebx: cpu.ebx,
        esi: cpu.esi,
        edi: cpu.edi,
    };
    let slot = {
        let mut mem = rt.memory();
        // Preflight: the entry's return word is still the one it was
        // called with, and the frame is the last slot's call.
        if GuestMemory::u32(&*mem, p.entry_esp).ok() != Some(pending.ret) {
            refused(stage, Refusal::Layout);
            return None;
        }
        let slot = match first_person::final_slot(&*mem, p, &at) {
            Ok(slot) => slot,
            Err(why) => {
                // An earlier slot, or a frame the plan does not describe.
                refused(stage, why);
                return None;
            }
        };
        if let Err(why) = try_passes(&mut *mem, p, &slot.call) {
            refused(stage, why);
            return None;
        }
        stage.count(SceneListResult::Native);
        slot
    };
    // Return from 0x000404b0 to the captured return address, as its loop
    // exit and epilogue would: EAX the exit's, the caller's saved
    // registers, ESP past the return word and the mode. No guest memory
    // is touched after the commit.
    let saved = pending.saved;
    c.cpu.eax = slot.ret.eax;
    c.cpu.ebx = saved.ebx;
    c.cpu.ebp = saved.ebp;
    c.cpu.esi = saved.esi;
    c.cpu.edi = saved.edi;
    c.cpu.esp = slot.exit_esp;
    Some(TransferAbi::ret(pending.ret))
}

static SCENE_SLOTS: AtomicU64 = AtomicU64::new(0);

/// First-person slots the scene path took (`render.scene`'s
/// `scene_first_person_cut_slots`).
pub fn scene_slots() -> u64 {
    SCENE_SLOTS.load(Ordering::Relaxed)
}

static KEPT_HOST: AtomicU64 = AtomicU64::new(0);

/// First-person slots the host did not draw, whose steps the guest made
/// (`scene_first_person_kept_host`).
pub fn kept_host_slots() -> u64 {
    KEPT_HOST.load(Ordering::Relaxed)
}

/// The scene's candidate for a first-person slot's light: its record,
/// its constants as [`Slot::read`] computed them (no shadow, so both
/// variants are the same), and its definition's gels.
pub(super) fn candidate(
    mem: &dyn GuestMemory,
    light: &super::Light,
    definition: u32,
    constants: frame_scene::LightConstants,
) -> frame_scene::LightCandidate {
    let word = |offset: u32| {
        mem.u32(definition.wrapping_add(offset))
            .ok()
            .filter(|&w| w != u32::MAX)
    };
    frame_scene::LightCandidate {
        position: light.position,
        forward: light.forward,
        up: light.up,
        colour_a: light.colour_a,
        colour_b: light.colour_b,
        intensity: light.intensity,
        bounds: light.bounds,
        owner: None,
        definition_kind: mem.u16(definition.wrapping_add(4)).unwrap_or(0),
        volume: None,
        view_eligible: Some(true),
        priority: None,
        weight: light.weight,
        gel_bitmap: word(engine::light_passes::DEFINITION_GEL),
        gel_shader: word(engine::light_passes::DEFINITION_GEL_SHADER),
        constants: Some(frame_scene::LightVariants {
            unshadowed: constants,
            shadowed: constants,
            projection: None,
        }),
    }
}

/// A first-person slot under the scene path, its records replayed: the
/// slot planned as [`Slot::read`] plans it, its light (with its
/// constants) into the first person's view, its layers drawn now on
/// `target` ([`super::draw_cut_light`]) and only then its groups put in.
/// False leaves the slot to the guest's steps.
fn scene_slot(mem: &mut dyn GuestMemory, plan: &first_person::Plan, call: &Call, target: Option<[u32; 8]>) -> bool {
    let slot = match Slot::read(&*mem, call, plan.fpu) {
        Ok(slot) => slot,
        Err(why) => {
            refused(SceneList::FirstPersonLights, why);
            return false;
        }
    };
    super::finish_candidate();
    super::FIRST_PERSON.set(None);
    if slot.steps.drawn() == 0 {
        return true;
    }
    let mut light = slot.scene_light;
    light.candidate = Some(candidate(&*mem, &slot.light, call.definition, slot.constants));
    if !crate::scene_step::first_person_light(&*mem, light, slot.steps.as_slice()) {
        return false;
    }
    let Some(light) = crate::scene_step::current_light() else {
        return false;
    };
    let passes = super::scene_frame::cut_passes(super::FIRST_PERSON_SLOT, &light, &slot.groups);
    if !super::draw_cut_light(mem, &passes, target) {
        KEPT_HOST.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    crate::scene_step::typed_light_groups(slot.groups);
    true
}

struct Slot {
    light: super::Light,
    globals: typed::Globals,
    constants: frame_scene::LightConstants,
    steps: Steps,
    groups: Vec<SceneGroup>,
    scene_light: frame_scene::SceneLight,
    tail: typed_tail::Tail,
}

impl Slot {
    /// The replay has produced this slot's item lists. Plan the whole slot,
    /// including its output ranges and the final target, before asking the
    /// host to reserve any geometry.
    fn read(mem: &dyn GuestMemory, call: &Call, fpu: Fpu) -> Result<Self, Refusal> {
        super::typed::check_view_layers(mem, call.definition, true)?;
        let globals =
            typed::plan(mem, call, fpu, typed::Site::FirstPerson).map_err(|error| match error {
                engine::light_constants::Error::Memory(_) => Refusal::Memory,
                _ => Refusal::Unsupported,
            })?;
        let constants = super::constants_of(mem, &globals.light, globals.flags, call.definition)
            .ok_or(Refusal::Layout)?;
        let inputs = light_passes::read(&Ro(mem), globals.flags, constants.definition_flags)
            .ok_or(Refusal::Layout)?;
        let steps = light_passes::first_person_steps(&inputs);
        if steps.drawn() != 0 {
            // The accepted slot opens this camera's first-person scene view.
            scene_extract::extract::camera(&Ro(mem)).ok_or(Refusal::Layout)?;
        }
        let tail = typed_tail::plan_layers(mem, steps.drawn()).ok_or(Refusal::Unsupported)?;
        let groups = super::typed_groups::plan(
            mem,
            GroupSite::FirstPerson,
            steps.as_slice(),
            globals.flags,
            constants.definition_flags,
            None,
        )
        .ok_or(Refusal::Unsupported)?;
        let light = super::read_light(mem, call.light, false, f32::from_bits(call.weight))
            .ok_or(Refusal::Layout)?;
        let scene_light = super::scene_light(&light, &inputs);
        Ok(Self {
            light,
            globals,
            constants,
            steps,
            groups,
            scene_light,
            tail,
        })
    }
}

fn try_passes(
    mem: &mut dyn GuestMemory,
    plan: &first_person::Plan,
    call: &Call,
) -> Result<(), Refusal> {
    let slot = Slot::read(mem, call, plan.fpu)?;
    let graph = light_graph::Pass::Light {
        site: GroupSite::FirstPerson,
        steps: slot.steps.as_slice(),
        additional: None,
        constants: &slot.constants,
        scissor: None,
        projection: None,
    };
    if !super::typed::submit(
        mem,
        graph,
        &slot.groups,
        &slot.globals,
        &slot.tail,
        Some((
            plan.draw_byte(),
            u8::from(slot.steps.as_slice().last().is_some_and(|step| step.draws)),
        )),
    ) {
        return Err(Refusal::Unsupported);
    }

    let drawn = slot.steps.drawn();
    if drawn != 0 {
        // The ordinary loop opens the light at its first drawn layer.
        // The graph has already consumed all layers of this slot.
        super::finish_candidate();
        super::FIRST_PERSON.set(None);
        if let Some(host) = super::HOST
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            host.clear_admission();
        }
        if crate::scene_step::first_person_light(mem, slot.scene_light, slot.steps.as_slice()) {
            crate::scene_step::typed_light_groups(slot.groups);
        }
    }
    Ok(())
}

/// One count per record that passed the filter: native for a record
/// appended in Rust, the fallback and its reason for one the guest's
/// per-part calls made.
fn count(records: &SlotRecords) {
    for &(_, made) in &records.made {
        match made {
            Made::Rust => SceneList::FirstPersonRecords.count(SceneListResult::Native),
            Made::Guest(why) => refused(SceneList::FirstPersonRecords, why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn a_cut_slots_candidate_carries_its_constants_and_its_definitions_gels() {
        let mut m = FlatMemory::new(0, 0x10000);
        let definition = 0x1000;
        m.set_u16(definition + 4, 3).unwrap();
        m.set_u32(definition + engine::light_passes::DEFINITION_GEL, u32::MAX).unwrap();
        m.set_u32(definition + engine::light_passes::DEFINITION_GEL_SHADER, 0x00e1_0007).unwrap();
        let light = super::super::Light {
            handle: 5,
            definition: 6,
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            colour_a: [0.5; 3],
            colour_b: [0.25; 3],
            intensity: 2.0,
            bounds: ([1.0, 2.0, 3.0], 4.0),
            shadow: false,
            weight: 1.0,
            constants: None,
        };
        let constants = frame_scene::LightConstants {
            frame: [0; 36],
            flags: 3,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: true,
        };
        let c = candidate(&m, &light, definition, constants);
        assert_eq!((c.position, c.weight, c.definition_kind), ([1.0, 2.0, 3.0], 1.0, 3));
        assert_eq!((c.gel_bitmap, c.gel_shader), (None, Some(0x00e1_0007)));
        let v = c.constants.unwrap();
        assert_eq!((v.unshadowed, v.shadowed), (constants, constants), "a first-person light casts no shadow");
        assert!(v.projection.is_none());
    }
}
