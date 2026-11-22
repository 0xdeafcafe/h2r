//! The play window paints the guest thread's Forge page with the disc UI
//! assets. Only the guest thread accepts edits to the map.

use std::path::Path;
use std::sync::{Arc, Mutex};

use app::forge_overlay::{Controller, Frame, Overlay};
use app::theater_overlay::Page;
use boot::forge_glue::gizmo::Axes;
use boot::ui_kit::{preview, spec::ScreenSpec, view::View};
use input::forge::Key;

pub(super) fn overlay(disc: &Path) -> anyhow::Result<Overlay> {
    let pages = Pages {
        disc: preview::Disc::open(disc)?,
        last: None,
    };
    Ok(Overlay(Arc::new(Window(Mutex::new(pages)))))
}

struct Window(Mutex<Pages>);

struct Pages {
    disc: preview::Disc,
    last: Option<Drawn>,
}

struct Drawn {
    spec: ScreenSpec,
    page: Arc<Page>,
    rows: Vec<HitRow>,
    with_axes: Option<(Axes, Arc<Page>)>,
}

struct HitRow {
    index: usize,
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
    middle: i32,
}

fn hit_rows(view: &View) -> Vec<HitRow> {
    let mut rows = Vec::new();
    for widget in &view.widgets {
        let Some(id) = widget
            .id
            .strip_prefix("row.")
            .and_then(|id| id.strip_suffix(".label"))
        else {
            continue;
        };
        let Ok(index) = id.parse() else { continue };
        let [left, top, width, height] = widget.pixels();
        let mut row = HitRow {
            index,
            left,
            top,
            right: left + width,
            bottom: top + height,
            middle: top + height / 2,
        };
        // The frames of adjacent rows overlap. Assign their common pixels to
        // the row whose label is closer, independent of the focus artwork.
        let prefix = format!("row.{index}.");
        for cell in view
            .widgets
            .iter()
            .filter(|w| w.id.starts_with(&prefix) && w.alpha != 0)
        {
            let [x, y, w, h] = cell.pixels();
            row.left = row.left.min(x);
            row.right = row.right.max(x + w);
            row.top = row.top.min(y);
            row.bottom = row.bottom.max(y + h);
        }
        rows.push(row);
    }
    rows
}

fn clicked_row(rows: &[HitRow], x: i32, y: i32) -> Option<usize> {
    rows.iter()
        .filter(|r| (r.left..r.right).contains(&x) && (r.top..r.bottom).contains(&y))
        .min_by_key(|r| r.middle.abs_diff(y))
        .map(|r| r.index)
}

impl Drawn {
    fn frame(&mut self, axes: Option<Axes>) -> Frame {
        let page = match axes {
            None => self.page.clone(),
            Some(axes) => {
                if let Some((old, page)) = &self.with_axes
                    && old == &axes
                {
                    page.clone()
                } else {
                    let mut rgba = self.page.rgba.clone();
                    axes.paint(&mut rgba, self.page.size);
                    let page = Arc::new(Page {
                        rgba,
                        size: self.page.size,
                        timeline: None,
                    });
                    self.with_axes = Some((axes, page.clone()));
                    page
                }
            }
        };
        Frame {
            modal: self.spec.dialog.is_some(),
            page,
        }
    }
}

impl Pages {
    fn render(&mut self, spec: ScreenSpec, axes: Option<Axes>) -> anyhow::Result<Frame> {
        if let Some(last) = &mut self.last
            && last.spec == spec
        {
            return Ok(last.frame(axes));
        }
        let from = preview::From::Disc(&self.disc);
        let view = from.view(&spec, "session")?;
        let canvas = preview::render_overlay(&view, from.source());
        let page = Arc::new(Page {
            size: (canvas.width, canvas.height),
            rgba: canvas.rgba,
            timeline: None,
        });
        Ok(self
            .last
            .insert(Drawn {
                spec,
                page,
                rows: hit_rows(&view),
                with_axes: None,
            })
            .frame(axes))
    }
}

impl Controller for Window {
    fn frame(&self) -> Option<Frame> {
        let mut pages = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(spec) = boot::forge_glue::page() else {
            pages.last = None;
            return None;
        };
        pages
            .render(spec, boot::forge_glue::gizmo())
            .map_err(|error| eprintln!("Forge page: {error:#}"))
            .ok()
    }

    fn key(&self, key: Key, down: bool, repeat: bool) -> bool {
        boot::forge_controls::key(key, down, repeat)
    }

    fn release_keys(&self) {
        boot::forge_controls::release_keys();
    }

    fn mouse_look(&self, dx: f32, dy: f32) -> bool {
        boot::forge_controls::mouse_look(dx, dy)
    }

    fn mouse_button(&self, left: bool, down: bool) -> bool {
        boot::forge_controls::mouse_button(left, down)
    }

    fn click(&self, x: i32, y: i32) {
        let pages = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last) = &pages.last else { return };
        if last.spec.dialog.is_none() || boot::forge_glue::page().as_ref() != Some(&last.spec) {
            return;
        }
        if let Some(row) = clicked_row(&last.rows, x, y) {
            boot::forge_glue::click(row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boot::ui_kit::forge::{Action, Dialogs, Hud};
    use boot::ui_kit::input::Input;

    #[test]
    fn a_scrolled_palette_click_uses_the_document_row_shown_in_the_view() {
        // In original map order Object 38 is palette index 1. The sorted
        // group displays it at row 38 in a viewport scrolled to that item.
        let items = (0..40)
            .rev()
            .map(|i| ("Scenery".into(), format!("Object {i:02}")))
            .collect();
        let mut dlg = Dialogs::grouped_palette(items, 1);
        dlg.press(Input::A);
        let view = preview::From::Synthetic
            .view(&dlg.spec(&Hud::default()), "scrolled")
            .unwrap();
        let rows = hit_rows(&view);
        assert!(rows.first().unwrap().index > 0);
        let [x, y, w, h] = view.widget("row.38.label").unwrap().pixels();
        let (x, y) = (x + w / 2, y + h / 2);
        let hit = clicked_row(&rows, x, y).unwrap();
        assert_eq!(hit, 38);
        assert_eq!(dlg.click(hit), Action::Select(1));
        assert_eq!(clicked_row(&rows, -1, y), None);
    }
}
