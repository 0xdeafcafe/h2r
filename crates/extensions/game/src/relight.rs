//! Local, display-only lightmaps. Validate both complete arms against the
//! resident BSP before publishing either to a native draw.
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use content::{Sha256, relight};
use native_render::{gpu::Image, world::LightmapKey};

type Pages = HashMap<LightmapKey, Image>;

pub(crate) fn load(
    map: &str,
    index: u16,
    checksum: u32,
    map_checksum: u32,
    retail: &Pages,
) -> Result<Option<[Pages; 2]>, String> {
    let dir = crate::native_map::data_root()
        .ok_or("no runtime data directory")?
        .join(relight::DIRECTORY);
    read(&dir, map, index, checksum, map_checksum, retail)
        .map_err(|e| format!("{}: {e:#}", dir.display()))
}

fn read(
    dir: &Path,
    map: &str,
    index: u16,
    checksum: u32,
    map_checksum: u32,
    retail: &Pages,
) -> Result<Option<[Pages; 2]>> {
    // Read the manifest once. Pixel digests catch a replaced directory
    // during a load, including mixtures of two fits for the same BSP.
    const LIMIT: u64 = 16 << 20;
    let manifest_path = dir.join(relight::MANIFEST);
    let file = File::open(&manifest_path).context("open lighting manifest")?;
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "lighting manifest exceeds {LIMIT} bytes"
    );
    let manifest: relight::Manifest = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest.format == relight::FORMAT,
        "unknown display lightmap format"
    );
    if manifest.bsp != index || !manifest.map.eq_ignore_ascii_case(map) {
        return Ok(None);
    }
    ensure!(
        manifest.structure_checksum == checksum && manifest.map_checksum == map_checksum,
        "installed bake is for a different revision of {map} BSP {index}"
    );
    ensure!(
        !retail.is_empty(),
        "the active BSP has no native lightmap pages"
    );

    let mut levels = BTreeMap::new();
    for level in manifest.levels {
        let key = LightmapKey {
            page: level.page,
            palette: Some(level.palette),
        };
        ensure!(
            levels.insert((key, level.mip), level).is_none(),
            "duplicate page/mip for {key:?}"
        );
    }
    let mut result = [Pages::new(), Pages::new()];
    let mut changed = false;
    for (&key, image) in retail {
        let mut on = Vec::new();
        let mut off = Vec::new();
        let (mut w, mut h) = (image.width, image.height);
        let chain = std::iter::once(&image.rgba).chain(&image.mips);
        for (mip, pixels) in chain.enumerate() {
            let mip = u32::try_from(mip)?;
            let record = levels
                .remove(&(key, mip))
                .with_context(|| format!("missing {key:?} mip {mip}"))?;
            let stem = relight::page_stem(key.page, key.palette.unwrap_or(u8::MAX), mip);
            ensure!(
                (record.width, record.height) == (w, h),
                "{stem}: dimensions differ"
            );
            ensure!(
                record.retail == Sha256::of(pixels),
                "{stem}: retail pixels differ from bake"
            );
            let suffix = if pixels.iter().skip(3).step_by(4).any(|&a| a != 255) {
                "-rgba"
            } else {
                ""
            };
            for (arm, digest, out) in [("on", record.on, &mut on), ("off", record.off, &mut off)] {
                let path = dir.join(format!("{stem}-{arm}{suffix}.png"));
                let rgba = read_png(&path, w, h, pixels, digest)
                    .with_context(|| format!("read {}", path.display()))?;
                out.push((w, h, rgba));
            }
            changed |= mip == 0 && record.on != record.off;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        ensure!(
            levels.range((key, 0)..=(key, u32::MAX)).next().is_none(),
            "extra mip for {key:?}"
        );
        for (pages, chain) in result.iter_mut().zip([on, off]) {
            let replacement =
                Image::from_levels(chain).context("invalid replacement base image")?;
            ensure!(
                replacement.mips.len() == image.mips.len(),
                "invalid mip chain for {key:?}"
            );
            pages.insert(key, replacement);
        }
    }
    // The split also describes bindings for undrawn sections. Only the
    // pages present in the native BSP can demonstrate a switched lamp.
    ensure!(
        changed,
        "placed lamp {} changes no native base page",
        manifest.placed_lamp
    );
    Ok(Some(result))
}

fn read_png(path: &Path, w: u32, h: u32, retail: &[u8], digest: Sha256) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "expected a PNG file");
    // Size and allocations are bounded by the trusted resident page.
    let max_file = 65536 + 2 * retail.len() as u64;
    ensure!(metadata.len() <= max_file, "PNG exceeds page size budget");
    let mut reader = png::Decoder::new(BufReader::new(file)).read_info()?;
    let info = reader.info();
    ensure!(
        (info.width, info.height) == (w, h)
            && info.color_type == png::ColorType::Rgba
            && info.bit_depth == png::BitDepth::Eight
            && info.animation_control.is_none()
            && reader.output_buffer_size() == Some(retail.len()),
        "expected one {w} x {h} RGBA8 image"
    );
    let mut rgba = vec![0; retail.len()];
    reader.next_frame(&mut rgba)?;
    reader.finish()?;
    ensure!(
        Sha256::of(&rgba) == digest,
        "pixel digest differs from manifest"
    );
    ensure!(
        rgba.iter()
            .skip(3)
            .step_by(4)
            .eq(retail.iter().skip(3).step_by(4)),
        "alpha differs from retail"
    );
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_override::tests::Temp;

    struct Bake {
        dir: Temp,
        retail: Pages,
        manifest: relight::Manifest,
    }

    impl Bake {
        fn new() -> Self {
            let dir = Temp::new();
            let mut levels = Vec::new();
            let mut pages = Pages::new();
            for palette in [2, 3] {
                let key = LightmapKey { page: 4, palette: Some(palette) };
                let mut chain = Vec::new();
                for (mip, width) in [(0, 2), (1, 1)] {
                    let alpha = if palette == 2 { 255 } else { 37 };
                    let retail = [80, 21, palette, alpha].repeat(width as usize);
                    let on = [79, 21, palette, alpha].repeat(width as usize);
                    let off = [19, 21, palette, alpha].repeat(width as usize);
                    let stem = relight::page_stem(key.page, palette, mip);
                    let suffix = if alpha == 255 { "" } else { "-rgba" };
                    for (arm, rgba) in [("on", &on), ("off", &off)] {
                        let path = dir.0.join(format!("{stem}-{arm}{suffix}.png"));
                        assets::png::write_rgba(File::create(path).unwrap(), width, 1, rgba)
                            .unwrap();
                    }
                    levels.push(relight::Level {
                        page: key.page,
                        palette,
                        mip,
                        width,
                        height: 1,
                        retail: Sha256::of(&retail),
                        on: Sha256::of(&on),
                        off: Sha256::of(&off),
                    });
                    chain.push((width, 1, retail));
                }
                pages.insert(key, Image::from_levels(chain).unwrap());
            }
            Self {
                dir,
                retail: pages,
                manifest: relight::Manifest {
                    format: relight::FORMAT.into(),
                    map: "invented".into(),
                    map_checksum: 71,
                    bsp: 1,
                    structure_checksum: 89,
                    placed_lamp: 4,
                    levels,
                },
            }
        }

        fn open(&self) -> Result<Option<[Pages; 2]>> {
            let path = self.dir.0.join(relight::MANIFEST);
            std::fs::write(path, serde_json::to_vec(&self.manifest)?)?;
            read(&self.dir.0, "invented", 1, 89, 71, &self.retail)
        }
    }

    #[test]
    fn binds_each_palette_and_mip_preserving_alpha_and_retail_pixels() {
        let bake = Bake::new();
        let [on, off] = bake.open().unwrap().unwrap();
        for (&key, retail) in &bake.retail {
            let (a, b) = (&on[&key], &off[&key]);
            assert_eq!((a.rgba[0], b.rgba[0], retail.rgba[0]), (79, 19, 80));
            assert_eq!(b.mips[0], [19, 21, key.palette.unwrap(), retail.rgba[3]]);
            assert_eq!(&a.rgba[1..4], &retail.rgba[1..4]);
        }
    }

    #[test]
    fn rejects_wrong_revision_or_source_pixels_and_ignores_unrelated_bsp() {
        let mut bake = Bake::new();
        bake.manifest.bsp = 0;
        assert!(bake.open().unwrap().is_none());
        bake.manifest.bsp = 1;
        bake.manifest.map = "other".into();
        assert!(bake.open().unwrap().is_none());
        bake.manifest.map = "invented".into();
        bake.manifest.structure_checksum += 1;
        assert!(bake.open().is_err());
        bake.manifest.structure_checksum -= 1;
        bake.manifest.map_checksum += 1;
        assert!(bake.open().is_err());
        bake.manifest.map_checksum -= 1;
        bake.manifest.levels[0].retail = Sha256::of(b"stale");
        assert!(bake.open().is_err());
    }

    #[test]
    fn rejects_a_mis_sized_or_swapped_display_page() {
        let mut bake = Bake::new();
        bake.manifest.levels[0].height += 1;
        assert!(
            bake.open()
                .unwrap_err()
                .to_string()
                .contains("dimensions differ")
        );
        bake.manifest.levels[0].height -= 1;
        let path = bake.dir.0.join("g0-page4-pal2-mip0-on.png");
        assets::png::write_rgba(File::create(&path).unwrap(), 1, 1, &[1; 4]).unwrap();
        assert!(format!("{:#}", bake.open().unwrap_err()).contains("expected one 2 x 1"));
        assets::png::write_rgba(File::create(&path).unwrap(), 2, 1, &[1; 8]).unwrap();
        assert!(format!("{:#}", bake.open().unwrap_err()).contains("pixel digest differs"));
        std::fs::remove_file(path).unwrap();
        assert!(bake.open().is_err());
    }

    #[test]
    fn refuses_both_arms_on_missing_extra_corrupt_or_duplicate_mip() {
        let mut bake = Bake::new();
        let level = bake.manifest.levels.pop().unwrap();
        assert!(bake.open().is_err());
        bake.manifest.levels.push(level);
        bake.manifest.levels[3].mip = 0;
        assert!(bake.open().unwrap_err().to_string().contains("duplicate"));
        bake.manifest.levels[3].mip = 1;
        let mut extra = bake.manifest.levels[3].clone();
        extra.mip = 2;
        bake.manifest.levels.push(extra);
        assert!(bake.open().unwrap_err().to_string().contains("extra mip"));
        bake.manifest.levels.pop();
        let path = bake.dir.0.join("g0-page4-pal3-mip1-off-rgba.png");
        std::fs::write(&path, b"truncated").unwrap();
        assert!(bake.open().is_err());
        // Even a well-formed replacement with a matching digest must retain alpha.
        let altered = [19, 21, 3, 255];
        assets::png::write_rgba(File::create(path).unwrap(), 1, 1, &altered).unwrap();
        bake.manifest.levels[3].off = Sha256::of(&altered);
        assert!(format!("{:#}", bake.open().unwrap_err()).contains("alpha differs"));
    }
}
