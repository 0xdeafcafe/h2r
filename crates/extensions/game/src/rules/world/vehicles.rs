//! Retail vehicle actions. Recheck the salted handle when queued work runs.
use engine::entry::{Cx, GuestCall};
use engine::handle::DatumHandle;
use engine::view::objects;
use engine::{GuestMemory, points};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Hover(bool),
    Unload,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hover(_) => "hover",
            Self::Unload => "unload",
        }
    }

    fn call(self, m: &dyn GuestMemory, object: u32) -> anyhow::Result<GuestCall> {
        let missing = || anyhow::anyhow!("{}: {object:#010x} is not a live vehicle", self.name());
        let handle = DatumHandle::from_raw(object).ok_or_else(missing)?;
        match objects::get(m, handle)
            .map_err(|e| anyhow::anyhow!("guest memory at {:#x} faulted", e.addr))?
        {
            Some((objects::VEHICLE, _)) => Ok(match self {
                Self::Hover(on) => GuestCall::new(points::VEHICLE_HOVER.base)
                    .eax(object)
                    .ecx(u32::from(on)),
                Self::Unload => GuestCall::new(points::VEHICLE_UNLOAD.base)
                    .edx(object)
                    .ecx(u32::MAX),
            }),
            _ => Err(missing()),
        }
    }

    pub async fn apply(self, cx: &Cx, object: u32) -> anyhow::Result<()> {
        let call = cx.read(|m| self.call(m, object))?;
        cx.call(call).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }
}
