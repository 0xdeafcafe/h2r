//! The paired editor's presentation methods (docs/editor/agent-api.md).
//! Hosts list the methods they register in `session.methods`.

use serde::{Deserialize, Serialize};

pub mod method {
    pub const LINK: &str = "editor.link";
    pub const FOCUS: &str = "editor.focus";
    pub const MARKS: &str = "editor.marks";
}

pub const MAX_MARKS: usize = 256;
pub const LINK_TTL_MS: u64 = 3000;
pub const TTL_MS_RANGE: std::ops::RangeInclusive<u64> = 500..=10000;
pub const TEXT_MAX: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    View,
    Edit,
    Blocked,
}

/// Set a plate with `editor` and `mode`, or send only `{"clear": true}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkResult {
    pub shown: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Milliseconds until expiry at the time of the answer, or zero when cleared or refused.
    pub expires_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusResult {
    pub focused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mark {
    #[serde(flatten)]
    pub geometry: MarkGeometry,
    pub color: [f32; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<MarkStyle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xray: Option<bool>,
}

impl<'de> Deserialize<'de> for Mark {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(default, deserialize_with = "present")]
            object: Option<String>,
            #[serde(default, rename = "box", deserialize_with = "present")]
            bounds: Option<MarkBox>,
            #[serde(default, deserialize_with = "present")]
            point: Option<[f32; 3]>,
            color: [f32; 3],
            style: Option<MarkStyle>,
            label: Option<String>,
            xray: Option<bool>,
        }

        // A geometry key must carry a value when it is present.
        fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
            deserializer: D,
        ) -> Result<Option<T>, D::Error> {
            T::deserialize(deserializer).map(Some)
        }

        let fields = Fields::deserialize(deserializer)?;
        let geometry = match (fields.object, fields.bounds, fields.point) {
            (Some(id), None, None) => MarkGeometry::Object(id),
            (None, Some(bounds), None) => MarkGeometry::Box(bounds),
            (None, None, Some(point)) => MarkGeometry::Point(point),
            _ => return Err(serde::de::Error::custom("mark needs exactly one of object, box or point")),
        };
        Ok(Self {
            geometry,
            color: fields.color,
            style: fields.style,
            label: fields.label,
            xray: fields.xray,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkGeometry {
    /// An eight-digit hex object handle.
    Object(String),
    Box(MarkBox),
    Point([f32; 3]),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkBox {
    pub origin: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    pub size: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkStyle {
    Selected,
    Candidate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarksParams {
    pub marks: Vec<Mark>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedMark {
    pub index: usize,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarksResult {
    /// The number of accepted marks in the replacement set.
    pub held: usize,
    pub drawn: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub refused: Vec<RefusedMark>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    fn round_trip<T: DeserializeOwned + Serialize>(wire: Value) {
        let typed: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), wire);
    }

    #[test]
    fn link_wire_round_trips() {
        round_trip::<LinkParams>(json!({"editor": "test-editor", "mode": "view"}));
        round_trip::<LinkParams>(
            json!({"editor": "test-editor", "mode": "edit", "text": "Aim select", "ttl_ms": 500}),
        );
        round_trip::<LinkParams>(json!({"editor": "test-editor", "mode": "blocked"}));
        round_trip::<LinkParams>(json!({"clear": true}));
        round_trip::<LinkResult>(json!({"shown": true, "expires_ms": 500}));
        round_trip::<LinkResult>(
            json!({"shown": false, "why": "this host has no window", "expires_ms": 0}),
        );
    }

    #[test]
    fn focus_wire_round_trips() {
        round_trip::<FocusResult>(json!({"focused": true}));
        round_trip::<FocusResult>(json!({"focused": false, "why": "this host has no window"}));
    }

    #[test]
    fn marks_wire_round_trips() {
        round_trip::<MarksParams>(json!({
            "marks": [
                {"object": "12345678", "color": [1.0, 0.0, 0.5], "style": "selected"},
                {
                    "box": {
                        "origin": [0.0, 1.0, 2.0], "forward": [1.0, 0.0, 0.0],
                        "up": [0.0, 0.0, 1.0], "size": [0.5, 1.0, 2.0]
                    },
                    "color": [0.25, 0.5, 0.75], "label": "sample", "xray": true
                },
                {"point": [2.0, 3.0, 4.0], "color": [0.0, 1.0, 0.0], "style": "candidate"}
            ],
            "ttl_ms": 800
        }));
        round_trip::<MarksResult>(json!({
            "held": 2, "drawn": true, "refused": [{"index": 0, "why": "object is gone"}]
        }));
        round_trip::<MarksResult>(
            json!({"held": 0, "drawn": false, "why": "no window", "refused": []}),
        );
    }

}
