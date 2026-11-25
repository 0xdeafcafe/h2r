//! Install the object-shadow list, row gather and mask caster as three
//! independent entries. A refused child runs whole at its own call site.

use engine::entry::{EntryError, Ret};
use engine::light_constants::Fpu;
use engine::render::light_gather::{
    Result,
    object_shadows::{self, caster, rows},
};
use engine::{memory::GuestMemory, points};
use hooks::{Builder, Cx, EntryFuture};
use timeline::native::{SceneList, SceneListResult};

use super::frame::{original, refused};
use crate::native_map_work::Stage;

#[derive(Clone, Copy)]
enum Entry {
    List,
    Rows,
    Caster,
}

enum Body {
    List(object_shadows::Plan),
    Rows(rows::Plan),
    Caster(caster::Plan),
}

impl Entry {
    fn plan(self, m: &dyn GuestMemory, esp: u32, ebx: u32, fpu: Fpu) -> Result<Body> {
        match self {
            Self::List => object_shadows::plan(m, esp).map(Body::List),
            Self::Rows => rows::plan(m, esp, ebx, fpu).map(Body::Rows),
            Self::Caster => caster::plan(m, esp, fpu).map(Body::Caster),
        }
    }
}

impl Body {
    async fn run(self, cx: &Cx) -> Result<Ret, EntryError> {
        match self {
            Self::List(p) => object_shadows::run(cx, p).await,
            Self::Rows(p) => rows::run(cx, p).await.map(|(ret, _)| ret),
            Self::Caster(p) => caster::run(cx, p).await,
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
                if addr != point.base || Stage::Lights.standing_down() {
                    return original(cpu, ctx, addr);
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
                    Err(why) => {
                        refused(stage, why);
                        return original(cpu, ctx, addr);
                    }
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
