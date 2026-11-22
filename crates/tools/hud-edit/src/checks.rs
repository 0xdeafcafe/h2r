//! Layout checks over a drawn HUD: what runs off the screen or out of the
//! HUD frame, art that partly covers other art, and text that spills out
//! of the art it sits on.

use crate::model::{WidgetKey, WidgetKind};
use crate::place::{Rect, SCREEN};
use crate::render::Picture;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth a look, often by design.
    Note,
    /// Likely a layout bug.
    Warning,
}

/// One finding, with the widgets it names (`(hud, key)`).
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub text: String,
    pub widgets: Vec<(usize, WidgetKey)>,
}

/// Opaque pixels two pieces of art must share to count as overlapping.
pub const OVERLAP_MIN: u32 = 4;

/// The checks, worst first:
///
/// - **off screen**: a drawn widget's rectangle leaves the 640 by 480
///   screen (warning);
/// - **outside the frame**: it leaves the HUD frame the anchors are the
///   corners of (note: retail art sits a pixel outside it);
/// - **art overlaps**: two drawn bitmap widgets share at least
///   [`OVERLAP_MIN`] opaque pixels and neither's rectangle holds the
///   other's (warning). Nested rectangles are layers by design (a panel
///   over its meter, a flash over its bar) and are not flagged;
/// - **text spills**: a drawn text widget's rectangle crosses a drawn
///   bitmap widget's but is not inside it (warning);
/// - **text over text**: two drawn texts share opaque pixels (warning).
pub fn check(p: &Picture) -> Vec<Finding> {
    let mut out = Vec::new();
    let screen = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: SCREEN[0],
        y1: SCREEN[1],
    };
    let frame = p.frame.rect();
    let drawn: Vec<_> = p.placed.iter().filter(|w| w.drawn && w.rect.is_some()).collect();
    for w in &drawn {
        let r = w.rect.unwrap();
        let id = vec![(w.hud, w.key)];
        if !screen.contains(&r) {
            out.push(Finding {
                severity: Severity::Warning,
                text: format!("{} runs off the screen ({})", w.name, show(&r)),
                widgets: id,
            });
        } else if !frame.contains(&r) {
            out.push(Finding {
                severity: Severity::Note,
                text: format!("{} leaves the HUD frame ({})", w.name, show(&r)),
                widgets: id,
            });
        }
    }
    for (i, a) in drawn.iter().enumerate() {
        for b in &drawn[i + 1..] {
            let (ra, rb) = (a.rect.unwrap(), b.rect.unwrap());
            if ra.intersect(&rb).is_none() {
                continue;
            }
            let ids = vec![(a.hud, a.key), (b.hud, b.key)];
            match (a.key.kind, b.key.kind) {
                (WidgetKind::Bitmap, WidgetKind::Bitmap) => {
                    if ra.contains(&rb) || rb.contains(&ra) {
                        continue;
                    }
                    let n = match (&a.cover, &b.cover) {
                        (Some(x), Some(y)) => x.overlap(y),
                        _ => 0,
                    };
                    if n >= OVERLAP_MIN {
                        out.push(Finding {
                            severity: Severity::Warning,
                            text: format!("{} and {} overlap ({n} opaque pixels)", a.name, b.name),
                            widgets: ids,
                        });
                    }
                }
                (WidgetKind::Text, WidgetKind::Text) => {
                    let n = match (&a.cover, &b.cover) {
                        (Some(x), Some(y)) => x.overlap(y),
                        _ => 0,
                    };
                    if n > 0 {
                        out.push(Finding {
                            severity: Severity::Warning,
                            text: format!("texts {} and {} overlap", a.name, b.name),
                            widgets: ids,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    // Text against the art it sits on: inside one piece of art it crosses,
    // or it spills.
    for t in drawn.iter().filter(|w| w.key.kind == WidgetKind::Text) {
        let rt = t.rect.unwrap();
        let crossed: Vec<_> = drawn
            .iter()
            .filter(|b| b.key.kind == WidgetKind::Bitmap)
            .filter(|b| b.rect.unwrap().intersect(&rt).is_some())
            .collect();
        if crossed.is_empty() || crossed.iter().any(|b| b.rect.unwrap().contains(&rt)) {
            continue;
        }
        let mut ids = vec![(t.hud, t.key)];
        ids.extend(crossed.iter().map(|b| (b.hud, b.key)));
        let names: Vec<_> = crossed.iter().map(|b| b.name.as_str()).collect();
        out.push(Finding {
            severity: Severity::Warning,
            text: format!(
                "{} ({}) spills out of {} ({})",
                t.name,
                show(&rt),
                names.join(", "),
                crossed.iter().map(|b| show(&b.rect.unwrap())).collect::<Vec<_>>().join(", ")
            ),
            widgets: ids,
        });
    }
    out.sort_by_key(|check| std::cmp::Reverse(check.severity));
    out
}

fn show(r: &Rect) -> String {
    let [x0, y0, x1, y1] = r.pixels();
    format!("x {x0}..{x1}, y {y0}..{y1}")
}
