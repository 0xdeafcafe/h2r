//! `blockout.*` (docs/game/forge-blockout.md, "Interfaces"): a block-out
//! definition drawn offscreen with its dev plates on (`blockout.render`),
//! the contact sheet of every shape of every family (`blockout.render`
//! with no shape), and what a definition is as text (`blockout.describe`).
//!
//! The geometry is the kit's own: `PieceDef::generate`, then
//! `blockout_kit::mesh::build` in the dev view, so a render shows the
//! faces, the cut-out plates and the glyph cells a map would carry. The
//! viewport has no textures, so the dev raster is read here: a tile face
//! is its family's colour, and each plate cell is drawn as the runs of
//! texels of its glyph in the font atlas (`dev_texture::font::font`),
//! one quad a run. Each picture is drawn from one camera at one scale on
//! a ground grid of 64 units, a heavier line every 256. Needs no map and
//! no disc.

use std::path::PathBuf;

use blockout_kit::mesh::{self, FacePlate, Mesh, Paint, View};
use dev_texture::label::{self, COLUMNS, ROWS};
use dev_texture::{Family, Image, font};
use forge_shapes::{ParsedName, PieceDef, Role, Shape, Solid};
use scenario_edit::geometry::Camera;
use scenario_edit::layers::{LayerId, Layers};
use scenario_edit::render::{Vertex, Viewport};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::view::Gpu;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;

/// The `blockout.*` provider: a GPU for the renders.
#[derive(Default)]
pub struct Blockout {
    gpu: Option<Result<Gpu, String>>,
}

const METHODS: &[MethodDef] = &[
    MethodDef {
        name: m::BLOCKOUT_RENDER,
        summary: "Draw a block-out definition offscreen with the viewport's renderer to a PNG (default: the cache directory's studio/blockout/; never into a Git checkout): its shell in the dev view, each face in its role's family colour, its edges, and the dev plates (the shape's name, the face's size in units and in metres) as the kit cuts them out, on a ground grid of 64 units with a heavier line every 256. The definition is a canonical `name` (a name from the starter set or any name `describe` gives, its digest checked) or `shape`, `size` ([x, y, z] units) and `param` (steps, sides, voussoirs or seed); the materials are all Dev. With neither `name` nor `shape`, the contact sheet: every structural piece of the starter set, then three seeds of each natural shape, one cell each at one camera and one scale (`fit`: sheet, the default, or shape, one scale a shape), each labelled with its canonical name and its family. `view`: angled (three-quarter, default), top, front or side. `plates` false draws each plate's cells as flat colour. Needs no map and no disc",
        params: r#"{"name?":"string","shape?":"string","size?":"[x,y,z]","param?":"integer","view?":"angled|top|front|side","pixels?":"[w,h] (a piece: [960,720]; the sheet: [3200,2000])","plates?":"boolean (default true)","fit?":"sheet|shape (the sheet only)","out?":"string"}"#,
        result: r#"{"path":"string","width":"integer","height":"integer","view":"string","name?":"string (a piece)","shape?":"string","size?":"[x,y,z]","param?":"integer","covered":"integer (pixels off the corner's background)","piece_pixels":"integer (pixels the piece changes from the same picture with no piece)","vertices":"integer","triangles":"integer","plates":"[{face,width,height,h,text:[string,string,string],visible,center:[x,y] (pixels),bounds:[x0,y0,x1,y1] (pixels)}]","grid_units":"integer","fit?":"string (the sheet)","cells?":"[{name,family,shape,size,param,cell:[col,row],rect:[x,y,w,h],center:[x,y],covered,piece_pixels,vertices,triangles,plates}] (the sheet)"}"#,
        example: r#"{"shape":"block","size":[256,256,256],"pixels":[320,240],"out":"$OUT/block.png"}"#,
        planned: None,
    },
    MethodDef {
        name: m::BLOCKOUT_DESCRIBE,
        summary: "A block-out definition as text: its canonical name and tag path, its family, the mesh the kit builds in the dev view (vertices and triangles, and a part a paint), the shell's faces, hulls and bounds, and the plate of each rectangular face that carries one: the face's size in units, the cell size `h` and the three rows of text label::layout gives. The definition is given as for blockout.render (a name, or a shape with its size and param); with none it is refused. Needs no map and no disc",
        params: r#"{"name?":"string","shape?":"string","size?":"[x,y,z]","param?":"integer"}"#,
        result: r#"{"name":"string","tag_path":"string","digest":"string","shape":"string","family":"structural|natural","size":"[x,y,z]","param":"integer","label":"string (the name a plate prints)","faces":"integer","hulls":"integer","hull_points":"integer","volume":"number (cubic world units)","bounds":"{min:[x,y,z],max:[x,y,z]} (world units)","vertices":"integer","triangles":"integer","parts":"[{paint,vertices,triangles}]","plates":"[{face,role,width,height,h,text:[string,string,string],metres:[string,string]}]","unplated":"integer (faces with no plate)"}"#,
        example: r#"{"shape":"block","size":[256,256,256]}"#,
        planned: None,
    },
];

impl Provider for Blockout {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, _app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        match method {
            m::BLOCKOUT_RENDER => Some(self.render(&p)),
            m::BLOCKOUT_DESCRIBE => Some(describe(&p)),
            _ => None,
        }
    }
}

/// The ground grid's spacing and its heavy line, in units.
const GRID_UNITS: f32 = 64.0;
const HEAVY_UNITS: f32 = 256.0;
/// The label's room under each sheet cell, in pixels.
const LABEL_H: u32 = 36;
/// A text cell's advance and height in pixels (the font's cell cropped).
const ADVANCE: u32 = 12;
const TEXT_H: u32 = 16;

fn invalid(msg: impl Into<String>) -> agent_proto::RpcError {
    err(code::INVALID_PARAMS, msg)
}

// ---- the definition ----

/// The family a shape belongs to on the sheet and in `describe`.
fn family_of(shape: Shape) -> &'static str {
    if shape.is_natural() { "natural" } else { "structural" }
}

fn shape_names() -> String {
    Shape::ALL.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
}

/// A definition from `name`, or `shape`, `size` and `param`, with every
/// material Dev; `None` when none of them is given.
fn def_of(p: &P) -> Result<Option<PieceDef>, agent_proto::RpcError> {
    if let Some(name) = p.opt_str("name")? {
        if p.has("shape") || p.has("size") || p.has("param") {
            return Err(invalid("name: give a name or a shape with its size, not both"));
        }
        let (shape, size, param) = match ParsedName::parse(name) {
            Ok(n) => (n.shape, n.size, n.param),
            // `<shape>_<x>x<y>x<z>_<param>`, the name without its digest.
            Err(e) => short_name(name).ok_or_else(|| invalid(format!("name: {e}")))?,
        };
        let def = PieceDef::dev(shape, size, param);
        if let Ok(n) = ParsedName::parse(name)
            && !n.matches(&def)
        {
            return Err(invalid(format!(
                "name: {name} is not a Dev definition (its digest is {}, a Dev {} has {})",
                n.digest,
                def.shape,
                def.digest12()
            )));
        }
        def.validate().map_err(|e| invalid(format!("name: {e}")))?;
        return Ok(Some(def));
    }
    let Some(shape) = p.opt_str("shape")? else {
        if p.has("size") || p.has("param") {
            return Err(invalid("shape: required with size and param"));
        }
        return Ok(None);
    };
    let shape = Shape::from_name(shape)
        .ok_or_else(|| invalid(format!("shape: one of {}, not {shape:?}", shape_names())))?;
    let size = size_param(p)?;
    let param = match p.opt_u64("param")? {
        None => 0,
        Some(n) => u16::try_from(n).map_err(|_| invalid(format!("param: 0 to 65535, not {n}")))?,
    };
    let def = PieceDef::dev(shape, size, param);
    def.validate().map_err(|e| {
        let what = if matches!(e, forge_shapes::Error::ParamRange { .. }) { "param" } else { "size" };
        invalid(format!("{what}: {e}"))
    })?;
    Ok(Some(def))
}

/// `<shape>_<x>x<y>x<z>_<param>`.
fn short_name(name: &str) -> Option<(Shape, [u16; 3], u16)> {
    let mut parts = name.rsplitn(3, '_');
    let param = parts.next()?.parse::<u16>().ok()?;
    let size = parts.next()?;
    let shape = Shape::from_name(parts.next()?)?;
    let axes: Vec<u16> = size.split('x').map(|a| a.parse().ok()).collect::<Option<_>>()?;
    let [x, y, z] = axes[..] else { return None };
    Some((shape, [x, y, z], param))
}

fn size_param(p: &P) -> Result<[u16; 3], agent_proto::RpcError> {
    let v = p.get("size").ok_or_else(|| invalid("size: required with shape ([x, y, z] units)"))?;
    let a: Option<Vec<u16>> = v.as_array().and_then(|a| {
        a.iter().map(|x| x.as_u64().and_then(|n| u16::try_from(n).ok())).collect()
    });
    match a.as_deref() {
        Some(&[x, y, z]) => Ok([x, y, z]),
        _ => Err(invalid(format!("size: [x, y, z] whole units, not {v}"))),
    }
}

/// A definition generated and drawn in the dev view.
struct Piece {
    solid: Solid,
    mesh: Mesh,
    min: [f32; 3],
    max: [f32; 3],
}

fn piece(def: &PieceDef) -> Result<Piece, agent_proto::RpcError> {
    let solid = def.generate().map_err(|e| invalid(format!("{}: {e}", def.name())))?;
    let mesh = mesh::build(def, &solid, View::Dev);
    let (min, max) = solid.bounds();
    Ok(Piece { solid, mesh, min, max })
}

impl Piece {
    /// Where the camera looks: the footprint's middle, at about the
    /// height's middle.
    fn target(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            self.max[2] * 0.45,
        ]
    }

    /// Half the longer side of the footprint.
    fn half_foot(&self) -> f32 {
        ((self.max[0] - self.min[0]).max(self.max[1] - self.min[1])) / 2.0
    }
}

/// The half-side of the ground drawn under a piece of footprint `half`.
fn floor_half(half: f32) -> f32 {
    (half * 1.35).max(0.6)
}

/// The radius of the sphere about `piece`'s target that holds the piece
/// and its floor of half-side `floor`.
fn radius_of(piece: &Piece, floor: f32) -> f32 {
    let t = piece.target();
    let mut r = 0.0_f32;
    let mut far = |q: [f32; 3]| {
        let d = [q[0] - t[0], q[1] - t[1], q[2] - t[2]];
        r = r.max((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
    };
    for x in [piece.min[0], piece.max[0]] {
        for y in [piece.min[1], piece.max[1]] {
            for z in [piece.min[2], piece.max[2]] {
                far([x, y, z]);
            }
        }
    }
    for dx in [-floor, floor] {
        for dy in [-floor, floor] {
            far([t[0] + dx, t[1] + dy, 0.0]);
        }
    }
    r
}

// ---- the drawing ----

/// `(yaw, pitch)` of a view; the eye is `target + (cos yaw, sin yaw) *
/// cos pitch, sin pitch` times the distance.
fn view_angles(view: &str) -> Result<(f32, f32), agent_proto::RpcError> {
    Ok(match view {
        // Three-quarter: the front (-y) and the +x side, from above.
        "angled" => (-0.97, 0.55),
        "top" => (-std::f32::consts::FRAC_PI_2, 1.5),
        "front" => (-std::f32::consts::FRAC_PI_2, 0.15),
        "side" => (0.0, 0.15),
        _ => return Err(invalid(format!("view: angled, top, front or side, not {view:?}"))),
    })
}

fn camera(view: &str, target: [f32; 3], radius: f32) -> Result<Camera, agent_proto::RpcError> {
    let (yaw, pitch) = view_angles(view)?;
    let mut cam = Camera { target, yaw, pitch, ..Camera::default() };
    cam.distance = (radius / (cam.fov_y * 0.5).sin() * 1.08).max(1.0);
    Ok(cam)
}

fn family_of_role(role: Role) -> Family {
    match role {
        Role::Floor => Family::Floor,
        Role::Wall => Family::Wall,
        Role::Ceiling => Family::Ceiling,
        Role::Trim => Family::Trim,
    }
}

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| f32::from(v) / 255.0)
}

fn vertex(position: [f32; 3], normal: [f32; 3], color: [f32; 3]) -> Vertex {
    Vertex { position, normal, color, center: position }
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn quad(out: &mut Vec<Vertex>, q: [[f32; 3]; 4], n: [f32; 3], c: [f32; 3]) {
    for k in [0, 1, 2, 0, 2, 3] {
        out.push(vertex(q[k], n, c));
    }
}

/// A plate cell's rectangle (4 vertices of the font part) on the face.
fn at(v: &[mesh::Vertex], s: f32, t: f32) -> [f32; 3] {
    let (a, b, d) = (v[0].position, v[1].position, v[3].position);
    let u = lerp(a, b, s);
    [
        u[0] + (d[0] - a[0]) * t,
        u[1] + (d[1] - a[1]) * t,
        u[2] + (d[2] - a[2]) * t,
    ]
}

/// The faces of the piece as triangles: tiles in their families' colour,
/// and each plate cell as the runs of the glyph's texels (`plates`) or
/// one flat quad in the face's colour.
fn body(piece: &Piece, atlas: &Image, plates: bool) -> Vec<Vertex> {
    let mut out = Vec::new();
    for part in &piece.mesh.parts {
        if let Paint::Tile(role) = &part.paint {
            let colour = rgb(family_of_role(*role).mid());
            for t in &part.triangles {
                for i in t {
                    let v = &part.vertices[usize::from(*i)];
                    out.push(vertex(v.position, v.normal, colour));
                }
            }
        }
    }
    let Some(font_part) = piece.mesh.part(&Paint::Font) else {
        return out;
    };
    for fp in &piece.mesh.plates {
        let role = piece.solid.shell[fp.face].role;
        let flat = rgb(family_of_role(role).mid());
        for cell in 0..COLUMNS * ROWS {
            let base = fp.first_vertex + cell * 4;
            let v = &font_part.vertices[base..base + 4];
            let n = v[0].normal;
            if !plates {
                quad(&mut out, [v[0].position, v[1].position, v[2].position, v[3].position], n, flat);
                continue;
            }
            // The atlas texels of the cell: u across, v down; v[0] is
            // its lower-left corner, so the top row is v[3]'s v.
            let x0 = (v[0].uv[0] * font::ATLAS_SIZE as f32).round() as u32;
            let y0 = (v[3].uv[1] * font::ATLAS_SIZE as f32).round() as u32;
            let step = 2;
            let blocks = font::CELL / step;
            for by in 0..blocks {
                let (t_hi, t_lo) = (
                    1.0 - by as f32 / blocks as f32,
                    1.0 - (by + 1) as f32 / blocks as f32,
                );
                let px = |bx: u32| atlas.pixel(x0 + bx * step, y0 + by * step);
                let mut bx = 0;
                while bx < blocks {
                    let c = px(bx);
                    let mut end = bx + 1;
                    while end < blocks && px(end) == c {
                        end += 1;
                    }
                    let (s0, s1) = (bx as f32 / blocks as f32, end as f32 / blocks as f32);
                    quad(
                        &mut out,
                        [at(v, s0, t_lo), at(v, s1, t_lo), at(v, s1, t_hi), at(v, s0, t_hi)],
                        n,
                        rgb([c[0], c[1], c[2]]),
                    );
                    bx = end;
                }
            }
        }
    }
    out
}

/// The shell's edges, dark, as lines.
fn edges(piece: &Piece) -> Vec<Vertex> {
    let mut out = Vec::new();
    for face in &piece.solid.shell {
        let n = face.ring.len();
        for i in 0..n {
            for p in [face.ring[i], face.ring[(i + 1) % n]] {
                out.push(vertex(p, [0.0, 0.0, 1.0], [0.04, 0.04, 0.05]));
            }
        }
    }
    out
}

/// The ground: a floor quad just under z = 0 and the grid on it, lines
/// every 64 units from the world's origin and a heavier one every 256,
/// across `half` world units either side of (`cx`, `cy`).
fn ground(cx: f32, cy: f32, half: f32) -> (Vec<Vertex>, Vec<Vertex>) {
    let mut floor = Vec::new();
    let z = -0.001;
    quad(
        &mut floor,
        [[cx - half, cy - half, z], [cx + half, cy - half, z], [cx + half, cy + half, z], [cx - half, cy + half, z]],
        [0.0, 0.0, 1.0],
        [0.17, 0.18, 0.21],
    );
    let mut grid = Vec::new();
    let step = GRID_UNITS / 100.0;
    let z = 0.003;
    let mut rule = |a: [f32; 3], b: [f32; 3], k: i32| {
        let heavy = (f64::from(k) * f64::from(GRID_UNITS)) % f64::from(HEAVY_UNITS) == 0.0;
        let colour = if k == 0 {
            [0.55, 0.30, 0.30]
        } else if heavy {
            [0.50, 0.52, 0.58]
        } else {
            [0.30, 0.32, 0.37]
        };
        for p in [a, b] {
            grid.push(vertex(p, [0.0, 0.0, 1.0], colour));
        }
    };
    for k in ((cx - half) / step).ceil() as i32..=((cx + half) / step).floor() as i32 {
        let x = k as f32 * step;
        rule([x, cy - half, z], [x, cy + half, z], k);
    }
    for k in ((cy - half) / step).ceil() as i32..=((cy + half) / step).floor() as i32 {
        let y = k as f32 * step;
        rule([cx - half, y, z], [cx + half, y, z], k);
    }
    (floor, grid)
}

/// A picture: RGBA rows top to bottom.
struct Shot {
    rgba: Vec<u8>,
    w: u32,
    h: u32,
}

impl Shot {
    fn corner(&self) -> [u8; 3] {
        [self.rgba[0], self.rgba[1], self.rgba[2]]
    }

    /// Pixels that are not `other`'s (at 2 levels of slack), or not the
    /// corner's colour when `other` is `None`.
    fn differing(&self, other: Option<&Shot>) -> usize {
        let corner = self.corner();
        self.rgba
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, px)| match other {
                Some(o) => (0..3).any(|k| px[k].abs_diff(o.rgba[i * 4 + k]) > 2),
                None => (0..3).any(|k| px[k].abs_diff(corner[k]) > 2),
            })
            .count()
    }
}

/// What one piece's picture is made of.
struct Drawn {
    vertices: usize,
    shot: Shot,
    bare: Shot,
}

fn shoot(
    gpu: &Gpu,
    vp: &mut Viewport,
    world: &[Vertex],
    lines: &[Vertex],
    cam: &Camera,
    size: [u32; 2],
) -> Result<Shot, agent_proto::RpcError> {
    vp.set_world_vertices(&gpu.device, world);
    vp.set_layer_overlay(&gpu.device, LayerId::Markers, &[], lines);
    vp.render(&gpu.device, &gpu.queue, cam, size);
    let texture = vp
        .color_texture()
        .ok_or_else(|| err(code::FAILED, "the viewport drew nothing"))?;
    let rgba = crate::headless::read_texture(&gpu.device, &gpu.queue, texture)
        .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
    Ok(Shot { rgba, w: texture.width(), h: texture.height() })
}

/// How a piece's plates are drawn.
struct Style<'a> {
    atlas: &'a Image,
    plates: bool,
}

/// The piece on its ground, and the same ground with no piece.
fn draw(
    gpu: &Gpu,
    vp: &mut Viewport,
    piece: &Piece,
    style: &Style,
    floor_half: f32,
    cam: &Camera,
    size: [u32; 2],
) -> Result<Drawn, agent_proto::RpcError> {
    let t = piece.target();
    let (floor, grid) = ground(t[0], t[1], floor_half);
    let bare = shoot(gpu, vp, &floor, &grid, cam, size)?;
    let mut world = floor;
    world.extend(body(piece, style.atlas, style.plates));
    let vertices = world.len();
    let mut lines = grid;
    lines.extend(edges(piece));
    let shot = shoot(gpu, vp, &world, &lines, cam, size)?;
    Ok(Drawn { vertices, shot, bare })
}

fn counts(piece: &Piece) -> (usize, usize) {
    (piece.mesh.vertices(), piece.mesh.triangles())
}

/// The plates as the answer lists them, their pixels in the picture
/// whose top left is at `origin` (the sheet's cell, or 0).
fn plates_json(piece: &Piece, cam: &Camera, size: [u32; 2], origin: [u32; 2]) -> Vec<Value> {
    let Some(font_part) = piece.mesh.part(&Paint::Font) else {
        return Vec::new();
    };
    let aspect = size[0] as f32 / size[1] as f32;
    let eye = cam.eye();
    piece
        .mesh
        .plates
        .iter()
        .map(|fp| {
            let v = &font_part.vertices[fp.first_vertex..fp.first_vertex + COLUMNS * ROWS * 4];
            let mut sum = [0.0_f32; 3];
            let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
            let to_px = |q: [f32; 3]| {
                cam.project(q, aspect).map(|[u, w]| {
                    [origin[0] as f32 + u * size[0] as f32, origin[1] as f32 + w * size[1] as f32]
                })
            };
            for x in v {
                for k in 0..3 {
                    sum[k] += x.position[k];
                }
                if let Some(px) = to_px(x.position) {
                    for k in 0..2 {
                        lo[k] = lo[k].min(px[k]);
                        hi[k] = hi[k].max(px[k]);
                    }
                }
            }
            let c = sum.map(|s| s / v.len() as f32);
            let n = v[0].normal;
            let visible = (0..3).map(|k| n[k] * (eye[k] - c[k])).sum::<f32>() > 0.0;
            let mut j = plate_json(fp, &piece.solid);
            j["visible"] = json!(visible);
            j["center"] = json!(to_px(c).map(|p| [p[0].round(), p[1].round()]));
            j["bounds"] = json!([lo[0].floor(), lo[1].floor(), hi[0].ceil(), hi[1].ceil()]);
            j
        })
        .collect()
}

fn plate_json(fp: &FacePlate, solid: &Solid) -> Value {
    json!({
        "face": fp.face,
        "role": format!("{:?}", solid.shell[fp.face].role).to_lowercase(),
        "width": fp.width,
        "height": fp.height,
        "h": fp.plate.h,
        "text": fp.plate.text(),
    })
}

fn def_json(def: &PieceDef) -> Value {
    json!({
        "name": def.name(),
        "shape": def.shape.name(),
        "size": def.size,
        "param": def.param,
    })
}

// ---- text on the sheet ----

fn put(buf: &mut [u8], bw: u32, bh: u32, x: u32, y: u32, c: [u8; 3]) {
    if x < bw && y < bh {
        let at = (y as usize * bw as usize + x as usize) * 4;
        buf[at..at + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    }
}

/// `text` in the dev font at (`x`, `y`), one 12 by 16 cell a character,
/// not past `max_x`; an underscore is a bar on the cell's foot.
fn blit_text(buf: &mut [u8], (bw, bh): (u32, u32), (x, y): (u32, u32), text: &str, max_x: u32, atlas: &Image) {
    let mut cx = x;
    for ch in text.chars() {
        if cx + ADVANCE > max_x {
            break;
        }
        let cell = u32::from(font::index_of(ch));
        let (ax, ay) = (
            (cell % font::CELLS_PER_ROW) * font::CELL,
            (cell / font::CELLS_PER_ROW) * font::CELL,
        );
        for ty in 0..TEXT_H {
            for tx in 0..ADVANCE {
                let px = atlas.pixel(ax + 2 + tx, ay + ty);
                put(buf, bw, bh, cx + tx, y + ty, [px[0], px[1], px[2]]);
            }
        }
        if ch == '_' {
            for tx in 0..ADVANCE {
                for ty in [13, 14] {
                    put(buf, bw, bh, cx + tx, y + ty, [245, 245, 235]);
                }
            }
        }
        cx += ADVANCE;
    }
}

// ---- the sheet's items ----

/// A cell of the sheet.
pub(super) struct Item {
    pub family: &'static str,
    pub def: PieceDef,
}

/// The sheet: every structural piece of the starter set (each structural
/// shape at its sizes), then three seeds of each natural shape.
pub(super) fn sheet_items() -> Vec<Item> {
    let mut items: Vec<Item> = forge_shapes::starter()
        .into_iter()
        .filter(|d| !d.shape.is_natural())
        .map(|def| Item { family: "structural", def })
        .collect();
    let natural: [(Shape, [u16; 3]); 5] = [
        (Shape::Boulder, [192, 192, 128]),
        (Shape::Slab, [256, 192, 48]),
        (Shape::Cliff, [256, 96, 256]),
        (Shape::TerrainWedge, [256, 256, 128]),
        (Shape::Spire, [96, 96, 320]),
    ];
    for (shape, size) in natural {
        for seed in 1..=3 {
            items.push(Item { family: "natural", def: PieceDef::dev(shape, size, seed) });
        }
    }
    items
}

/// The columns that make the cells largest on a `w` by `h` sheet whose
/// families each start a row: (columns, rows, cell width, cell height).
fn layout(groups: &[usize], w: u32, h: u32) -> (usize, usize, u32, u32) {
    let total: usize = groups.iter().sum();
    let mut best = (1, total, 0_u32, 0_u32);
    let mut best_score = 0;
    for cols in 1..=total.max(1) {
        let rows: usize = groups.iter().map(|g| g.div_ceil(cols)).sum();
        let (cw, ch) = (w / cols as u32, h / rows.max(1) as u32);
        let score = cw.min(ch * 4 / 3);
        if score > best_score {
            best_score = score;
            best = (cols, rows, cw, ch);
        }
    }
    best
}

fn pixels(p: &P, default: [u32; 2]) -> Result<[u32; 2], agent_proto::RpcError> {
    let Some(v) = p.get("pixels") else { return Ok(default) };
    let a: Option<Vec<u32>> = v.as_array().map(|a| {
        a.iter().filter_map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok())).collect()
    });
    match a.as_deref() {
        Some(&[w, h]) if (1..=8192).contains(&w) && (1..=8192).contains(&h) => Ok([w, h]),
        _ => Err(invalid(format!("pixels: [w, h] of 1..8192, not {v}"))),
    }
}

impl Blockout {
    fn gpu(&mut self) -> Result<&Gpu, agent_proto::RpcError> {
        match self.gpu.get_or_insert_with(|| Gpu::new().map_err(|e| format!("{e:#}"))) {
            Ok(g) => Ok(g),
            Err(e) => Err(err(code::NO_GPU, format!("no renderer here: {e}"))),
        }
    }

    fn render(&mut self, p: &P) -> Reply {
        let view = p.opt_str("view")?.unwrap_or("angled");
        view_angles(view)?;
        let plates = match p.get("plates") {
            None => true,
            Some(v) => v.as_bool().ok_or_else(|| invalid(format!("plates: true or false, not {v}")))?,
        };
        let def = def_of(p)?;
        let fit = p.opt_str("fit")?;
        if let Some(f) = fit {
            if !matches!(f, "sheet" | "shape") {
                return Err(invalid(format!("fit: sheet or shape, not {f:?}")));
            }
            if def.is_some() {
                return Err(invalid("fit: only the contact sheet has one"));
            }
        }
        let size = pixels(p, if def.is_some() { [960, 720] } else { [3200, 2000] })?;
        let items = sheet_items();
        let pieces = match &def {
            Some(d) => vec![piece(d)?],
            None => items.iter().map(|i| piece(&i.def)).collect::<Result<_, _>>()?,
        };
        if def.is_none() {
            let groups = [
                items.iter().filter(|i| i.family == "structural").count(),
                items.iter().filter(|i| i.family == "natural").count(),
            ];
            let (_, _, cw, ch) = layout(&groups, size[0], size[1]);
            if cw < 96 || ch < 96 + LABEL_H {
                return Err(invalid(format!(
                    "pixels: the sheet of {} cells needs cells of 96 by {}, {cw} by {ch} here; give a larger picture",
                    items.len(),
                    96 + LABEL_H
                )));
            }
        }
        let out = match p.opt_str("out")? {
            Some(o) => PathBuf::from(o),
            None => {
                let stem = match &def {
                    Some(d) => d.name(),
                    None => "sheet".to_owned(),
                };
                studio_proto::output_dir().join("blockout").join(format!("{stem}_{view}.png"))
            }
        };
        super::docs::outside_git(&out)?;
        let atlas = font::font();
        let style = Style { atlas: &atlas, plates };
        let gpu = self.gpu()?;
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
        }
        let mut vp = Viewport::new(&gpu.device);
        vp.set_layers(&Layers::default());
        let write = |rgba: &[u8], w: u32, h: u32| {
            crate::headless::write_png(&out, rgba, w, h)
                .map_err(|e| err(code::FAILED, format!("{e:#}")))
        };

        if let Some(def) = &def {
            let piece = &pieces[0];
            let floor = floor_half(piece.half_foot());
            let cam = camera(view, piece.target(), radius_of(piece, floor))?;
            let d = draw(gpu, &mut vp, piece, &style, floor, &cam, size)?;
            write(&d.shot.rgba, d.shot.w, d.shot.h)?;
            let (_, triangles) = counts(piece);
            let mut j = def_json(def);
            j["path"] = json!(out.display().to_string());
            j["width"] = json!(d.shot.w);
            j["height"] = json!(d.shot.h);
            j["view"] = json!(view);
            j["covered"] = json!(d.shot.differing(None));
            j["piece_pixels"] = json!(d.shot.differing(Some(&d.bare)));
            j["vertices"] = json!(d.vertices);
            j["triangles"] = json!(triangles);
            j["plates"] = json!(plates_json(piece, &cam, size, [0, 0]));
            j["grid_units"] = json!(GRID_UNITS as u32);
            return Ok(j);
        }

        // The sheet: one cell a piece, each its own picture of the same
        // camera angle and distance, put side by side with a label under.
        let fit = fit.unwrap_or("sheet");
        let floors: Vec<f32> = pieces.iter().map(|p| floor_half(p.half_foot())).collect();
        // The floor and the camera's radius a cell shares: the whole
        // sheet's, or its shape's.
        let reach = |i: usize| -> (f32, f32) {
            let same = |j: usize| fit == "sheet" || items[j].def.shape == items[i].def.shape;
            let floor = (0..pieces.len()).filter(|&j| same(j)).map(|j| floors[j]).fold(0.0, f32::max);
            let radius = (0..pieces.len())
                .filter(|&j| same(j))
                .map(|j| radius_of(&pieces[j], floor))
                .fold(0.0, f32::max);
            (floor, radius)
        };
        let groups = [
            items.iter().filter(|i| i.family == "structural").count(),
            items.iter().filter(|i| i.family == "natural").count(),
        ];
        let (cols, _, cw, ch) = layout(&groups, size[0], size[1]);
        let cell_px = [cw - 4, ch - LABEL_H - 4];
        let mut sheet = vec![0u8; size[0] as usize * size[1] as usize * 4];
        for px in sheet.chunks_exact_mut(4) {
            px.copy_from_slice(&[20, 22, 28, 255]);
        }
        let mut cells = Vec::new();
        let (mut row, mut col, mut last) = (0usize, 0usize, items[0].family);
        for (i, (item, piece)) in items.iter().zip(&pieces).enumerate() {
            if item.family != last {
                // A family starts its own row, unless the last one just
                // filled its row.
                if col != 0 {
                    (row, col) = (row + 1, 0);
                }
                last = item.family;
            }
            let (floor, radius) = reach(i);
            let cam = camera(view, piece.target(), radius)?;
            let d = draw(gpu, &mut vp, piece, &style, floor, &cam, cell_px)?;
            let (x0, y0) = (col as u32 * cw, row as u32 * ch);
            for y in 0..d.shot.h {
                let from = y as usize * d.shot.w as usize * 4;
                let to = ((y0 + 2 + y) as usize * size[0] as usize + (x0 + 2) as usize) * 4;
                let n = d.shot.w as usize * 4;
                sheet[to..to + n].copy_from_slice(&d.shot.rgba[from..from + n]);
            }
            let name = item.def.name();
            let text_at = y0 + 2 + d.shot.h + 2;
            let short = name.rsplit_once('_').map_or(name.as_str(), |(head, _)| head);
            blit_text(&mut sheet, (size[0], size[1]), (x0 + 4, text_at), short, x0 + cw - 2, &atlas);
            let digest = item.def.digest12();
            blit_text(
                &mut sheet,
                (size[0], size[1]),
                (x0 + 4, text_at + TEXT_H),
                &format!("{} {digest}", item.family),
                x0 + cw - 2,
                &atlas,
            );
            let (vertices, triangles) = counts(piece);
            let t = piece.target();
            let aspect = cell_px[0] as f32 / cell_px[1] as f32;
            let center = cam.project(t, aspect).map(|[u, v]| {
                [(x0 + 2) as f32 + (u * cell_px[0] as f32).round(), (y0 + 2) as f32 + (v * cell_px[1] as f32).round()]
            });
            let mut j = def_json(&item.def);
            j["family"] = json!(item.family);
            j["cell"] = json!([col, row]);
            j["rect"] = json!([x0 + 2, y0 + 2, d.shot.w, d.shot.h]);
            j["center"] = json!(center);
            j["covered"] = json!(d.shot.differing(None));
            j["piece_pixels"] = json!(d.shot.differing(Some(&d.bare)));
            j["vertices"] = json!(vertices);
            j["triangles"] = json!(triangles);
            j["plates"] = json!(plates_json(piece, &cam, cell_px, [x0 + 2, y0 + 2]));
            cells.push(j);
            col += 1;
            if col == cols {
                (row, col) = (row + 1, 0);
            }
        }
        write(&sheet, size[0], size[1])?;
        // What is not the sheet's own background.
        let covered = sheet.chunks_exact(4).filter(|px| px[..3] != [20, 22, 28]).count();
        let piece_pixels: u64 = cells.iter().map(|c| c["piece_pixels"].as_u64().unwrap_or(0)).sum();
        Ok(json!({
            "path": out.display().to_string(),
            "width": size[0],
            "height": size[1],
            "view": view,
            "fit": fit,
            "covered": covered,
            "piece_pixels": piece_pixels,
            "vertices": cells.iter().map(|c| c["vertices"].as_u64().unwrap_or(0)).sum::<u64>(),
            "triangles": cells.iter().map(|c| c["triangles"].as_u64().unwrap_or(0)).sum::<u64>(),
            "plates": cells.iter().flat_map(|c| c["plates"].as_array().cloned().unwrap_or_default()).collect::<Vec<_>>(),
            "grid_units": GRID_UNITS as u32,
            "cells": cells,
        }))
    }
}

// ---- describe ----

fn paint_name(paint: &Paint) -> String {
    match paint {
        Paint::Tile(role) => format!("tile:{}", format!("{role:?}").to_lowercase()),
        Paint::Font => "font".to_owned(),
        Paint::Map(path) => format!("map:{path}"),
    }
}

fn describe(p: &P) -> Reply {
    let Some(def) = def_of(p)? else {
        return Err(invalid(format!(
            "name: give a canonical name, or a shape ({}) with its size",
            shape_names()
        )));
    };
    let piece = piece(&def)?;
    let (vertices, triangles) = counts(&piece);
    let plates: Vec<Value> = piece
        .mesh
        .plates
        .iter()
        .map(|fp| {
            let mut j = plate_json(fp, &piece.solid);
            let rows = label::rows(&mesh::label_name(def.shape), fp.width, fp.height);
            j["metres"] = json!([rows[1].clone(), rows[2].clone()]);
            j
        })
        .collect();
    let parts: Vec<Value> = piece
        .mesh
        .parts
        .iter()
        .map(|part| {
            json!({
                "paint": paint_name(&part.paint),
                "vertices": part.vertices.len(),
                "triangles": part.triangles.len(),
            })
        })
        .collect();
    let mut j = def_json(&def);
    j["tag_path"] = json!(def.tag_path());
    j["digest"] = json!(def.digest12());
    j["family"] = json!(family_of(def.shape));
    j["label"] = json!(mesh::label_name(def.shape));
    j["faces"] = json!(piece.solid.shell.len());
    j["hulls"] = json!(piece.solid.hulls.len());
    j["hull_points"] = json!(piece.solid.hulls.iter().map(|h| h.points.len()).sum::<usize>());
    j["volume"] = json!(piece.solid.volume());
    j["bounds"] = json!({"min": piece.min, "max": piece.max});
    j["vertices"] = json!(vertices);
    j["triangles"] = json!(triangles);
    j["parts"] = json!(parts);
    j["unplated"] = json!(piece.solid.shell.len() - plates.len());
    j["plates"] = json!(plates);
    Ok(j)
}
