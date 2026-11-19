//! Halo 1 object functions as Halo 2 ones, so what Halo 1 scales by an
//! object's A to D output (an engine's looping sound, a headlight) is
//! scaled the same way in Halo 2.
//!
//! Halo 2 (E): an attachment's primary scale (`obje +0x94` element
//! `+0x10`) is a name that `0x000bab40` evaluates on the object: an empty
//! name is 1, `one` 1 and `zero` 0; then the object's type handlers
//! (`0x00108d90`, through the type's chain at `0x00468630`, each handler
//! at `+0x4c`: object `0x000bcc20`, unit `0x000c6b90`, vehicle
//! `0x000f6d10`) evaluate their import names; then the object's
//! functions (`obje +0x64`, `0x20` each) by their export name at `+8`.
//! A name none of them knows gives 0, which the looping sound takes as
//! off (`0x0018a826`). A function (`0x000ba8c0`) evaluates its import
//! name (`+4`) through the same handlers, inverts it when flag bit 0 is
//! set, and maps it through its data (`+0x14`, `0x0013bb90`: the
//! function blob of `tags::effects::function`, its result mapped onto
//! the header's range; empty data is 0).
//!
//! Halo 1: each function is "one" scaled by an input (A to D in,
//! [`crate::h1::FunctionInputs`]), then bounded; the attachment's primary
//! scale names its output by index ([`crate::h1::Attachment`]). Every
//! Halo 1 vehicle's function has that shape
//! (`tests/object_functions_disc.rs`).

use tags::effects::function::{Function, FunctionType};

use crate::h1;

/// The export name a converted function's output takes, by Halo 1 output
/// index (A to D). Not an import name of any object type, so the lookup
/// reaches the functions block (the handlers come first).
pub const OUTPUT_NAMES: [&str; 4] = ["a_out", "b_out", "c_out", "d_out"];

/// Halo 2's import name for Halo 1's `obje` input source (`+0x108`), or
/// `None` where Halo 2 has none. Halo 1's enum: 0 none, 1 body vitality,
/// 2 shield vitality, 3 recent body damage, 4 recent shield damage, 5
/// random constant, 6 to 9 the umbrella shield's, 10 to 17 region
/// damage, 18 alive, 19 compass. The Halo 2 names are the object
/// handler's (`0x000bcc20`; `tests/object_functions_disc.rs`).
pub fn object_import(source: u16) -> Option<&'static str> {
    Some(match source {
        1 => "body_vitality",
        2 => "shield_vitality",
        3 => "current_body_damage",
        4 => "current_shield_damage",
        5 => "random_constant",
        18 => "alive",
        19 => "compass",
        _ => return None,
    })
}

/// Halo 2's import name for Halo 1's unit input source (`unit +0x198`):
/// 1 driver seat power, 2 gunner seat power, 3 aiming change, 4 mouth
/// aperture, 5 integrated light power, 6 can blink, 7 shield sapping.
/// The Halo 2 names are the unit handler's (`0x000c6b90`), and
/// `driver_seat_power`, which retail vehicles import (the warthog's
/// headlights) though no handler compares it outright; `gunner_seat_power`
/// is neither, so it isn't written.
pub fn unit_import(source: u16) -> Option<&'static str> {
    Some(match source {
        1 => "driver_seat_power",
        3 => "aiming_change",
        4 => "mouth_aperture",
        5 => "integrated_light_power",
        6 => "can_blink",
        _ => return None,
    })
}

/// Halo 2's import name for Halo 1's vehicle input source (`vehi
/// +0x31c`). Halo 1's enum, in order from 1: speed absolute, forward,
/// backward; slide absolute, left, right; speed slide maximum; turn
/// absolute, left, right; crouch, jump, walk; velocity air, water,
/// ground, forward, left, up; left and right tread position and
/// velocity; each tyre's (front left, front right, back left, back
/// right) position and velocity; wingtip contrail, hover, thrust, engine
/// hack, wingtip contrail new. V: Halo 2 keeps these names (`jump`,
/// `hover`, `thrust`, `engine_hack` among the vehicle handler's,
/// `0x000f6d10`), and the retail warthog's taillights scale by `jump`
/// where Halo 1's by its source 12. A name the handler doesn't compare
/// outright (it searches its names as a tree) is left out, empty here:
/// a name nothing evaluates would turn its attachments off.
pub fn vehicle_import(source: u16) -> Option<&'static str> {
    const NAMES: [&str; 36] = [
        "speed_absolute",
        "speed_forward",
        "speed_backward",
        "slide_absolute",
        "", // slide_left: not proven
        "slide_right",
        "speed_slide_maximum",
        "turn_absolute",
        "turn_left",
        "turn_right",
        "crouch",
        "jump",
        "", // walk: not proven
        "velocity_air",
        "", // velocity_water: not proven
        "velocity_ground",
        "velocity_forward",
        "", // velocity_left: not proven
        "velocity_up",
        "", // left_tread_position: not proven
        "right_tread_position",
        "left_tread_velocity",
        "right_tread_velocity",
        "front_left_tire_position",
        "front_left_tire_velocity",
        "front_right_tire_position",
        "front_right_tire_velocity",
        "back_left_tire_position",
        "back_left_tire_velocity",
        "back_right_tire_position",
        "", // back_right_tire_velocity: not proven
        "wingtip_contrail",
        "hover",
        "thrust",
        "engine_hack",
        "wingtip_contrail_new",
    ];
    NAMES
        .get(usize::from(source).checked_sub(1)?)
        .copied()
        .filter(|n| !n.is_empty())
}

/// Weapon source 14 reads the power of a unit's integrated light. Halo 1
/// forwards that power to the held weapon at `0x45741`. The boot feature
/// makes Halo 2's regular integrated-light input available on weapons.
fn weapon_import(source: u16) -> Option<&'static str> {
    (source == 14).then_some("integrated_light_power")
}

/// The Halo 2 import name of input `slot` (0 to 3, A to D): the
/// vehicle's source where it names one, else the unit's, else the
/// object's ([`h1::FunctionInputs`]). `None` when none is set or the one
/// set has no Halo 2 name.
pub fn input_import(inputs: &h1::FunctionInputs, slot: usize) -> Option<&'static str> {
    let v = inputs.vehicle[slot];
    let u = inputs.unit[slot];
    let w = inputs.weapon[slot];
    if v != 0 {
        vehicle_import(v)
    } else if u != 0 {
        unit_import(u)
    } else if w != 0 {
        weapon_import(w)
    } else {
        object_import(inputs.object[slot])
    }
}

/// One converted function: the `obje +0x64` element's fields.
#[derive(Clone, Debug, PartialEq)]
pub struct FunctionSpec {
    /// `+0`: Halo 1's invert (bit 0) and always active (bit 2), which
    /// Halo 2 reads at the same bits (E `0x000ba966` inverts on bit 0;
    /// V: every retail vehicle's engine function holds 4, as Halo 1's
    /// engine functions do).
    pub flags: u32,
    /// `+4`, the input.
    pub import: String,
    /// `+8`, [`OUTPUT_NAMES`].
    pub export: String,
    /// `+0xc`, the export of the function that turns it off, or empty.
    pub turn_off_with: String,
    /// `+0x14`, the function blob.
    pub data: Vec<u8>,
}

/// Halo 1's functions as Halo 2's, by output index: `None` for one this
/// can't carry (its reason in `notes`). An attachment naming such an
/// output keeps an empty primary scale (1, as before).
pub fn convert(
    functions: &[h1::Function],
    inputs: &h1::FunctionInputs,
    notes: &mut Vec<String>,
) -> Vec<Option<FunctionSpec>> {
    let mut out: Vec<Option<FunctionSpec>> = functions
        .iter()
        .take(4)
        .enumerate()
        .map(|(k, f)| match one(f, inputs) {
            Ok(mut s) => {
                s.export = OUTPUT_NAMES[k].into();
                if f.flags & 2 != 0 {
                    notes.push(format!("function {}: Halo 1's additive flag dropped", OUTPUT_NAMES[k]));
                }
                Some(s)
            }
            Err(why) => {
                notes.push(format!("function {}: not converted ({why})", OUTPUT_NAMES[k]));
                None
            }
        })
        .collect();
    let exports: Vec<Option<String>> = out.iter().map(|s| s.as_ref().map(|s| s.export.clone())).collect();
    for (s, f) in out.iter_mut().zip(functions) {
        if let (Some(s), Ok(i)) = (s, usize::try_from(f.turn_off_with)) {
            s.turn_off_with = exports.get(i).cloned().flatten().unwrap_or_default();
        }
    }
    out
}

/// Halo 1's function as a Halo 2 one, its export still empty.
fn one(f: &h1::Function, inputs: &h1::FunctionInputs) -> Result<FunctionSpec, String> {
    if f.function != 0 {
        return Err(format!("function {} is not one", f.function));
    }
    if f.map_to != 0 || f.add != 0 || f.scale_result_by != 0 || f.step_count > 1 || f.sawtooth_count > 1 {
        return Err("a shape past one scaled by an input".into());
    }
    let import = match f.scale_function_by {
        0 => "one",
        k @ 1..=4 => input_import(inputs, usize::from(k - 1))
            .ok_or_else(|| format!("input {k} has no Halo 2 import"))?,
        k => return Err(format!("scaled by output {}", k - 4)),
    };
    Ok(FunctionSpec {
        flags: f.flags & 0b101,
        import: import.into(),
        export: String::new(),
        turn_off_with: String::new(),
        data: bounded(f.bounds_mode, f.bounds)?,
    })
}

/// Halo 1's bounds as a function blob whose result, mapped onto its
/// range, is Halo 1's: clip to `[lo, hi]`, clip and normalise to `[0,
/// 1]`, or scale `[0, 1]` to fit `[lo, hi]`. The blob's result is
/// clamped to `[0, 1]` before the range ([`Function::evaluate`]).
fn bounded(mode: u16, [lo, hi]: [f32; 2]) -> Result<Vec<u8>, String> {
    if !(hi > lo) {
        return Err(format!("bounds {lo} to {hi}"));
    }
    let identity = |range| Function {
        kind: FunctionType::Identity,
        flags: 0,
        sub_types: [0; 2],
        range,
        unknown_0c: [0; 8],
        payload: Vec::new(),
    };
    // (x - lo) / (hi - lo): words 4 and 5 of a linear blob's first set.
    let normalise = |range| {
        let mut payload = vec![0u8; 8 * 6];
        let (a, b) = (1.0 / (hi - lo), -lo / (hi - lo));
        payload[16..20].copy_from_slice(&a.to_le_bytes());
        payload[20..24].copy_from_slice(&b.to_le_bytes());
        Function {
            kind: FunctionType::Linear,
            payload,
            ..identity(range)
        }
    };
    let f = match mode {
        0 if lo == 0.0 && hi == 1.0 => identity([0.0, 1.0]),
        0 => normalise([lo, hi]),
        1 => normalise([0.0, 1.0]),
        2 => identity([lo, hi]),
        m => return Err(format!("bounds mode {m}")),
    };
    Ok(f.to_bytes())
}

/// An attachment's primary scale: the export of Halo 1's output `index`
/// (1 to 4) where it converted, else empty.
pub fn primary_scale(index: u16, converted: &[Option<FunctionSpec>]) -> String {
    usize::from(index)
        .checked_sub(1)
        .and_then(|k| converted.get(k))
        .and_then(|s| s.as_ref())
        .map(|s| s.export.clone())
        .unwrap_or_default()
}
