//! Every `impl.*` switch, in one table: name, default, owner, the date it
//! appeared and the condition for deleting it (docs/reviews/2026-11-20-architecture.md
//! section 2.6).
//!
//! A switch not in the table does not parse ([`super::Switches::parse`]
//! refuses it), and a test scans the sources so every `impl.*` name the
//! code reads is listed. [`BUDGET`] bounds the count: adding a switch means
//! raising it in the same commit, where a reviewer sees it. Each row's
//! `remove` says when it goes; a switch off for more than one integration
//! cycle is deleted with its code or turned on.
//!
//! Besides the rows, two kinds of name parse:
//!
//! - a parent of a row (`impl.sim`, `impl.render`): `implementation()`
//!   falls back through it;
//! - an engine leaf, `impl.engine.<group>.<function>`, one per adapter in
//!   `guest_layout::abi::OVERRIDES` (`crate::leaves`), so the table carries the
//!   groups only.

/// What a switch is when nothing sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initial {
    On,
    Off,
    /// A parent with no value of its own: setting it sets every child
    /// that `implementation()` reads and that has no setting of its own.
    Group,
}

/// One switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switch {
    pub name: &'static str,
    pub initial: Initial,
    /// The area that owns it and answers for removing it.
    pub owner: &'static str,
    /// The day it appeared in the code (`git log -S`).
    pub added: &'static str,
    /// When it is deleted.
    pub remove: &'static str,
}

/// The most rows the table may hold. Raise it only with a reason in the
/// commit; lower it when a switch goes.
pub const BUDGET: usize = 23;

use Initial::{Group, Off, On};

const fn sw(
    name: &'static str,
    initial: Initial,
    owner: &'static str,
    added: &'static str,
    remove: &'static str,
) -> Switch {
    Switch {
        name,
        initial,
        owner,
        added,
        remove,
    }
}

const DIAGNOSTIC: &str = "a diagnostic: becomes an agent API toggle, then goes";
const EXACT: &str = "both paths are exact: make the default unconditional";

pub const TABLE: &[Switch] = &[
    // Rendering.
    sw("impl.render.native", On, "render", "2026-11-14",
        "the native renderer owns the frame (review 4.3): delete with the D3D path"),
    sw("impl.render.native.visibility", On, "render", "2026-11-27",
        "the guest's view projection and visibility producers replaced (retire-d3d.md R4): delete when R4's differentials hold on the gate corpus"),
    sw("impl.render.native-hud", On, "render", "2026-11-27",
        "HUD widget quads from their Rust material, no guest shader setup (docs/graphics/native-hud-overlay.md): deleted with the guest HUD walk (retire-d3d R6)"),
    sw("impl.render.native-hud-walk", On, "render", "2026-11-27",
        "HUD bitmap, text and screen-effect widgets laid out in Rust, no guest draw once proven (docs/graphics/native-hud-overlay.md): deleted with the guest HUD walk (retire-d3d R6)"),
    sw("impl.render.native-decal-decorators", On, "render", "2026-11-28",
        "decal decorators drawn at their sorted place, the draw 0x4ebd0 cut once a frame proves the stage (docs/graphics/retire-d3d-decal-decorators.md): deleted with the D3D path (R10)"),
    // Engine leaves (the per-function keys are `crate::leaves`').
    sw("impl.engine", Group, "engine", "2026-11-16", "goes with its last group"),
    sw("impl.engine.locale", On, "engine", "2026-11-16", EXACT),
    sw("impl.engine.tags", On, "engine", "2026-11-16",
        "hot leaves: on once thin transfers make an override free (review 5 item 1), else delete"),
    sw("impl.engine.datums", On, "engine", "2026-11-16",
        "hot leaves: on once thin transfers make an override free (review 5 item 1), else delete"),
    // Audio.
    sw("impl.audio.packet-clock", On, "audio", "2026-11-20", EXACT),
    // Simulation: the subsystem switches.
    sw("impl.sim", Group, "sim", "2026-11-20", "goes with its last child"),
    sw("impl.sim.native", Group, "sim", "2026-11-20",
        "the parent of the subsystem rewrites (lane rules): each child goes when its subsystem defaults on"),
    sw("impl.sim.native.sound", Off, "sim", "2026-11-25",
        "phase 4 sound simulation: promote after voice, arena and timing parity"),
    sw("impl.sim.present", On, "sim", "2026-11-27",
        "ticks with no local user skip the presentation phases (sim-bridge/src/present.rs); h2 sim diff of lockout-2v2 is identical over 9000 ticks"),
    sw("impl.sim.sound-events", On, "sim", "2026-11-27",
        "the sound simulation (docs/engine/sound-sim.md); h2 sim diff of firefight-beavercreek is identical over 10351 ticks"),
    sw("impl.sim.sound-events.table", Off, "sim", "2026-11-27",
        "the sound table's entry bodies and tick phase (docs/engine/sound-instances.md): on after h2 sim diff parity with sound.table in the series"),
    sw("impl.sim.arena-hash", Off, "sim", "2026-11-20", DIAGNOSTIC),
    sw("impl.sim.frame-guard", Off, "sim", "2026-11-28",
        "the frame guard (lockstep::frame_guard, docs/engine/sim-render-split.md): a test mode; goes when the render-blind gate (slice 10) holds the rule in h2 sim and the ladder"),
    sw("impl.sim.metrics-projection", On, "sim", "2026-11-20", EXACT),
    sw("impl.sim.page-census", On, "sim", "2026-11-22",
        "the guest page census in the report (docs/perf/sim-light.md, L1): a timing arm turns it off; deleted when the page figures are measured for good"),
    sw("impl.sim.native_simd", On, "sim", "2026-11-20", EXACT),
    sw("impl.sim.phantom-alias", On, "sim", "2026-11-22",
        "the phantoms' action build on live RAM with an undo journal (docs/perf/sim-phantom-build.md, P1): the view path stays as the check mode's reference and the flat-RAM path; the switch goes once a release has run with it on"),
];

/// The row named `name`.
pub fn get(name: &str) -> Option<&'static Switch> {
    TABLE.iter().find(|s| s.name == name)
}

/// Whether `name` (a full `impl.*` key) parses: a row, a parent of one, or
/// an engine leaf's key.
pub fn known(name: &str) -> bool {
    if get(name).is_some() {
        return true;
    }
    let parent = TABLE.iter().any(|s| {
        s.name
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('.'))
    });
    if parent {
        return true;
    }
    if let Some(rest) = name.strip_prefix("impl.engine.")
        && let Some((group, function)) = rest.split_once('.')
    {
        return guest_layout::abi::OVERRIDES
            .iter()
            .any(|o| o.name == function && crate::leaves::group(o.name) == group);
    }
    false
}

/// The table as text, one switch a line, for `--help`-style listings.
pub fn text() -> String {
    TABLE
        .iter()
        .map(|s| {
            let initial = match s.initial {
                On => "on".to_owned(),
                Off => "off".to_owned(),
                Group => "group".to_owned(),
            };
            format!("{} [{initial}] {} {}: {}", s.name, s.owner, s.added, s.remove)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
