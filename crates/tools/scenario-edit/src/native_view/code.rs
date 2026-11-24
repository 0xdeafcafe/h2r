//! The programs of one pass of a material as text: the Shader tab's Code
//! section and Studio's `shader.code` (docs/editor/rich-editors.md,
//! "Shaders", slice S4). Each program is its disassembly, the WGSL the
//! translators make of it (`shaders::vsh::translate`,
//! `shaders::psh::translate`) and the translators' warnings, so what the
//! renderer would be given is read without drawing anything.
//!
//! A program that cannot be listed or translated is shown with its
//! refusal in `warnings` and the missing text `None`: a state program
//! (`0x7378`) has a listing and no WGSL.

use std::io::{Read, Seek};

use map::{Map, TagId};
use native_render::look;
use shaders::psh::{self, PSDEF_SIZE, PixelShaderDef};
use shaders::vsh::{self, VertexOptions};
use tags::material::ShaderLibrary;

/// The world-space classification of the vertex shader tag, the program
/// the editor's material preview draws (`look::WORLD_CLASSIFICATION`).
pub const VERTEX_CLASSIFICATION: &str = "world";

/// One program as text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramCode {
    /// The disassembly, or `None` when the program does not decode.
    pub listing: Option<String>,
    /// The translation (a standalone WGSL module), or `None` when the
    /// program is refused.
    pub wgsl: Option<String>,
    /// What the translator approximated, dropped or refused, each in a
    /// sentence.
    pub warnings: Vec<String>,
}

/// One pass implementation's two programs and where it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassCode {
    /// The world layer, at the first level of detail.
    pub layer: usize,
    /// The pass within its layer.
    pub pass_in_layer: usize,
    /// The pass counted across the layers, in layer order: the index
    /// `shader.code` takes.
    pub pass: usize,
    /// Every pass the shader has at the first level of detail.
    pub passes: usize,
    /// The `spas` tag.
    pub pass_tag: TagId,
    pub implementation: usize,
    /// The implementations this pass has.
    pub implementations: usize,
    pub vertex: ProgramCode,
    pub pixel: ProgramCode,
}

/// A vertex program's text from its microcode (`'x'`, kind, count,
/// instructions), or why there is none.
pub fn vertex_code(microcode: Option<&[u8]>) -> ProgramCode {
    let Some(bytes) = microcode else {
        return ProgramCode {
            warnings: vec![format!(
                "the vertex shader has no {VERTEX_CLASSIFICATION} classification program"
            )],
            ..Default::default()
        };
    };
    let program = match vsh::Program::parse(bytes) {
        Ok(p) => p,
        Err(e) => {
            return ProgramCode {
                warnings: vec![format!("the vertex program does not decode: {e}")],
                ..Default::default()
            };
        }
    };
    let listing = Some(vsh::disassemble(&program));
    match vsh::translate(&program, &VertexOptions::default()) {
        Ok(v) => ProgramCode {
            listing,
            wgsl: Some(v.module()),
            warnings: Vec::new(),
        },
        Err(e) => ProgramCode {
            listing,
            wgsl: None,
            warnings: vec![e.to_string()],
        },
    }
}

/// A pixel program's text from its final `D3DPIXELSHADERDEF`.
pub fn pixel_code(def: &[u8; PSDEF_SIZE]) -> ProgramCode {
    let def = match PixelShaderDef::from_bytes(def) {
        Ok(d) => d,
        Err(e) => {
            return ProgramCode {
                warnings: vec![format!("the pixel shader does not decode: {e:?}")],
                ..Default::default()
            };
        }
    };
    let t = psh::translate(&def, &Default::default());
    ProgramCode {
        listing: Some(psh::disassemble(&def)),
        wgsl: Some(t.module()),
        warnings: t.warnings,
    }
}

/// The programs of pass number `pass` (counted across the layers of the
/// first level of detail, in layer order) and implementation
/// `implementation` of `shader` in `map`. `Err` is why the material does
/// not resolve or the pass is not there.
pub fn pass_code<R: Read + Seek>(
    map: &mut Map<R>,
    shader: TagId,
    pass: usize,
    implementation: usize,
) -> Result<PassCode, String> {
    let mut lib = ShaderLibrary::read(map).map_err(|e| e.to_string())?;
    let material = lib
        .material(shader, None)
        .map_err(|e| format!("the material does not resolve: {e}"))?;
    let flat: Vec<_> = material
        .layers
        .iter()
        .filter(|l| l.lod == 0)
        .flat_map(|l| l.passes.iter().map(move |p| (l.layer, p)))
        .collect();
    let Some(&(layer, p)) = flat.get(pass) else {
        return Err(format!(
            "the shader has {} passes at its first level of detail, not {}",
            flat.len(),
            pass + 1
        ));
    };
    let Some(imp) = p.implementations.get(implementation) else {
        return Err(format!(
            "pass {pass} has {} implementations, not {}",
            p.implementations.len(),
            implementation + 1
        ));
    };
    let microcode = imp
        .vertex_shader
        .and_then(|v| look::world_program(map, lib.metadata(), v));
    Ok(PassCode {
        layer,
        pass_in_layer: p.index,
        pass,
        passes: flat.len(),
        pass_tag: p.pass,
        implementation,
        implementations: p.implementations.len(),
        vertex: vertex_code(microcode.as_deref()),
        pixel: pixel_code(&imp.pixel_shader),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{self, LAYERED_LAYER};

    fn map(layer: u8) -> Map<std::io::Cursor<Vec<u8>>> {
        Map::from_reader(std::io::Cursor::new(fixture::fixture_with_shader_colour(layer))).unwrap()
    }

    /// An invented pass lists both programs and their translations.
    #[test]
    fn an_invented_pass_lists_both_programs() {
        let c = pass_code(&mut map(LAYERED_LAYER), fixture::id(6), 0, 0).unwrap();
        assert_eq!(
            (c.layer, c.pass_in_layer, c.pass, c.passes, c.implementation, c.implementations),
            (usize::from(LAYERED_LAYER), 0, 0, 1, 0, 1)
        );
        for (name, p) in [("vertex", &c.vertex), ("pixel", &c.pixel)] {
            assert!(p.listing.as_deref().is_some_and(|l| !l.is_empty()), "{name}");
            assert!(p.wgsl.as_deref().is_some_and(|w| w.contains("fn ")), "{name}");
        }
        assert!(c.vertex.warnings.is_empty(), "{:?}", c.vertex.warnings);
        assert!(c.vertex.listing.unwrap().starts_with("000: "));
    }

    #[test]
    fn a_pass_or_implementation_that_is_not_there_is_named() {
        let e = pass_code(&mut map(LAYERED_LAYER), fixture::id(6), 1, 0).unwrap_err();
        assert!(e.contains("1 passes"), "{e}");
        let e = pass_code(&mut map(LAYERED_LAYER), fixture::id(6), 0, 3).unwrap_err();
        assert!(e.contains("1 implementations"), "{e}");
    }

    /// A state program has a listing and no WGSL: its refusal is shown.
    #[test]
    fn a_state_program_shows_its_refusal() {
        let mut m = map(LAYERED_LAYER);
        let lib = ShaderLibrary::read(&mut m).unwrap();
        let mut code = look::world_program(&mut m, lib.metadata(), fixture::id(12)).unwrap();
        assert_eq!(&code[..2], &[0x78, 0x20]);
        code[1] = 0x73;
        let p = vertex_code(Some(&code));
        assert!(p.listing.is_some());
        assert_eq!(p.wgsl, None);
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        assert!(p.warnings[0].contains("state programs"), "{:?}", p.warnings);
    }

    #[test]
    fn a_program_that_does_not_decode_or_is_missing_says_so() {
        let p = vertex_code(Some(&[1, 2, 3, 4]));
        assert_eq!((p.listing, p.wgsl), (None, None));
        assert!(p.warnings[0].contains("does not decode"), "{:?}", p.warnings);
        let p = vertex_code(None);
        assert!(p.warnings[0].contains("no world classification"), "{:?}", p.warnings);
    }
}
