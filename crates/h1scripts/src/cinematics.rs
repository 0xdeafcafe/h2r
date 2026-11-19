//! Checkpointed scenario globals shared by the cinematic script compiler and renderer.
pub const VIDEO_BITMAPS: [&str; 2] = [r"h1_cinematic\video_mask", r"h1_cinematic\video_noise"];
pub const MARKER: &str = "h1_cine_state";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Field {
    Active,
    Clear,
    Passes,
    Kernel,
    RadiusFrom,
    RadiusTo,
    BlurSeconds,
    BlurTick,
    LightFrom,
    LightTo,
    DesatFrom,
    DesatTo,
    FilterFlag,
    FilterSeconds,
    FilterTick,
    Red,
    Green,
    Blue,
    Video,
    VideoMode,
    Noise,
    Reflect0,
    Reflect1,
    Reflect2,
    Reflect3,
}

impl Field {
    pub const ALL: [Self; 25] = [
        Self::Active,
        Self::Clear,
        Self::Passes,
        Self::Kernel,
        Self::RadiusFrom,
        Self::RadiusTo,
        Self::BlurSeconds,
        Self::BlurTick,
        Self::LightFrom,
        Self::LightTo,
        Self::DesatFrom,
        Self::DesatTo,
        Self::FilterFlag,
        Self::FilterSeconds,
        Self::FilterTick,
        Self::Red,
        Self::Green,
        Self::Blue,
        Self::Video,
        Self::VideoMode,
        Self::Noise,
        Self::Reflect0,
        Self::Reflect1,
        Self::Reflect2,
        Self::Reflect3,
    ];

    pub fn ty(self) -> u16 {
        match self {
            Self::Active | Self::Clear | Self::FilterFlag | Self::Video => 5,
            Self::RadiusFrom
            | Self::RadiusTo
            | Self::BlurSeconds
            | Self::LightFrom
            | Self::LightTo
            | Self::DesatFrom
            | Self::DesatTo
            | Self::FilterSeconds
            | Self::Red
            | Self::Green
            | Self::Blue
            | Self::Noise
            | Self::Reflect0
            | Self::Reflect1
            | Self::Reflect2
            | Self::Reflect3 => 6,
            Self::Passes | Self::Kernel | Self::VideoMode => 7,
            Self::BlurTick | Self::FilterTick => 8,
        }
    }

    pub fn name(self) -> String {
        format!("h1_cine_{self:?}").to_ascii_lowercase()
    }
}
