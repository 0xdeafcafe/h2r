//! Xbox (NV2A) shader programs to WGSL.
//!
//! Two independent translators:
//!
//! - [`vsh`]: NV2A vertex program microcode (what `XGAssembleShader` emits and
//!   what Halo 2's `vrtx` tags store) to a `@vertex` entry point.
//! - [`psh`]: a `D3DPIXELSHADERDEF` (texture-stage modes plus register
//!   combiners, which is what Halo 2's `spas` tags store) to a `@fragment`
//!   entry point.
//!
//! Both sides meet in [`VARYINGS_STRUCT`]; [`link`] puts a vertex and a pixel
//! translation into one module with `vs_main` and `fs_main`. Each side also
//! has a reference CPU interpreter ([`vsh::interp`], [`psh::interp`]) that
//! defines the semantics the WGSL is tested against.
//!
//! Resource layout of a linked module (the contract with the renderer):
//!
//! | group | binding | contents |
//! |---|---|---|
//! | 0 | 0 | `XvsUniforms` (vertex constants, attribute defaults, viewport), vertex stage |
//! | 0 | 1 | `XpsUniforms` (combiner constants, fog, bump-env, alpha ref), fragment stage |
//! | 1 | 2n | texture for stage n (dimension per [`psh::PixelShader::textures`]) |
//! | 1 | 2n+1 | sampler for stage n |
//!
//! Vertex register `vN` is shader location `N`. See `docs/shaders.md`.
#![forbid(unsafe_code)]

pub mod psh;
pub mod vsh;

pub use psh::PixelShaderDef;
pub use vsh::Program as VertexProgram;

pub(crate) mod common {
    /// The varyings struct, with or without flat colour interpolation.
    pub fn varyings_struct(flat: bool) -> String {
        let interp = if flat { " @interpolate(flat)" } else { "" };
        format!(
            "struct XboxVaryings {{
    @builtin(position) position: vec4f,
    @location(0){interp} d0: vec4f,
    @location(1){interp} d1: vec4f,
    @location(2){interp} b0: vec4f,
    @location(3){interp} b1: vec4f,
    @location(4) t0: vec4f,
    @location(5) t1: vec4f,
    @location(6) t2: vec4f,
    @location(7) t3: vec4f,
    @location(8) fog: f32,
}}
"
        )
    }
}

/// The struct both stages share (smooth interpolation). Locations: 0 = oD0,
/// 1 = oD1, 2 = oB0, 3 = oB1, 4..=7 = oT0..oT3, 8 = fog coordinate (oFog.x).
pub const VARYINGS_STRUCT: &str = "XboxVaryings";

/// A vertex/pixel pair could not be linked.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkError {
    /// Flat interpolation must match on both sides.
    #[error("vertex and pixel shaders disagree on flat shading")]
    FlatMismatch,
}

/// One WGSL module with `vs_main` and `fs_main`.
pub fn link(
    vertex: &vsh::wgsl::VertexShader,
    pixel: &psh::PixelShader,
) -> Result<String, LinkError> {
    if vertex.flat_shading != pixel.flat_shading {
        return Err(LinkError::FlatMismatch);
    }
    Ok(format!(
        "{}{}{}",
        common::varyings_struct(vertex.flat_shading),
        vertex.body,
        pixel.body
    ))
}

/// A WGSL module that naga rejected.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

/// Parses and validates WGSL with naga (the front end wgpu uses), returning
/// the module for further inspection.
pub fn validate(source: &str) -> Result<(naga::Module, naga::valid::ModuleInfo), ValidationError> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|e| ValidationError(e.emit_to_string(source)))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| ValidationError(e.emit_to_string(source)))?;
    Ok((module, info))
}
