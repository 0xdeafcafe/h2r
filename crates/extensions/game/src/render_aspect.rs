//! Display aspect on captured GPU constants and the retail callbacks
//! that own on-screen UI packets. All guest coordinates remain 640 × 480.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use d3d::backend::{PixelRect, UiFit};
use engine::{GuestMemory, points};
use hooks::{Builder, Feature, Flow, Manifest};
use points::render_aspect::{
    ANCHOR, ANCHOR_READY, DOC, FLARE_COMPOSITE_DONE, FLARE_COMPOSITE_LOOP, FLARE_QUERY,
    FLARE_QUERY_END, FLARE_REFLECTION, FLARE_REFLECTION_END, GROUPS, MENU_FILL,
    MENU_FILL_END, VOLUME_DEPTH, VOLUME_DEPTH_END, VOLUME_SPRITE, VOLUME_SPRITE_END,
};
use points::scene_effects::{EFFECT_QUAD_READY, HUD_QUAD_READY};
use points::ui_render::{ITEM_CALL, RECT_SUBMIT_RETURN, SCREEN_QUAD_READY};

type Size = Arc<dyn Fn() -> (u32, u32) + Send + Sync>;
type Position = Arc<dyn Fn() -> u32 + Send + Sync>;

/// One installed rendering device. The drain pins the layout to each draw
/// before its packet moves to the GPU thread or becomes a native UI pass.
pub trait UiLayoutSink: Send + Sync {
    fn set_projection_factor(&self, factor: f32);
    fn set_camera_projection_factor(&self, _: f32) { self.set_projection_factor(1.0); }
    fn set_fit(&self, guest: &mut runtime::Guest, fit: Option<UiFit>);
}

impl<B: d3d::Backend + Send> UiLayoutSink for Mutex<d3d::Device<B>> {
    fn set_projection_factor(&self, factor: f32) {
        self.lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_projection_factor(factor);
    }
    fn set_camera_projection_factor(&self, factor: f32) {
        self.lock().unwrap_or_else(|e| e.into_inner()).set_camera_projection_factor(factor);
    }
    fn set_fit(&self, guest: &mut runtime::Guest, fit: Option<UiFit>) {
        if let Err(err) = self
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_ui_fit(&mut d3d::runtime_glue::GuestAdapter::new(guest), fit)
        {
            eprintln!("render-aspect: draining the UI draw boundary: {err}");
        }
    }
}

struct RenderAspect {
    size: Size,
    single_copy: bool,
    position: Position,
    sink: Arc<dyn UiLayoutSink>,
}

pub fn install(
    registry: &mut hooks::Registry,
    size: Size,
    single_copy: bool,
    position: Position,
    sink: Arc<dyn UiLayoutSink>,
) -> anyhow::Result<()> {
    registry
        .add(&RenderAspect {
            size,
            single_copy,
            position,
            sink,
        })
        .map_err(|e| anyhow::anyhow!("render aspect: {e}"))?;
    Ok(())
}

fn pixel_aspect((w, h): (u32, u32)) -> Option<f32> {
    (w != 0 && h != 0 && u64::from(w) * 3 != u64::from(h) * 4)
        .then(|| (w as f32 / 640.0) / (h as f32 / 480.0))
}

// The width term selected by the retail projection at 0x2fc45..0x2fc90.
// Reading it here allows machines already in dashboard widescreen mode to
// use the display aspect exactly once. The display lens changes only X.
fn projection_factor(mem: &impl GuestMemory, output: (u32, u32)) -> f32 {
    let Some(pixel) = pixel_aspect(output) else {
        return 1.0;
    };
    let suppressed = mem
        .u32(0x004e_6948)
        .ok()
        .filter(|&p| p != 0)
        .is_some_and(|p| {
            p.checked_add(8).is_some()
                && mem.u8(p + 1).is_ok_and(|b| b != 0)
                && mem.u16(p + 2).is_ok_and(|v| v != u16::MAX)
                && mem.u32(p + 8).ok() == Some(3)
        });
    let retail = if !suppressed && mem.u8(0x0048_5ac2).is_ok_and(|v| v != 0) {
        mem.f32(0x0045_dfdc)
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(1.0)
    } else {
        1.0
    };
    retail / pixel
}

// 0x2fd90 reserves 0x74 stack bytes. After its call to 0x2fb70,
// +0x54 holds the X/Y biases, X/Y scales and four tangent bounds.
// All three consumers (matrix, CPU pixel scale and frustum) use these
// terms. Keep the vertical lens and the centre of an off-axis window.
fn fit_view_projection(mem: &mut impl GuestMemory, esp: u32, factor: f32) -> Option<()> {
    if factor == 1.0 || !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    let caller = mem.u32(esp.checked_add(0x74)?).ok()?;
    match caller {
        // Active and saved camera of render_view_scene and render_alternate_view.
        0x0002_badc | 0x0002_bb21 | 0x0002_c7cc | 0x0002_c7ff => {}
        // Popping the last nested view rebuilds the displayed camera.
        0x0001_3cc0 if mem.u32(0x0046_701c).ok() == Some(0) => {}
        _ => return None,
    }
    let at = esp.checked_add(0x54)?;
    let mut words = [0u8; 24];
    mem.read(at, &mut words).ok()?;
    let get = |i: usize| f32::from_le_bytes(words[i..i + 4].try_into().unwrap());
    let (bias, scale, left, right) = (get(0), get(8), get(16), get(20));
    let centre = (left + right) * 0.5;
    let half = (right - left) * 0.5 / factor;
    let terms = [bias * factor, scale * factor, centre - half, centre + half];
    if !terms.iter().all(|v| v.is_finite()) || terms[1] <= 0.0 || terms[2] >= terms[3] {
        return None;
    }
    for (i, value) in [0, 8, 16, 20].into_iter().zip(terms) {
        words[i..i + 4].copy_from_slice(&value.to_le_bytes());
    }
    mem.write(at, &words).ok()
}

fn viewport(mem: &impl GuestMemory, address: u32) -> Option<PixelRect> {
    let mut bytes = [0; 8];
    mem.read(address, &mut bytes).ok()?;
    let [top, left, bottom, right] = std::array::from_fn(|i| {
        u32::from(u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]))
    });
    (top < bottom && bottom <= 480 && left < right && right <= 640).then(|| PixelRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

// These callers fade the whole view: the two fades in UI_RENDER_LAYERS,
// the start-screen transition, and the modal dim in the window stack.
// MENU_FILL rebases their bounds to (0, 0). The rectangle shader then
// projects those local pixels over the active raster rectangle.
const LAYERS_DIM_RETURN: u32 = 0x0014_7b54;
const LAYERS_FADE_RETURN: u32 = 0x0014_7c66;
const START_SCREEN_FADE_RETURN: u32 = 0x0023_3782;
const MODAL_FILL_RETURN: u32 = 0x0023_511d;
const RASTER_RECT: u32 = 0x0048_5648;

fn fill_bounds(mem: &impl GuestMemory, esp: u32, bounds: u32) -> u32 {
    let view_fill = matches!(
        mem.u32(esp).ok(),
        Some(LAYERS_DIM_RETURN | LAYERS_FADE_RETURN | START_SCREEN_FADE_RETURN | MODAL_FILL_RETURN)
    );
    if view_fill && viewport(mem, RASTER_RECT).is_some() {
        RASTER_RECT
    } else {
        bounds
    }
}

fn local_raster(mem: &impl GuestMemory) -> Option<PixelRect> {
    viewport(mem, RASTER_RECT).map(|rect| PixelRect { x: 0, y: 0, ..rect })
}

// 0x48b00 maps clip X to guest pixel centres. Query edges at the
// call to Begin (0x2e82b) have already been rounded, and their *original*
// area is the game's denominator. Translate this footprint rigidly. The
// reflected billboard has no readback and can take the entire lens map.
fn projected_effect_fit(
    mem: &impl GuestMemory,
    output: (u32, u32),
    query_stack: Option<u32>,
) -> Option<UiFit> {
    let factor = projection_factor(mem, output);
    if factor == 1.0 {
        return None;
    }
    let bounds = viewport(mem, 0x0048_5648)?;
    let centre = bounds.x as f32 + bounds.width as f32 * 0.5;
    if let Some(esp) = query_stack {
        let left = mem.f32(esp.checked_add(0x14)?).ok()?;
        let right = mem.f32(esp.checked_add(0x18)?).ok()?;
        let x = (left + right) * 0.5;
        if !x.is_finite() || right <= left {
            return None;
        }
        let delta = (x - (centre - 0.5)) * (factor - 1.0);
        Some(UiFit::projected_sprite(1.0, delta))
    } else {
        Some(UiFit::projected_sprite(factor, centre * (1.0 - factor)))
    }
}

#[derive(Clone, Copy)]
struct Canvas {
    output: (u32, u32),
    rect: PixelRect,
    hud: bool,
    pinned: bool,
    projection_factor: f32,
    projected_points_fitted: bool,
}

impl Canvas {
    fn centre(self) -> [f32; 2] {
        [
            self.rect.x as f32 + self.rect.width as f32 * 0.5,
            self.rect.y as f32 + self.rect.height as f32 * 0.5,
        ]
    }

    fn fit(self, anchor: Option<(u32, [f32; 2])>) -> Option<UiFit> {
        let mut pivot = self.centre();
        if let Some((id, point)) = anchor.filter(|_| self.hud) {
            if id >= 4 {
                // Centre/crosshair and tracked world points keep their
                // projected location; only their art scales around it.
                if point.iter().all(|v| v.is_finite()) {
                    pivot = point;
                }
            } else if self.pinned {
                let right = id == 0 || id == 3;
                let bottom = id >= 2;
                pivot = [
                    (self.rect.x + if right { self.rect.width } else { 0 }) as f32,
                    (self.rect.y + if bottom { self.rect.height } else { 0 }) as f32,
                ];
            }
        }
        let menu = UiFit::about(self.output, self.centre());
        if !self.hud {
            return menu;
        }
        // The Xbox widescreen HUD is already laid out for anamorphic pixels.
        // Scale the artwork only by the difference between that display and
        // ours. Its *corner placement* still obeys the user's 4:3 safe area.
        let mut fit =
            UiFit::from_pixel_ratio(self.projection_factor, pivot).unwrap_or(UiFit::IDENTITY);
        if let Some((id, _)) = anchor
            && id < 4
            && !self.pinned
        {
            let [safe_x, safe_y, _, _] = menu.unwrap_or(UiFit::IDENTITY).transform;
            let edge_x = if id == 0 || id == 3 { 0.5 } else { -0.5 };
            let edge_y = if id >= 2 { 0.5 } else { -0.5 };
            fit.transform[2] += edge_x * self.rect.width as f32 * (safe_x - fit.transform[0]);
            fit.transform[3] += edge_y * self.rect.height as f32 * (safe_y - fit.transform[1]);
        }
        if anchor.is_some_and(|(id, _)| id >= 4) && self.hud && !self.projected_points_fitted {
            // The guest's tracked point is in the retail lens. Move its
            // centre with the world, then fit the artwork about that point.
            fit.transform[2] += (pivot[0] - self.centre()[0]) * (self.projection_factor - 1.0);
        }
        (fit != UiFit::IDENTITY).then_some(fit)
    }
}

/// The HUD's bottom-left corner group (anchor 2: the motion tracker and
/// the shield meter).
const BOTTOM_LEFT: u32 = 2;
/// The HUD's centre anchor (the crosshair: anchor 4 and up keep their
/// projected point, `Canvas::fit`).
const CENTRE: u32 = 4;

/// The first view's bottom-left HUD fit ([`UiFit::transform`]), as its
/// last HUD draw set it; identity while the output is 4:3.
static HUD_BOTTOM_LEFT: Mutex<[f32; 4]> = Mutex::new(UiFit::IDENTITY.transform);

/// The HUD's centre where the game's reticle is (`rects::RETICLE`'s
/// centre) on the fullscreen frame, guest pixels.
pub const RETICLE_CENTRE: [f32; 2] = [320.0, 240.0];

/// The first view's centre point and the fit of the HUD art about it, as
/// its last HUD draw set them: the place of the game's reticle.
static HUD_CENTRE: Mutex<([f32; 2], [f32; 4])> =
    Mutex::new((RETICLE_CENTRE, UiFit::IDENTITY.transform));

/// Keeps the bottom-left fit and the centre of the view at the frame's
/// top left (the first player's in a split screen; S: that the first
/// local player is drawn there), for host marks laid over the motion
/// tracker and the reticle.
fn publish_hud_corner(active: Option<Canvas>) {
    let (corner, centre) = match active {
        None => (UiFit::IDENTITY, (RETICLE_CENTRE, UiFit::IDENTITY)),
        Some(c) if c.rect.x == 0 && c.rect.y == 0 => (
            c.fit(Some((BOTTOM_LEFT, [0.0; 2])))
                .unwrap_or(UiFit::IDENTITY),
            (
                c.centre(),
                c.fit(Some((CENTRE, c.centre()))).unwrap_or(UiFit::IDENTITY),
            ),
        ),
        Some(_) => return,
    };
    *HUD_BOTTOM_LEFT.lock().unwrap_or_else(|e| e.into_inner()) = corner.transform;
    *HUD_CENTRE.lock().unwrap_or_else(|e| e.into_inner()) = (centre.0, centre.1.transform);
}

/// Where the game's reticle is in the first view: its centre in guest
/// pixels, and the fit ([`UiFit::transform`]) of HUD art about that
/// centre (identity at 4:3).
pub fn hud_centre_fit() -> ([f32; 2], [f32; 4]) {
    *HUD_CENTRE.lock().unwrap_or_else(|e| e.into_inner())
}

/// How the host moves the first view's bottom-left HUD group: guest pixel
/// `p` is drawn at `p * [t0, t1] + [t2, t3]` (docs/game/superbounce.md,
/// "The bounce view").
pub fn hud_bottom_left_fit() -> [f32; 4] {
    *HUD_BOTTOM_LEFT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Marks the group of one menu quad that is background art (a panel, a
/// scrolling strip, a full-screen bitmap): it keeps the full view, as on a
/// widescreen Xbox, so the menu covers the display while its widgets keep
/// the uniform fit.
const FILL: u32 = HUD_QUAD_READY.base;

/// Guest pixels a background quad's edge may stop short of the view's edge
/// (pixel-centred corners sit half a pixel in).
const FILL_EDGE: f32 = 1.0;

/// Four per-corner values: window pixels or normalised texture coordinates.
type Corners = [[f32; 2]; 4];

/// One menu quad at its draw site: the window-pixel corners and, at the UI
/// rectangle site, the normalised texture coordinates at the same corners.
/// The shader and screen quad sites give none: an emblem draws a unit
/// quad (0..1) through the shader site, placed by its transform, and its
/// texture coordinates there span about 2e-6, which the bitmap-length test
/// would take for art far longer than the view (Choose Profile, 2560x1080).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Quad {
    xy: Corners,
    uv: Option<Corners>,
}

fn extent(corners: &Corners, axis: usize) -> (f32, f32) {
    corners.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), c| {
        (lo.min(c[axis]), hi.max(c[axis]))
    })
}

/// Whether `quad` is background art along every axis `fit` shrinks. It
/// reaches both edges of `rect` on that axis, or it reaches one edge and
/// its whole bitmap, at the scale it is drawn, is at least as long as the
/// view: a scrolling strip drawn as two pieces that meet where its
/// texture wraps, or art that runs off the view. A widget's bitmap (a
/// panel, the Friends tab, a slide-in) is shorter than the view.
fn is_background(rect: PixelRect, fit: UiFit, quad: &Quad) -> bool {
    let [sx, sy, _, _] = fit.transform;
    let along = |axis: usize, start: u32, length: u32| {
        let (lo, hi) = extent(&quad.xy, axis);
        let first = lo <= start as f32 + FILL_EDGE;
        let last = hi >= (start + length) as f32 - FILL_EDGE;
        if first && last {
            return true;
        }
        (first || last)
            && quad.uv.is_some_and(|uv| {
                let (low, high) = extent(&uv, axis);
                let span = high - low;
                span > 0.0 && (hi - lo) / span >= length as f32 - FILL_EDGE
            })
    };
    (sx < 1.0 || sy < 1.0)
        && (sx >= 1.0 || along(0, rect.x, rect.width))
        && (sy >= 1.0 || along(1, rect.y, rect.height))
}

/// Four vertices from `first`, `stride` bytes apart: the corner at `at`
/// bytes into each.
fn read_corners(mem: &impl GuestMemory, first: u32, stride: u32, at: u32) -> Option<Corners> {
    let mut out = [[0.0; 2]; 4];
    for (i, corner) in out.iter_mut().enumerate() {
        let vertex = first.checked_add(stride.checked_mul(i as u32)?)?;
        let f = |offset: u32| mem.f32(vertex.checked_add(offset)?).ok();
        let (x, y) = (f(at)?, f(at.checked_add(4)?)?);
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        *corner = [x, y];
    }
    Some(out)
}

/// The shader quad at HUD_QUAD_READY: four 0x1c-byte vertices at
/// EBP - 0x70. The float4 position comes first, then the UV for register 3
/// (render_bridge::native_hud).
fn shader_quad(mem: &impl GuestMemory, ebp: u32) -> Option<Quad> {
    let first = ebp.checked_sub(0x70)?;
    Some(Quad {
        xy: read_corners(mem, first, 0x1c, 0)?,
        uv: None,
    })
}

/// The screen quad at UI_SCREEN_QUAD_READY: the pointer at ESP + 0x18
/// gives four 0x14-byte vertices in (x, y, u, v, colour) order
/// (render_bridge::native_hud::ui).
fn screen_quad(mem: &impl GuestMemory, esp: u32) -> Option<Quad> {
    let first = mem.u32(esp.checked_add(0x18)?).ok()?;
    Some(Quad {
        xy: read_corners(mem, first, 0x14, 0)?,
        uv: None,
    })
}

/// The UI's inline rectangle (`0x00052040`: bitmap widgets, the menus'
/// background bitmaps and scrolling strips) at the dynamic upload helper's
/// `EFFECT_QUAD_READY` (`0x0004b252`): three saved registers, the return
/// to `RECT_SUBMIT_RETURN`, then (mode, primitive, count, builder,
/// vertices); four 20-byte vertices, (x, y) then (u, v) at `+8`. V: at
/// 2560x1080 with (u, v) read at `+12` (v, 0..1 on every strip), only the
/// strip pieces that reached the right edge filled.
fn rect_quad(mem: &impl GuestMemory, esp: u32) -> Option<Quad> {
    let word = |offset: u32| mem.u32(esp.checked_add(offset)?).ok();
    if word(12)? != RECT_SUBMIT_RETURN || word(24)? != 4 {
        return None;
    }
    let first = word(32).filter(|&p| p != 0)?;
    Some(Quad {
        xy: read_corners(mem, first, 20, 0)?,
        uv: read_corners(mem, first, 20, 8),
    })
}

#[derive(Default)]
struct Layout {
    active: Option<Canvas>,
    anchor: Option<(u32, u32)>,
    groups: Vec<(u32, Option<UiFit>)>,
}

impl Layout {
    fn end_group(&mut self, owner: u32) -> bool {
        let Some(index) = self.groups.iter().rposition(|&(end, _)| end == owner) else {
            return false;
        };
        self.groups.truncate(index);
        true
    }

    /// A menu quad is about to draw. Returns the fit to set when it differs
    /// from the previous quad's: the full view for a quad that spans the
    /// fitted axis, else the enclosing fit.
    fn place_quad(&mut self, raster: Option<PixelRect>, quad: Option<Quad>) -> Option<Option<UiFit>> {
        let canvas = self.active.filter(|canvas| !canvas.hud)?;
        let rect = raster.unwrap_or(canvas.rect);
        let was_filled = self.groups.last().is_some_and(|&(id, _)| id == FILL);
        if was_filled {
            self.groups.pop();
        }
        let fill = match (self.current(), quad) {
            (Some(fit), Some(quad)) => is_background(rect, fit, &quad),
            _ => false,
        };
        if fill {
            self.groups.push((FILL, None));
        }
        (was_filled != fill).then(|| self.current())
    }

    /// Another widget starts: a background quad's full view ends.
    fn end_fill(&mut self) -> Option<Option<UiFit>> {
        if !self.groups.last().is_some_and(|&(id, _)| id == FILL) {
            return None;
        }
        self.groups.pop();
        Some(self.current())
    }

    fn current(&self) -> Option<UiFit> {
        self.groups
            .last()
            .map(|g| g.1)
            .unwrap_or_else(|| self.active.and_then(|c| c.fit(None)))
    }
}

impl Feature for RenderAspect {
    fn manifest(&self) -> Manifest {
        Manifest::new("render-aspect", "Render projection and on-screen UI layout").doc(DOC)
    }

    fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
        let view_lens = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let single_copy = self.single_copy;
        let projected = Arc::new(AtomicBool::new(false));
        for point in [&points::RENDER_GAME_VIEW, &points::RENDER_ALTERNATE_VIEW] {
            let (size, sink, lens) = (self.size.clone(), self.sink.clone(), view_lens.clone());
            let projected = projected.clone();
            b.raw()
                .observe_before(point, "select the GPU projection", move |o| {
                    let factor = projection_factor(&*o.guest(), size());
                    projected.store(false, Ordering::Relaxed);
                    lens.store(factor.to_bits(), Ordering::Relaxed);
                    sink.set_projection_factor(factor);
                    #[cfg(feature = "desktop-runtime")]
                    {
                        use render_bridge::scene_step::{VisibilityLens, set_visibility_lens};
                        set_visibility_lens(if !single_copy && factor < 1.0 {
                            VisibilityLens::Display(factor)
                        } else {
                            VisibilityLens::Retail
                        });
                    }
                })
                // Reads this view's guest memory and tells the sink, as the
                // UI-fit hooks below do (all context-only).
                .context_only()
                .optional();
        }

        if single_copy {
            let projected = projected.clone();
            let sink = self.sink.clone();
            b.raw()
                .before(&points::render_aspect::VIEW_PROJECTION_TERMS, "fit the displayed camera projection", move |c| {
                    let factor = f32::from_bits(view_lens.load(Ordering::Relaxed));
                    if fit_view_projection(&mut *c.ctx.memory(), c.cpu.esp, factor).is_some() {
                        projected.store(true, Ordering::Relaxed);
                        sink.set_camera_projection_factor(factor);
                        #[cfg(feature = "desktop-runtime")]
                        render_bridge::scene_step::set_visibility_lens(
                            render_bridge::scene_step::VisibilityLens::Guest(factor),
                        );
                    }
                    Ok(Flow::Continue)
                })
                .context_only()
                .optional();
        }

        let layout = Arc::new(Mutex::new(Layout::default()));
        for (point, hud) in [
            (&points::UI_HUD_DRAW, true),
            (&points::UI_RENDER_LAYERS, false),
        ] {
            let (layout, size, position, sink) = (
                layout.clone(),
                self.size.clone(),
                self.position.clone(),
                self.sink.clone(),
            );
            let projected = projected.clone();
            b.raw()
                .before(point, "fit this view's UI", move |c| {
                    let output = size();
                    let mut layout = layout.lock().unwrap_or_else(|e| e.into_inner());
                    let was_active = layout.active.is_some();
                    *layout = Layout::default();
                    let has_returns = c.ctx.runtime().read_dispatch(|d| {
                        if hud {
                            d.has_generated(points::ui_render::HUD_RETURN.base)
                                && d.has_generated(points::ui_render::HUD_HIDDEN_RETURN.base)
                        } else {
                            d.has_generated(points::ui_render::LAYERS_RETURN.base)
                        }
                    });
                    if has_returns && pixel_aspect(output).is_some() {
                        let mem = c.ctx.memory();
                        // UI layers receive the bounds of the specific player
                        // or fullscreen fallback view as their second argument.
                        let bounds = if hud {
                            None
                        } else {
                            c.cpu
                                .esp
                                .checked_add(8)
                                .and_then(|at| GuestMemory::u32(&*mem, at).ok())
                        };
                        let rect = bounds
                            .and_then(|addr| viewport(&*mem, addr))
                            .or_else(|| viewport(&*mem, 0x004b_9dd0))
                            .unwrap_or(PixelRect {
                                x: 0,
                                y: 0,
                                width: 640,
                                height: 480,
                            });
                        layout.active = Some(Canvas {
                            output,
                            projection_factor: projection_factor(&*mem, output),
                            projected_points_fitted: projected.load(Ordering::Relaxed),
                            rect,
                            hud,
                            pinned: hud && position() == crate::setting_defs::HUD_PINNED,
                        });
                    }
                    if hud {
                        publish_hud_corner(layout.active);
                    }
                    if was_active || layout.active.is_some() {
                        sink.set_fit(&mut c.ctx.memory(), layout.current());
                    }
                    Ok(Flow::Continue)
                })
                .context_only()
                .after_feature("native-hud")
                .optional();
        }
        for point in [
            &points::ui_render::HUD_RETURN,
            &points::ui_render::HUD_HIDDEN_RETURN,
            &points::ui_render::LAYERS_RETURN,
        ] {
            let (layout, sink) = (layout.clone(), self.sink.clone());
            b.raw()
                .before(point, "finish this view's UI", move |c| {
                    let mut layout = layout.lock().unwrap_or_else(|e| e.into_inner());
                    if layout.active.is_some() {
                        sink.set_fit(&mut c.ctx.memory(), None);
                        *layout = Layout::default();
                    }
                    Ok(Flow::Continue)
                })
                .context_only()
                .after_feature("native-hud")
                .optional();
        }
        for (begin, end, query) in [
            (&FLARE_QUERY, &FLARE_QUERY_END, true),
            (&FLARE_REFLECTION, &FLARE_REFLECTION_END, false),
            (&VOLUME_SPRITE, &VOLUME_SPRITE_END, false),
            (&VOLUME_DEPTH, &VOLUME_DEPTH_END, false),
            (&FLARE_COMPOSITE_LOOP, &FLARE_COMPOSITE_DONE, false),
        ] {
            let (layout, size, sink) = (layout.clone(), self.size.clone(), self.sink.clone());
            let projected = projected.clone();
            b.raw()
                .before(
                    begin,
                    "align the CPU-projected effect with the display lens",
                    move |c| {
                        if projected.load(Ordering::Relaxed) || !c.ctx.runtime().read_dispatch(|d| d.has_generated(end.base)) {
                            return Ok(Flow::Continue);
                        }
                        let mut l = layout.lock().unwrap_or_else(|e| e.into_inner());
                        let mut mem = c.ctx.memory();
                        if let Some(fit) = projected_effect_fit(&*mem, size(), query.then_some(c.cpu.esp)) {
                            let fit = if begin.base == VOLUME_DEPTH.base {
                                fit.with_screen_input(1)
                            } else {
                                fit
                            };
                            l.groups.push((end.base, Some(fit)));
                            sink.set_fit(&mut mem, Some(fit));
                        }
                        Ok(Flow::Continue)
                    },
                )
                .context_only()
                .optional();
        }
        let on_anchor = layout.clone();
        b.raw()
            .before(&ANCHOR, "identify the HUD element's anchor", move |c| {
                let mut l = on_anchor.lock().unwrap_or_else(|e| e.into_inner());
                if l.active.is_some_and(|canvas| canvas.hud) {
                    let ret = GuestMemory::u32(&*c.ctx.memory(), c.cpu.esp).ok();
                    l.anchor = GROUPS
                        .iter()
                        .find(|(caller, _)| ret == Some(*caller))
                        .map(|(_, end)| (end.base, c.cpu.eax));
                }
                Ok(Flow::Continue)
            })
            .context_only()
            .optional();
        let (on_ready, sink) = (layout.clone(), self.sink.clone());
        b.raw()
            .before(&ANCHOR_READY, "place the anchored HUD group", move |c| {
                let mut l = on_ready.lock().unwrap_or_else(|e| e.into_inner());
                if let (Some(canvas), Some((end, id))) = (l.active, l.anchor.take())
                    && c.ctx.runtime().read_dispatch(|d| d.has_generated(end))
                {
                    let mut mem = c.ctx.memory();
                    let at = c.cpu.esi;
                    let point = at
                        .checked_add(4)
                        .and_then(|y| {
                            Some([
                                GuestMemory::f32(&*mem, at).ok()?,
                                GuestMemory::f32(&*mem, y).ok()?,
                            ])
                        })
                        .unwrap_or_else(|| canvas.centre());
                    let fit = canvas.fit(Some((id, point)));
                    l.groups.push((end, fit));
                    sink.set_fit(&mut mem, fit);
                }
                Ok(Flow::Continue)
            })
            .context_only()
            .optional();
        // Menu background art keeps the full view. The fit is chosen per
        // quad, before its Begin, on the guest and the native path alike,
        // and holds until the next quad or widget.
        #[derive(Clone, Copy)]
        enum Site {
            Shader,
            Screen,
            Rect,
        }
        for (point, site) in [
            (&HUD_QUAD_READY, Site::Shader),
            (&SCREEN_QUAD_READY, Site::Screen),
            (&EFFECT_QUAD_READY, Site::Rect),
        ] {
            let (on_quad, sink) = (layout.clone(), self.sink.clone());
            b.raw()
                .observe_before(point, "stretch menu background art to the full view", move |o| {
                    let mut l = on_quad.lock().unwrap_or_else(|e| e.into_inner());
                    if !l.active.is_some_and(|canvas| !canvas.hud) {
                        return;
                    }
                    let mut mem = o.guest();
                    let quad = match site {
                        Site::Shader => shader_quad(&*mem, o.cpu.ebp),
                        Site::Screen => screen_quad(&*mem, o.cpu.esp),
                        // The helper also uploads effects: only the UI's
                        // rectangle is a menu quad.
                        Site::Rect => match rect_quad(&*mem, o.cpu.esp) {
                            Some(quad) => Some(quad),
                            None => return,
                        },
                    };
                    // Screen and rectangle positions are local to the
                    // rectangle their shaders use. Shader quads use the
                    // menu's window coordinates.
                    let (rect, quad) = match site {
                        Site::Shader => (None, quad),
                        Site::Screen | Site::Rect => {
                            let rect = local_raster(&*mem);
                            (rect, rect.and(quad))
                        }
                    };
                    if let Some(fit) = l.place_quad(rect, quad) {
                        sink.set_fit(&mut mem, fit);
                    }
                })
                .context_only()
                .optional();
        }
        let (on_item, sink) = (layout.clone(), self.sink.clone());
        b.raw()
            .observe_before(&ITEM_CALL, "restore the widget fit after background art", move |o| {
                let mut l = on_item.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(fit) = l.end_fill() {
                    sink.set_fit(&mut o.guest(), fit);
                }
            })
            .context_only()
            .optional();

        let (on_fill, sink) = (layout.clone(), self.sink.clone());
        b.raw()
            .before(
                &MENU_FILL,
                "cover the entire menu view with its fade",
                move |c| {
                    let mut l = on_fill.lock().unwrap_or_else(|e| e.into_inner());
                    if l.active.is_some_and(|canvas| !canvas.hud)
                        && c.ctx
                            .runtime()
                            .read_dispatch(|d| d.has_generated(MENU_FILL_END.base))
                    {
                        c.cpu.ecx = fill_bounds(&*c.ctx.memory(), c.cpu.esp, c.cpu.ecx);
                        l.groups.push((MENU_FILL_END.base, None));
                        sink.set_fit(&mut c.ctx.memory(), None);
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only()
            .optional();
        for end in GROUPS.iter().map(|(_, end)| end).chain([
            &MENU_FILL_END,
            &FLARE_QUERY_END,
            &FLARE_REFLECTION_END,
            &VOLUME_SPRITE_END,
            &VOLUME_DEPTH_END,
            &FLARE_COMPOSITE_DONE,
        ]) {
            let (layout, sink) = (layout.clone(), self.sink.clone());
            b.raw()
                .before(end, "restore the enclosing HUD layout", move |c| {
                    let mut l = layout.lock().unwrap_or_else(|e| e.into_inner());
                    if l.end_group(c.point.base) {
                        sink.set_fit(&mut c.ctx.memory(), l.current());
                    }
                    Ok(Flow::Continue)
                })
                .context_only()
                .optional();
        }
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const FULL: PixelRect = PixelRect {
        x: 0,
        y: 0,
        width: 640,
        height: 480,
    };

    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.001, "{a} != {b}");
    }

    #[test]
    fn the_first_views_bottom_left_fit_and_centre_are_published_for_host_marks() {
        let canvas = |rect, pinned| Canvas {
            output: (1920, 1080),
            rect,
            hud: true,
            pinned,
            projection_factor: 0.75,
            projected_points_fitted: false,
        };
        // Pinned at 16:9: squeezed about the view's bottom-left corner.
        publish_hud_corner(Some(canvas(FULL, true)));
        let [sx, sy, ox, oy] = hud_bottom_left_fit();
        close(sx, 0.75);
        close(sy, 1.0);
        close(ox, 0.0);
        close(oy, 0.0);
        // A second view elsewhere leaves the first view's fit.
        let lower = PixelRect {
            x: 0,
            y: 240,
            width: 640,
            height: 240,
        };
        publish_hud_corner(Some(canvas(lower, true)));
        close(hud_bottom_left_fit()[0], 0.75);
        // 4:3: no fit.
        publish_hud_corner(None);
        assert_eq!(hud_bottom_left_fit(), UiFit::IDENTITY.transform);
        assert_eq!(
            hud_centre_fit(),
            (RETICLE_CENTRE, UiFit::IDENTITY.transform)
        );

        // The reticle's centre and fit, from the same publish.
        publish_hud_corner(Some(canvas(FULL, true)));
        let (centre, [sx, sy, ox, oy]) = hud_centre_fit();
        assert_eq!(centre, [320.0, 240.0]);
        close(sx, 0.75);
        close(sy, 1.0);
        // The centre maps to itself.
        close(centre[0] * sx + ox, 320.0);
        close(centre[1] * sy + oy, 240.0);
        // The first view of a split screen is centred on its own rect.
        let upper = PixelRect {
            x: 0,
            y: 0,
            width: 640,
            height: 240,
        };
        publish_hud_corner(Some(canvas(upper, true)));
        assert_eq!(hud_centre_fit().0, [320.0, 120.0]);
        // A second view elsewhere leaves it.
        publish_hud_corner(Some(canvas(lower, true)));
        assert_eq!(hud_centre_fit().0, [320.0, 120.0]);
        publish_hud_corner(None);
    }

    #[test]
    fn render_projection_preserves_the_vertical_lens_and_widens_the_horizontal_edges() {
        let mut m = FakeGuest::new();
        let vertical = 56.0f32.to_radians();
        let retail_x = 1.0 / ((vertical / 2.0).tan() * (4.0 / 3.0));
        for (w, h) in [
            (1600, 1200),
            (2560, 1600),
            (5120, 2880),
            (3440, 1440),
            (5120, 1440),
        ] {
            let corrected_x = retail_x * projection_factor(&m, (w, h));
            let aspect = w as f32 / h as f32;
            close(2.0 * ((1.0 / corrected_x) / aspect).atan(), vertical);
        }
        m.put_u8(0x0048_5ac2, 1).put_f32(0x0045_dfdc, 4.0 / 3.0);
        close(projection_factor(&m, (1920, 1080)), 1.0);
        // A 4:3 presentation leaves even a preconfigured anamorphic lens
        // as the retail game chose it. Mode 3 suppresses the widescreen
        // term before the dashboard flag is tested.
        close(projection_factor(&m, (1600, 1200)), 1.0);
        m.put_u32(0x004e_6948, 0x1000)
            .put_u8(0x1001, 1)
            .put_u16(0x1002, 2)
            .put_u32(0x1008, 3);
        close(projection_factor(&m, (1920, 1080)), 0.75);
    }

    #[test]
    fn displayed_camera_maps_the_side_rays_to_the_full_viewport() {
        let mut mem = FakeGuest::new();
        let esp = 0x1000;
        let at = esp + 0x54;
        let terms = [0.25, -0.125, 1.0, 2.0, -1.25, 0.75, -0.5, 0.5];
        for (i, value) in terms.into_iter().enumerate() {
            mem.put_f32(at + i as u32 * 4, value);
        }
        mem.put_u32(0x0046_701c, 0);
        for caller in [0x0002_badc, 0x0002_bb21, 0x0002_c7cc, 0x0002_c7ff, 0x0001_3cc0] {
            let mut camera = mem.clone();
            camera.put_u32(esp + 0x74, caller);
            fit_view_projection(&mut camera, esp, 0.75).unwrap();
            let bias = camera.f32(at).unwrap();
            let scale = camera.f32(at + 8).unwrap();
            for (offset, pixel) in [(16, 0.), (20, 5120.)] {
                let ray = camera.f32(at + offset).unwrap();
                close((ray * scale + bias + 1.) * 2560., pixel);
            }
            for i in [1, 3, 6, 7] {
                assert_eq!(camera.f32(at + 4 * i).unwrap(), terms[i as usize]);
            }
        }
        mem.put_u32(0x0046_701c, 1);
        for caller in [0x0003_f794, 0x0001_3cc0] {
            mem.put_u32(esp + 0x74, caller);
            assert_eq!(fit_view_projection(&mut mem, esp, 0.75), None);
            assert_eq!(mem.f32(at + 8).unwrap(), terms[2]);
        }
    }

    fn physical(fit: Option<UiFit>, output: (u32, u32), xy: [f32; 2]) -> [f32; 2] {
        let [sx, sy, ox, oy] = fit.map_or([1.0, 1.0, 0.0, 0.0], |f| f.transform);
        [
            (xy[0] * sx + ox) * (output.0 as f32 / 640.0),
            (xy[1] * sy + oy) * (output.1 as f32 / 480.0),
        ]
    }

    #[test]
    fn menu_view_fades_use_the_rectangle_their_shader_projects_over() {
        let mut mem = FakeGuest::new();
        let esp = 0x1000;
        let bounds = 0x2000;
        let output = (2560, 1600);
        for (top, height) in [(0u16, 480u16), (240, 240)] {
            for (at, width) in [(bounds, 480), (RASTER_RECT, 640)] {
                mem.put_u16(at, top)
                    .put_u16(at + 2, 0)
                    .put_u16(at + 4, top + height)
                    .put_u16(at + 6, width);
            }
            for caller in [
                LAYERS_DIM_RETURN,
                LAYERS_FADE_RETURN,
                START_SCREEN_FADE_RETURN,
                MODAL_FILL_RETURN,
            ] {
                mem.put_u32(esp, caller);
                let rect = viewport(&mem, fill_bounds(&mem, esp, bounds)).unwrap();
                let origin = physical(None, output, [rect.x as f32, rect.y as f32]);
                let edge = physical(
                    None,
                    output,
                    [(rect.x + rect.width) as f32, (rect.y + rect.height) as f32],
                );
                assert_eq!(origin, [0.0, if top == 0 { 0.0 } else { 800.0 }]);
                assert_eq!(edge, [2560.0, 1600.0]);
                assert_eq!(rect.y, u32::from(top));
            }
        }
        mem.put_u32(esp, 0x3000);
        assert_eq!(fill_bounds(&mem, esp, bounds), bounds);
        mem.put_u32(esp, MODAL_FILL_RETURN).put_u16(RASTER_RECT + 6, 0);
        assert_eq!(fill_bounds(&mem, esp, bounds), bounds);
    }

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Corners {
        [[left, top], [right, top], [right, bottom], [left, bottom]]
    }

    fn quad(left: f32, top: f32, right: f32, bottom: f32) -> Option<Quad> {
        Some(Quad {
            xy: rect(left, top, right, bottom),
            uv: None,
        })
    }

    /// A quad whose texture runs from `u0` to `u1` across it.
    fn textured(left: f32, top: f32, right: f32, bottom: f32, u0: f32, u1: f32) -> Option<Quad> {
        Some(Quad {
            xy: rect(left, top, right, bottom),
            uv: Some(rect(u0, 0.0, u1, 1.0)),
        })
    }

    fn menu(output: (u32, u32), rect: PixelRect) -> Layout {
        Layout {
            active: Some(Canvas {
                output,
                rect,
                hud: false,
                pinned: true,
                projection_factor: projection_factor(&FakeGuest::new(), output),
                projected_points_fitted: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn menu_background_art_covers_the_display_and_widgets_keep_the_uniform_fit() {
        // A 5K Studio Display renders 16:9 at the 4K budget or full size.
        for output in [(3840, 2160), (5120, 2880), (3440, 1440)] {
            let mut layout = menu(output, FULL);
            let widget = layout.current();
            assert!(widget.is_some());
            // A full-width panel or scrolling strip, wider than the view
            // as a scrolling strip is, reaches both display edges.
            for strip in [quad(0.0, 300.0, 640.0, 330.0), quad(-160.5, 12.0, 800.0, 20.0)] {
                assert_eq!(layout.place_quad(None, strip), Some(None));
                let fit = layout.current();
                close(physical(fit, output, [strip.unwrap().xy[0][0].max(0.0), 0.0])[0], 0.0);
                close(physical(fit, output, [640.0, 0.0])[0], output.0 as f32);
                // The next background quad keeps the full view.
                assert_eq!(layout.place_quad(None, quad(0.5, 0.0, 639.5, 480.0)), None);
                // A list row or a glyph goes back to the 4:3 box.
                assert_eq!(layout.place_quad(None, quad(80.0, 120.0, 560.0, 150.0)), Some(widget));
                assert_eq!(layout.place_quad(None, quad(90.0, 120.0, 99.0, 140.0)), None);
                // An unreadable quad keeps the widget fit.
                layout.place_quad(None, strip);
                assert_eq!(layout.place_quad(None, None), Some(widget));
            }
            // The next widget ends a background's full view.
            layout.place_quad(None, quad(0.0, 0.0, 640.0, 480.0));
            assert_eq!(layout.end_fill(), Some(widget));
            assert_eq!(layout.end_fill(), None);
            let [left, right] = [0.0, 640.0].map(|x| physical(widget, output, [x, 0.0])[0]);
            let unit = output.1 as f32 / 480.0;
            close(left, (output.0 as f32 - 640.0 * unit) * 0.5);
            close(right - left, 640.0 * unit);
        }
    }

    #[test]
    fn background_fill_follows_the_fitted_axis_and_view_and_skips_the_hud() {
        // Taller than 4:3 the fit shrinks y: a full-height quad fills.
        let mut tall = menu((1200, 1600), FULL);
        assert_eq!(tall.place_quad(None, quad(100.0, 0.0, 200.0, 480.0)), Some(None));
        assert_eq!(tall.end_fill(), Some(tall.current()));
        assert_eq!(tall.place_quad(None, quad(0.0, 100.0, 640.0, 200.0)), None);

        // A split-screen menu fills its own view, not the frame.
        let lower = PixelRect { x: 0, y: 240, width: 640, height: 240 };
        let mut split = menu((1920, 1080), lower);
        assert_eq!(split.place_quad(None, quad(0.0, 240.0, 640.0, 480.0)), Some(None));
        let mut split = menu((1920, 1080), PixelRect { x: 320, y: 0, width: 320, height: 480 });
        assert_eq!(split.place_quad(None, quad(0.0, 0.0, 320.0, 480.0)), None);
        assert_eq!(split.place_quad(None, quad(320.0, 0.0, 640.0, 480.0)), Some(None));

        // The HUD keeps its anchored groups; 4:3 has no fit to leave.
        let mut hud = menu((1920, 1080), FULL);
        hud.active = hud.active.map(|c| Canvas { hud: true, ..c });
        assert_eq!(hud.place_quad(None, quad(0.0, 0.0, 640.0, 480.0)), None);
        let mut square = Layout::default();
        assert_eq!(square.place_quad(None, quad(0.0, 0.0, 640.0, 480.0)), None);
    }

    #[test]
    fn a_scrolling_strips_two_pieces_fill_and_a_widget_at_the_edge_keeps_the_fit() {
        // The main menu's strips are 1024-texel bitmaps drawn at one texel
        // a pixel, scrolled: two pieces that meet where the texture wraps
        // (render.frame_layout at 2560x1080: 0..414.8 with u 0.595..1,
        // then 414.8..1024 with u 0..0.595).
        let mut layout = menu((2560, 1080), FULL);
        let widget = layout.current();
        assert!(widget.is_some());
        let wrap = 414.847_9_f32;
        let u = 1.0 - wrap / 1024.0;
        assert_eq!(layout.place_quad(None, textured(0.0, 172.0, wrap, 180.0, u, 1.0)), Some(None));
        assert_eq!(layout.place_quad(None, textured(wrap, 172.0, 1024.0, 180.0, 0.0, u)), None);
        // A sliver at the wrap.
        assert_eq!(layout.place_quad(None, textured(0.0, 420.0, 9.94, 428.0, 0.990_29, 1.0)), None);
        // The menu's 1024x512 background bitmap reaches both edges.
        assert_eq!(layout.place_quad(None, textured(0.0, 0.0, 1024.0, 512.0, 0.0, 1.0)), None);

        // A tab at the left edge whose whole bitmap is on screen, a panel
        // sliding in from off the view and a flat fill keep the 4:3 box.
        assert_eq!(layout.place_quad(None, textured(0.0, 400.0, 220.0, 480.0, 0.0, 1.0)), Some(widget));
        assert_eq!(layout.place_quad(None, textured(-100.0, 100.0, 200.0, 300.0, 0.0, 1.0)), None);
        assert_eq!(layout.place_quad(None, textured(0.0, 100.0, 300.0, 300.0, 0.5, 0.5)), None);
        // Without texture coordinates one edge is not enough.
        assert_eq!(layout.place_quad(None, quad(0.0, 172.0, wrap, 180.0)), None);
        // An emblem's unit quad at the shader site keeps the fit.
        assert_eq!(layout.place_quad(None, quad(0.0, 0.0, 1.0, 1.0)), None);
        assert_eq!(layout.current(), widget);
    }

    #[test]
    fn screen_art_uses_raster_local_bounds_in_the_right_hand_view() {
        let mut mem = FakeGuest::new();
        mem.put_u16(RASTER_RECT, 0)
            .put_u16(RASTER_RECT + 2, 320)
            .put_u16(RASTER_RECT + 4, 480)
            .put_u16(RASTER_RECT + 6, 640);
        let view = viewport(&mem, RASTER_RECT).unwrap();
        let raster = local_raster(&mem);
        let mut layout = menu((2560, 1600), view);
        let widget = layout.current();
        assert_eq!(layout.place_quad(raster, quad(0.5, 0.0, 319.5, 480.0)), Some(None));
        assert_eq!(layout.place_quad(raster, quad(0.0, 0.0, 240.0, 480.0)), Some(widget));
    }

    #[test]
    fn quads_are_read_at_the_shader_screen_and_rect_sites() {
        let mut mem = FakeGuest::new();
        let corners = rect(-2.0, 10.0, 642.0, 40.0);
        let uv = rect(0.25, 0.0, 0.75, 1.0);

        // Shader quad: float4 position, then UV, 0x1c bytes apart.
        let ebp = 0x2000;
        for i in 0..4 {
            let at = ebp - 0x70 + 0x1c * i as u32;
            mem.put_f32(at, corners[i][0])
                .put_f32(at + 4, corners[i][1])
                .put_f32(at + 8, 0.0)
                .put_f32(at + 0xc, 2.0)
                .put_f32(at + 0x10, uv[i][0])
                .put_f32(at + 0x14, uv[i][1]);
        }
        assert_eq!(shader_quad(&mem, ebp), Some(Quad { xy: corners, uv: None }));

        // Screen quad: (x, y), (u, v), then colour.
        let (esp, first) = (0x3000, 0x4000);
        mem.put_u32(esp + 0x18, first);
        for i in 0..4 {
            let at = first + 0x14 * i as u32;
            mem.put_f32(at, corners[i][0])
                .put_f32(at + 4, corners[i][1])
                .put_f32(at + 8, uv[i][0])
                .put_f32(at + 12, uv[i][1])
                .put_u32(at + 16, 0xffff_ffff);
        }
        assert_eq!(screen_quad(&mem, esp), Some(Quad { xy: corners, uv: None }));
        mem.put_f32(first + 0x14, f32::NAN);
        assert_eq!(screen_quad(&mem, esp), None);

        // The UI rectangle at the upload helper: (x, y) then (u, v).
        let (esp, first) = (0x5000, 0x6000);
        mem.put_u32(esp + 12, RECT_SUBMIT_RETURN)
            .put_u32(esp + 16, 0)
            .put_u32(esp + 20, 3)
            .put_u32(esp + 24, 4)
            .put_u32(esp + 32, first);
        for i in 0..4 {
            let at = first + 20 * i as u32;
            mem.put_f32(at, corners[i][0])
                .put_f32(at + 4, corners[i][1])
                .put_f32(at + 8, uv[i][0])
                .put_f32(at + 12, uv[i][1])
                .put_u32(at + 16, 0xffff_ffff);
        }
        assert_eq!(rect_quad(&mem, esp), Some(Quad { xy: corners, uv: Some(uv) }));
        // Another caller of the helper (an effect) is not a menu quad.
        mem.put_u32(esp + 12, 0x0004_1234);
        assert_eq!(rect_quad(&mem, esp), None);
    }

    #[test]
    fn hud_groups_keep_their_safe_inset_size_and_crosshair_on_each_output() {
        for output in [
            (1600, 1200),
            (1920, 1080),
            (3440, 1440),
            (5120, 1440),
            (1200, 1600),
        ] {
            let unit = (output.0 as f32 / 640.0).min(output.1 as f32 / 480.0);
            let canvas = Canvas {
                output,
                rect: FULL,
                hud: true,
                pinned: true,
                projection_factor: projection_factor(&FakeGuest::new(), output),
                projected_points_fitted: false,
            };
            for (id, point) in [
                (0, [605., 20.]),
                (1, [35., 20.]),
                (2, [35., 460.]),
                (3, [605., 460.]),
            ] {
                let pinned = canvas.fit(Some((id, point)));
                let [x, y] = physical(pinned, output, point);
                close(
                    if point[0] < 320. {
                        x
                    } else {
                        output.0 as f32 - x
                    },
                    35. * unit,
                );
                close(
                    if point[1] < 240. {
                        y
                    } else {
                        output.1 as f32 - y
                    },
                    20. * unit,
                );
                let next = physical(pinned, output, [point[0] + 8., point[1] + 8.]);
                close(next[0] - x, 8. * unit);
                close(next[1] - y, 8. * unit);

                let centred = Canvas {
                    pinned: false,
                    ..canvas
                }
                .fit(Some((id, point)));
                let fixed = physical(centred, output, point);
                close(fixed[0], output.0 as f32 / 2. + (point[0] - 320.) * unit);
                close(fixed[1], output.1 as f32 / 2. + (point[1] - 240.) * unit);
                // A menu always uses the centred canvas.
                assert_eq!(
                    Canvas {
                        hud: false,
                        ..canvas
                    }
                    .fit(Some((id, point))),
                    centred
                );
            }
            for (anchor, point) in [(4, [320., 280.]), (5, [74., 315.])] {
                let xy = physical(canvas.fit(Some((anchor, point))), output, point);
                close(
                    xy[0],
                    (320.0 + (point[0] - 320.0) * canvas.projection_factor)
                        * (output.0 as f32 / 640.0),
                );
                close(xy[1], point[1] * (output.1 as f32 / 480.0));
            }
        }
    }

    #[test]
    fn anamorphic_hud_uses_only_the_remaining_display_correction() {
        let mut guest = FakeGuest::new();
        guest.put_u8(0x0048_5ac2, 1).put_f32(0x0045_dfdc, 4.0 / 3.0);
        for output in [(1920, 1080), (3440, 1440), (5120, 1440)] {
            let canvas = Canvas {
                output,
                rect: FULL,
                hud: true,
                pinned: true,
                projection_factor: projection_factor(&guest, output),
                projected_points_fitted: false,
            };
            // An authored square is 12 × 16 guest pixels on this source.
            let p = [24., 18.];
            for pinned in [false, true] {
                let fit = Canvas { pinned, ..canvas }.fit(Some((1, p)));
                let a = physical(fit, output, p);
                let b = physical(fit, output, [p[0] + 12., p[1] + 16.]);
                close(b[0] - a[0], b[1] - a[1]);
                let unit = output.1 as f32 / 480.;
                let inset = if pinned {
                    0.
                } else {
                    (output.0 as f32 - 640. * unit) * 0.5
                };
                close(a[0], inset + 32. * unit);
                if output == (1920, 1080) && pinned {
                    assert_eq!(fit, None);
                }
            }
        }
    }

    #[test]
    fn a_flare_probe_reads_the_widened_column_with_its_guest_area_and_viewport() {
        let mut mem = FakeGuest::new();
        for (index, v) in [0, 320, 480, 640].into_iter().enumerate() {
            mem.put_u16(0x0048_5648 + 2 * index as u32, v);
        }
        let (esp, left, right) = (0x1000, 553., 565.);
        mem.put_f32(esp + 0x14, left).put_f32(esp + 0x18, right);
        let viewport = PixelRect {
            x: 320,
            y: 0,
            width: 320,
            height: 480,
        };
        assert_eq!(projected_effect_fit(&mem, (1600, 1200), Some(esp)), None);
        for output in [(1920, 1080), (3440, 1440), (5120, 1440)] {
            let lens = projection_factor(&mem, output);
            let probe = projected_effect_fit(&mem, output, Some(esp)).unwrap();
            let sprite = projected_effect_fit(&mem, output, None).unwrap();
            let x = |fit: UiFit, x: f32| x * fit.transform[0] + fit.transform[2];
            let old = (left + right) * 0.5;
            close(x(probe, old), 479.5 + (old - 479.5) * lens);
            close(x(probe, right) - x(probe, left), right - left);
            close(x(probe, old) + 0.5, x(sprite, old + 0.5));
            assert_eq!(probe.map_rect(viewport), viewport);
            assert_eq!(sprite.map_rect(viewport), viewport);
        }
    }

    #[test]
    fn a_projected_billboard_keeps_square_pixels_at_an_off_centre_anchor() {
        let mut mem = FakeGuest::new();
        for (i, edge) in [0, 0, 480, 640].into_iter().enumerate() {
            mem.put_u16(0x0048_5648 + i as u32 * 2, edge);
        }
        for size in [(5120, 2880), (3440, 1440), (1200, 1600), (640, 480)] {
            let fit = projected_effect_fit(&mem, size, None);
            let [x, y] = physical(fit, size, [416., 176.]);
            let end = physical(fit, size, [440., 200.]);
            close(end[0] - x, end[1] - y);
            close(x, size.0 as f32 * 0.5 + 96. * size.1 as f32 / 480.);
        }
    }

    #[test]
    fn a_split_hud_and_its_glyph_scissors_use_the_same_view_origin() {
        let rect = PixelRect {
            x: 320,
            y: 0,
            width: 320,
            height: 480,
        };
        let canvas = Canvas {
            output: (1920, 1080),
            projection_factor: 0.75,
            projected_points_fitted: false,
            rect,
            hud: true,
            pinned: false,
        };
        let fit = canvas.fit(None).unwrap();
        assert_eq!(
            fit.map_rect(rect),
            PixelRect {
                x: 360,
                y: 0,
                width: 240,
                height: 480
            }
        );
        let glyph = PixelRect {
            x: 400,
            y: 44,
            width: 80,
            height: 19,
        };
        assert_eq!(
            fit.map_rect(glyph),
            PixelRect {
                x: 420,
                y: 44,
                width: 60,
                height: 19
            }
        );
        let pinned = Canvas {
            pinned: true,
            ..canvas
        }
        .fit(Some((0, [610., 30.])))
        .unwrap();
        assert_eq!(pinned.map_rect(rect).x + pinned.map_rect(rect).width, 640);
    }
}
