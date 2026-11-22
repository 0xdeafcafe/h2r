//! The converter (`h2 ui export-yaml`): every screen of
//! `crate::ui_kit::screens::NAMES` in every state, read off the specs its
//! Rust builds, written as `assets/ui/screens/<name>.yaml`, and
//! `assets/ui/design.yaml` grown by what the screens use
//! (docs/game/ui-builder.md "Screens in YAML").
//!
//! - A widget whose style (font, colour, justification) matches a preset of
//!   the design system names it; one that matches none gets a preset of
//!   its own, `<role>.<screen>.<n>`, and a colour that is in no palette
//!   entry gets one, `<screen>.<n>`: names with a dot are what the
//!   converter found, not the design system's own, and the consistency
//!   check reports the widgets that use them.
//! - A text that is the same in every state is written as it is; one that
//!   changes, or that [`DYNAMIC`] names, is a lone slot (`{title}`) that
//!   Rust fills.
//! - A place (`at`, `size`) that is the same in every state is written;
//!   one that moves stays Rust's.
//! - Rows are written for the screens whose rows are fixed ([`FIXED_ROWS`],
//!   the specimens and layouts); Rust makes the others'.
//! - The design system's rules ([`house_rules`]) are applied to the
//!   screens drawn from their docs (`super::FROM_DOC`), whose Rust holds no
//!   copy to disagree with (docs/game/ui-builder.md "Consistency").

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;

use super::{
    ArtDoc, ComponentDoc, DesignDoc, Glyphs, PromptDoc, RowDoc, ScreenDoc, StateDoc, WidgetDoc,
};
use crate::ui_kit::components::Kind;
use crate::ui_kit::dialog::DialogBase;
use crate::ui_kit::spec::{ListPlace, Press, RowKind, ScreenSpec};
use crate::ui_kit::view::{Art, Justify, Opener};

/// Texts that are a player's or a map's even where the samples show one
/// value: `(screen, widget)`.
pub const DYNAMIC: [(&str, &str); 8] = [
    ("mods.detail", "title"),
    ("mods.pack", "title"),
    ("theater.film", "title"),
    ("forge.properties", "title"),
    ("kit.picker", "title"),
    ("kit.picker.game", "title"),
    ("kit.picker.forge-map", "title"),
    // The card's profile line: the bot's, live.
    (crate::clanker_card::SCREEN, "description"),
];

/// Our screens whose rows are fixed: their labels go in the doc.
pub const FIXED_ROWS: [&str; 6] = [
    "kit.example",
    "kit.dialog",
    "kit.dialog.large",
    "kit.choices",
    "lab.takeover",
    crate::clanker_card::SCREEN,
];

fn dynamic(screen: &str, widget: &str) -> bool {
    DYNAMIC.contains(&(screen, widget))
}

/// What the converter learns about the design system as it goes.
struct Design {
    doc: DesignDoc,
}

fn justify_name(j: Justify) -> &'static str {
    match j {
        Justify::Left => "left",
        Justify::Right => "right",
        Justify::Centre => "centre",
    }
}

/// A view's `[u8; 3]` as a palette value that converts back to it.
fn from_rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| (f32::from(v) + 0.5) / 255.0)
}

impl Design {
    /// The palette entry for `c`: an entry of that colour, or a new one
    /// named `<screen>.<n>`.
    fn colour(&mut self, c: [f32; 3], screen: &str) -> String {
        let rgb = crate::ui_kit::components::rgb(c);
        if let Some((n, _)) = self
            .doc
            .palette
            .iter()
            .find(|(_, p)| **p == c)
            .or_else(|| {
                self.doc
                    .palette
                    .iter()
                    .find(|(_, p)| crate::ui_kit::components::rgb(**p) == rgb)
            })
        {
            return n.clone();
        }
        let name = (self.doc.palette.len()..)
            .map(|n| format!("{screen}.{n}"))
            .find(|n| !self.doc.palette.contains_key(n))
            .expect("a free name");
        self.doc.palette.insert(name.clone(), c);
        name
    }
    /// The font role for `font`, preferring `role`.
    fn font(&mut self, font: i16, role: &str) -> String {
        if self.doc.fonts.get(role) == Some(&font) {
            return role.into();
        }
        if let Some((n, _)) = self.doc.fonts.iter().find(|(_, f)| **f == font) {
            return n.clone();
        }
        let name = format!("font_{font}");
        self.doc.fonts.insert(name.clone(), font);
        name
    }
    /// The preset for a text of `role` in `font`, `colour`, `justify`: one
    /// of `prefer` (first), any preset of that style, or a new one.
    fn component(
        &mut self,
        prefer: &[&str],
        role: &str,
        screen: &str,
        font: i16,
        colour: [f32; 3],
        justify: Justify,
    ) -> String {
        let colour_name = self.colour(colour, screen);
        let matches = |c: &ComponentDoc, d: &DesignDoc| {
            c.font.as_deref().and_then(|f| d.fonts.get(f)) == Some(&font)
                && c.colour.as_deref() == Some(colour_name.as_str())
                && c.justify() == justify
        };
        for p in prefer {
            if let Some(c) = self.doc.components.get(*p)
                && matches(c, &self.doc)
            {
                return (*p).into();
            }
        }
        if let Some((n, _)) = self
            .doc
            .components
            .iter()
            .find(|(n, c)| n.starts_with(role) && matches(c, &self.doc))
        {
            return n.clone();
        }
        let font_name = self.font(font, if role == "title" { "title" } else { "body" });
        let name = (self.doc.components.len()..)
            .map(|n| format!("{role}.{screen}.{n}"))
            .find(|n| !self.doc.components.contains_key(n))
            .expect("a free name");
        self.doc.components.insert(
            name.clone(),
            ComponentDoc {
                role: format!("{role} of {screen}{FOUND}"),
                font: Some(font_name),
                colour: Some(colour_name),
                justify: Some(justify_name(justify).into()),
                at: None,
                size: None,
                variants: Vec::new(),
                specimen: None,
                pattern: None,
                ids: Vec::new(),
            },
        );
        name
    }
}

/// The value every state has, or `None` when they differ.
fn same<T: PartialEq + Clone>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut it = values.into_iter();
    let first = it.next()?;
    it.all(|v| v == first).then_some(first)
}

fn opener_name(o: &Opener) -> Option<&'static str> {
    [
        "main-menu",
        "game-setup",
        "game-setup.clankers",
        "game-setup.firefight",
        "game-setup.forge",
        "xbox-live-menu",
    ]
    .into_iter()
    .find(|n| super::opener_named(n).as_ref() == Some(o))
}

/// `[top, left, bottom, right]` as `at` and `size`.
fn at_size(r: [i16; 4]) -> ([i16; 2], [u16; 2]) {
    let [top, left, bottom, right] = r;
    ([left, top], [(right - left).max(0) as u16, (top - bottom).max(0) as u16])
}

/// A text widget: literal when every state has `texts`' one value and the
/// widget isn't [`DYNAMIC`], else its lone slot.
fn text_of(screen: &str, id: &str, slot: &str, texts: &[String]) -> Option<String> {
    let t = same(texts.iter().cloned())?;
    if dynamic(screen, id) || t.contains(['{', '}']) {
        return Some(format!("{{{slot}}}"));
    }
    Some(t)
}

fn kind_and_layout(s: &ScreenSpec) -> (&'static str, &'static str) {
    match Kind::of(s) {
        Kind::Hud => ("hud", "none"),
        Kind::CodeEntry => ("code_entry", "large"),
        Kind::Dialog(DialogBase::ValuePicker) => (
            "picker",
            match s.dialog.as_ref().map(|d| d.base) {
                Some(DialogBase::ValuePicker) => "picker",
                _ => "large",
            },
        ),
        Kind::Dialog(DialogBase::QuickOptions) => ("dialog", "settings"),
        Kind::Dialog(DialogBase::Large) => ("dialog", "large"),
        Kind::List => (
            "list",
            match s.list {
                ListPlace::Controls => "controls",
                ListPlace::Moved { skin, anchor, .. }
                    if skin == crate::skulls_menu::LIST_LAYOUT.skin
                        && anchor == crate::skulls_menu::LIST_LAYOUT.anchor =>
                {
                    "level_select"
                }
                ListPlace::Moved { .. } => "moved",
            },
        ),
    }
}

fn row_doc(r: &crate::ui_kit::spec::RowSpec) -> RowDoc {
    let mut d = RowDoc {
        text: r.label.clone(),
        help: r.description.clone(),
        ..RowDoc::default()
    };
    match (&r.kind, &r.press) {
        (RowKind::Action, Press::Open(name)) => {
            d.kind = "open".into();
            d.open = Some(name.clone());
        }
        (RowKind::Action, _) => d.kind = "action".into(),
        (RowKind::Value { values, current }, press) => {
            d.kind = if matches!(press, Press::Open(_)) { "pick" } else { "value" }.into();
            d.values = Some(values.clone());
            d.current = Some(*current);
        }
        // `components::row::disabled`: a label alone, never pressed.
        (RowKind::Info { value }, Press::Nothing) if value.is_empty() => {
            d.kind = "disabled".into();
        }
        (RowKind::Info { value }, _) => {
            d.kind = "info".into();
            d.value = Some(value.clone());
        }
        (RowKind::Text { value, max_chars }, _) => {
            d.kind = "text".into();
            d.value = Some(value.clone());
            d.max_chars = Some(*max_chars);
        }
    }
    d
}

/// Screen `name`'s doc from its states' specs, growing `design`.
fn screen_doc(name: &str, states: &[(String, ScreenSpec)], design: &mut Design) -> ScreenDoc {
    let first = &states[0].1;
    let all = || states.iter().map(|(_, s)| s);
    let (kind, layout) = kind_and_layout(first);
    let (backing, opener) = if first.transparent && (first.hud.is_some() || first.dialog.is_none()) {
        (Some("none".to_string()), None)
    } else if let Some(u) = &first.underlay {
        if crate::ui_kit::screens::NAMES.contains(&u.name.as_str()) {
            (Some("screen".to_string()), Some(u.name.clone()))
        } else {
            (None, None)
        }
    } else if let Some(d) = &first.dialog {
        match opener_name(&d.opener) {
            Some(o) => (Some("over".to_string()), Some(o.to_string())),
            None => (None, None),
        }
    } else {
        (Some("shell".to_string()), None)
    };
    let mut doc = ScreenDoc {
        screen: name.into(),
        kind: kind.into(),
        layout: Some(layout.into()),
        backing,
        opener,
        scrolls: first.scrolls,
        states: IndexMap::new(),
        widgets: IndexMap::new(),
        rows: IndexMap::new(),
    };
    for (n, s) in states {
        doc.states.insert(
            n.clone(),
            StateDoc {
                focus: s.focus,
                cursor: s.dialog.as_ref().and_then(|d| d.cells.as_ref()).and_then(|c| c.cursor),
            },
        );
    }
    // The title (its style is the header layout's).
    let titles: Vec<String> = all().map(|s| s.title.clone()).collect();
    if first.hud.is_none() {
        doc.widgets.insert(
            "title".into(),
            WidgetDoc {
                component: Some("title".to_string()),
                text: text_of(name, "title", "title", &titles),
                ..WidgetDoc::default()
            },
        );
    }
    // A dialog's body and status.
    if first.dialog.is_some() {
        let bodies: Vec<String> = all().map(|s| s.dialog.as_ref().map_or(String::new(), |d| d.body.clone())).collect();
        if bodies.iter().any(|b| !b.is_empty()) {
            doc.widgets.insert(
                "body".into(),
                WidgetDoc {
                    component: Some("body".into()),
                    text: text_of(name, "body", "body", &bodies),
                    ..WidgetDoc::default()
                },
            );
        }
        let statuses: Vec<Option<String>> = all().map(|s| s.dialog.as_ref().and_then(|d| d.status.clone())).collect();
        if statuses.iter().any(Option::is_some) {
            let text = match same(statuses.iter().cloned()) {
                Some(Some(t)) if !dynamic(name, "status") => Some(t),
                _ => Some("{status}".to_string()),
            };
            doc.widgets.insert(
                "status".into(),
                WidgetDoc {
                    component: Some("status".into()),
                    text,
                    ..WidgetDoc::default()
                },
            );
        }
        if let Some(c) = first.dialog.as_ref().and_then(|d| d.cells.as_ref()) {
            doc.widgets.insert(
                "cells".into(),
                WidgetDoc {
                    component: Some("cells".into()),
                    digits: Some(c.text.concat()),
                    gap_after: c.gap_after,
                    ..WidgetDoc::default()
                },
            );
        }
    }
    // The description block and its help line.
    if let Some(d) = first.description {
        let blocks: Vec<_> = all().filter_map(|s| s.description).collect();
        let fixed = same(blocks.iter().map(|b| (b.rect, b.font, b.colour.map(f32::to_bits))));
        let mut w = WidgetDoc::default();
        if fixed.is_some() {
            let component = design.component(
                &["description", "description.pane", "description.settings", "description.picker", "legend.panel"],
                "description",
                name,
                d.font,
                d.colour,
                Justify::Left,
            );
            let (at, size) = at_size(d.rect);
            let preset = design.doc.components.get(&component).map(|c| (c.at, c.size));
            if preset != Some((Some(at), Some(size))) {
                w.at = Some(at);
                w.size = Some(size);
            }
            w.component = Some(component);
        }
        let helps: Vec<Option<String>> = all().map(|s| s.help.clone()).collect();
        if helps.iter().any(Option::is_some) {
            w.text = match same(helps.iter().cloned()) {
                Some(Some(t)) if !dynamic(name, "description") && !t.contains(['{', '}']) => Some(t),
                _ => Some("{help}".into()),
            };
        }
        doc.widgets.insert("description".into(), w);
    }
    // The legend.
    if all().any(|s| !s.prompts.is_empty()) {
        let prompts = same(all().map(|s| s.prompts.clone())).map(|ps| {
            ps.iter()
                .map(|p| {
                    let names: Vec<String> = p.glyphs.iter().map(|g| g.name().to_string()).collect();
                    let glyphs = if names.len() == 1 {
                        Glyphs::One(names[0].clone())
                    } else {
                        Glyphs::Many(names)
                    };
                    PromptDoc(glyphs, p.text.to_uppercase())
                })
                .collect()
        });
        doc.widgets.insert(
            "legend".into(),
            WidgetDoc {
                component: Some(if first.footer || first.dialog.is_some() { "legend" } else { "legend.panel" }.into()),
                prompts,
                ..WidgetDoc::default()
            },
        );
    }
    // Texts of no row, by id.
    let mut ids: Vec<String> = Vec::new();
    for s in all() {
        for t in &s.texts {
            if !ids.contains(&t.id) {
                ids.push(t.id.clone());
            }
        }
    }
    for id in &ids {
        let of: Vec<_> = all().filter_map(|s| s.texts.iter().find(|t| t.id == *id)).collect();
        let t0 = of[0];
        let style = same(of.iter().map(|t| (t.font, t.colour.map(f32::to_bits), t.right)));
        let rect = same(of.iter().map(|t| t.rect));
        let slot = id.replace(['.', '-'], "_");
        let texts: Vec<String> = of.iter().map(|t| t.text.clone()).collect();
        let mut w = WidgetDoc {
            text: text_of(name, id, &slot, &texts).or_else(|| Some(format!("{{{slot}}}"))),
            ..WidgetDoc::default()
        };
        if style.is_some() {
            let justify = if t0.right { Justify::Right } else { Justify::Left };
            w.component = Some(design.component(
                &["text.label", "text.value", "text.heading", "text.emphasis"],
                "text",
                name,
                t0.font,
                t0.colour,
                justify,
            ));
        }
        if let Some(r) = rect {
            let (at, size) = at_size(r);
            w.at = Some(at);
            w.size = Some(size);
        }
        doc.widgets.insert(id.clone(), w);
    }
    // The screen's own art.
    for (k, a) in first.art.iter().enumerate() {
        let Art::Screen {
            screen_id,
            pane,
            index,
            image,
        } = a.art
        else {
            continue;
        };
        let fixed = all().all(|s| s.art.get(k) == Some(a));
        if !fixed {
            continue;
        }
        doc.widgets.insert(
            format!("art.{k}"),
            WidgetDoc {
                component: Some("art".into()),
                at: Some(a.at),
                size: Some(a.size),
                art: Some(ArtDoc {
                    screen: screen_id,
                    pane,
                    index,
                    image,
                }),
                ..WidgetDoc::default()
            },
        );
    }
    // A HUD's texts.
    if let Some(hud) = &first.hud {
        for h in &hud.widgets {
            let crate::ui_kit::view::Kind::Text {
                font,
                justify,
                colour,
                ..
            } = &h.kind
            else {
                continue;
            };
            let of: Vec<_> = all()
                .filter_map(|s| s.hud.as_ref()?.widgets.iter().find(|x| x.id == h.id))
                .collect();
            let texts: Vec<String> = of.iter().filter_map(|x| x.text().map(String::from)).collect();
            let slot = h.id.replace(['.', '-'], "_");
            let mut w = WidgetDoc {
                text: text_of(name, &h.id, &slot, &texts).or_else(|| Some(format!("{{{slot}}}"))),
                ..WidgetDoc::default()
            };
            let styles = same(of.iter().map(|x| match &x.kind {
                crate::ui_kit::view::Kind::Text { font, justify, colour, .. } => Some((*font, *justify, *colour)),
                _ => None,
            }));
            if styles.flatten().is_some() {
                w.component = Some(design.component(
                    &["hud.text", "hud.label"],
                    "hud",
                    name,
                    *font,
                    from_rgb(*colour),
                    *justify,
                ));
            }
            if let Some(r) = same(of.iter().map(|x| x.rect)) {
                let (at, size) = at_size(r);
                w.at = Some(at);
                w.size = Some(size);
            }
            doc.widgets.insert(h.id.clone(), w);
        }
    }
    // Fixed rows.
    let fixed = FIXED_ROWS.contains(&name) || name.starts_with("design.") || name.starts_with("layout.");
    if fixed && first.hud.is_none() {
        for (i, r) in first.rows.iter().enumerate() {
            doc.rows.insert(format!("row.{i}"), row_doc(r));
        }
    }
    house_rules(&mut doc);
    doc
}

/// The design system's rules, on a screen drawn from its doc: a
/// dialog's, a code entry's and a picker's title in capitals (its slots
/// as they are). A screen whose Rust still carries its presentation is
/// fixed in its Rust, so its doc and its Rust agree.
pub(crate) fn house_rules(doc: &mut ScreenDoc) {
    if !super::FROM_DOC.contains(&doc.screen.as_str()) {
        return;
    }
    if matches!(doc.kind.as_str(), "dialog" | "code_entry" | "picker")
        && let Some(w) = doc.widgets.get_mut("title")
        && let Some(t) = &mut w.text
    {
        *t = upper_outside_slots(t);
    }
}

/// `t` in capitals, its `{slot}` names as they are.
fn upper_outside_slots(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut in_slot = false;
    for c in t.chars() {
        match c {
            '{' => in_slot = true,
            '}' => in_slot = false,
            _ => {}
        }
        if in_slot || c == '}' {
            out.push(c);
        } else {
            out.extend(c.to_uppercase());
        }
    }
    out
}

/// Writes `design.yaml` and `screens/<name>.yaml` for every screen into
/// `dir` (`assets/ui` in a checkout); `design.yaml` there is the seed it
/// grows. Returns the files written.
pub fn export(dir: &Path) -> Result<Vec<PathBuf>> {
    let seed = dir.join("design.yaml");
    let text = std::fs::read_to_string(&seed).with_context(|| seed.display().to_string())?;
    let mut design = Design {
        doc: DesignDoc::parse(&text).map_err(|e| anyhow::anyhow!("{e}"))?,
    };
    let screens = dir.join("screens");
    std::fs::create_dir_all(&screens).with_context(|| screens.display().to_string())?;
    let mut written = Vec::new();
    let mut docs = Vec::new();
    for name in crate::ui_kit::screens::NAMES {
        let states = crate::ui_kit::screens::raw_states(name)
            .with_context(|| format!("no states for {name}"))?;
        anyhow::ensure!(!states.is_empty(), "{name} has no states");
        let doc = screen_doc(name, &states, &mut design);
        let path = screens.join(format!("{name}.yaml"));
        std::fs::write(&path, super::to_yaml(&doc)).with_context(|| path.display().to_string())?;
        written.push(path);
        docs.push(doc);
    }
    prune(&mut design.doc, &docs);
    std::fs::write(&seed, design_to_yaml(&design.doc)).with_context(|| seed.display().to_string())?;
    written.push(seed);
    Ok(written)
}

/// The end of the role of a component the converter made for one screen's
/// style (no preset matched it).
const FOUND: &str = ", as the converter found it";

/// Drops what an earlier export made and no screen uses now: components
/// the converter made ([`FOUND`]) that no screen names, then palette
/// entries it named after a screen (`<screen>.<n>`) that no component
/// names. The design system's own entries always stay.
pub(crate) fn prune(design: &mut DesignDoc, docs: &[ScreenDoc]) {
    let used: std::collections::HashSet<&str> = docs
        .iter()
        .flat_map(|d| d.widgets.values().filter_map(|w| w.component.as_deref()))
        .collect();
    design
        .components
        .retain(|n, c| !c.role.ends_with(FOUND) || used.contains(n.as_str()));
    let colours: std::collections::HashSet<String> =
        design.components.values().filter_map(|c| c.colour.clone()).collect();
    let screens_own = |n: &str| {
        crate::ui_kit::screens::NAMES
            .iter()
            .any(|s| n.strip_prefix(s).and_then(|r| r.strip_prefix('.')).is_some_and(|r| r.parse::<usize>().is_ok()))
    };
    design.palette.retain(|n, _| !screens_own(n) || colours.contains(n));
}

/// The design doc as its file (the converter's form: no comments).
pub fn design_to_yaml(d: &DesignDoc) -> String {
    use std::fmt::Write;
    let q = super::emit::quoted;
    let mut out = String::new();
    out.push_str("palette:\n");
    for (n, c) in &d.palette {
        let _ = writeln!(out, "  {n}: [{}, {}, {}]", c[0], c[1], c[2]);
    }
    out.push_str("fonts:\n");
    for (n, f) in &d.fonts {
        let _ = writeln!(out, "  {n}: {f}");
    }
    out.push_str("spacing:\n");
    for (n, v) in &d.spacing {
        let _ = writeln!(out, "  {n}: {v}");
    }
    out.push_str("alpha:\n");
    for (n, v) in &d.alpha {
        let _ = writeln!(out, "  {n}: {v}");
    }
    let _ = writeln!(
        out,
        "legend:\n  glyph_gap: {}\n  prompt_gap: {}\n  words: {}",
        d.legend.glyph_gap, d.legend.prompt_gap, d.legend.words
    );
    out.push_str("components:\n");
    for (n, c) in &d.components {
        let _ = writeln!(out, "  {n}:");
        let _ = writeln!(out, "    role: {}", q(&c.role));
        for (k, v) in [("font", &c.font), ("colour", &c.colour), ("justify", &c.justify)] {
            if let Some(v) = v {
                let _ = writeln!(out, "    {k}: {v}");
            }
        }
        if let Some([x, y]) = c.at {
            let _ = writeln!(out, "    at: [{x}, {y}]");
        }
        if let Some([w, h]) = c.size {
            let _ = writeln!(out, "    size: [{w}, {h}]");
        }
        if !c.variants.is_empty() {
            let _ = writeln!(out, "    variants: [{}]", c.variants.join(", "));
        }
        if let Some(s) = &c.specimen {
            let _ = writeln!(out, "    specimen: {s}");
        }
        if let Some(p) = &c.pattern {
            let _ = writeln!(out, "    pattern: {p}");
        }
        if !c.ids.is_empty() {
            let ids: Vec<String> = c.ids.iter().map(|i| q(i)).collect();
            let _ = writeln!(out, "    ids: [{}]", ids.join(", "));
        }
    }
    out
}
