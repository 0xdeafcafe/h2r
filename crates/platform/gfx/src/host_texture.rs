//! A texture uploaded for a stage, with the view the shaders sample it
//! through.

/// A texture with the view the shaders sample it through (`None` when its
/// format cannot be bound as a filterable float texture: the program then
/// reads a stand-in).
#[derive(Clone, Debug)]
pub struct HostTexture {
    /// The texture.
    pub texture: wgpu::Texture,
    /// The sampled view.
    pub view: Option<wgpu::TextureView>,
}

impl HostTexture {
    pub fn new(texture: wgpu::Texture, view: Option<wgpu::TextureView>) -> Self {
        Self { texture, view }
    }

    /// Whether `other` samples the same texture through the same view.
    pub fn same_sample(&self, other: &Self) -> bool {
        self.texture == other.texture && self.view == other.view
    }

    /// Another handle to the same texture and view.
    pub fn shared(&self) -> Self {
        self.clone()
    }
}
