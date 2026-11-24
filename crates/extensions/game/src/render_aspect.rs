//! Display aspect on captured GPU constants and the retail callbacks
//! that own on-screen UI packets. All guest coordinates remain 640 × 480.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use d3d::backend::{PixelRect, UiFit};
use engine::{GuestMemory, points};
use hooks::{Builder, Feature, Flow, Manifest};
use points::render_aspect::{
    ANCHOR, ANCHOR_READY, DOC, FLARE_QUERY, FLARE_QUERY_END, FLARE_REFLECTION,
    FLARE_REFLECTION_END, GROUPS, MENU_BACKDROP, MENU_FILL, MENU_FILL_END,
};
use points::scene_effects::{EFFECT_QUAD_READY, HUD_QUAD_READY};
use points::ui_render::{ITEM_CALL, RECT_SUBMIT_RETURN, SCREEN_QUAD_READY};

type Size = Arc<dyn Fn() -> (u32, u32) + Send + Sync>;
type Position = Arc<dyn Fn() -> u32 + Send + Sync>;

/// One installed rendering device. The drain pins the layout to each draw
/// before its packet moves to the GPU thread or becomes a native UI pass.
pub trait UiLayoutSink: Send + Sync {
    fn set_projection_factor(&self, factor: f32);
    fn set_fit(&self, guest: &mut runtime::Guest, fit: Option<UiFit>);
}

impl<B: d3d::Backend + Send> UiLayoutSink for Mutex<d3d::Device<B>> {
    fn set_projection_factor(&self, factor: f32) {
        self.lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_projection_factor(factor);
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
// use the display aspect exactly once. The FOV and all guest matrices stay
// in the game's coordinate system for the visibility walk and the tick.
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

// 0x441b0 copies the projection's tangent rectangle into the root polygon
// and builds its bounds and planes from the same rectangle. Feed both uses
// the local copy at +0xa0, the rectangle field of the output frustum. The
// portal flood and its per-window sphere tests inherit these side planes.
fn display_frustum_rect(
    mem: &mut impl GuestMemory,
    source: u32,
    frustum: u32,
    factor: f32,
) -> Option<u32> {
    if factor == 1.0 || !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    let mut rect = [0; 16];
    mem.read(source, &mut rect).ok()?;
    let left = f32::from_le_bytes(rect[..4].try_into().ok()?);
    let right = f32::from_le_bytes(rect[4..8].try_into().ok()?);
    let centre = (left + right) * 0.5;
    let half = (right - left) * 0.5 / factor;
    let (left, right) = (centre - half, centre + half);
    if !left.is_finite() || !right.is_finite() || left >= right {
        return None;
    }
    rect[..4].copy_from_slice(&left.to_le_bytes());
    rect[4..8].copy_from_slice(&right.to_le_bytes());
    let local = frustum.checked_add(0xa0)?;
    mem.write(local, &rect).ok()?;
    Some(local)
}

fn viewport(mem: &impl GuestMemory, address: u32) -> Option<PixelRect> {
    let mut edges = [0; 4];
    for (i, edge) in edges.iter_mut().enumerate() {
        *edge = u32::try_from(mem.u16(address.checked_add(i as u32 * 2)?).ok()? as i16).ok()?;
    }
    let [top, left, bottom, right] = edges;
    (top < bottom && bottom <= 480 && left < right && right <= 640).then(|| PixelRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

// 0x48b00 maps clip X to guest pixel centres. Query edges at the
// call to Begin (0x2e82b) have already been rounded, and their *original*
// area is the game's denominator. Translate this footprint rigidly. The
// reflected billboard has no readback and can take the entire lens map.
fn flare_fit(
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
        if anchor.is_some_and(|(id, _)| id >= 4) && self.hud {
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

/// The shader quad at `HUD_QUAD_READY` (`0x0022a7f6`): four `0x1c`-byte
/// vertices `0x70` bytes below EBP, the projective texture coordinate
/// (s, t, r, q) first and the position at `+0x10`
/// (`render_bridge::native_hud`).
fn shader_quad(mem: &impl GuestMemory, ebp: u32) -> Option<Quad> {
    let first = ebp.checked_sub(0x70)?;
    Some(Quad {
        xy: read_corners(mem, first, 0x1c, 0x10)?,
        uv: None,
    })
}

/// The screen quad at `UI_SCREEN_QUAD_READY` (`0x000363c4`): the pointer
/// at ESP + 0x18, above the return address, to four `0x14`-byte vertices,
/// texture coordinate first and the position at `+8`
/// (`render_bridge::native_hud::ui`).
fn screen_quad(mem: &impl GuestMemory, esp: u32) -> Option<Quad> {
    let first = mem.u32(esp.checked_add(0x18)?).ok()?;
    Some(Quad {
        xy: read_corners(mem, first, 0x14, 8)?,
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
    /// A full-view pass over the menu (`owner`: the backdrop compositor)
    /// starts. Returns whether the fit changes.
    fn begin_menu_composite(&mut self, owner: u32) -> bool {
        if self.active.is_none_or(|canvas| canvas.hud) {
            return false;
        }
        let was_fitted = self.current().is_some();
        self.groups.push((owner, None));
        was_fitted
    }

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
    fn place_quad(&mut self, quad: Option<Quad>) -> Option<Option<UiFit>> {
        let canvas = self.active.filter(|canvas| !canvas.hud)?;
        let was_filled = self.groups.last().is_some_and(|&(id, _)| id == FILL);
        if was_filled {
            self.groups.pop();
        }
        let fill = match (self.current(), quad) {
            (Some(fit), Some(quad)) => is_background(canvas.rect, fit, &quad),
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
        for point in [&points::RENDER_GAME_VIEW, &points::RENDER_ALTERNATE_VIEW] {
            let (size, sink, lens) = (self.size.clone(), self.sink.clone(), view_lens.clone());
            b.raw()
                .observe_before(point, "select the GPU projection", move |o| {
                    let factor = projection_factor(&*o.guest(), size());
                    lens.store(factor.to_bits(), Ordering::Relaxed);
                    sink.set_projection_factor(factor);
                    #[cfg(feature = "desktop-runtime")]
                    render_bridge::scene_step::set_display_visibility_factor(if single_copy { 1.0 } else { factor });
                })
                // Reads this view's guest memory and tells the sink, as the
                // UI-fit hooks below do (all context-only).
                .context_only()
                .optional();
        }

        if single_copy {
            b.raw()
                .before(&points::render_aspect::VIEW_FRUSTUM_RECT, "fit the main visibility frustum", move |c| {
                    let mut mem = c.ctx.memory();
                    let caller = c.cpu.esp.checked_add(8).and_then(|sp| GuestMemory::u32(&*mem, sp).ok());
                    if caller == Some(points::render_aspect::VIEW_FRUSTUM_CALL_RETURN) {
                        let factor = f32::from_bits(view_lens.load(Ordering::Relaxed));
                        if let Some(rect) = display_frustum_rect(&mut *mem, c.cpu.ecx, c.cpu.esi, factor) {
                            c.cpu.ecx = rect;
                        }
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
        ] {
            let (layout, size, sink) = (layout.clone(), self.size.clone(), self.sink.clone());
            b.raw()
                .before(
                    begin,
                    "align the CPU-projected flare with the display lens",
                    move |c| {
                        if !c.ctx.runtime().read_dispatch(|d| d.has_generated(end.base)) {
                            return Ok(Flow::Continue);
                        }
                        let mut l = layout.lock().unwrap_or_else(|e| e.into_inner());
                        let mut mem = c.ctx.memory();
                        if let Some(fit) = flare_fit(&*mem, size(), query.then_some(c.cpu.esp)) {
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
        // The backdrop processes the scene underneath the widget layers.
        // Drain at the helper's boundaries so guest and native fans use
        // the same full-viewport mapping throughout the target chain. (The
        // blur under a dialog copies whole frames inside the widget fit;
        // the device draws frame copies unfitted, `Device` draw resolution.)
        let (on_begin, sink) = (layout.clone(), self.sink.clone());
        b.raw()
            .observe_before(&MENU_BACKDROP, "begin the full-view menu backdrop", move |o| {
                let mut l = on_begin.lock().unwrap_or_else(|e| e.into_inner());
                if l.begin_menu_composite(MENU_BACKDROP.base) {
                    sink.set_fit(&mut o.guest(), None);
                }
            })
            .context_only()
            .optional();
        let (on_end, sink) = (layout.clone(), self.sink.clone());
        b.raw()
            .observe_after(&MENU_BACKDROP, "restore the menu widget fit", move |o| {
                let mut l = on_end.lock().unwrap_or_else(|e| e.into_inner());
                if l.end_group(MENU_BACKDROP.base) {
                    sink.set_fit(&mut o.guest(), l.current());
                }
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
                    if let Some(fit) = l.place_quad(quad) {
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
    fn main_gather_uses_the_display_edges_and_keeps_the_off_centre_lens() {
        let mut mem = FakeGuest::new();
        let src = 0x1000;
        let frustum = 0x2000;
        mem.map(frustum, 0x1bc);
        for (i, bound) in [-0.75, 1.25, -0.6, 0.5].into_iter().enumerate() {
            mem.put_f32(src + 4 * i as u32, bound);
        }
        for output in [(1920, 1080), (3440, 1440), (900, 1200)] {
            let factor = projection_factor(&mem, output);
            let rect = display_frustum_rect(&mut mem, src, frustum, factor).unwrap();
            for (offset, ndc) in [(0, -1.0), (4, 1.0)] {
                close((mem.f32(rect + offset).unwrap() - 0.25) * factor, ndc);
            }
            assert_eq!(mem.f32(rect + 8), mem.f32(src + 8));
            assert_eq!(mem.f32(rect + 12), mem.f32(src + 12));
        }
        assert_eq!(mem.f32(src).unwrap(), -0.75);
        assert_eq!(display_frustum_rect(&mut mem, src, frustum, 1.0), None);
    }

    fn physical(fit: Option<UiFit>, output: (u32, u32), xy: [f32; 2]) -> [f32; 2] {
        let [sx, sy, ox, oy] = fit.map_or([1.0, 1.0, 0.0, 0.0], |f| f.transform);
        [
            (xy[0] * sx + ox) * (output.0 as f32 / 640.0),
            (xy[1] * sy + oy) * (output.1 as f32 / 480.0),
        ]
    }

    #[test]
    fn the_menu_backdrop_uses_the_scene_raster_and_restores_the_widget_fit() {
        for output in [(2768, 1342), (2560, 1920), (1200, 1600)] {
            let guest = FakeGuest::new();
            let lens = projection_factor(&guest, output);
            let canvas = Canvas {
                output,
                rect: FULL,
                hud: false,
                pinned: false,
                projection_factor: lens,
            };
            let mut layout = Layout {
                active: pixel_aspect(output).map(|_| canvas),
                ..Default::default()
            };
            let widget_fit = layout.current();
            assert_eq!(
                layout.begin_menu_composite(MENU_BACKDROP.base),
                widget_fit.is_some()
            );
            let backdrop_fit = layout.current();
            let origin = physical(backdrop_fit, output, [0., 0.]);
            let bounds = physical(backdrop_fit, output, [640., 480.]);
            assert_eq!(origin, [0., 0.]);
            close(bounds[0], output.0 as f32);
            close(bounds[1], output.1 as f32);

            // A full-frame copy with 0–1 UVs covers the first and last
            // output pixel centres on both axes.
            for (axis, extent) in [output.0, output.1].into_iter().enumerate() {
                let uv = |pixel| (pixel - origin[axis]) / (bounds[axis] - origin[axis]);
                assert!(uv(0.5) > 0.0);
                assert!(uv(extent as f32 - 0.5) < 1.0);
            }

            // A scene with the display lens has equal physical units
            // around the centre along both axes.
            let x = physical(None, output, [320. + 20. * lens, 240.]);
            let y = physical(None, output, [320., 260.]);
            close(x[0] - y[0], y[1] - x[1]);

            assert_eq!(layout.end_group(MENU_BACKDROP.base), widget_fit.is_some());
            assert_eq!(layout.current(), widget_fit);
            let origin = physical(widget_fit, output, [0., 0.]);
            let bounds = physical(widget_fit, output, [640., 480.]);
            let unit = (output.0 as f32 / 640.).min(output.1 as f32 / 480.);
            close(bounds[0] - origin[0], 640. * unit);
            close(bounds[1] - origin[1], 480. * unit);
            close(bounds[0] + origin[0], output.0 as f32);
            close(bounds[1] + origin[1], output.1 as f32);
        }
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
                assert_eq!(layout.place_quad(strip), Some(None));
                let fit = layout.current();
                close(physical(fit, output, [strip.unwrap().xy[0][0].max(0.0), 0.0])[0], 0.0);
                close(physical(fit, output, [640.0, 0.0])[0], output.0 as f32);
                // The next background quad keeps the full view.
                assert_eq!(layout.place_quad(quad(0.5, 0.0, 639.5, 480.0)), None);
                // A list row or a glyph goes back to the 4:3 box.
                assert_eq!(layout.place_quad(quad(80.0, 120.0, 560.0, 150.0)), Some(widget));
                assert_eq!(layout.place_quad(quad(90.0, 120.0, 99.0, 140.0)), None);
                // An unreadable quad keeps the widget fit.
                layout.place_quad(strip);
                assert_eq!(layout.place_quad(None), Some(widget));
            }
            // The next widget ends a background's full view.
            layout.place_quad(quad(0.0, 0.0, 640.0, 480.0));
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
        assert_eq!(tall.place_quad(quad(100.0, 0.0, 200.0, 480.0)), Some(None));
        assert_eq!(tall.end_fill(), Some(tall.current()));
        assert_eq!(tall.place_quad(quad(0.0, 100.0, 640.0, 200.0)), None);

        // A split-screen menu fills its own view, not the frame.
        let lower = PixelRect { x: 0, y: 240, width: 640, height: 240 };
        let mut split = menu((1920, 1080), lower);
        assert_eq!(split.place_quad(quad(0.0, 240.0, 640.0, 480.0)), Some(None));
        let mut split = menu((1920, 1080), PixelRect { x: 320, y: 0, width: 320, height: 480 });
        assert_eq!(split.place_quad(quad(0.0, 0.0, 320.0, 480.0)), None);
        assert_eq!(split.place_quad(quad(320.0, 0.0, 640.0, 480.0)), Some(None));

        // The HUD keeps its anchored groups; 4:3 has no fit to leave.
        let mut hud = menu((1920, 1080), FULL);
        hud.active = hud.active.map(|c| Canvas { hud: true, ..c });
        assert_eq!(hud.place_quad(quad(0.0, 0.0, 640.0, 480.0)), None);
        let mut square = Layout::default();
        assert_eq!(square.place_quad(quad(0.0, 0.0, 640.0, 480.0)), None);

        // Inside the backdrop compositor the view is already full.
        let mut backdrop = menu((1920, 1080), FULL);
        backdrop.begin_menu_composite(MENU_BACKDROP.base);
        assert_eq!(backdrop.place_quad(quad(0.0, 0.0, 640.0, 480.0)), None);
        assert_eq!(backdrop.current(), None);
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
        assert_eq!(layout.place_quad(textured(0.0, 172.0, wrap, 180.0, u, 1.0)), Some(None));
        assert_eq!(layout.place_quad(textured(wrap, 172.0, 1024.0, 180.0, 0.0, u)), None);
        // A sliver at the wrap.
        assert_eq!(layout.place_quad(textured(0.0, 420.0, 9.94, 428.0, 0.990_29, 1.0)), None);
        // The menu's 1024x512 background bitmap reaches both edges.
        assert_eq!(layout.place_quad(textured(0.0, 0.0, 1024.0, 512.0, 0.0, 1.0)), None);

        // A tab at the left edge whose whole bitmap is on screen, a panel
        // sliding in from off the view and a flat fill keep the 4:3 box.
        assert_eq!(layout.place_quad(textured(0.0, 400.0, 220.0, 480.0, 0.0, 1.0)), Some(widget));
        assert_eq!(layout.place_quad(textured(-100.0, 100.0, 200.0, 300.0, 0.0, 1.0)), None);
        assert_eq!(layout.place_quad(textured(0.0, 100.0, 300.0, 300.0, 0.5, 0.5)), None);
        // Without texture coordinates one edge is not enough.
        assert_eq!(layout.place_quad(quad(0.0, 172.0, wrap, 180.0)), None);
        // An emblem's unit quad at the shader site keeps the fit.
        assert_eq!(layout.place_quad(quad(0.0, 0.0, 1.0, 1.0)), None);
        assert_eq!(layout.current(), widget);
    }

    #[test]
    fn quads_are_read_at_the_shader_screen_and_rect_sites() {
        let mut mem = FakeGuest::new();
        let corners = rect(-2.0, 10.0, 642.0, 40.0);
        let uv = rect(0.25, 0.0, 0.75, 1.0);

        // Shader quad: (s, t, r, q) then the position, 0x1c bytes apart.
        let ebp = 0x2000;
        for i in 0..4 {
            let at = ebp - 0x70 + 0x1c * i as u32;
            mem.put_f32(at, uv[i][0] * 2.0)
                .put_f32(at + 4, uv[i][1] * 2.0)
                .put_f32(at + 8, 0.0)
                .put_f32(at + 0xc, 2.0)
                .put_f32(at + 0x10, corners[i][0])
                .put_f32(at + 0x14, corners[i][1]);
        }
        assert_eq!(shader_quad(&mem, ebp), Some(Quad { xy: corners, uv: None }));

        // Screen quad: (u, v), the position, then the colour.
        let (esp, first) = (0x3000, 0x4000);
        mem.put_u32(esp + 0x18, first);
        for i in 0..4 {
            let at = first + 0x14 * i as u32;
            mem.put_f32(at, uv[i][0])
                .put_f32(at + 4, uv[i][1])
                .put_f32(at + 8, corners[i][0])
                .put_f32(at + 12, corners[i][1])
                .put_u32(at + 16, 0xffff_ffff);
        }
        assert_eq!(screen_quad(&mem, esp), Some(Quad { xy: corners, uv: None }));
        mem.put_f32(first + 0x14 + 8, f32::NAN);
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
        assert_eq!(flare_fit(&mem, (1600, 1200), Some(esp)), None);
        for output in [(1920, 1080), (3440, 1440), (5120, 1440)] {
            let lens = projection_factor(&mem, output);
            let probe = flare_fit(&mem, output, Some(esp)).unwrap();
            let sprite = flare_fit(&mem, output, None).unwrap();
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
