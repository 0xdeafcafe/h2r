//! The one render scale (docs/graphics/high-resolution.md): every scene
//! target is made at it, and nothing keeps a 1x copy.

pub use gfx::renderer::RenderScale;

/// The shadow maps' factor at a render scale ([`crate::lighting`]).
pub use crate::lighting::shadow_scale;

/// The largest whole render scale: 6x of 640x480 is 3840x2880.
pub use gfx::external::MAX_SCALE;

/// Parses the integer scale `H2_RENDER_SCALE`. Empty is 1.
pub fn render_scale_from(value: Option<&str>) -> Result<u32, String> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(1);
    };
    match v.parse::<u32>() {
        Ok(n) if (1..=MAX_SCALE).contains(&n) => Ok(n),
        _ => Err(format!("H2_RENDER_SCALE={v:?}: expected a whole number from 1 to {MAX_SCALE}")),
    }
}

/// [`render_scale_from`] of the environment.
pub fn render_scale() -> Result<u32, String> {
    render_scale_from(std::env::var("H2_RENDER_SCALE").ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_parse_inside_the_range_only() {
        assert_eq!(render_scale_from(None), Ok(1));
        assert_eq!(render_scale_from(Some(" 3 ")), Ok(3));
        assert!(render_scale_from(Some("7")).is_err());
        assert!(render_scale_from(Some("0")).is_err());
    }
}
