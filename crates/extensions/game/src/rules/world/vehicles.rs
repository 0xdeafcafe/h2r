//! Retail vehicle actions. Recheck the salted handle when queued work runs.
mod flight;
mod target;

pub use flight::{Flights, MAX_FLIGHTS, Step, Stepped};

use ::rules::caps::id;
use ::rules::vm::{Arg, Call as VmCall};
use guest_layout::entry::{Cx, GuestCall};
use guest_layout::{GuestMemory, points};

use super::super::census::Table;
use super::super::runtime::Call;

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

    pub async fn apply(self, cx: &Cx, datum: Option<u32>) -> anyhow::Result<()> {
        let object =
            datum.ok_or_else(|| anyhow::anyhow!("the target has no unique live vehicle"))?;
        anyhow::ensure!(
            cx.read(|m| target::is_vehicle(m, object)),
            "vehicle {object:#010x} is no longer live"
        );
        let call = match self {
            Self::Hover(on) => GuestCall::new(points::VEHICLE_HOVER.base)
                .eax(object)
                .ecx(u32::from(on)),
            Self::Unload => GuestCall::new(points::VEHICLE_UNLOAD.base)
                .edx(object)
                .ecx(u32::MAX),
        };
        cx.call(call).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }
}

pub(crate) fn plan(m: &dyn GuestMemory, table: Option<&Table>, call: &VmCall) -> Option<Call> {
    let (action, operand) = match (call.entry.id, call.args.as_slice()) {
        (id::HOVER, [Arg::Object(v), Arg::Bool(on)]) => (Action::Hover(*on), *v),
        (id::UNLOAD, [Arg::Object(v)]) => (Action::Unload, *v),
        _ => return None,
    };
    Some(Call::Vehicle(
        action,
        operand.and_then(|v| target::resolve(m, table, v)),
    ))
}
