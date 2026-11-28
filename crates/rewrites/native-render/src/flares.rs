//! Lens flare fans. [`Site`] is pure data: each site's name, counter and
//! vertex register set. docs/graphics/native-particles.md.

/// One flare fan. Each is one `TriangleFan` of four vertices, no indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Site {
    /// Read-only visibility fan inside an active z-pass report.
    Visibility,
    /// Reflection billboard: position, uv and a carried colour.
    Reflection,
    /// Position only; the colour is a carried default.
    Mask,
    /// Position and uv.
    MaskSample,
    /// Downsample: position and uv.
    FilterSource,
    /// Filter: position and uv.
    Filter,
    /// Additive composite: position, uv and a carried register 9.
    Composite,
}

impl Site {
    /// Every site, in the order a frame issues them.
    pub const ALL: [Site; 7] = [
        Self::Visibility,
        Self::Reflection,
        Self::Mask,
        Self::MaskSample,
        Self::FilterSource,
        Self::Filter,
        Self::Composite,
    ];

    /// The site's part of a counter's name (`scene_kept_flare_<site>_*`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Visibility => "visibility",
            Self::Reflection => "reflection",
            Self::Mask => "mask",
            Self::MaskSample => "mask_sample",
            Self::FilterSource => "filter_source",
            Self::Filter => "filter",
            Self::Composite => "composite",
        }
    }

    /// The vertex registers written for the fan (bit `r` is register `r`).
    /// Registers outside it come from the carried defaults.
    pub const fn registers(self) -> u16 {
        match self {
            Self::Visibility | Self::Mask => 1,
            Self::MaskSample | Self::FilterSource | Self::Filter => 1 | 1 << 3,
            Self::Reflection | Self::Composite => 1 | 1 << 3 | 1 << 9,
        }
    }

    /// Whether `written` is exactly the site's register set.
    pub const fn accepts_registers(self, written: u16) -> bool {
        written == self.registers()
    }
}
