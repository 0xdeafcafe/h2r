//! A screen's doc laid over the spec its Rust built: the title, the
//! legend, the description block and its help line, a dialog's body and
//! status, its texts, art and HUD widgets, its fixed rows' labels, and a
//! settings dialog's header and rows' helps, each as the doc has them
//! (docs/game/ui-builder.md "Screens in YAML").
//!
//! A text that is one slot alone (`{title}`) is the screen's dynamic
//! value: it stays as Rust set it. A template with words around its slots
//! is filled from the values Rust set, by slot name. What the doc leaves
//! out stays as Rust set it.

use crate::ui_kit::spec::{DescriptionBlock, ScreenSpec};
use crate::ui_kit::view::{Art, Kind};

use super::{DataError, DesignDoc, ScreenDoc, WidgetDoc, fill, slots_of};

/// `t` is one slot and nothing else.
fn lone_slot(t: &str) -> bool {
    slots_of(t).is_ok_and(|s| s.len() == 1 && t == format!("{{{}}}", s[0]))
}

/// Widget `id`'s text over `current` (Rust's value, `slot` its slot
/// name): unchanged when the doc has no text or the text is a lone slot.
fn text_over(
    doc: &ScreenDoc,
    id: &str,
    w: &WidgetDoc,
    slot: &str,
    current: &str,
) -> Result<Option<String>, DataError> {
    let Some(t) = &w.text else { return Ok(None) };
    if lone_slot(t) {
        return Ok(None);
    }
    fill(t, &[(slot, current)])
        .map(Some)
        .map_err(|what| DataError::new(&format!("screens/{}.yaml", doc.screen), &format!("widgets.{id}.text"), what))
}

/// `[top, left, bottom, right]` from a doc's `at` and `size` over `rect`.
fn placed(rect: [i16; 4], at: Option<[i16; 2]>, size: Option<[u16; 2]>) -> [i16; 4] {
    let [mut top, mut left, bottom, right] = rect;
    let (mut w, mut h) = (right - left, top - bottom);
    if let Some([x, y]) = at {
        left = x;
        top = y;
    }
    if let Some([sw, sh]) = size {
        w = sw as i16;
        h = sh as i16;
    }
    [top, left, top - h, left + w]
}

/// The spec `s` built by Rust, presented as screen doc `doc` says, with
/// the design system `design`'s presets.
pub fn present(s: &ScreenSpec, doc: &ScreenDoc, design: &DesignDoc) -> Result<ScreenSpec, DataError> {
    let mut s = s.clone();
    let style = |c: &Option<String>| -> Result<Option<(i16, [f32; 3])>, DataError> {
        match c {
            Some(c) if design.component(c)?.font.is_some() => Ok(Some(design.text_style(c)?)),
            _ => Ok(None),
        }
    };
    for (id, w) in &doc.widgets {
        let on_hud = s.hud.as_ref().is_some_and(|h| h.widgets.iter().any(|x| x.id == *id));
        match id.as_str() {
            _ if on_hud => {
                let slot = id.replace(['.', '-'], "_");
                if let Some(hud) = &mut s.hud
                    && let Some(h) = hud.widgets.iter_mut().find(|h| h.id == *id)
                {
                    h.rect = placed(h.rect, w.at, w.size);
                    let justify = match &w.component {
                        Some(c) => Some(design.component(c)?.justify()),
                        None => None,
                    };
                    let style = style(&w.component)?;
                    if let Kind::Text {
                        text,
                        font,
                        colour,
                        justify: j,
                    } = &mut h.kind
                    {
                        if let Some(t) = text_over(doc, id, w, &slot, &text.clone())? {
                            *text = t;
                        }
                        if let Some((f, c)) = style {
                            *font = f;
                            *colour = crate::ui_kit::components::rgb(c);
                        }
                        if let Some(js) = justify {
                            *j = js;
                        }
                    }
                }
            }
            "title" => {
                if let Some(t) = text_over(doc, id, w, "title", &s.title)? {
                    s.title = t;
                }
            }
            "legend" => {
                if w.prompts.is_some() {
                    s.prompts = doc.prompts(id)?;
                }
            }
            "description" => {
                if let Some(t) = text_over(doc, id, w, "help", s.help.as_deref().unwrap_or(""))? {
                    s.help = Some(t);
                }
                if let Some(d) = &mut s.description {
                    if let Some((font, colour)) = style(&w.component)? {
                        d.font = font;
                        d.colour = colour;
                    }
                    d.rect = placed(d.rect, w.at, w.size);
                } else if let (Some(c), Some(at), Some(size)) = (&w.component, w.at, w.size)
                    && let Some((font, colour)) = style(&Some(c.clone()))?
                {
                    s.description = Some(DescriptionBlock {
                        rect: placed([0; 4], Some(at), Some(size)),
                        font,
                        colour,
                    });
                }
            }
            "body" | "status" => {
                if let Some(d) = &mut s.dialog {
                    if id == "body" {
                        if let Some(t) = text_over(doc, id, w, "body", &d.body.clone())? {
                            d.body = t;
                        }
                    } else if let Some(current) = d.status.clone()
                        && let Some(t) = text_over(doc, id, w, "status", &current)?
                    {
                        d.status = Some(t);
                    }
                }
            }
            _ => {
                if let Some(k) = id.strip_prefix("art.").and_then(|k| k.parse::<usize>().ok())
                    && let Some(a) = s.art.get_mut(k)
                {
                    if let Some(r) = w.art {
                        a.art = Art::Screen {
                            screen_id: r.screen,
                            pane: r.pane,
                            index: r.index,
                            image: r.image,
                        };
                    }
                    if let Some(at) = w.at {
                        a.at = at;
                    }
                    if let Some(size) = w.size {
                        a.size = size;
                    }
                    continue;
                }
                let slot = id.replace(['.', '-'], "_");
                if let Some(t) = s.texts.iter_mut().find(|t| t.id == *id) {
                    if let Some(text) = text_over(doc, id, w, &slot, &t.text.clone())? {
                        t.text = text;
                    }
                    if let Some((font, colour)) = style(&w.component)? {
                        t.font = font;
                        t.colour = colour;
                    }
                    if let Some(c) = &w.component {
                        t.right = design.component(c)?.justify.as_deref() == Some("right");
                    }
                    t.rect = placed(t.rect, w.at, w.size);
                    continue;
                }
            }
        }
    }
    for (i, row) in s.rows.iter_mut().enumerate() {
        if let Some(label) = doc.row_text(i) {
            row.label = label.into();
        }
    }
    if let Some(d) = &mut s.dialog {
        for (i, row) in d.rows.iter_mut().enumerate() {
            if let Some(label) = doc.row_text(i) {
                row.label = label.into();
            }
        }
    }
    if doc.settings.is_some() {
        settings_over(&mut s, doc);
    }
    Ok(s)
}

/// A settings dialog's `settings:` block and rows' helps over spec `s`
/// (docs/game/ui-builder.md "The settings dialog"): the header's name and
/// about lines, its icon, and each row's help, so an edited doc shows as
/// the game would build it.
fn settings_over(s: &mut ScreenSpec, doc: &ScreenDoc) {
    let Some(st) = &doc.settings else { return };
    for t in &mut s.texts {
        match t.id.as_str() {
            "header.name" => {
                if let Some(n) = &st.name {
                    t.text = n.clone();
                }
            }
            "header.about" => t.text = st.about().to_string(),
            _ => {}
        }
    }
    if let Some(icon) = st.icon.as_deref().and_then(crate::ui_kit::dialog::icon::named)
        && let Some(Art::Screen { image, .. }) = s.art.first_mut().map(|a| &mut a.art)
    {
        *image = icon as i16;
    }
    let help = |i: usize| doc.rows.get(&format!("row.{i}")).and_then(|r| r.help.clone());
    for (i, row) in s.rows.iter_mut().enumerate() {
        if let Some(h) = help(i) {
            row.description = Some(h);
        }
    }
    if let Some(d) = &mut s.dialog {
        for (i, row) in d.rows.iter_mut().enumerate() {
            if let Some(h) = help(i) {
                row.description = Some(h);
            }
        }
    }
}
