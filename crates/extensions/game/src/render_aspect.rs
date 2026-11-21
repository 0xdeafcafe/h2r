//! Display aspect on captured GPU constants and the retail callbacks
//! that own on-screen UI packets. All guest coordinates remain 640 × 480.
use std::sync::{Arc, Mutex};

use d3d::backend::{PixelRect, UiFit};
use engine::{GuestMemory, points};
use hooks::{Builder, Feature, Flow, Manifest};
use points::render_aspect::{
    ANCHOR, ANCHOR_READY, DOC, FLARE_QUERY, FLARE_QUERY_END, FLARE_REFLECTION,
    FLARE_REFLECTION_END, GROUPS, MENU_FILL, MENU_FILL_END,
};

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
    position: Position,
    sink: Arc<dyn UiLayoutSink>,
}

pub fn install(
    registry: &mut hooks::Registry,
    size: Size,
    position: Position,
    sink: Arc<dyn UiLayoutSink>,
) -> anyhow::Result<()> {
    registry
        .add(&RenderAspect {
            size,
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

#[derive(Default)]
struct Layout {
    active: Option<Canvas>,
    anchor: Option<(u32, u32)>,
    groups: Vec<(u32, Option<UiFit>)>,
}

impl Layout {
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
        let (size, sink) = (self.size.clone(), self.sink.clone());
        b.raw()
            .observe_before(
                &points::RENDER_GAME_VIEW,
                "select the GPU projection",
                move |o| {
                    sink.set_projection_factor(projection_factor(&*o.guest(), size()));
                },
            )
            .optional();

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
                    if let Some(index) = l
                        .groups
                        .iter()
                        .rposition(|&(owner, _)| owner == c.point.base)
                    {
                        l.groups.truncate(index);
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

    fn physical(fit: Option<UiFit>, output: (u32, u32), xy: [f32; 2]) -> [f32; 2] {
        let [sx, sy, ox, oy] = fit.map_or([1.0, 1.0, 0.0, 0.0], |f| f.transform);
        [
            (xy[0] * sx + ox) * (output.0 as f32 / 640.0),
            (xy[1] * sy + oy) * (output.1 as f32 / 480.0),
        ]
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
