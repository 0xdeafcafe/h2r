//! The preview clock's reading of a shader's animated properties
//! (docs/editor/rich-editors.md, "Shaders", slice S2): the value of each
//! property at a render time, by the game's own evaluation
//! (`native_render::layer_pass::property_value`: time over the period,
//! wrapped unless the function is periodic, the mapping function, then
//! the output range). The Shader tab and `shader.preview` read the same
//! values, so the number beside a curve is the number the plan holds.

use native_render::layer_pass::{LayerRefusal, property_value};
use tags::shader::AnimatedProperty;

/// Why a property driven by an object function has no value in the
/// preview: its input is the object's, and the preview has none.
pub const OBJECT_DRIVEN: &str =
    "driven by an object function, whose input the preview has no object to give";
/// Why a time-driven property with no usable period has no value.
pub const NO_PERIOD: &str = "its period is not a positive number of seconds";
/// Why a property whose function does not parse has no value.
pub const UNREADABLE: &str = "its function does not read as a curve";

/// Property `index`'s value at render time `time` seconds, or why it has
/// none.
pub fn value_at(property: &AnimatedProperty, index: usize, time: f64) -> Result<f32, String> {
    if property.input == 0 {
        let period = f32::from_bits(property.input_b);
        if !(period.is_finite() && period > 0.0) {
            return Err(NO_PERIOD.to_owned());
        }
    }
    property_value(property, index, Some(time), false).map_err(|e| match e {
        LayerRefusal::ObjectAnimation { .. } => OBJECT_DRIVEN.to_owned(),
        LayerRefusal::Property { .. } => UNREADABLE.to_owned(),
        LayerRefusal::Wave { wave, .. } => format!("periodic wave {wave} needs an engine lookup table"),
        other => format!("not evaluated ({})", other.name()),
    })
}

/// Every property's value at `time`, in the instance's order.
pub fn values_at(properties: &[AnimatedProperty], time: f64) -> Vec<Result<f32, String>> {
    properties
        .iter()
        .enumerate()
        .map(|(i, p)| value_at(p, i, time))
        .collect()
}

/// The length of the preview clock's loop, in seconds: the longest period
/// of a time-driven property, or 1 when none has one.
pub fn clock_length(properties: &[AnimatedProperty]) -> f64 {
    properties
        .iter()
        .filter(|p| p.input == 0)
        .map(|p| f64::from(f32::from_bits(p.input_b)))
        .filter(|p| p.is_finite() && *p > 0.0)
        .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A function blob: header (type, flags, sub-types, output range,
    /// eight unread bytes) and its payload words.
    fn blob(kind: u8, subs: [u8; 2], range: [f32; 2], words: &[f32]) -> Vec<u8> {
        let mut v = vec![kind, 0, subs[0], subs[1]];
        v.extend(range[0].to_le_bytes());
        v.extend(range[1].to_le_bytes());
        v.extend([0u8; 8]);
        for w in words {
            v.extend(w.to_le_bytes());
        }
        v
    }

    /// An invented time-driven property: a cosine of one cycle over
    /// `period` seconds, mapped onto `[2, 4]`.
    fn cosine(period: f32) -> AnimatedProperty {
        AnimatedProperty {
            input: 0,
            input_a: 0,
            input_b: period.to_bits(),
            // Set 0 is `wave(frequency * x + phase)` between 0 and 1,
            // and set 1 is unused: eight words each.
            function: blob(3, [2, 0], [2.0, 4.0], &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
        }
    }

    #[test]
    fn a_periodic_property_changes_between_two_times() {
        let p = cosine(2.0);
        let start = value_at(&p, 0, 0.0).unwrap();
        let half = value_at(&p, 0, 1.0).unwrap();
        let again = value_at(&p, 0, 2.0).unwrap();
        // The cosine starts at one end of its range, is at the other half
        // a period on and is back after a whole one.
        assert!((2.0..=4.0).contains(&start) && (2.0..=4.0).contains(&half));
        assert!((start - half).abs() > 1.0, "{start} {half}");
        assert!((start - again).abs() < 1e-3, "{start} {again}");
    }

    #[test]
    fn a_property_with_no_value_says_why() {
        let object = AnimatedProperty {
            input: 7,
            ..cosine(1.0)
        };
        assert_eq!(value_at(&object, 0, 0.0), Err(OBJECT_DRIVEN.to_owned()));
        assert_eq!(
            value_at(&cosine(0.0), 0, 0.0),
            Err(NO_PERIOD.to_owned())
        );
        assert_eq!(
            value_at(&cosine(f32::NAN), 0, 0.0),
            Err(NO_PERIOD.to_owned())
        );
        let broken = AnimatedProperty {
            function: vec![3, 0, 0],
            ..cosine(1.0)
        };
        assert_eq!(value_at(&broken, 0, 0.0), Err(UNREADABLE.to_owned()));
    }

    #[test]
    fn the_clock_loops_over_the_longest_period() {
        assert_eq!(clock_length(&[]), 1.0);
        assert_eq!(clock_length(&[cosine(2.5), cosine(0.5)]), 2.5);
        // An object-driven property has no period; a bad one is skipped.
        let object = AnimatedProperty {
            input: 3,
            ..cosine(9.0)
        };
        assert_eq!(clock_length(&[object, cosine(-1.0), cosine(0.25)]), 0.25);
        let all = values_at(&[cosine(1.0), cosine(0.0)], 0.0);
        assert!(all[0].is_ok() && all[1].is_err());
    }
}
