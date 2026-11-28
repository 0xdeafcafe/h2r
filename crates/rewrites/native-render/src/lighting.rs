//! The view's lighting (docs/graphics/e3-lighting-setting.md), chosen at
//! launch by `H2_LIGHTING`; no setting holds it.

pub use gfx::external::Lighting;

use crate::render_scale::MAX_SCALE;

/// Parses the lighting selected at startup. Empty is retail.
pub fn lighting_from(value: Option<&str>) -> Result<Lighting, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(Lighting::Retail),
        Some(v) if v.eq_ignore_ascii_case("retail") => Ok(Lighting::Retail),
        Some(v) if v.eq_ignore_ascii_case("e3") => Ok(Lighting::E3),
        Some(v) => Lighting::relight_from_name(v)
            .ok_or_else(|| format!("H2_LIGHTING={v:?}: expected retail, e3, relight-on or relight-off")),
    }
}

/// [`lighting_from`] of the environment.
pub fn lighting() -> Result<Lighting, String> {
    lighting_from(std::env::var("H2_LIGHTING").ok().as_deref())
}

/// The factor shadow maps are made at for render scale `scale`: 1 for
/// retail lighting; for E3 the render scale, at least 2, so a one-tap
/// edge is sharper than retail's filtered one at 1x too.
pub fn shadow_scale(scale: u32, lighting: Lighting) -> u32 {
    match lighting {
        Lighting::Retail | Lighting::RelightOn | Lighting::RelightOff => 1,
        Lighting::E3 => scale.clamp(2, MAX_SCALE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parse_and_e3_shadows_are_at_least_twice() {
        assert_eq!(lighting_from(None), Ok(Lighting::Retail));
        assert_eq!(lighting_from(Some(" E3 ")), Ok(Lighting::E3));
        assert_eq!(lighting_from(Some("relight-off")), Ok(Lighting::RelightOff));
        assert!(lighting_from(Some("bright")).is_err());
        assert_eq!(shadow_scale(1, Lighting::E3), 2);
        assert_eq!(shadow_scale(4, Lighting::E3), 4);
        assert_eq!(shadow_scale(4, Lighting::Retail), 1);
    }
}
