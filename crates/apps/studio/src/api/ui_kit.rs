//! `ui.consistency` and `ui.components`: the design system's checks and
//! its catalogue (docs/game/ui-builder.md "The design system" and
//! "Consistency"). Both read the kit alone: no disc, no window state.

use serde_json::Value;
use studio_proto::{code, method as m};

use super::{P, Reply, err};
use crate::App;

pub fn call(_app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::UI_CONSISTENCY => consistency(p),
        m::UI_COMPONENTS => Ok(boot::ui_kit::components::catalogue_json()),
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

/// Every screen (or `screen`) in every state, against the design system.
fn consistency(p: &P) -> Reply {
    let screen = p.opt_str("screen")?;
    if let Some(s) = screen
        && !boot::ui_kit::screens::NAMES.contains(&s)
    {
        return Err(err(code::INVALID_PARAMS, format!("no screen {s:?}")));
    }
    let v: Value = boot::ui_kit::consistency::report_json(screen);
    Ok(v)
}
