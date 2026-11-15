//! The host side of the device: where surfaces live and draws go.
//!
//! The device resolves every guest-visible thing (state arrays, resource
//! headers, vertex bytes, shader inputs) into a [`ResolvedDraw`] and hands it
//! to a [`Backend`]. [`GfxBackend`](crate::gfx_backend::GfxBackend) renders
//! through `gfx`; [`RecordingBackend`] records calls for CPU-only tests.

use gfx::texture::DecodedTexture;
use gfx::vertex::{PrimitiveType, VertexLayout};
use gfx::{Clear, D3DFormat};

use gfx::texture::TextureKind;

use crate::shader::ShaderInputs;
use crate::state::{DrawState, StageExtras, StateWords};

/// The render target a draw goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Colour format (as the guest header describes it).
    pub format: D3DFormat,
    /// Depth/stencil format, if a depth buffer is bound.
    pub depth_format: Option<D3DFormat>,
}

/// A bound texture, as its guest header describes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureInfo {
    /// Texture header address.
    pub header: u32,
    /// Palette header (P8 only, else 0).
    pub palette: u32,
    /// Format.
    pub format: D3DFormat,
    /// 2D, cube or volume.
    pub kind: TextureKind,
    /// Base width.
    pub width: u32,
    /// Base height.
    pub height: u32,
    /// Base depth.
    pub depth: u32,
    /// Mip levels.
    pub levels: u32,
}

/// One vertex stream, already read out of guest memory (and de-indexed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamBytes {
    /// Bytes, first vertex first.
    pub data: Vec<u8>,
    /// Stride in bytes.
    pub stride: u32,
}

/// `D3DVIEWPORT8`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Near depth.
    pub min_z: f32,
    /// Far depth.
    pub max_z: f32,
}

/// A fully resolved draw: nothing in it refers to guest memory.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedDraw {
    /// Entry point that issued it (for events).
    pub entry: &'static str,
    /// Primitive type.
    pub primitive: PrimitiveType,
    /// Vertices in `streams`.
    pub vertex_count: u32,
    /// Vertex format.
    pub layout: VertexLayout,
    /// One per stream of `layout`.
    pub streams: Vec<StreamBytes>,
    /// Translated render and stage state.
    pub state: DrawState,
    /// The raw state words the translation came from: everything the guest
    /// set, including what `DrawState` does not model (combiner registers,
    /// deferred states, ...).
    pub raw: Box<StateWords>,
    /// Stage state outside `TextureStageState` (alpha kill, colour sign,
    /// bump environment, colour key).
    pub extras: [StageExtras; 4],
    /// The render target.
    pub target: TargetInfo,
    /// Bound textures (dimensions and format per stage).
    pub texture_info: [Option<TextureInfo>; 4],
    /// Shader inputs.
    pub shader: ShaderInputs,
    /// Current viewport.
    pub viewport: Viewport,
    /// Header addresses of the bound textures (identity for the backend's
    /// own bookkeeping; the resolved host textures are passed separately).
    pub textures: [Option<u32>; 4],
}

/// Result of [`Backend::draw`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrawOutcome {
    /// Rendered.
    Drawn,
    /// Not rendered; the device records `Event::DrawSkipped` with the reason.
    Skipped(String),
    /// The backend would render this wrongly (state it cannot express):
    /// the device stops with `D3dError::Unimplemented`.
    Unsupported(String),
}

/// Host rendering for the device.
/// `Send` (with its handles) so a device can sit behind the runtime's
/// `Send + Sync` override handlers.
pub trait Backend: Send {
    /// Handle of a colour or depth/stencil surface.
    type Surface: Copy + Eq + std::fmt::Debug + Send;
    /// A host texture made from guest texture memory.
    type Texture: Send;

    /// Creates a surface (`depth` selects depth/stencil).
    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String>;
    /// Destroys a surface made by [`create_surface`](Self::create_surface).
    /// The handle must not be used again.
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String>;
    /// Uploads a decoded guest texture.
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String>;
    /// Whether BC formats pass through (decides the decode path).
    fn bc_supported(&self) -> bool;
    /// Clears the whole of the given attachments.
    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String>;
    /// Draws into `color` (+ `depth`).
    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String>;
    /// Shows `surface` (`Swap`). `flags` are the guest's `D3DSWAP_*` flags.
    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String>;
}

/// What [`RecordingBackend`] saw.
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    /// `create_surface`.
    Surface {
        /// Handle.
        id: u32,
        /// Format.
        format: D3DFormat,
        /// Size.
        size: (u32, u32),
        /// Depth surface?
        depth: bool,
    },
    /// `upload_texture`.
    Upload {
        /// Handle.
        id: u32,
        /// Size.
        size: (u32, u32),
    },
    /// `clear`.
    Clear {
        /// Target.
        color: u32,
        /// What.
        clear: Clear,
    },
    /// `draw`.
    Draw {
        /// Target.
        color: u32,
        /// The draw.
        draw: Box<ResolvedDraw>,
        /// Texture handles per stage.
        textures: [Option<u32>; 4],
    },
    /// `destroy_surface`.
    DestroySurface {
        /// Handle.
        id: u32,
    },
    /// `present`.
    Present {
        /// Surface shown.
        surface: u32,
        /// Flags.
        flags: u32,
    },
}

/// Records every call; surfaces and textures are numbers.
#[derive(Default)]
pub struct RecordingBackend {
    /// Calls in order.
    pub calls: Vec<Recorded>,
    next: u32,
}

impl RecordingBackend {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Surfaces created and not destroyed.
    pub fn live_surfaces(&self) -> usize {
        let created = self
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::Surface { .. }))
            .count();
        let destroyed = self
            .calls
            .iter()
            .filter(|c| matches!(c, Recorded::DestroySurface { .. }))
            .count();
        created - destroyed
    }

    /// Number of texture uploads so far.
    pub fn uploads(&self) -> usize {
        self.calls
            .iter()
            .filter(|c| matches!(c, Recorded::Upload { .. }))
            .count()
    }

    /// The recorded draws.
    pub fn draws(&self) -> Vec<&ResolvedDraw> {
        self.calls
            .iter()
            .filter_map(|c| {
                if let Recorded::Draw { draw, .. } = c {
                    Some(&**draw)
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Backend for RecordingBackend {
    type Surface = u32;
    type Texture = u32;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<u32, String> {
        self.next += 1;
        self.calls.push(Recorded::Surface {
            id: self.next,
            format,
            size: (width, height),
            depth,
        });
        Ok(self.next)
    }
    fn destroy_surface(&mut self, id: u32) -> Result<(), String> {
        let live = self
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::Surface { id: i, .. } if *i == id));
        let gone = self
            .calls
            .iter()
            .any(|c| matches!(c, Recorded::DestroySurface { id: i } if *i == id));
        if !live || gone {
            return Err(format!("surface {id} is not live"));
        }
        self.calls.push(Recorded::DestroySurface { id });
        Ok(())
    }
    fn upload_texture(&mut self, t: &DecodedTexture) -> Result<u32, String> {
        self.next += 1;
        self.calls.push(Recorded::Upload {
            id: self.next,
            size: (t.desc.width, t.desc.height),
        });
        Ok(self.next)
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn clear(&mut self, color: u32, _depth: Option<u32>, clear: Clear) -> Result<(), String> {
        self.calls.push(Recorded::Clear { color, clear });
        Ok(())
    }
    fn draw(
        &mut self,
        color: u32,
        _depth: Option<u32>,
        draw: &ResolvedDraw,
        textures: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        self.calls.push(Recorded::Draw {
            color,
            draw: Box::new(draw.clone()),
            textures: textures.map(|t| t.copied()),
        });
        Ok(DrawOutcome::Drawn)
    }
    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        self.calls.push(Recorded::Present { surface, flags });
        Ok(())
    }
}
