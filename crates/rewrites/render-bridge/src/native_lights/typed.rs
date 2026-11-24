//! Whole-pass admission after the current invocation supplies its item lists.
//! A refusal leaves L5 at its next instruction. Only the committed pass puts
//! groups in the scene or replaces guest constants and tail state.

use engine::entry::{EntryError, Ret};
use engine::light_constants as lc;
use engine::memory::{GuestMemory, MemFault};
use engine::render::light_gather::{Refusal, Writes, passes, typed, typed_tail};
use frame_scene::{GroupSite, LightConstants, SceneLight};
use native_render::{light_graph::Pass, light_passes};
use timeline::native::{SceneList, SceneListResult};

use super::{
    CANDIDATE, HOST, Ro, SceneGroup, TypedPasses, VIEW_TARGET, constants_of, typed_groups,
};

/// The bounds helper starts with an empty, exception-free x87 stack.
pub(super) fn fpu_ready(tag: u16, status: u16, control: u16) -> bool {
    tag == u16::MAX && status & 0x80 == 0 && status & !control & 0x3f == 0
}

/// Record the same stores the guest-memory writers make. The device
/// commits them with its own journal after every copy has reserved its draws.
struct Mirror<'a> {
    mem: &'a dyn GuestMemory,
    writes: Writes,
}

impl GuestMemory for Mirror<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        buf.copy_from_slice(&self.writes.read(self.mem, addr, buf.len())?);
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.writes.array(addr, data.to_vec());
        Ok(())
    }
}

pub(super) fn submit(
    mem: &mut dyn GuestMemory,
    graph: Pass<'_>,
    groups: &[SceneGroup],
    globals: &typed::Globals,
    tail: &typed_tail::Tail,
    draw_byte: Option<(u32, u8)>,
) -> bool {
    let mut target = [0; 8];
    target[..2].copy_from_slice(&tail.surfaces);
    target[2..].copy_from_slice(&tail.viewport);
    if VIEW_TARGET.get() != Some(target) {
        return false;
    }
    let Ok(view) = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX) else {
        return false;
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let mut mirror = Mirror {
        mem,
        writes: Writes::new(),
    };
    if globals.write(&mut mirror).is_err() || tail.write(&mut mirror).is_err() {
        return false;
    }
    if let Some((addr, value)) = draw_byte {
        mirror.writes.byte(addr, value);
    }
    let writes = mirror.writes;
    host.draw_typed(
        mem,
        &TypedPasses {
            graph,
            groups,
            view_target: target,
            clip: tail.clip,
            clip_header: tail.clip_header,
            writes: &writes,
            view,
        },
    )
}

struct Dynamic {
    globals: typed::Globals,
    tail: typed_tail::Tail,
    constants: LightConstants,
    groups: Vec<SceneGroup>,
    steps: light_passes::Steps,
}

fn plan(mem: &dyn GuestMemory, p: &passes::Plan) -> Result<Dynamic, Refusal> {
    let casters = mem.u32(light_passes::CASTERS + 8)?;
    let shadow =
        engine::light_passes::shadow_argument(mem.u32(p.entry_esp.wrapping_add(12))?, casters);
    let call = lc::Call {
        light: p.light,
        frame: p.position,
        definition: p.data,
        context: p.context,
        shadow,
        weight: mem.u32(p.entry_esp.wrapping_add(16))?,
    };
    let globals =
        typed::plan(mem, &call, p.fpu, typed::Site::Dynamic).map_err(|error| match error {
            lc::Error::Memory(_) => Refusal::Memory,
            _ => Refusal::Unsupported,
        })?;
    let constants =
        constants_of(mem, &globals.light, globals.flags, p.data).ok_or(Refusal::Memory)?;
    if constants.animated_gel {
        return Err(Refusal::Unsupported);
    }
    let inputs = light_passes::read(&Ro(mem), globals.flags, constants.definition_flags)
        .ok_or(Refusal::Memory)?;
    let steps = light_passes::steps(&inputs);
    let scene_light = SceneLight {
        handle: call.light,
        definition: p.definition,
        flags: inputs.flags,
        definition_flags: inputs.definition_flags,
        additional_casters: inputs.additional_casters,
        guest_weight: Some(f32::from_bits(call.weight)),
        ..Default::default()
    };
    if !crate::scene_step::current_light_matches(&scene_light, steps.as_slice()) {
        return Err(Refusal::Layout);
    }
    // The equivalent scene-alone walk would return no entries to the
    // guest handlers only when this exact candidate still owns every draw.
    if steps.drawn() != 0
        && !CANDIDATE.with_borrow(|candidate| {
            candidate.as_ref().is_some_and(|c| {
                !c.first_person
                    && c.inside.is_none()
                    && c.layers == steps.drawn()
                    && c.light.handle == p.light
                    && c.light.constants == Some(constants)
            })
        })
    {
        return Err(Refusal::Layout);
    }
    let tail = typed_tail::plan_dynamic(mem, p, globals.flags, steps.drawn())
        .ok_or(Refusal::Unsupported)?;
    // The same-target tail only covers direct view draws. Additional
    // casters produce their own list suffixes between the base layers.
    if inputs.additional_casters || globals.shadow.is_some() {
        return Err(Refusal::Unsupported);
    }
    let groups = typed_groups::plan(
        mem,
        GroupSite::Light,
        steps.as_slice(),
        globals.flags,
        constants.definition_flags,
        None,
    )
    .ok_or(Refusal::Unsupported)?;
    Ok(Dynamic {
        globals,
        tail,
        constants,
        steps,
        groups,
    })
}

pub(super) fn dynamic(
    mem: &mut dyn GuestMemory,
    p: &passes::Plan,
) -> Result<Option<(Ret, passes::LightPasses)>, EntryError> {
    let stage = SceneList::TypedLightPasses;
    let pass = match plan(mem, p) {
        Ok(pass) => pass,
        Err(why) => {
            super::frame::refused(stage, why);
            return Ok(None);
        }
    };
    let graph = Pass::Light {
        site: GroupSite::Light,
        steps: pass.steps.as_slice(),
        additional: None,
        constants: &pass.constants,
        scissor: None,
        projection: None,
    };
    if !submit(mem, graph, &pass.groups, &pass.globals, &pass.tail, None) {
        super::frame::refused(stage, Refusal::Unsupported);
        return Ok(None);
    }
    crate::scene_step::typed_light_groups(&pass.groups);
    CANDIDATE.take();
    stage.count(SceneListResult::Native);
    Ok(Some((
        Ret::new(pass.tail.eax, 0x10),
        passes::LightPasses {
            shadow: pass.globals.light.shadow != 0,
            flags: pass.globals.flags,
            reached: pass.steps.iter().fold(0, |mask, s| mask | (1 << s.layer)),
            drawn: pass.steps.drawn(),
            additional_casters: false,
        },
    )))
}
