//! Thumbnails (docs/editor/studio-redesign.md, "The browser"): the small
//! picture a Browser row draws and `asset.thumb` writes.
//!
//! | Asset | Picture | Read from |
//! |---|---|---|
//! | a bitmap tag (`tag:bitm:<path>`) the shown map holds | image 0 at its base level | the map (`App::bitmap_picture`) |
//! | a package's `bitmap` item with a PNG source | the PNG | the package's file (`workspace::decode_png`) |
//!
//! Every other asset has none yet ([`ThumbRefusal::NoPicture`]). A
//! picture is shrunk to [`THUMB_EDGE`] on its longer side.
use super::*;
use crate::app::map_pack::Picture;

/// The longer side of a thumbnail, in pixels.
pub const THUMB_EDGE: u32 = 64;

/// `pic` at most `edge` pixels on its longer side, by the nearest texel.
/// A picture that fits is returned as it is.
pub fn shrink(pic: &Picture, edge: u32) -> Picture {
    let (w, h) = (pic.width, pic.height);
    let long = w.max(h);
    if long <= edge || w == 0 || h == 0 || pic.rgba.len() != (w * h * 4) as usize {
        return pic.clone();
    }
    let (nw, nh) = ((w * edge / long).max(1), (h * edge / long).max(1));
    let mut rgba = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        let sy = (y * h / nh).min(h - 1);
        for x in 0..nw {
            let sx = (x * w / nw).min(w - 1);
            let at = ((sy * w + sx) * 4) as usize;
            rgba.extend_from_slice(&pic.rgba[at..at + 4]);
        }
    }
    Picture {
        width: nw,
        height: nh,
        rgba,
    }
}

/// Whether a Browser row is of a kind that has a picture: a bitmap tag,
/// or a package's `bitmap` item with a PNG source.
pub fn has_thumb(row: &Value) -> bool {
    match row["root"].as_str() {
        Some("tags") => row["group"].as_str().is_some_and(|g| g.trim() == "bitm"),
        Some("packages") => {
            row["item_kind"] == "bitmap"
                && row["source"]
                    .as_str()
                    .is_some_and(|s| s.to_ascii_lowercase().ends_with(".png"))
        }
        _ => false,
    }
}

impl App {
    /// The thumbnail of an asset, shrunk to [`THUMB_EDGE`].
    pub fn thumb_picture(&self, id: &AssetId) -> Result<Picture, ThumbRefusal> {
        use ThumbRefusal::{Failed, NoPicture};
        match id {
            AssetId::Item(package, n) => {
                let set = read_set(&self.data_root());
                let loaded = find(&set, package).map_err(Failed)?;
                let item = loaded
                    .package
                    .manifest
                    .items
                    .get(*n)
                    .ok_or_else(|| Failed(format!("package {package:?} has no item {n}")))?;
                let bytes = item
                    .source
                    .as_deref()
                    .and_then(|source| loaded.files.get(source))
                    .filter(|b| item.kind == ItemKind::Bitmap && b.starts_with(&[0x89, b'P', b'N', b'G']))
                    .ok_or_else(|| {
                        NoPicture(format!(
                            "item {n} of {package:?} ({:?}) is not a PNG bitmap source",
                            item.kind
                        ))
                    })?;
                let png = ::workspace::decode_png(bytes).map_err(Failed)?;
                let pic = Picture {
                    width: png.width.into(),
                    height: png.height.into(),
                    rgba: png.rgba,
                };
                Ok(shrink(&pic, THUMB_EDGE))
            }
            AssetId::Tag(key) if fourcc_text(key.group).trim() == "bitm" => {
                let text = format!("{}:{}", fourcc_text(key.group), key.path);
                let tag = tag_api::find_tag(self.shown_tags(), &text).ok_or_else(|| {
                    NoPicture(format!(
                        "{text} is not in the open map: a bitmap's picture is read from a map that holds it"
                    ))
                })?;
                let pic = self.bitmap_picture(tag, 0).map_err(Failed)?;
                Ok(shrink(&pic, THUMB_EDGE))
            }
            _ => Err(NoPicture(
                "only a bitmap tag of the open map and a package's PNG bitmap item have a thumbnail so far"
                    .to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_shrinks_to_the_edge_on_its_longer_side_and_a_small_one_is_kept() {
        // 8 by 4, each texel its own column in red.
        let rgba: Vec<u8> = (0..4u32)
            .flat_map(|_| (0..8u8).flat_map(|x| [x, 0, 0, 255]))
            .collect();
        let pic = Picture { width: 8, height: 4, rgba };
        let small = shrink(&pic, 4);
        assert_eq!((small.width, small.height), (4, 2));
        assert_eq!(small.rgba.len(), 4 * 2 * 4);
        // Every other column is kept.
        let reds: Vec<u8> = small.rgba.chunks_exact(4).take(4).map(|p| p[0]).collect();
        assert_eq!(reds, [0, 2, 4, 6]);
        assert_eq!(shrink(&pic, 8), pic);
        assert_eq!(shrink(&pic, 64), pic);
    }

    #[test]
    fn a_bitmap_tag_and_a_png_bitmap_item_have_pictures_and_other_rows_none() {
        assert!(has_thumb(&json!({"root": "tags", "group": "bitm"})));
        assert!(!has_thumb(&json!({"root": "tags", "group": "weap"})));
        assert!(has_thumb(&json!({"root": "packages", "item_kind": "bitmap", "source": "art/Icon.PNG"})));
        assert!(!has_thumb(&json!({"root": "packages", "item_kind": "bitmap", "source": "art/icon.tga"})));
        assert!(!has_thumb(&json!({"root": "packages", "item_kind": "sound", "source": "a.png"})));
        assert!(!has_thumb(&json!({"root": "voices"})));
    }
}
