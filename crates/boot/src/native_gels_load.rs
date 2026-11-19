//! Map-resident gel programs and bitmap resources. Live constants are
//! captured after guest setup by native_gels_glue; animation functions
//! are never approximated here.

use std::collections::HashMap;
use std::io::BufReader;
use std::sync::Arc;

use map::{Map, ResourceFile, TagId};
use native_render::gpu::Image;
use tags::material::{AnimationKind, ShaderLibrary, TextureSource};
use tags::shader::PSDEF_SIZE;

use crate::native_world_glue::{StageDecoder, locate, region};

pub struct Asset {
    pub code: Vec<u8>,
    pub pixel_shader: [u8; PSDEF_SIZE],
    pub images: [Option<Image>; 4],
    pub uses_light_bitmap: bool,
}

pub type Assets = HashMap<(u32, u32), Result<Arc<Asset>, String>>;

pub fn load(name: &str) -> Result<Assets, String> {
    let file = locate(name).ok_or("gel map not found")?;
    let reader = BufReader::new(std::fs::File::open(&file.path).map_err(|e| e.to_string())?);
    let mut map = Map::from_region(reader, file.offset, file.len).map_err(|e| e.to_string())?;
    let mut files = assets::ResourceFiles::new();
    files
        .set(ResourceFile::Local, region(&file))
        .map_err(|e| e.to_string())?;
    for (which, short) in [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ] {
        if let Some(f) = locate(short) {
            files.set(which, region(&f)).map_err(|e| e.to_string())?;
        }
    }
    let mut lib = ShaderLibrary::read(&mut map).map_err(|e| e.to_string())?;
    let definitions: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"ligh"))
        .filter_map(|t| t.location.as_ref().map(|l| l.address))
        .collect();
    let mut out = HashMap::new();
    for address in definitions {
        let word = |at| {
            lib.metadata()
                .bytes(at, 4)
                .ok()
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        let (Some(shader), Some(gel)) = (word(address + 0xe0), word(address + 0x7c)) else {
            continue;
        };
        if shader == 0 || shader == u32::MAX || out.contains_key(&(shader, gel)) {
            continue;
        }
        let result = (|| {
            let material = lib
                .material(TagId(shader), None)
                .map_err(|e| e.to_string())?;
            let layer = material
                .layers
                .iter()
                .find(|l| l.lod == 0 && l.layer == native_render::gels::LAYER as usize)
                .ok_or("gel has no layer 1")?;
            if layer.passes.len() != 1 {
                return Err("gel has multiple layer-1 passes".into());
            }
            let imp = layer.passes[0]
                .implementations
                .iter()
                .find(|i| i.index == 0)
                .ok_or("gel has no implementation 0")?;
            if imp.variant {
                return Err("gel uses a combiner variant".into());
            }
            if imp.animated.iter().any(|a| a.kind == AnimationKind::Bitmap) {
                return Err("gel animates bitmap identity".into());
            }
            let vertex = imp.vertex_shader.ok_or("gel has no vertex shader")?;
            let address = map
                .tag(vertex)
                .map_err(|e| e.to_string())?
                .location
                .as_ref()
                .ok_or("gel vertex tag is not resident")?
                .address;
            let code = native_render::look::vertex_program(lib.metadata(), address, 0)
                .ok_or("gel has no classification-0 vertex program")?;
            let mut decoder = StageDecoder::new(Default::default());
            let mut images = HashMap::new();
            let mut stages = [None; 4];
            let mut uses_light_bitmap = false;
            for (n, t) in imp.textures.iter().enumerate() {
                let source = match t.map(|t| t.source) {
                    None | Some(TextureSource::None) => continue,
                    Some(TextureSource::Bitmap { bitmap, image, .. }) => (bitmap, image),
                    // While BUILDING is set, engine texture 10 uses the
                    // definition's bitmap, avoiding feedback from target 13.
                    Some(TextureSource::Engine(10)) if gel != u32::MAX => {
                        uses_light_bitmap = true;
                        (TagId(gel), 0)
                    }
                    _ => return Err(format!("gel stage {n} has an unresolved texture source")),
                };
                decoder.decode(
                    &mut map,
                    &mut files,
                    &mut images,
                    source.0,
                    source.1,
                    n,
                    true,
                )?;
                stages[n] = Some(source);
            }
            let images = stages.map(|s| s.and_then(|s| images.get(&s).cloned()));
            Ok(Arc::new(Asset {
                code,
                pixel_shader: imp.pixel_shader,
                images,
                uses_light_bitmap,
            }))
        })();
        out.insert((shader, gel), result);
    }
    Ok(out)
}
