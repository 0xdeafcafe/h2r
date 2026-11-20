//! `input.host` (`h2 agent hostpads`): the host's controllers as the input
//! layer sees them, and the debug menu's gate (docs/runtime/agent-api.md).
//!
//! Three views, so one call made while pressing a button shows where the
//! press stops:
//!
//! * `devices`: each controller as its own backend reads it: for Apple
//!   GameController the profile class (`dualsense`, `dualshock4`,
//!   `extended`, `micro`) and the product category, the touchpad, the
//!   buttons ([`input::report::DeviceInfo`]).
//! * `pads`: each pad the input manager polled, as the gate is handed it,
//!   and its port. With the keyboard on, player one is the first pad merged
//!   with the keyboard ([`input::report::PadInfo`]).
//! * `gate`: whether the debug menu is open, and per pad whether it has a
//!   touchpad, the click, the chord, and Start waiting for the click
//!   ([`debug_menu::gate::GateReport`]). `null` with no menu.
//!
//! `notes` says, in words, where a touchpad goes missing.

use agent_proto::method;
use debug_menu::Shared;
use debug_menu::gate::GateReport;
use input::host::{HostButton, HostButtons, HostPadState, Touchpad};
use input::report::{HostReport, HostReportHandle};
use serde_json::{Value, json};

use crate::{Access, Agent};

/// Adds `input.host` over the input manager's report and, when there is
/// one, the debug menu.
pub fn add_host_pads(agent: &Agent, report: HostReportHandle, menu: Option<Shared>) {
    agent.add_method(method::INPUT_HOST, Access::Read, move |_| {
        let gate = menu.as_ref().map(Shared::gate_report);
        Ok(host_pads_json(&report.get(), gate.as_ref()))
    });
}

fn buttons(b: HostButtons) -> Vec<&'static str> {
    HostButton::ALL
        .into_iter()
        .filter(|x| b.contains(*x))
        .map(HostButton::name)
        .collect()
}

fn touchpad(t: Option<Touchpad>) -> Value {
    match t {
        None => Value::Null,
        Some(t) => json!({
            "click": t.click,
            "primary": t.primary.map(|(x, y)| [x, y]),
            "secondary": t.secondary.map(|(x, y)| [x, y]),
        }),
    }
}

fn state(s: &HostPadState) -> Value {
    json!({
        "touchpad": touchpad(s.touchpad),
        "buttons": buttons(s.buttons),
        "left_stick": [s.left_stick.0, s.left_stick.1],
        "right_stick": [s.right_stick.0, s.right_stick.1],
        "triggers": [s.left_trigger, s.right_trigger],
    })
}

/// The answer to `input.host`.
pub fn host_pads_json(r: &HostReport, gate: Option<&(bool, GateReport)>) -> Value {
    let devices: Vec<Value> = r
        .devices
        .iter()
        .map(|d| {
            let mut v = state(&d.state);
            v["backend"] = json!(d.backend);
            v["id"] = json!(d.id);
            v["name"] = json!(d.name);
            v["profile"] = json!(d.profile);
            v["category"] = json!(d.category);
            v
        })
        .collect();
    let pads: Vec<Value> = r
        .pads
        .iter()
        .map(|p| {
            let mut v = state(&p.state);
            v["backend"] = json!(p.backend);
            v["key"] = json!([p.key.0, p.key.1]);
            v["name"] = json!(p.name);
            v["physical"] = json!(p.physical);
            v["port"] = json!(p.port);
            v
        })
        .collect();
    let gate_json = gate.map(|(open, g)| {
        let pads: Vec<Value> = g
            .pads
            .iter()
            .map(|p| {
                json!({
                    "key": [p.key.0, p.key.1],
                    "touchpad": p.touchpad,
                    "click": p.click,
                    "chord": p.chord,
                    "chord_pending": p.chord_pending,
                    "opened_by_press": p.opened_by_press,
                    "latched": buttons(p.latched),
                })
            })
            .collect();
        json!({ "open": open, "syncs": g.syncs, "pads": pads })
    });
    json!({
        "syncs": r.syncs,
        "devices": devices,
        "pads": pads,
        "gate": gate_json,
        "notes": notes(r, gate),
    })
}

/// Where a touchpad goes missing, in words.
fn notes(r: &HostReport, gate: Option<&(bool, GateReport)>) -> Vec<String> {
    let mut out = Vec::new();
    if r.syncs == 0 {
        out.push("input has not synced yet: the game hasn't read its pads".to_owned());
        return out;
    }
    let has = |p: &Option<String>| matches!(p.as_deref(), Some("dualsense" | "dualshock4"));
    for d in &r.devices {
        if d.state.touchpad.is_none() && d.profile.as_deref() == Some("extended") {
            out.push(format!(
                "{:?}: GameController gives it a plain extended profile, not a DualSense or \
                 DualShock 4 one, so it has no touchpad",
                d.name
            ));
        } else if has(&d.profile) && d.state.touchpad.is_none() {
            out.push(format!(
                "{:?}: a {} profile without touchpad elements",
                d.name,
                d.profile.as_deref().unwrap_or_default()
            ));
        }
    }
    let device_touchpad = r.devices.iter().any(|d| d.state.touchpad.is_some());
    let pad_touchpad = r.pads.iter().any(|p| p.state.touchpad.is_some());
    if device_touchpad && !pad_touchpad {
        out.push(
            "a controller has a touchpad but no pad the gate sees does: it is lost between \
             the backend and the gate"
                .to_owned(),
        );
    }
    match gate {
        None => {
            out.push("no debug menu in this game (H2_DEBUG_MENU=0, or not a window)".to_owned())
        }
        Some((_, g)) if g.syncs == 0 => {
            out.push("the debug menu's gate has not seen a sync: it is not on the input".to_owned())
        }
        Some(_) => {}
    }
    out
}

#[cfg(test)]
mod tests;
