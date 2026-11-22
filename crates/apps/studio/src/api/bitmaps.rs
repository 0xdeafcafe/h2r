//! The Textures tool as the agent API's `bitmap.*` ([`super::Provider`],
//! registered in [`super::seams`]): the open map's bitmaps, their images,
//! PNG export and import, strokes, fills and adjustments, and a read of
//! pixels. Every edit goes through `Action::Bitmap`, so an agent's edit is
//! the window's undo step; docs/editor/modeling.md ("Texture editing"),
//! docs/editor/agent-api.md.
//!
//! No method writes to the map: edits stay in the editor (the `bitmaps`
//! document is dirty while any bitmap is edited) until the encoder slice.

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
use crate::app::textures::{BitmapAction, MEMORY_ONLY};

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
        "One bitmap's images (opening it decodes them): size, depth, format, kind (D2, D3, Cube), mips stored, where LOD 0's pixels are, the layers each has, why one is read only, and the write class (`memory`: edits stay in the editor until the encoder writes them)",
        r#"{"bitmap":"string","image?":"integer"}"#,
        r#"{"bitmap":"string","name":"string","edited":"boolean","images":"[{image,width,height,depth,format,kind,mips,pixels,layers,write,locked,error,edited}]"}"#,
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
    let f = |k: &str| v.get(k).ok_or_else(|| bad(format!("rect: {k} is required")));
    Ok(Rect::new(n(f("x")?)?, n(f("y")?)?, n(f("w")?)?, n(f("h")?)?))
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
        *o = x
            .as_u64()
            .filter(|n| *n <= 255)
            .ok_or_else(want)? as u8;
    }
    Ok(out)
}

fn channels(p: &P) -> Result<Channels, RpcError> {
    match p.opt_str("channels")? {
        None => Ok(Channels::ALL),
        Some(s) => Channels::parse(s)
            .ok_or_else(|| bad(format!("channels: {s:?} is not letters of rgba (\"rgb\", \"a\")"))),
    }
}

fn num(v: &Value, k: &str) -> Result<f32, RpcError> {
    v.as_f64()
        .filter(|f| f.is_finite())
        .map(|f| f as f32)
        .ok_or_else(|| bad(format!("{k}: expected a number, not {v}")))
}

fn opt_num(obj: &Value, k: &str) -> Result<Option<f32>, RpcError> {
    obj.get(k).filter(|v| !v.is_null()).map(|v| num(v, k)).transpose()
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

fn images_json(app: &App, tag: TagId, only: Option<usize>) -> Value {
    let Some(open) = app.textures.open.get(&tag) else {
        return Value::Null;
    };
    Value::Array(
        open.images
            .iter()
            .filter(|i| only.is_none_or(|n| n == i.index))
            .map(|i| {
                let edited = i.layers.iter().any(|id| {
                    open.bitmap.layer(*id).is_some_and(|l| l.is_edited())
                });
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
                    "write": if i.locked.is_some() || i.error.is_some() { "none" } else { MEMORY_ONLY },
                    "locked": i.locked,
                    "error": i.error,
                    "edited": edited,
                })
            })
            .collect(),
    )
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
    json!({ "doc": "bitmaps", "dirty": app.textures.is_edited(), "edited": edited })
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
                    "edited": open.is_some_and(|o| o.bitmap.is_edited()),
                    "images": open.map(|o| o.images.len()),
                }));
            }
            Ok(Value::Array(out))
        }
        m::BITMAP_READ => {
            let tag = bitmap(app, p)?;
            let only = p.opt_usize("image")?;
            run(app, BitmapAction::Select { tag, image: only.unwrap_or(0) })?;
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
                "edited": open.bitmap.is_edited(),
                "images": images_json(app, tag, only),
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
            let size = p
                .opt_f32("size")?
                .ok_or_else(|| bad("size is required"))?;
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
            edit(app, tag, l, BitmapOp::Paint { layer: l, points, brush })
        }
        m::BITMAP_FILL => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let at = match p.get("at") {
                None => None,
                Some(v) => match v.as_array().map(Vec::as_slice) {
                    Some([x, y]) => match (x.as_u64(), y.as_u64()) {
                        (Some(x), Some(y)) if x <= u64::from(u32::MAX) && y <= u64::from(u32::MAX) => {
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
        m::BITMAP_PIXELS => {
            let tag = bitmap(app, p)?;
            let l = layer(p)?;
            let r = rect_of(p.get("rect").ok_or_else(|| bad("rect is required"))?)?;
            if u64::from(r.w) * u64::from(r.h) > MAX_PIXELS {
                return Err(err(
                    code::REFUSED,
                    format!("refused: {}x{} is over {MAX_PIXELS} pixels: read less", r.w, r.h),
                ));
            }
            run(app, BitmapAction::Select { tag, image: l.image as usize })?;
            let open = &app.textures.open[&tag];
            let (got, rgba) = open
                .bitmap
                .pixels(l, r)
                .map_err(|e| bad(e.to_string()))?;
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
