//! `render.particles.refusals` (docs/runtime/agent-api.md): why the native
//! particle path refused geometry, by named check, and what the first
//! refused draw of each looked like. One sample per reason, kept for the
//! run; the per-window counters live in `native_refusals`.
use std::collections::BTreeMap;
use std::sync::Mutex;

use agent_proto::{RpcError, code};
use serde_json::{Value, json};

struct Entry {
    producer: &'static str,
    count: u64,
    shape: Value,
}

pub(super) struct Samples(Mutex<BTreeMap<&'static str, Entry>>);

impl Samples {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(BTreeMap::new()))
    }

    /// Counts one refusal of `reason`; the first one keeps its draw's shape.
    pub(super) fn record(
        &self,
        reason: &'static str,
        producer: &'static str,
        draw: &d3d::ResolvedDraw,
    ) {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .entry(reason)
            .and_modify(|e| e.count += 1)
            .or_insert_with(|| Entry {
                producer,
                count: 1,
                shape: shape(draw),
            });
    }

    #[cfg(test)]
    pub(super) fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// `{refused, samples: {reason: {label, producer, count, shape}}}`.
    pub(super) fn answer(&self) -> Value {
        let entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let samples: serde_json::Map<String, Value> = entries
            .iter()
            .map(|(reason, e)| {
                let label = super::geometry_label(reason);
                (
                    (*reason).to_string(),
                    json!({"label": label, "producer": e.producer, "count": e.count, "shape": e.shape}),
                )
            })
            .collect();
        json!({"refused": entries.values().map(|e| e.count).sum::<u64>(), "samples": samples})
    }
}

/// The draw's shape: what the geometry checks looked at.
fn shape(draw: &d3d::ResolvedDraw) -> Value {
    let attributes: Vec<Value> = draw
        .layout
        .streams
        .iter()
        .flat_map(|s| {
            s.attributes.iter().map(|a| {
                json!({
                    "stream": s.stream,
                    "register": a.register.0,
                    "data_type": format!("{:?}", a.data_type),
                    "offset": a.offset,
                })
            })
        })
        .collect();
    json!({
        "entry": draw.entry,
        "primitive": format!("{:?}", draw.primitive),
        "vertex_count": draw.vertex_count,
        "indexed": draw.indices.is_some(),
        "pretransformed": draw.layout.pretransformed,
        "layouts": draw.layout.streams.len(),
        "streams": draw.streams.len(),
        "layout_strides": draw.layout.streams.iter().map(|s| s.stride).collect::<Vec<_>>(),
        "stream_strides": draw.streams.iter().map(|s| s.stride).collect::<Vec<_>>(),
        "stream_bytes": draw.streams.iter().map(|s| s.data.len()).collect::<Vec<_>>(),
        "attribute_count": attributes.len(),
        "attributes": attributes,
        "provided_inputs": draw.shader.provided_inputs,
    })
}

/// The method's handler: no params.
pub(super) fn answer(params: &Value, samples: &Samples) -> Result<Value, RpcError> {
    match params {
        Value::Null => Ok(samples.answer()),
        Value::Object(m) if m.is_empty() => Ok(samples.answer()),
        _ => Err(RpcError::new(
            code::INVALID_PARAMS,
            "render.particles.refusals takes no params",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_particles_admission::tests::particle_draw;

    #[test]
    fn the_first_draw_of_a_reason_keeps_its_shape_and_later_ones_count() {
        let samples = Samples::new();
        let (mut draw, _) = particle_draw();
        samples.record("primitive", "Particle", &draw);
        draw.vertex_count = 9;
        samples.record("primitive", "Particle", &draw);
        samples.record("inputs", "Dynamic", &draw);
        let a = samples.answer();
        assert_eq!(a["refused"], 3);
        let p = &a["samples"]["primitive"];
        assert_eq!(
            (p["count"].as_u64(), p["label"].as_str()),
            (Some(2), Some("Geometry/primitive"))
        );
        assert_eq!(p["shape"]["vertex_count"], 4);
        assert_eq!(p["shape"]["primitive"], "QuadList");
        assert_eq!(p["shape"]["attribute_count"], 1);
        assert_eq!(p["shape"]["attributes"][0]["data_type"], "Short4");
        assert_eq!(p["shape"]["layout_strides"], json!([8]));
        assert_eq!(p["shape"]["provided_inputs"], 1);
        assert_eq!(p["shape"]["pretransformed"], false);
        assert_eq!(a["samples"]["inputs"]["producer"], "Dynamic");
        assert_eq!(a["samples"]["inputs"]["shape"]["vertex_count"], 9);
    }

    #[test]
    fn no_refusals_is_an_empty_answer_and_params_are_refused() {
        let samples = Samples::new();
        assert_eq!(
            answer(&Value::Null, &samples).unwrap(),
            json!({"refused": 0, "samples": {}})
        );
        assert!(answer(&json!({}), &samples).is_ok());
        assert!(answer(&json!({"reason": "x"}), &samples).is_err());
    }
}
