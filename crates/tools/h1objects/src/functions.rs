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
//! Halo 1: each function is "one" or a periodic wave, scaled by an input
//! (A to D in, [`crate::h1::FunctionInputs`]), then bounded; the
//! attachment's primary scale names its output by index
//! ([`crate::h1::Attachment`]). Every Halo 1 vehicle's function is "one"
//! of that shape (`tests/object_functions_disc.rs`); emitters, flares and
//! ambient sounds use the waves ([`periodic`]).

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

/// Halo 1's weapon input table (E 0x000beca7) and names recognised
/// by Halo 2's weapon handler (E 0x000ff5f0).
fn weapon_import(source: u16) -> Option<&'static str> {
    Some(match source {
        1 => "heat",
        2 => "primary_ammunition",
        3 => "secondary_ammunition",
        4 => "primary_rate_of_fire",
        5 => "secondary_rate_of_fire",
        7 => "primary_ejection_port",
        8 => "secondary_ejection_port",
        10 => "primary_charged",
        11 => "secondary_charged",
        12 => "illumination",
        13 => "age",
        14 => "integrated_light_power",
        _ => return None,
    })
}

/// Halo 1's device input table (E 0x00117c83) and names recognised
/// by Halo 2's device handler (E 0x00106d70).
fn device_import(source: u16) -> Option<&'static str> {
    Some(match source {
        1 => "power",
        2 => "change_in_power",
        3 => "position",
        4 => "change_in_position",
        5 => "locked",
        6 => "delay",
        _ => return None,
    })
}

/// The object type and source number Halo 1 reads input `slot` (A to D)
/// from: a type-specific source takes precedence over the object's base
/// source in the same slot.
pub fn input_source(inputs: &h1::FunctionInputs, slot: usize) -> (&'static str, u16) {
    [
        ("vehicle", inputs.vehicle[slot]),
        ("unit", inputs.unit[slot]),
        ("weapon", inputs.weapon[slot]),
        ("device", inputs.device[slot]),
    ]
    .into_iter()
    .find(|(_, n)| *n != 0)
    .unwrap_or(("object", inputs.object[slot]))
}

/// Halo 2's import name for Halo 1's input `slot`, or `None` where Halo 2
/// has none ([`input_source`] names the source).
pub fn input_import(inputs: &h1::FunctionInputs, slot: usize) -> Option<&'static str> {
    match input_source(inputs, slot) {
        ("vehicle", n) => vehicle_import(n),
        ("unit", n) => unit_import(n),
        ("weapon", n) => weapon_import(n),
        ("device", n) => device_import(n),
        (_, n) => object_import(n),
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

/// Halo 1's functions as Halo 2's, by output index: `Err` with the reason
/// for one this can't carry (also in `notes`). A consumer naming such an
/// output (an attachment's scale, [`primary_scale`]; a device change
/// colour) fails the conversion with that reason.
pub fn convert(
    functions: &[h1::Function],
    inputs: &h1::FunctionInputs,
    notes: &mut Vec<String>,
) -> Vec<Result<FunctionSpec, String>> {
    let mut out: Vec<Result<FunctionSpec, String>> = functions
        .iter()
        .take(4)
        .enumerate()
        .map(|(k, f)| {
            // H1 0x8c06d..0x8c088: the additive flag adds the output's
            // previous value and wraps the sum by one (0x180ada), an
            // accumulator Halo 2's function blobs cannot hold.
            let s = one(f, inputs).and_then(|s| {
                if f.flags & 2 != 0 {
                    Err("additive accumulation, H1 0x8c06d".to_string())
                } else {
                    Ok(s)
                }
            });
            s.map(|mut s| {
                s.export = OUTPUT_NAMES[k].into();
                s
            })
        })
        .collect();
    let exports: Vec<Option<String>> = out.iter().map(|s| s.as_ref().ok().map(|s| s.export.clone())).collect();
    for (slot, f) in out.iter_mut().zip(functions) {
        let Ok(i) = usize::try_from(f.turn_off_with) else { continue };
        // H1 0x8c050..0x8c06b: the function is off unless function `i`'s
        // active bit is set. One with no converted function `i` cannot
        // carry that switch.
        if slot.is_err() {
            continue;
        }
        match exports.get(i).cloned().flatten() {
            Some(export) => {
                if let Ok(s) = slot {
                    s.turn_off_with = export;
                }
            }
            None => *slot = Err(format!("turns off with function {i}, which has no converted output")),
        }
    }
    for (k, s) in out.iter().enumerate() {
        if let Err(why) = s {
            notes.push(format!("function {}: not converted ({why})", OUTPUT_NAMES[k]));
        }
    }
    out
}

/// Halo 1's function as a Halo 2 one, its export still empty.
fn one(f: &h1::Function, inputs: &h1::FunctionInputs) -> Result<FunctionSpec, String> {
    if f.add != 0 || f.scale_result_by != 0 || f.step_count > 1 || f.sawtooth_count > 1 {
        return Err("a shape past a wave scaled by an input".into());
    }
    let import = match f.scale_function_by {
        0 => "one",
        k @ 1..=4 => {
            let slot = usize::from(k - 1);
            input_import(inputs, slot).ok_or_else(|| {
                let (kind, source) = input_source(inputs, slot);
                format!("input {k} ({kind} source {source}) has no Halo 2 import")
            })?
        }
        k => return Err(format!("scaled by output {}", k - 4)),
    };
    let data = match (f.function, f.map_to) {
        (0, 0) => bounded(f)?,
        // The Sentinel's shield glow source: H1 0x8bf9e selects table
        // 2 (0x173c65, pow(x, 0.25)); H2 transition 2 uses the same curve.
        (0, 2)
            if f.bounds_mode == 0
                && f.bounds == [0.0, 1.0]
                && f.square_threshold == 0.0
                && (f.result_scale <= 0.0 || f.result_scale == 1.0) => Function {
            kind: FunctionType::Transition,
            flags: 0,
            sub_types: [2; 2],
            range: [0.0, 1.0],
            unknown_0c: [0; 8],
            payload: [0.0f32, 1.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect(),
        }.to_bytes(),
        (0, mode) => return Err(format!("unsupported map-to {mode} with bounds {:?}", f.bounds)),
        (1, _) => return Err("function 1 (zero) is not converted".into()),
        (2..=11, _) => periodic(f, import != "one")?,
        (k, _) => return Err(format!("function {k} is no Halo 1 function kind")),
    };
    Ok(FunctionSpec {
        flags: f.flags & 0b101,
        import: import.into(),
        export: String::new(),
        turn_off_with: String::new(),
        data,
    })
}

/// Halo 1's periodic functions (kinds 2 to 11: cosine, its variable
/// period form, diagonal wave and slide with theirs, noise, jitter,
/// wander, spark) as a Halo 2 periodic blob, wave `kind` in both sets.
/// Halo 2's wave table has the same twelve slots
/// (`tags::effects::function::Tables`, `0x00468d48`); that Halo 1's bytes
/// are the same is `tests/object_functions_disc.rs`
/// `halo1_wave_table_is_halo2s`. A variable period form is a table of its
/// own, so it carries as its slot.
///
/// Halo 2's export evaluator (`0x000ba8c0`) feeds a periodic blob
/// elapsed game seconds as its argument (`0x000ba9cb`) and 1.0 as its
/// range input, then multiplies the result, mapped onto the header's
/// range, by the import (`0x000baa27`). So set 0 is the wave at frequency
/// `1 / period` with Halo 1's scale and bounds, and the import carries
/// Halo 1's scale by an input. Halo 1 bounds the product, Halo 2 scales
/// after the bounds: the two agree when the bounds start at 0 and the
/// scaled wave never clips, and anything else is refused by name.
fn periodic(f: &h1::Function, scaled: bool) -> Result<Vec<u8>, String> {
    let kind = f.function;
    if !(f.period.is_finite() && f.period > 0.0) {
        return Err(format!("function {kind} with period {}", f.period));
    }
    if f.scale_period_by != 0 {
        return Err(format!(
            "function {kind} with its period scaled by source {}: Halo 2's time input has no period scale (0xba9cb)",
            f.scale_period_by
        ));
    }
    if f.wobble {
        return Err(format!("function {kind} with a wobble (H1 +0xe..+0x18): no Halo 2 periodic field holds it"));
    }
    if f.flags & 1 != 0 {
        return Err(format!("inverted function {kind}: Halo 2's time path skips the invert (0xba95e)"));
    }
    if f.map_to != 0 || f.square_threshold != 0.0 {
        return Err(format!(
            "function {kind} with map-to {} and threshold {}",
            f.map_to, f.square_threshold
        ));
    }
    let mode = f.bounds_mode;
    let [lo, hi] = f.bounds;
    let gain = if f.result_scale > 0.0 { f.result_scale } else { 1.0 };
    if ![lo, hi, f.result_scale].into_iter().all(f32::is_finite) {
        return Err("non-finite function bounds or scale".into());
    }
    if hi <= lo {
        return Err(format!("bounds {lo} to {hi}"));
    }
    if scaled && !(lo == 0.0 && (mode == 2 || gain <= hi)) {
        return Err(format!(
            "function {kind} scaled by an input with bounds mode {mode} {lo} to {hi} and scale {gain}: \
             Halo 2 scales by the import after the bounds (0xbaa27)"
        ));
    }
    // The blob's set is clamped to [0, 1] and then mapped onto the range
    // ([`Function::evaluate`]): clip to [lo, hi], clip and normalise, or
    // scale to fit, of the wave times Halo 1's scale.
    let (min, max, range) = match mode {
        0 => (-lo / (hi - lo), (gain - lo) / (hi - lo), [lo, hi]),
        1 => (-lo / (hi - lo), (gain - lo) / (hi - lo), [0.0, 1.0]),
        2 => (0.0, 1.0, [lo, lo + (hi - lo) * gain]),
        m => return Err(format!("bounds mode {m}")),
    };
    let set = [f.period.recip(), 0.0, min, max];
    Ok(Function {
        kind: FunctionType::Periodic,
        flags: 0,
        sub_types: [kind as u8; 2],
        range,
        unknown_0c: [0; 8],
        payload: set.into_iter().chain(set).flat_map(f32::to_le_bytes).collect(),
    }
    .to_bytes())
}

/// Halo 1's bounds as a function blob whose result, mapped onto its
/// range, is Halo 1's: clip to `[lo, hi]`, clip and normalise to `[0,
/// 1]`, or scale `[0, 1]` to fit `[lo, hi]`. The blob's result is
/// clamped to `[0, 1]` before the range ([`Function::evaluate`]).
fn bounded(source: &h1::Function) -> Result<Vec<u8>, String> {
    let mode = source.bounds_mode;
    let [lo, hi] = source.bounds;
    let gain = if source.result_scale > 0.0 { source.result_scale } else { 1.0 };
    if ![lo, hi, source.square_threshold, source.result_scale]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err("non-finite function bounds, threshold or scale".into());
    }
    if hi <= lo {
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
    // H1 0x8bea4: a nonzero threshold is a strict greater-than comparison.
    // 0x8bfad then applies a positive scale before bounds. One input ULP
    // spans the step exactly for the f32 power value evaluated by H2.
    let threshold = source.square_threshold;
    if threshold != 0.0 {
        let bounded_value = |x: f32| match mode {
            0 => Ok(x.clamp(lo, hi)),
            1 => Ok(((x - lo) / (hi - lo)).clamp(0.0, 1.0)),
            2 => Ok(lo + (hi - lo) * x),
            _ => Err(format!("bounds mode {mode}")),
        };
        let mut keys = [0.0f32; 40];
        keys[9] = threshold;
        keys[13] = (threshold.next_up() - threshold).recip();
        keys[17] = 1.0;
        return Ok(Function {
            kind: FunctionType::LinearKey,
            payload: keys.into_iter().flat_map(f32::to_le_bytes).collect(),
            ..identity([bounded_value(0.0)?, bounded_value(gain)?])
        }.to_bytes());
    }
    let linear = |range, a: f32, b: f32| {
        let mut payload = vec![0u8; 8 * 6];
        payload[16..20].copy_from_slice(&a.to_le_bytes());
        payload[20..24].copy_from_slice(&b.to_le_bytes());
        Function {
            kind: FunctionType::Linear,
            payload,
            ..identity(range)
        }
    };
    let f = match mode {
        0 if lo == 0.0 && hi == 1.0 && gain == 1.0 => identity([0.0, 1.0]),
        0 => linear([lo, hi], gain / (hi - lo), -lo / (hi - lo)),
        1 => linear([0.0, 1.0], gain / (hi - lo), -lo / (hi - lo)),
        2 => identity([lo, lo + (hi - lo) * gain]),
        m => return Err(format!("bounds mode {m}")),
    };
    Ok(f.to_bytes())
}

/// An attachment's scale: empty for index 0 (none), else the export of
/// Halo 1's output `index` (1 to 4). An output with no converted function
/// is an error with the function's reason: Halo 1 scales the attachment by
/// that output, while an empty Halo 2 scale leaves it at one.
pub fn primary_scale(index: u16, converted: &[Result<FunctionSpec, String>]) -> Result<String, String> {
    let Some(k) = usize::from(index).checked_sub(1) else {
        return Ok(String::new());
    };
    match converted.get(k) {
        Some(Ok(s)) => Ok(s.export.clone()),
        Some(Err(why)) => Err(format!("output {index}'s function did not convert: {why}")),
        None => Err(format!("output {index} has no function")),
    }
}
