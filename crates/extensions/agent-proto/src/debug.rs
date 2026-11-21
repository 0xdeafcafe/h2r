//! Controls for the in-game debug world-line source.
use serde::{Deserialize, Serialize};

pub mod method {
    pub const VIEWS: &str = "debug.views";
    pub const VIEW: &str = "debug.view";
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum View {
    GeometryEdges,
    TriggerVolumes,
    KillVolumes,
    InvisibleBarriers,
    CollisionGeometry,
    Hitboxes,
    Traces,
}

impl View {
    pub const ALL: [Self; 7] = [
        Self::GeometryEdges,
        Self::TriggerVolumes,
        Self::KillVolumes,
        Self::InvisibleBarriers,
        Self::CollisionGeometry,
        Self::Hitboxes,
        Self::Traces,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::GeometryEdges => "Geometry edges",
            Self::TriggerVolumes => "Trigger volumes",
            Self::KillVolumes => "Kill / death volumes",
            Self::InvisibleBarriers => "Invisible barriers",
            Self::CollisionGeometry => "Collision geometry",
            Self::Hitboxes => "Hitboxes",
            Self::Traces => "Traces",
        }
    }

    pub fn color(self) -> [f32; 3] {
        match self {
            Self::GeometryEdges => [0.32, 0.76, 1.0],
            Self::TriggerVolumes => [0.38, 1.0, 0.48],
            Self::KillVolumes => [1.0, 0.22, 0.22],
            Self::InvisibleBarriers => [1.0, 0.42, 0.85],
            Self::CollisionGeometry => [1.0, 0.7, 0.18],
            Self::Hitboxes => [0.74, 0.48, 1.0],
            Self::Traces => [1.0, 1.0, 0.56],
        }
    }
}

#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewsParams {}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ViewParams {
    Toggle { name: View, on: bool },
    Range { range: u16 },
    TraceTicks { trace_ticks: u16 },
}

pub const RANGE: std::ops::RangeInclusive<u16> = 1..=512;
pub const TRACE_TICKS: std::ops::RangeInclusive<u16> = 1..=300;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    pub name: View,
    pub label: String,
    pub on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewsResult {
    pub views: Vec<ViewState>,
    pub range: u16,
    pub trace_ticks: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn view_requests_round_trip_and_set_one_control() {
        for p in [
            json!({"name":"kill_volumes","on":true}),
            json!({"range":20}),
            json!({"trace_ticks":42}),
        ] {
            assert_eq!(
                to_value(from_value::<ViewParams>(p.clone()).unwrap()).unwrap(),
                p
            );
        }
        for p in [
            json!({}),
            json!({"on":true}),
            json!({"name":"unknown","on":true}),
            json!({"range":5,"trace_ticks":10}),
            json!({"through_walls":true}),
        ] {
            assert!(from_value::<ViewParams>(p).is_err());
        }
    }
}
