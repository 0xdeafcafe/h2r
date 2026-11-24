//! The Textures tool as the agent API's `bitmap.*` ([`super::Provider`],
//! registered in [`super::seams`]): the open map's bitmaps, their images,
//! PNG export and import, strokes, fills and adjustments, and a read of
//! pixels. Every edit goes through `Action::Bitmap`, so an agent's edit is
//! the window's undo step; docs/editor/modeling.md ("Texture editing"),
//! docs/editor/agent-api.md.
//!
//! Edits stay in the editor until a save: `doc.save` and `doc.save_as` on
//! the `bitmaps` document are the map's, and encode every edited image
//! (`assets::write`) into the file. `bitmap.reshape` and
//! `bitmap.image.resize` ask for a new size or format, `bitmap.image.add`
//! and `bitmap.sequence.add` grow the bitmap (the save then moves the tag),
//! `bitmap.sequence.set` and `bitmap.sprite.*` edit its sequences and
//! sprites (the sprite editor, which also moves the tag),
//! `bitmap.spans` shows the stored bytes an edit changes.

use std::path::PathBuf;

use agent_proto::RpcError;
use bitmap_edit::{Adjust, BitmapOp, Brush, Channels, LayerId, Rect};
use map::TagId;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::docs::{hex, outside_git};
use super::scenario::refused;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::Action;
use crate::app::textures::BitmapAction;
use tags::PixelFormat;

/// The Textures tool's provider.
#[derive(Default)]
pub struct Bitmaps;

const fn def(
    name: &'static str,
    summary: &'static str,
    params: &'static str,
    result: &'static str,
    example: &'static str,
) -> MethodDef {
    MethodDef {
        name,
        summary,
        params,
        result,
        example,
        planned: None,
    }
}

/// Most pixels `bitmap.pixels` returns in one call.
const MAX_PIXELS: u64 = 65_536;

/// `bitmap` is a tag id (`0x…`), the bitmap's path, the end of its path
/// (`floor`) or a part of it that names one bitmap. `image` (default 0),
/// `face` (cube maps) and `slice` (volumes) pick the layer.
const METHODS: &[MethodDef] = &[
    def(
        m::BITMAP_LIST,
        "The open map's bitmap tags: id, path, whether it is open for editing and edited, and its image count once open. `name` keeps those whose path has it",
        r#"{"name?":"string"}"#,
        r#"[{"bitmap":"string","name":"string","open":"boolean","edited":"boolean","images":"integer|null"}]"#,
        r#"{}"#,
    ),
    def(
        m::BITMAP_READ,
        "One bitmap's images (opening it decodes them): size, depth, format, kind (D2, D3, Cube), mips stored, where LOD 0's pixels are, the layers each has, why one is read only, the write class (`in_place`: every stored LOD is in this map and a save patches the touched bytes; `appended`: a LOD is in a shared cache and a save copies it into the map; `new`: an added image, written whole; `none`: read only), any `reshape` asked for, and the bitmap's sequences (label, first image, image count, sprites; `added` for one added in Studio). `grown` says a save writes the whole tag again (images or sequences added); `movable` is null, or why it cannot gain any",
        r#"{"bitmap":"string","image?":"integer"}"#,
        r#"{"bitmap":"string","name":"string","edited":"boolean","grown":"boolean","movable":"string|null","images":"[{image,width,height,depth,format,kind,mips,pixels,layers,write,reshape,locked,error,edited,added_from}]","sequences":"[{sequence,label,first,count,sprites,added}]"}"#,
        r#"{"bitmap":"floor"}"#,
    ),
    def(
        m::BITMAP_EXPORT,
        "Write a layer as an 8-bit RGBA PNG (`mip` halves it that many times, box filtered). Never into a Git checkout: maps and renders from your disc stay out of repositories",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","mip?":"integer","out":"string"}"#,
        r#"{"bitmap":"string","layer":"string","width":"integer","height":"integer","path":"string"}"#,
        r#"{"bitmap":"floor","out":"$OUT/floor.png"}"#,
    ),
    def(
        m::BITMAP_IMPORT,
        "Replace a layer, or `rect` of it, from a PNG (grey, RGB, RGBA, palette; 8 or 16 bit); resampled to the target when the sizes differ (box down, bilinear up, in linear light). One undo step",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","path":"string","rect?":"{x,y,w,h}"}"#,
        r#"{"bitmap":"string","layer":"string","dirty":"{x,y,w,h}|null","tiles":"integer","edited":"boolean"}"#,
        r#"{"bitmap":"floor","path":"$OUT/in.png"}"#,
    ),
    def(
        m::BITMAP_PAINT,
        "One stroke through `points` (pixel coordinates; a pixel's centre is at x + 0.5): a round brush of `size` pixels, `hardness` 0 to 1 (default 0.8), `opacity` 0 to 1 of each stamp (default 1), `spacing` as a fraction of the size (0.25); blended in linear light. `erase` takes alpha away instead. `channels` limits it (`rgba`, `a`, `rgb`, ...). One undo step",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","points":"[[x,y]]","color?":"[r,g,b,a?]|#rrggbb[aa]","size":"number","hardness?":"number","opacity?":"number","spacing?":"number","erase?":"boolean","channels?":"string"}"#,
        r#"{"bitmap":"string","layer":"string","dirty":"{x,y,w,h}|null","tiles":"integer","edited":"boolean"}"#,
        r#"{"bitmap":"floor","points":[[1,1],[2,2]],"color":[255,0,0,255],"size":2}"#,
    ),
    def(
        m::BITMAP_FILL,
        "Flood from `at` ([x, y]; by `tolerance` 0 to 1 of each channel's range, within `rect` when given), or fill every pixel of `rect`, with `color`; `channels` limits what is written. One undo step",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","at?":"[x,y]","rect?":"{x,y,w,h}","color":"[r,g,b,a?]|#rrggbb[aa]","tolerance?":"number","channels?":"string"}"#,
        r#"{"bitmap":"string","layer":"string","dirty":"{x,y,w,h}|null","tiles":"integer","edited":"boolean"}"#,
        r##"{"bitmap":"floor","rect":{"x":0,"y":0,"w":2,"h":2},"color":"#336699"}"##,
    ),
    def(
        m::BITMAP_ADJUST,
        "One of `invert` (true), `levels` ({black, white, gamma}: 0 <= black < white <= 1, gamma in (0, 10]) or `hsv` ({hue in degrees, saturation and value as multipliers}) over `rect` (the whole layer by default) on `channels`. One undo step",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","invert?":"boolean","levels?":"{black?,white?,gamma?}","hsv?":"{hue?,saturation?,value?}","rect?":"{x,y,w,h}","channels?":"string"}"#,
        r#"{"bitmap":"string","layer":"string","dirty":"{x,y,w,h}|null","tiles":"integer","edited":"boolean"}"#,
        r#"{"bitmap":"floor","invert":true,"channels":"rgb"}"#,
    ),
    def(
        m::BITMAP_RESHAPE,
        "Ask for a new size and/or format for an image (`format`: a8, y8, ay8, a8y8, rgb565, argb1555, argb4444, xrgb8888, argb8888, dxt1, dxt3 or dxt5; what is left out keeps its value). A save writes the image whole at that size: every layer resampled, every LOD it stores re-encoded and appended to the map, the record's size, format, flags and mips patched. A cube stays square, DXT needs power of two sides, a side that is not a power of two is linear without mips. Refused now when the save would refuse. One undo step; `doc.close` with `discard` drops it. Giving the image's own size and format clears it",
        r#"{"bitmap":"string","image?":"integer","width?":"integer","height?":"integer","format?":"string"}"#,
        r#"{"bitmap":"string","image":"integer","reshape":"{width,height,format}|null","edited":"boolean"}"#,
        r#"{"bitmap":"floor","width":8,"height":8,"format":"dxt1"}"#,
    ),
    def(
        m::BITMAP_SPANS,
        "The stored bytes the edits change, as the encoder writes them (nothing is written): per LOD, `offset` into the LOD's bytes and `len`, with the bytes as hex up to 65,536 in all. `rect` (with `face` and `slice`) asks for that region of mip 0 instead of the edited tiles. The unit a live push sends",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","rect?":"{x,y,w,h}"}"#,
        r#"{"bitmap":"string","image":"integer","write":"string","spans":"[{lod,offset,len,bytes}]","bytes":"integer"}"#,
        r#"{"bitmap":"floor"}"#,
    ),
    def(
        m::BITMAP_IMAGE_ADD,
        "Add an image after the bitmap's last: its record copied from image `from` (default 0; a 2D image), at `size` and `format` (default `from`'s), its pixels transparent black until painted, imported or generated into. Answers the new index. One undo step. A save writes the whole tag again at the end of the map's metadata, the new image's pixels as new raw blocks, and points the tag's index entry at the copy; the other images keep their pixels and every other tag its bytes",
        r#"{"bitmap":"string","from?":"integer","size?":"[w,h]","format?":"string"}"#,
        r#"{"bitmap":"string","image":"integer","width":"integer","height":"integer","format":"string","images":"integer"}"#,
        r#"{"bitmap":"floor","size":[8,8]}"#,
    ),
    def(
        m::BITMAP_IMAGE_RESIZE,
        "Give an image a new size and/or format, as bitmap.reshape, as one undo step: every layer resampled and every LOD written whole by the save. An added image is resized the same way",
        r#"{"bitmap":"string","image?":"integer","size?":"[w,h]","width?":"integer","height?":"integer","format?":"string"}"#,
        r#"{"bitmap":"string","image":"integer","reshape":"{width,height,format}|null","edited":"boolean"}"#,
        r#"{"bitmap":"floor","size":[8,8]}"#,
    ),
    def(
        m::BITMAP_SEQUENCE_ADD,
        "Add a sequence of `count` images (default 1) from image `first` after the bitmap's last sequence, labelled `label` (31 bytes at most), shaped like its first sequence and with one sprite per image when that one has sprites. Answers the new index and the bitmap's sequences. One undo step; the save moves the tag as bitmap.image.add does",
        r#"{"bitmap":"string","first":"integer","count?":"integer","label?":"string"}"#,
        r#"{"bitmap":"string","sequence":"integer","sequences":"[{sequence,label,first,count,sprites,added}]"}"#,
        r#"{"bitmap":"floor","first":0,"label":"floor"}"#,
    ),
    def(
        m::BITMAP_SEQUENCE_SET,
        "Change a sequence (the tag's own or one added): its `label` (31 bytes at most), `first` image and image `count`; a field left out stays. Its sprites are not touched. Answers the bitmap's sequences. One undo step (none when nothing changes); the save moves the tag as bitmap.image.add does. Refused when the tag cannot be laid out again (`bitmap.read`'s `movable`)",
        r#"{"bitmap":"string","sequence":"integer","label?":"string","first?":"integer","count?":"integer"}"#,
        r#"{"bitmap":"string","sequence":"integer","sequences":"[{sequence,label,first,count,sprites,added}]"}"#,
        r#"{"bitmap":"floor","sequence":0,"label":"icons 2"}"#,
    ),
    def(
        m::BITMAP_SPRITES,
        "The sprites of the bitmap's sequences (or of `sequence`): each one's image, its bounds on that image as fractions (`left`, `right`, `top`, `bottom`) and its registration point. Opens the bitmap; nothing is written",
        r#"{"bitmap":"string","sequence?":"integer"}"#,
        r#"{"bitmap":"string","sequences":"[{sequence,label,sprites:[{sprite,image,left,right,top,bottom,registration:[x,y]}]}]"}"#,
        r#"{"bitmap":"floor"}"#,
    ),
    def(
        m::BITMAP_SPRITE_ADD,
        "Append a sprite to `sequence`: a copy of its last sprite, or over all of the sequence's first image when it has none, then `image`, `bounds` ([left, right, top, bottom], fractions of the image, left <= right and top <= bottom) and `registration` ([x, y]) as given. Answers the new sprite's index and the sequence's sprites. One undo step; the save moves the tag",
        r#"{"bitmap":"string","sequence":"integer","image?":"integer","bounds?":"[left,right,top,bottom]","registration?":"[x,y]"}"#,
        r#"{"bitmap":"string","sequence":"integer","sprite":"integer","sprites":"[{sprite,image,left,right,top,bottom,registration}]"}"#,
        r#"{"bitmap":"floor","sequence":0,"bounds":[0,0.5,0,0.5]}"#,
    ),
    def(
        m::BITMAP_SPRITE_SET,
        "Change sprite `sprite` of `sequence`: its `image`, `bounds` and/or `registration`, as bitmap.sprite.add takes them; a field left out stays. A bounds pair that is inverted or a number that is not finite is refused and nothing changes. One undo step (none when nothing changes); the save moves the tag",
        r#"{"bitmap":"string","sequence":"integer","sprite":"integer","image?":"integer","bounds?":"[left,right,top,bottom]","registration?":"[x,y]"}"#,
        r#"{"bitmap":"string","sequence":"integer","sprite":"integer","sprites":"[{sprite,image,left,right,top,bottom,registration}]"}"#,
        r#"{"bitmap":"floor","sequence":0,"sprite":0,"registration":[0.5,0.5]}"#,
    ),
    def(
        m::BITMAP_SPRITE_REMOVE,
        "Remove sprite `sprite` from `sequence`; the sprites after it move down one. One undo step; the save moves the tag",
        r#"{"bitmap":"string","sequence":"integer","sprite":"integer"}"#,
        r#"{"bitmap":"string","sequence":"integer","sprites":"[{sprite,image,left,right,top,bottom,registration}]"}"#,
        r#"{"bitmap":"floor","sequence":0,"sprite":0}"#,
    ),
    def(
        m::BITMAP_PIXELS,
        "The pixels of `rect` (clipped to the layer; at most 65,536) as hex RGBA, row by row: for tests and checks",
        r#"{"bitmap":"string","image?":"integer","face?":"integer","slice?":"integer","rect":"{x,y,w,h}"}"#,
        r#"{"bitmap":"string","layer":"string","rect":"{x,y,w,h}","width":"integer","height":"integer","rgba":"string"}"#,
        r#"{"bitmap":"floor","rect":{"x":0,"y":0,"w":2,"h":2}}"#,
    ),
];

impl Provider for Bitmaps {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !METHODS.iter().any(|d| d.name == method) {
            return None;
        }
        Some(call(app, method, &P(params)))
    }
}

fn run(app: &mut App, a: BitmapAction) -> Result<Vec<String>, RpcError> {
    app.perform(Action::Bitmap(a), false).map_err(refused)
}

fn bad(e: impl Into<String>) -> RpcError {
    err(code::INVALID_PARAMS, e)
}

/// The `bitmap` param as a tag of the open map.
fn bitmap(app: &mut App, p: &P) -> Result<TagId, RpcError> {
    let key = p.str("bitmap")?;
    app.resolve_bitmap(key).map_err(|e| {
        if e.starts_with("no map") {
            err(code::NO_DOCUMENT, e)
        } else {
            bad(e)
        }
    })
}

/// `image`, `face` and `slice`.
fn layer(p: &P) -> Result<LayerId, RpcError> {
    let n = |k: &str| -> Result<u32, RpcError> {
        Ok(p.opt_u64(k)?.unwrap_or(0).min(u64::from(u32::MAX)) as u32)
    };
    Ok(LayerId::new(n("image")?, n("face")?, n("slice")?))
}

fn rect_of(v: &Value) -> Result<Rect, RpcError> {
    let n = |x: &Value| {
        x.as_u64()
            .filter(|n| *n <= u64::from(u32::MAX))
            .map(|n| n as u32)
            .ok_or_else(|| bad("rect: x, y, w and h are whole numbers of 0 or more"))
    };
    if let Some(a) = v.as_array() {
        let [x, y, w, h] = &a[..] else {
            return Err(bad("rect: [x, y, w, h] or {x, y, w, h}"));
        };
        return Ok(Rect::new(n(x)?, n(y)?, n(w)?, n(h)?));
    }
    let f = |k: &str| {
        v.get(k)
            .ok_or_else(|| bad(format!("rect: {k} is required")))
    };
    Ok(Rect::new(
        n(f("x")?)?,
        n(f("y")?)?,
        n(f("w")?)?,
        n(f("h")?)?,
    ))
}

fn opt_rect(p: &P) -> Result<Option<Rect>, RpcError> {
    p.get("rect").map(rect_of).transpose()
}

fn rect_json(r: Rect) -> Value {
    json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h })
}

/// `[r, g, b]`, `[r, g, b, a]` or `"#rrggbb"` / `"#rrggbbaa"`.
fn color_of(v: &Value) -> Result<[u8; 4], RpcError> {
    let want = || bad("color: [r, g, b, a?] of 0 to 255, or \"#rrggbb[aa]\"");
    if let Some(s) = v.as_str() {
        let h = s.strip_prefix('#').ok_or_else(want)?;
        if (h.len() != 6 && h.len() != 8) || !h.is_ascii() {
            return Err(want());
        }
        let byte = |i: usize| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).map_err(|_| want());
        return Ok([
            byte(0)?,
            byte(1)?,
            byte(2)?,
            if h.len() == 8 { byte(3)? } else { 255 },
        ]);
    }
    let a = v.as_array().ok_or_else(want)?;
    if a.len() != 3 && a.len() != 4 {
        return Err(want());
    }
    let mut out = [0, 0, 0, 255];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_u64().filter(|n| *n <= 255).ok_or_else(want)? as u8;
    }
    Ok(out)
}

fn channels(p: &P) -> Result<Channels, RpcError> {
    match p.opt_str("channels")? {
        None => Ok(Channels::ALL),
        Some(s) => Channels::parse(s).ok_or_else(|| {
            bad(format!(
                "channels: {s:?} is not letters of rgba (\"rgb\", \"a\")"
            ))
        }),
    }
}

fn num(v: &Value, k: &str) -> Result<f32, RpcError> {
    v.as_f64()
        .filter(|f| f.is_finite())
        .map(|f| f as f32)
        .ok_or_else(|| bad(format!("{k}: expected a number, not {v}")))
}

fn sprite_json(k: usize, v: tags::bitmap_grow::SpriteView) -> Value {
    json!({
        "sprite": k,
        "image": v.image,
        "left": v.left,
        "right": v.right,
        "top": v.top,
        "bottom": v.bottom,
        "registration": v.registration,
    })
}

/// The sprites of sequence `sequence`, as `bitmap.sprites` lists them.
fn sprites_json(app: &App, tag: TagId, sequence: usize) -> Value {
    let Some(open) = app.textures.open.get(&tag) else {
        return Value::Null;
    };
    let n = open.sequence(sequence).map_or(0, |s| s.sprites.len());
    Value::Array(
        (0..n)
            .filter_map(|k| open.sprite(sequence, k).map(|v| sprite_json(k, v)))
            .collect(),
    )
}

/// A fixed-length array of numbers.
fn nums<const N: usize>(p: &P, k: &str) -> Result<Option<[f32; N]>, RpcError> {
    let Some(v) = p.get(k) else {
        return Ok(None);
    };
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| bad(format!("{k}: expected {N} numbers")))?;
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = num(x, k)?;
    }
    Ok(Some(out))
}

fn opt_num(obj: &Value, k: &str) -> Result<Option<f32>, RpcError> {
    obj.get(k)
        .filter(|v| !v.is_null())
        .map(|v| num(v, k))
        .transpose()
}

fn layer_name(l: LayerId) -> String {
    l.to_string()
}

/// What an edit changed, as the last action recorded it.
fn edit_result(app: &App, tag: TagId, l: LayerId) -> Value {
    let (dirty, tiles) = match &app.textures.last {
        Some((t, a)) if *t == tag && a.layer == l && !a.dirty.is_empty() => {
            (rect_json(a.dirty), a.tiles.len())
        }
        _ => (Value::Null, 0),
    };
    let edited = app
        .textures
        .open
        .get(&tag)
        .is_some_and(|o| o.bitmap.is_edited());
    json!({
        "bitmap": super::scenario::tag_id(tag),
        "layer": layer_name(l),
        "dirty": dirty,
        "tiles": tiles,
        "edited": edited,
        "can_undo": app.textures.can_undo(),
    })
}

/// Performs `op` as one undo step and answers what it changed.
fn edit(app: &mut App, tag: TagId, l: LayerId, op: BitmapOp) -> Reply {
    app.textures.last = None;
    run(app, BitmapAction::Apply { tag, op })?;
    Ok(edit_result(app, tag, l))
}

/// A pixel format by the name its tag gives it, case blind (`dxt5`).
fn format_of(s: &str) -> Result<PixelFormat, RpcError> {
    use PixelFormat as F;
    const ALL: [F; 12] = [
        F::A8,
        F::Y8,
        F::Ay8,
        F::A8Y8,
        F::Rgb565,
        F::Argb1555,
        F::Argb4444,
        F::Xrgb8888,
        F::Argb8888,
        F::Dxt1,
        F::Dxt3,
        F::Dxt5,
    ];
    let key = s.to_ascii_lowercase().replace(['_', '-'], "");
    ALL.into_iter()
        .find(|f| format!("{f:?}").to_ascii_lowercase() == key)
        .ok_or_else(|| {
            bad(format!(
                "format: {s:?} is not one of {}",
                ALL.map(|f| format!("{f:?}").to_ascii_lowercase())
                    .join(", ")
            ))
        })
}

fn images_json(app: &App, tag: TagId, only: Option<usize>) -> Value {
    let Some(open) = app.textures.open.get(&tag) else {
        return Value::Null;
    };
    Value::Array(
        open.images
            .iter()
            .filter(|i| only.is_none_or(|n| n == i.index))
            .map(|i| {
                let edited = i
                    .layers
                    .iter()
                    .any(|id| open.bitmap.layer(*id).is_some_and(|l| l.is_edited()));
                json!({
                    "image": i.index,
                    "width": i.width,
                    "height": i.height,
                    "depth": i.depth,
                    "format": i.format,
                    "kind": i.kind,
                    "mips": i.mips,
                    "pixels": i.pixels,
                    "layers": i.layers.iter().map(|l| layer_name(*l)).collect::<Vec<_>>(),
                    "write": i.write_class(),
                    "reshape": app.textures.reshape.get(&(tag, i.index)).map(|r| json!({
                        "width": r.width,
                        "height": r.height,
                        "format": format!("{:?}", r.format),
                    })),
                    "locked": i.locked,
                    "error": i.error,
                    "edited": edited,
                    "added_from": i.added.as_ref().map(|a| a.from),
                })
            })
            .collect(),
    )
}

fn sequences_json(app: &App, tag: TagId) -> Value {
    let Some(open) = app.textures.open.get(&tag) else {
        return Value::Null;
    };
    Value::Array(
        open.sequences()
            .into_iter()
            .map(|s| {
                json!({
                    "sequence": s.index,
                    "label": s.label,
                    "first": s.first,
                    "count": s.count,
                    "sprites": s.sprites,
                    "added": s.added,
                })
            })
            .collect(),
    )
}

/// `size` as `[w, h]`, else `width` and `height`.
fn size_of(p: &P) -> Result<(Option<u32>, Option<u32>), RpcError> {
    let dim = |k: &str| -> Result<Option<u32>, RpcError> {
        p.opt_u64(k)?
            .map(|n| u32::try_from(n).map_err(|_| bad(format!("{k}: too large"))))
            .transpose()
    };
    match p.get("size") {
        None | Some(Value::Null) => Ok((dim("width")?, dim("height")?)),
        Some(v) => match v.as_array().map(Vec::as_slice) {
            Some([w, h]) => match (w.as_u64(), h.as_u64()) {
                (Some(w), Some(h)) if (1..=4096).contains(&w) && (1..=4096).contains(&h) => {
                    Ok((Some(w as u32), Some(h as u32)))
                }
                _ => Err(bad("size is [w, h], each 1 to 4096")),
            },
            _ => Err(bad("size is [w, h]")),
        },
    }
}

fn reshape_json(app: &App, tag: TagId, image: usize) -> Value {
    json!({
        "bitmap": super::scenario::tag_id(tag),
        "image": image,
        "reshape": app.textures.reshape.get(&(tag, image)).map(|r| json!({
            "width": r.width,
            "height": r.height,
            "format": format!("{:?}", r.format),
        })),
        "edited": app.textures.open.get(&tag).is_some_and(|o| app.textures.edited(o)),
    })
}

/// `doc.diff` of the `bitmaps` document: which layers differ from how
/// they were read, by tile.
pub fn diff(app: &App) -> Value {
    let mut edited = Vec::new();
    let mut tags: Vec<&TagId> = app.textures.open.keys().collect();
    tags.sort_by_key(|t| t.0);
    for tag in tags {
        let open = &app.textures.open[tag];
        let layers: Vec<Value> = open
            .bitmap
            .layers()
            .iter()
            .filter(|l| l.is_edited())
            .map(|l| json!({ "layer": layer_name(l.id), "tiles": l.changed_tiles().len() }))
            .collect();
        if !layers.is_empty() {
            edited.push(json!({
                "bitmap": super::scenario::tag_id(*tag),
                "name": open.name,
                "layers": layers,
            }));
        }
    }
    let reshaped: Vec<Value> = {
        let mut keys: Vec<_> = app.textures.reshape.iter().collect();
        keys.sort_by_key(|((t, i), _)| (t.0, *i));
        keys.into_iter()
            .map(|((t, i), r)| {
                json!({
                    "bitmap": super::scenario::tag_id(*t),
                    "image": i,
                    "width": r.width,
                    "height": r.height,
                    "format": format!("{:?}", r.format),
                })
            })
            .collect()
    };
    json!({
        "doc": "bitmaps",
        "dirty": app.tool_dirty(crate::ToolKind::Textures),
        "edited": edited,
        "reshape": reshaped,
    })
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    app.sync_textures();
    match method {
        m::BITMAP_LIST => {
            let name = p.opt_str("name")?;
            let mut out = Vec::new();
            for (tag, path) in app.texture_tags() {
                if name.is_some_and(|n| !path.contains(&n.replace('/', "\\"))) {
                    continue;
                }
                let open = app.textures.open.get(&tag);
                out.push(json!({
                    "bitmap": super::scenario::tag_id(tag),
                    "name": path,
                    "open": open.is_some(),
                    "edited": open.is_some_and(|o| app.textures.edited(o)),
                    "images": open.map(|o| o.images.len()),
                }));
            }
            Ok(Value::Array(out))
        }
        m::BITMAP_READ => {
            let tag = bitmap(app, p)?;
            let only = p.opt_usize("image")?;
            run(
                app,
                BitmapAction::Select {
                    tag,
                    image: only.unwrap_or(0),
                },
            )?;
            let open = &app.textures.open[&tag];
            if let Some(n) = only
                && n >= open.images.len()
            {
                return Err(bad(format!(
                    "image {n}: the bitmap has {} images",
                    open.images.len()
                )));
            }
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "name": open.name,
                "edited": app.textures.edited(open),
                "grown": open.grown(),
                "movable": open.parts.as_ref().err(),
                "images": images_json(app, tag, only),
                "sequences": sequences_json(app, tag),
            }))
        }
        m::BITMAP_IMAGE_ADD => {
            let tag = bitmap(app, p)?;
            let from = p.opt_usize("from")?;
            let (width, height) = size_of(p)?;
            let format = p.opt_str("format")?.map(format_of).transpose()?;
            run(
                app,
                BitmapAction::AddImage {
                    tag,
                    from,
                    width,
                    height,
                    format,
                },
            )?;
            let open = &app.textures.open[&tag];
            let i = open.images.last().ok_or_else(|| bad("the bitmap has no image"))?;
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "image": i.index,
                "width": i.width,
                "height": i.height,
                "format": i.format,
                "images": open.images.len(),
            }))
        }
        m::BITMAP_IMAGE_RESIZE => {
            let tag = bitmap(app, p)?;
            let image = p.opt_usize("image")?.unwrap_or(0);
            let (width, height) = size_of(p)?;
            let format = p.opt_str("format")?.map(format_of).transpose()?;
            if width.is_none() && height.is_none() && format.is_none() {
                return Err(bad("give size, width, height and/or format"));
            }
            run(
                app,
                BitmapAction::Reshape {
                    tag,
                    image,
                    width,
                    height,
                    format,
                },
            )?;
            Ok(reshape_json(app, tag, image))
        }
        m::BITMAP_SEQUENCE_ADD => {
            let tag = bitmap(app, p)?;
            let first = p
                .opt_usize("first")?
                .ok_or_else(|| bad("first is required"))?;
            let count = p.opt_u64("count")?.unwrap_or(1);
            let count = u16::try_from(count).map_err(|_| bad("count: too large"))?;
            let label = p.opt_str("label")?.unwrap_or("").to_owned();
            run(
                app,
                BitmapAction::AddSequence {
                    tag,
                    first,
                    count,
                    label,
                },
            )?;
            let n = app.textures.open[&tag].sequences().len();
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "sequence": n - 1,
                "sequences": sequences_json(app, tag),
            }))
        }
        m::BITMAP_SEQUENCE_SET => {
            let tag = bitmap(app, p)?;
            let sequence = p.usize("sequence")?;
            let label = p.opt_str("label")?.map(str::to_owned);
            let first = p.opt_usize("first")?;
            let count = p
                .opt_u64("count")?
                .map(|c| u16::try_from(c).map_err(|_| bad("count: too large")))
                .transpose()?;
            run(
                app,
                BitmapAction::SetSequence {
                    tag,
                    sequence,
                    label,
                    first,
                    count,
                },
            )?;
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "sequence": sequence,
                "sequences": sequences_json(app, tag),
            }))
        }
        m::BITMAP_SPRITES => {
            let tag = bitmap(app, p)?;
            app.ensure_bitmap(tag).map_err(bad)?;
            let only = p.opt_usize("sequence")?;
            let n = app.textures.open[&tag].sequences().len();
            if let Some(i) = only.filter(|i| *i >= n) {
                return Err(bad(format!("sequence {i}: the bitmap has {n}")));
            }
            let list: Vec<Value> = (0..n)
                .filter(|i| only.is_none_or(|o| o == *i))
                .map(|i| {
                    let label = app.textures.open[&tag]
                        .sequence(i)
                        .map(|s| s.label())
                        .unwrap_or_default();
                    json!({"sequence": i, "label": label, "sprites": sprites_json(app, tag, i)})
                })
                .collect();
            Ok(json!({"bitmap": super::scenario::tag_id(tag), "sequences": list}))
        }
        m::BITMAP_SPRITE_ADD | m::BITMAP_SPRITE_SET => {
            let tag = bitmap(app, p)?;
            let sequence = p.usize("sequence")?;
            let sprite = if method == m::BITMAP_SPRITE_SET {
                Some(p.usize("sprite")?)
            } else {
                None
            };
            run(
                app,
                BitmapAction::SetSprite {
                    tag,
                    sequence,
                    sprite,
                    image: p.opt_usize("image")?,
                    bounds: nums::<4>(p, "bounds")?,
                    registration: nums::<2>(p, "registration")?,
                },
            )?;
            let sprites = sprites_json(app, tag, sequence);
            let sprite = sprite.unwrap_or_else(|| sprites.as_array().map_or(0, |a| a.len()) - 1);
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "sequence": sequence,
                "sprite": sprite,
                "sprites": sprites,
            }))
        }
        m::BITMAP_SPRITE_REMOVE => {
            let tag = bitmap(app, p)?;
            let sequence = p.usize("sequence")?;
            let sprite = p.usize("sprite")?;
            run(
                app,
                BitmapAction::RemoveSprite {
                    tag,
                    sequence,
                    sprite,
                },
            )?;
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "sequence": sequence,
                "sprites": sprites_json(app, tag, sequence),
            }))
        }
        m::BITMAP_EXPORT => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let out = PathBuf::from(p.str("out")?);
            outside_git(&out)?;
            let mip = p.opt_u64("mip")?.unwrap_or(0).min(15) as u32;
            run(
                app,
                BitmapAction::Export {
                    tag,
                    layer: l,
                    mip,
                    path: out.clone(),
                },
            )?;
            let c = app.textures.open[&tag]
                .bitmap
                .layer(l)
                .map(|x| (x.canvas.width(), x.canvas.height()));
            let (w, h) = c.unwrap_or((0, 0));
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "layer": layer_name(l),
                "width": (w >> mip).max(1),
                "height": (h >> mip).max(1),
                "path": out.display().to_string(),
            }))
        }
        m::BITMAP_IMPORT => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let path = PathBuf::from(p.str("path")?);
            let rect = opt_rect(p)?;
            app.textures.last = None;
            run(
                app,
                BitmapAction::Import {
                    tag,
                    layer: l,
                    rect,
                    path,
                },
            )?;
            Ok(edit_result(app, tag, l))
        }
        m::BITMAP_PAINT => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let points = p
                .get("points")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("points: [[x, y], ...] is required"))?
                .iter()
                .map(|pt| match pt.as_array().map(Vec::as_slice) {
                    Some([x, y]) => Ok([num(x, "points")?, num(y, "points")?]),
                    _ => Err(bad("points: each is [x, y]")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let erase = p.bool("erase")?;
            let color = match p.get("color") {
                Some(v) => color_of(v)?,
                None if erase => [0, 0, 0, 255],
                None => return Err(bad("color is required (unless erase is true)")),
            };
            let size = p.opt_f32("size")?.ok_or_else(|| bad("size is required"))?;
            let d = Brush::default();
            let brush = Brush {
                size,
                hardness: p.opt_f32("hardness")?.unwrap_or(d.hardness),
                opacity: p.opt_f32("opacity")?.unwrap_or(d.opacity),
                spacing: p.opt_f32("spacing")?.unwrap_or(d.spacing),
                color,
                erase,
                channels: channels(p)?,
            };
            brush.validate().map_err(|e| bad(e.to_string()))?;
            edit(
                app,
                tag,
                l,
                BitmapOp::Paint {
                    layer: l,
                    points,
                    brush,
                },
            )
        }
        m::BITMAP_FILL => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let at = match p.get("at") {
                None => None,
                Some(v) => match v.as_array().map(Vec::as_slice) {
                    Some([x, y]) => match (x.as_u64(), y.as_u64()) {
                        (Some(x), Some(y))
                            if x <= u64::from(u32::MAX) && y <= u64::from(u32::MAX) =>
                        {
                            Some([x as u32, y as u32])
                        }
                        _ => return Err(bad("at: [x, y] whole numbers of 0 or more")),
                    },
                    _ => return Err(bad("at: [x, y]")),
                },
            };
            let rect = opt_rect(p)?;
            if at.is_none() && rect.is_none() {
                return Err(bad("a fill needs `at` (a flood) or `rect`"));
            }
            let color = color_of(p.get("color").ok_or_else(|| bad("color is required"))?)?;
            let tolerance = p.opt_f32("tolerance")?.unwrap_or(0.0);
            edit(
                app,
                tag,
                l,
                BitmapOp::Fill {
                    layer: l,
                    at,
                    rect,
                    color,
                    tolerance,
                    channels: channels(p)?,
                },
            )
        }
        m::BITMAP_ADJUST => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let invert = p.bool("invert")?;
            let levels = p.get("levels");
            let hsv = p.get("hsv");
            let adjust = match (invert, levels, hsv) {
                (true, None, None) => Adjust::Invert,
                (false, Some(v), None) => Adjust::Levels {
                    black: opt_num(v, "black")?.unwrap_or(0.0),
                    white: opt_num(v, "white")?.unwrap_or(1.0),
                    gamma: opt_num(v, "gamma")?.unwrap_or(1.0),
                },
                (false, None, Some(v)) => Adjust::Hsv {
                    hue: opt_num(v, "hue")?.unwrap_or(0.0),
                    saturation: opt_num(v, "saturation")?.unwrap_or(1.0),
                    value: opt_num(v, "value")?.unwrap_or(1.0),
                },
                _ => return Err(bad("give exactly one of invert, levels, hsv")),
            };
            adjust.validate().map_err(|e| bad(e.to_string()))?;
            edit(
                app,
                tag,
                l,
                BitmapOp::Adjust {
                    layer: l,
                    adjust,
                    rect: opt_rect(p)?,
                    channels: channels(p)?,
                },
            )
        }
        m::BITMAP_RESHAPE => {
            let tag = bitmap(app, p)?;
            let image = p.opt_usize("image")?.unwrap_or(0);
            let dim = |k: &str| -> Result<Option<u32>, RpcError> {
                p.opt_u64(k)?
                    .map(|n| u32::try_from(n).map_err(|_| bad(format!("{k}: too large"))))
                    .transpose()
            };
            let (width, height) = (dim("width")?, dim("height")?);
            let format = p.opt_str("format")?.map(format_of).transpose()?;
            if width.is_none() && height.is_none() && format.is_none() {
                return Err(bad("give width, height and/or format"));
            }
            run(
                app,
                BitmapAction::Reshape {
                    tag,
                    image,
                    width,
                    height,
                    format,
                },
            )?;
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "image": image,
                "reshape": app.textures.reshape.get(&(tag, image)).map(|r| json!({
                    "width": r.width,
                    "height": r.height,
                    "format": format!("{:?}", r.format),
                })),
                "edited": app.textures.open.get(&tag).is_some_and(|o| app.textures.edited(o)),
            }))
        }
        m::BITMAP_SPANS => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            run(
                app,
                BitmapAction::Select {
                    tag,
                    image: l.image as usize,
                },
            )?;
            let rects: Vec<(LayerId, Rect)> = match opt_rect(p)? {
                Some(r) => vec![(l, r)],
                None => app.textures.open[&tag]
                    .bitmap
                    .layers()
                    .iter()
                    .filter(|x| x.id.image == l.image)
                    .flat_map(|x| {
                        crate::app::textures::tight_changes(x)
                            .into_iter()
                            .map(move |r| (x.id, r))
                    })
                    .collect(),
            };
            let spans = app
                .bitmap_tile_spans(tag, l.image as usize, &rects)
                .map_err(bad)?;
            let total: usize = spans.iter().map(|s| s.bytes.len()).sum();
            let mut left = 65_536usize;
            let list: Vec<Value> = spans
                .iter()
                .map(|s| {
                    let bytes = (s.bytes.len() <= left).then(|| {
                        left -= s.bytes.len();
                        hex(&s.bytes)
                    });
                    json!({ "lod": s.lod, "offset": s.offset, "len": s.bytes.len(), "bytes": bytes })
                })
                .collect();
            let write = app.textures.open[&tag]
                .images
                .get(l.image as usize)
                .map_or("none", |i| i.write_class());
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "image": l.image,
                "write": write,
                "spans": list,
                "bytes": total,
            }))
        }
        m::BITMAP_PIXELS => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let r = rect_of(p.get("rect").ok_or_else(|| bad("rect is required"))?)?;
            if u64::from(r.w) * u64::from(r.h) > MAX_PIXELS {
                return Err(err(
                    code::REFUSED,
                    format!(
                        "refused: {}x{} is over {MAX_PIXELS} pixels: read less",
                        r.w, r.h
                    ),
                ));
            }
            run(
                app,
                BitmapAction::Select {
                    tag,
                    image: l.image as usize,
                },
            )?;
            let open = &app.textures.open[&tag];
            let (got, rgba) = open.bitmap.pixels(l, r).map_err(|e| bad(e.to_string()))?;
            Ok(json!({
                "bitmap": super::scenario::tag_id(tag),
                "layer": layer_name(l),
                "rect": rect_json(got),
                "width": got.w,
                "height": got.h,
                "rgba": hex(&rgba),
            }))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}
