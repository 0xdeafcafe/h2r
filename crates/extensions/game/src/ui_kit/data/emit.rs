//! A screen doc written back as its file: the canonical form the files in
//! `content/ui/screens/` are in, so a load and a save with no edit gives the
//! same bytes (docs/game/ui-builder.md "Screens in YAML").
//!
//! The form: keys in [`super::ScreenDoc`]'s order, two-space indents, a
//! section left out when it is empty and a field when it is unset, names
//! (screen, kind, layout, ids, glyphs) plain, every text double-quoted,
//! numbers and pairs in flow (`[12, -40]`), prompts as
//! `[[A, "SELECT"], [[Left, Right], "CHANGE"]]`, art as
//! `{ screen: 11, pane: 0, index: 1, image: 0 }`, a settings dialog's
//! actions one flow map a line
//! (`- { button: X, words: "CLEAR FILTERS", bind: browser.clear }`). No
//! comments: a save can't keep them.

use std::fmt::Write;

use super::{Glyphs, PropDoc, RowDoc, ScreenDoc, SettingsDoc, WidgetDoc};

/// `s` double-quoted, YAML-escaped.
pub(super) fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A key as written: plain when YAML reads it as itself, else quoted.
fn key(s: &str) -> String {
    if super::plain(s) { s.to_string() } else { quoted(s) }
}

fn pair<T: std::fmt::Display>(v: [T; 2]) -> String {
    format!("[{}, {}]", v[0], v[1])
}

fn widget(out: &mut String, id: &str, w: &WidgetDoc) {
    let _ = writeln!(out, "  {}:", key(id));
    if let Some(c) = &w.component {
        let _ = writeln!(out, "    component: {}", key(c));
    }
    if let Some(t) = &w.text {
        let _ = writeln!(out, "    text: {}", quoted(t));
    }
    if let Some(a) = w.at {
        let _ = writeln!(out, "    at: {}", pair(a));
    }
    if let Some(s) = w.size {
        let _ = writeln!(out, "    size: {}", pair(s));
    }
    if let Some(ps) = &w.prompts {
        let items: Vec<String> = ps
            .iter()
            .map(|p| {
                let g = match &p.0 {
                    Glyphs::One(g) => g.clone(),
                    Glyphs::Many(gs) => format!("[{}]", gs.join(", ")),
                };
                format!("[{g}, {}]", quoted(&p.1))
            })
            .collect();
        let _ = writeln!(out, "    prompts: [{}]", items.join(", "));
    }
    if let Some(d) = &w.digits {
        let _ = writeln!(out, "    digits: {}", quoted(d));
    }
    if let Some(g) = w.gap_after {
        let _ = writeln!(out, "    gap_after: {g}");
    }
    if let Some(a) = w.art {
        let _ = writeln!(
            out,
            "    art: {{ screen: {}, pane: {}, index: {}, image: {} }}",
            a.screen, a.pane, a.index, a.image
        );
    }
}

fn row(out: &mut String, id: &str, r: &RowDoc) {
    let _ = writeln!(out, "  {}:", key(id));
    if !r.kind.is_empty() {
        let _ = writeln!(out, "    kind: {}", r.kind);
    }
    let _ = writeln!(out, "    text: {}", quoted(&r.text));
    if let Some(v) = &r.value {
        let _ = writeln!(out, "    value: {}", quoted(v));
    }
    if let Some(vs) = &r.values {
        let items: Vec<String> = vs.iter().map(|v| quoted(v)).collect();
        let _ = writeln!(out, "    values: [{}]", items.join(", "));
    }
    if let Some(c) = r.current {
        let _ = writeln!(out, "    current: {c}");
    }
    if let Some(h) = &r.help {
        let _ = writeln!(out, "    help: {}", quoted(h));
    }
    if let Some(k) = &r.setting {
        let _ = writeln!(out, "    setting: {}", key(k));
    }
    if let Some(b) = &r.bind {
        let _ = writeln!(out, "    bind: {}", key(b));
    }
    if let Some(e) = &r.explain {
        let _ = writeln!(out, "    explain: {}", quoted(e));
    }
    if let Some(o) = &r.open {
        let _ = writeln!(out, "    open: {}", key(o));
    }
    if let Some(m) = r.max_chars {
        let _ = writeln!(out, "    max_chars: {m}");
    }
}

fn prop_row(out: &mut String, id: &str, p: &PropDoc) {
    let _ = writeln!(out, "  {}:", key(id));
    let _ = writeln!(out, "    text: {}", quoted(&p.text));
    if let Some(h) = &p.help {
        let _ = writeln!(out, "    help: {}", quoted(h));
    }
    if !p.on.is_empty() {
        let on: Vec<String> = p.on.iter().map(|k| key(k)).collect();
        let _ = writeln!(out, "    on: [{}]", on.join(", "));
    }
    let _ = writeln!(out, "    prop: {}", key(&p.prop));
    let _ = writeln!(out, "    open: {}", key(&p.open));
    if let Some(vs) = &p.values {
        let items: Vec<String> = vs.iter().map(|v| quoted(v)).collect();
        let _ = writeln!(out, "    values: [{}]", items.join(", "));
    }
}

fn settings(out: &mut String, st: &SettingsDoc) {
    out.push_str("settings:\n");
    let _ = writeln!(out, "  host: {}", key(&st.host));
    if let Some(i) = &st.icon {
        let _ = writeln!(out, "  icon: {}", key(i));
    }
    if let Some(n) = &st.name {
        let _ = writeln!(out, "  name: {}", quoted(n));
    }
    if let Some(a) = &st.about {
        let _ = writeln!(out, "  about: {}", quoted(a));
    }
    if let Some(c) = &st.caller {
        out.push_str("  caller:\n");
        let _ = writeln!(out, "    label: {}", quoted(&c.label));
        let _ = writeln!(out, "    help: {}", quoted(&c.help));
    }
    if !st.actions.is_empty() {
        out.push_str("  actions:\n");
        for a in &st.actions {
            let _ = writeln!(
                out,
                "    - {{ button: {}, words: {}, bind: {} }}",
                a.button,
                quoted(&a.words),
                key(&a.bind)
            );
        }
    }
}

/// `doc` as its file.
pub fn to_yaml(doc: &ScreenDoc) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "screen: {}", doc.screen);
    let _ = writeln!(out, "kind: {}", doc.kind);
    for (key, value) in [
        ("layout", &doc.layout),
        ("backing", &doc.backing),
        ("opener", &doc.opener),
    ] {
        if let Some(v) = value {
            let _ = writeln!(out, "{key}: {v}");
        }
    }
    if doc.scrolls {
        out.push_str("scrolls: true\n");
    }
    if !doc.states.is_empty() {
        out.push_str("states:\n");
        for (name, s) in &doc.states {
            let _ = writeln!(out, "  {}:", key(name));
            let _ = writeln!(out, "    focus: {}", s.focus);
            if let Some(c) = s.cursor {
                let _ = writeln!(out, "    cursor: {c}");
            }
            if let Some(p) = s.slide {
                let _ = writeln!(out, "    slide: {p}");
            }
            if let Some(p) = s.point {
                let _ = writeln!(out, "    point: {}", pair(p));
            }
        }
    }
    if let Some(st) = &doc.settings {
        settings(&mut out, st);
    }
    if !doc.widgets.is_empty() {
        out.push_str("widgets:\n");
        for (id, w) in &doc.widgets {
            widget(&mut out, id, w);
        }
    }
    if !doc.rows.is_empty() {
        out.push_str("rows:\n");
        for (id, r) in &doc.rows {
            row(&mut out, id, r);
        }
    }
    if !doc.props.is_empty() {
        out.push_str("props:\n");
        for (id, p) in &doc.props {
            prop_row(&mut out, id, p);
        }
    }
    if let Some(c) = &doc.cards {
        out.push_str("cards:\n");
        let _ = writeln!(out, "  layout: {}", key(&c.layout));
        if let Some(b) = &c.bind {
            let _ = writeln!(out, "  bind: {}", key(b));
        }
        if let Some(o) = &c.open {
            let _ = writeln!(out, "  open: {}", key(o));
        }
        out.push_str("  items:\n");
        for (id, card) in &c.items {
            let _ = writeln!(out, "    {}:", key(id));
            let _ = writeln!(out, "      name: {}", quoted(&card.name));
            if !card.stats.is_empty() {
                let _ = writeln!(out, "      stats: {}", quoted(&card.stats));
            }
            if let Some(l) = &card.level {
                let _ = writeln!(out, "      level: {}", key(l));
            }
            if let Some(h) = &card.help {
                let _ = writeln!(out, "      help: {}", quoted(h));
            }
            if let Some(o) = &card.open {
                let _ = writeln!(out, "      open: {}", key(o));
            }
        }
    }
    if let Some(f) = &doc.floating {
        out.push_str("floating:\n");
        let _ = writeln!(out, "  anchor: {}", key(&f.anchor));
        if let Some(o) = f.offset {
            let _ = writeln!(out, "  offset: {}", pair(o));
        }
        if f.pinned {
            out.push_str("  pinned: true\n");
        }
        out.push_str("  actions:\n");
        for (id, a) in &f.actions {
            let _ = writeln!(out, "    {}:", key(id));
            let _ = writeln!(out, "      text: {}", quoted(&a.text));
            if let Some(act) = &a.act {
                let _ = writeln!(out, "      act: {}", key(act));
            }
            if let Some(o) = &a.open {
                let _ = writeln!(out, "      open: {}", key(o));
            }
        }
    }
    out
}
