//! Install the object-shadow list, row gather and mask caster as three
//! independent entries. The list is a cut (`crate::cut`, `object_shadows`):
//! a call the Rust body cannot take, or a child it refuses, panics
//! (`lights: object_shadows <entry> <cause>`), and the guest body never
//! runs. The shadows draw in [`SLOT`], before layer 7.

use engine::entry::{EntryError, Ret};
use engine::light_constants::Fpu;
use engine::render::light_gather::{
    Result,
    object_shadows::{self, caster, rows},
};
use engine::{memory::GuestMemory, points};
use hooks::{Builder, Cx, EntryFuture};
use timeline::native::{SceneList, SceneListResult};

use crate::native_map_work::Stage;
use frame_scene::ViewSlot;

/// The view slot the object shadows draw in: slot 3, before layer 7.
#[allow(dead_code)] // read once the shadows draw in their slot (nor-6c)
pub const SLOT: ViewSlot = ViewSlot::ShadowsAndLayer7;

#[derive(Clone, Copy)]
enum Entry {
    List,
    Rows,
    Caster,
}

enum Body {
    List(object_shadows::Plan),
    Rows(rows::Plan),
    /// The caster: it stops after its item lists, and its shadow goes
    /// into the scene ([`super::object_shadow_scene::cut`]).
    Caster(caster::Plan),
}

impl Entry {
    /// Argument bytes the entry pops (`ret n`): the list's one word, the
    /// row gather's and the caster's two.
    fn pop(self) -> u16 {
        match self {
            Self::List => 4,
            Self::Rows | Self::Caster => 8,
        }
    }

    fn plan(self, m: &dyn GuestMemory, esp: u32, ebx: u32, fpu: Fpu) -> Result<Body> {
        match self {
            Self::List => object_shadows::plan(m, esp).map(Body::List),
            Self::Rows => rows::plan(m, esp, ebx, fpu).map(Body::Rows),
            Self::Caster => {
                if super::has_host() && !super::scene_path_ready() {
                    unimplemented!("lights: object_shadows host");
                }
                caster::plan(m, esp, fpu).map(Body::Caster)
            }
        }
    }
}

impl Body {
    async fn run(self, cx: &Cx) -> Result<Ret, EntryError> {
        match self {
            Self::List(p) => object_shadows::run(cx, p).await,
            Self::Rows(p) => rows::run(cx, p).await.map(|(ret, _)| ret),
            Self::Caster(p) => {
                caster::run_with(cx, p, |m, p| {
                    super::object_shadow_scene::cut(&*m, p.fpu());
                    true
                })
                .await
            }
        }
    }
}

pub(super) fn install(b: &mut Builder) {
    for (entry, point, label, stage) in [
        (
            Entry::List,
            &points::OBJECT_SHADOW_LIST,
            "native_lights_object_shadow_list",
            SceneList::ObjectShadowList,
        ),
        (
            Entry::Rows,
            &points::GATHER_OBJECT_SHADOWS,
            "native_lights_object_shadow_rows",
            SceneList::ObjectShadowRows,
        ),
        (
            Entry::Caster,
            &points::OBJECT_SHADOW,
            "native_lights_object_shadow_caster",
            SceneList::ObjectShadowCaster,
        ),
    ] {
        b.raw()
            .implement(point, label, move |cpu, ctx, addr| {
                if addr != point.base {
                    unimplemented!("lights: object_shadows {label} at {addr:#010x}");
                }
                if Stage::Lights.standing_down() {
                    return dropped(entry, cpu, ctx);
                }
                let fpu = Fpu {
                    mxcsr: cpu.mxcsr,
                    x87_control: cpu.x87.control,
                };
                let body = {
                    let runtime = std::sync::Arc::clone(ctx.runtime());
                    let mem = runtime.memory();
                    entry.plan(&*mem, cpu.esp, cpu.ebx, fpu)
                };
                let body = match body {
                    Ok(body) => body,
                    Err(why) => unimplemented!("lights: object_shadows {label} {why:?}"),
                };
                hooks::entry::start_render(cpu, ctx, addr, move |cx: Cx| -> EntryFuture {
                    Box::pin(async move {
                        let ret = body.run(&cx).await?;
                        stage.count(SceneListResult::Native);
                        Ok(ret)
                    })
                })
            })
            .context_only();
    }
}

/// The call answered with nothing drawn on a map with nothing native for
/// the lights: no rows, no caster. The list's cut makes the frame step's
/// writes first, as every cut does.
fn dropped(
    entry: Entry,
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
) -> std::result::Result<runtime::TransferAbi, runtime::GuestFault> {
    use runtime::guest::{Access, FaultReason};
    let mut guest = ctx.memory();
    if matches!(entry, Entry::List) {
        crate::frame_step::before(&mut *guest, "object_shadows").map_err(|f| {
            runtime::GuestFault {
                addr: f.addr,
                len: f.len as usize,
                access: if f.write { Access::Write } else { Access::Read },
                reason: FaultReason::Unmapped,
            }
        })?;
    }
    cpu.eax = 0;
    guest.ret(cpu, u32::from(entry.pop()))
}
