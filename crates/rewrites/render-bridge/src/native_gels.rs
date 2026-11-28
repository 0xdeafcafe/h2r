//! Shader-gel production at its original template/quad sites. On with
//! the native renderer (`impl.render.native`, on by default). Guest setup
//! and geometry assembly execute; draw admission
//! transactionally replaces only complete target-13 packets.

use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use guest_layout::{memory::GuestMemory, points};
use native_render::gels;
use std::sync::{Arc, Mutex};

pub const ID: &str = "native-gels";

pub fn on(_: &hooks::Switches) -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub shader: u32,
    pub definition: u32,
}

pub trait Host: Send + Sync {
    /// Every template entry invalidates the previous pending gel. Only
    /// the exact gel site has `Some(call)`.
    fn template(&self, mem: &dyn GuestMemory, call: Option<Call>);
    fn quad(&self, mem: &mut dyn GuestMemory);
    fn quad_end(&self, mem: &mut dyn GuestMemory);
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub fn producer_call(mem: &dyn GuestMemory, esp: u32) -> Option<Call> {
    let ret = mem.u32(esp).ok()?;
    let mut args = [0; 6];
    for (n, a) in args.iter_mut().enumerate() {
        *a = mem.u32(esp.checked_add(4 + n as u32 * 4)?).ok()?;
    }
    if !gels::is_producer(args, ret, mem.u8(gels::BUILDING).ok()?) {
        return None;
    }
    let definition = mem.u32(gels::DEFINITION).ok()?;
    if definition == 0 || mem.u32(definition.checked_add(0xe0)?).ok()? != args[0] {
        return None;
    }
    Some(Call {
        shader: args[0],
        definition,
    })
}

pub struct NativeGels;
impl Feature for NativeGels {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Shader-driven light gels into native target 13")
            .doc("docs/graphics/native-gels.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .before(&points::TEMPLATE_PASS, "native_gels_template", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mem = rt.memory();
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(host) = host {
                    host.template(&*mem, producer_call(&*mem, c.cpu.esp));
                }
                Ok(Flow::Continue)
            })
            .after_feature(crate::native_post::ID)
            .context_only();
        b.raw()
            .before(&points::TEMPLATE_QUAD, "native_gels_quad", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                if mem.u32(c.cpu.esp).ok() == Some(gels::QUAD_RETURN)
                    && mem.u32(c.cpu.esp.wrapping_add(4)).ok() == Some(gels::TARGET)
                    && mem.u8(gels::BUILDING).ok() == Some(1)
                {
                    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(host) = host {
                        host.quad(&mut *mem);
                    }
                }
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw()
            .after(&points::TEMPLATE_QUAD, "native_gels_quad_end", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(host) = host {
                    host.quad_end(&mut *rt.memory());
                }
                Ok(())
            })
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}
