//! The eleven checks (docs/game/hud-kit.md, "The preview and the checks"),
//! over each state and screen mode of a HUD; each finding names the check,
//! the elements and, where it has one, the state and mode.

use anyhow::Result;
use hud_edit::Mode;
use hud_edit::model::WidgetKind;
use hud_edit::place::Rect;
use hud_edit::render::{self, Cover};
use serde_json::{Value as Json, json};

use crate::compile::{self, AsHudEdit, Compiled};
use crate::model::{Describe, Hud, Kind, NAVPOINT_FONT, Style, Values, WORLD_FONT};
use crate::preview::{self, MESSAGE_FONT, Shot, mode_name};
use crate::rects::{self, FRAME_SLACK, RESERVED, TRAINING_PAD};
use crate::source::Source;

/// The checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Check {
    /// Every text's `longest`, and every state's text, fits between its
    /// point and the next element or the frame edge.
    Fit,
    /// Every character is in the element's font.
    Glyphs,
    /// Every rectangle is inside the HUD frame (within
    /// [`FRAME_SLACK`]) and the screen.
    Frame,
    /// No element's opaque pixels meet a retail widget's at any HUD state
    /// of the sheet, and no widget element covers a reserved rectangle.
    Retail,
    /// No two elements of any registered HUDs overlap, texts included.
    Own,
    /// Every state has a value for every element and for nothing else.
    OneLayout,
    /// Every `Art` and the text template resolve on the map.
    Templates,
    /// A prompt, message or training text has at most two lines and fits
    /// the frame's width less the box.
    Prompt,
    /// An element with no split-screen offset is hidden there, and the
    /// widgets of one HUD show in the same screen modes.
    Modes,
    /// A text shown in a state, in a mode it shows in, puts opaque pixels
    /// on the screen (a transparent colour or a missing shader draws
    /// nothing).
    Drawn,
    /// A world mark a state shows has a sample pose without a fault,
    /// draws lines there and stays in the frame; a pose names a world
    /// mark; the go green is on a world mark alone.
    World,
}

impl Check {
    pub const ALL: [Check; 11] = [
        Check::Fit,
        Check::Glyphs,
        Check::Frame,
        Check::Retail,
        Check::Own,
        Check::OneLayout,
        Check::Templates,
        Check::Prompt,
        Check::Modes,
        Check::Drawn,
        Check::World,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Check::Fit => "fit",
            Check::Glyphs => "glyphs",
            Check::Frame => "frame",
            Check::Retail => "retail",
            Check::Own => "own",
            Check::OneLayout => "one layout",
            Check::Templates => "templates",
            Check::Prompt => "prompt",
            Check::Modes => "modes",
            Check::Drawn => "drawn",
            Check::World => "world",
        }
    }
}

/// One finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub check: Check,
    pub hud: &'static str,
    pub state: Option<String>,
    pub mode: Option<Mode>,
    pub elements: Vec<String>,
    pub text: String,
}

impl Finding {
    pub fn json(&self) -> Json {
        json!({
            "check": self.check.name(),
            "hud": self.hud,
            "state": self.state,
            "mode": self.mode.map(mode_name),
            "elements": self.elements,
            "text": self.text,
        })
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.check.name(), self.hud)?;
        if let Some(s) = &self.state {
            write!(f, " {s}")?;
        }
        if let Some(m) = self.mode {
            write!(f, " ({})", mode_name(m))?;
        }
        write!(f, ": {}", self.text)
    }
}

/// What the checks found, what they could not run, and the pictures they
/// drew.
#[derive(Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub skipped: Vec<(Check, String)>,
    pub notes: Vec<String>,
    pub shots: Vec<Shot>,
}

impl Report {
    pub fn of(&self, c: Check) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.check == c)
    }
    fn push(&mut self, f: Finding) {
        if !self.findings.iter().any(|o| {
            o.check == f.check && o.hud == f.hud && o.elements == f.elements && o.text == f.text
        }) {
            self.findings.push(f);
        }
    }
    fn skip(&mut self, c: Check, why: String) {
        if !self.skipped.iter().any(|(x, _)| *x == c) {
            self.skipped.push((c, why));
        }
    }
    pub fn json(&self) -> Json {
        json!({
            "findings": self.findings.iter().map(Finding::json).collect::<Vec<_>>(),
            "skipped": self.skipped.iter().map(|(c, why)| json!({"check": c.name(), "why": why})).collect::<Vec<_>>(),
            "notes": self.notes,
        })
    }
}

fn show(r: &Rect) -> String {
    let [x0, y0, x1, y1] = r.pixels();
    format!("x {x0}..{x1}, y {y0}..{y1}")
}

fn grown(r: Rect, by: f32) -> Rect {
    Rect {
        x0: r.x0 - by,
        y0: r.y0 - by,
        x1: r.x1 + by,
        y1: r.y1 + by,
    }
}

/// The retail widgets' opaque pixels over every HUD state of the sheet,
/// by widget name, in `mode`.
fn retail_covers(src: &mut Source, map_hud: &hud_edit::Hud, mode: Mode) -> Vec<(String, Cover)> {
    let mut out: Vec<(String, Cover)> = Vec::new();
    for st in preview::retail_states(mode, src.words) {
        let p = render::draw(
            &mut src.map,
            &mut src.art,
            src.fonts.as_ref(),
            &[map_hud],
            &st,
        );
        for w in p.placed.iter().filter(|w| w.drawn) {
            let Some(c) = &w.cover else { continue };
            match out.iter_mut().find(|(n, _)| *n == w.name) {
                Some((_, all)) => all.add(c),
                None => out.push((w.name.clone(), c.clone())),
            }
        }
    }
    out
}

/// The units of `text` the glyph check looks up (line breaks are not
/// drawn).
fn units(text: &str) -> impl Iterator<Item = (char, u16)> + '_ {
    text.chars().filter(|c| *c != '\n').flat_map(|c| {
        let mut b = [0u16; 2];
        let n = c.encode_utf16(&mut b).len();
        (0..n).map(move |i| (c, b[i]))
    })
}

/// Whether engine font `font` draws UTF-16 unit `unit` (`None`: no such
/// font): `hud_edit::render::Fonts::has_glyph` on the disc.
pub type GlyphLookup<'a> = &'a dyn Fn(i16, u16) -> Option<bool>;

fn glyphs_in(
    r: &mut Report,
    has: GlyphLookup,
    hud: &'static str,
    state: Option<&str>,
    id: &str,
    font: i16,
    text: &str,
) {
    for (c, u) in units(text) {
        let text = match has(font, u) {
            Some(true) => continue,
            Some(false) => format!("{id}: {c:?} (U+{u:04X}) is not in font {font}"),
            None => format!("{id}: font {font} is not on the disc"),
        };
        r.push(Finding {
            check: Check::Glyphs,
            hud,
            state: state.map(str::to_owned),
            mode: None,
            elements: vec![id.to_owned()],
            text,
        });
    }
}

/// The glyph check: every character of every text's longest and of every
/// state's text, in each font the element is drawn in (a channel's in
/// font 6 and, split screen, 5; a navpoint's number in [`NAVPOINT_FONT`];
/// a world mark's label in [`WORLD_FONT`]).
pub fn glyphs(r: &mut Report, hud: &Hud, states: &[(&'static str, Values)], has: GlyphLookup) {
    for e in &hud.elements {
        let fs: Vec<i16> = match &e.kind {
            Kind::Text { font, .. } => vec![font.engine()],
            Kind::Navpoint { .. } => vec![NAVPOINT_FONT.engine()],
            Kind::WorldMark { .. } => vec![WORLD_FONT.engine()],
            k if k.is_channel() => MESSAGE_FONT.to_vec(),
            _ => continue,
        };
        for f in fs {
            glyphs_in(
                r,
                has,
                hud.name,
                None,
                e.id,
                f,
                e.kind.longest().unwrap_or(""),
            );
            for (s, v) in states {
                if let Some(v) = v.get(e.id).filter(|v| v.shown) {
                    glyphs_in(r, has, hud.name, Some(*s), e.id, f, &v.text);
                }
            }
        }
    }
}

/// The text a channel may take: two lines for a message or the training
/// box, one for the prompt (the game's prompt line draws no line break, so
/// a second line runs into the first), as wide as the frame less the box
/// (the training box's margin; the prompt line runs from its start to the
/// frame's right edge).
fn prompt_fault(kind: &Kind, frame: &hud_edit::place::Frame, lines: &[[f32; 2]]) -> Option<String> {
    let most = if matches!(kind, Kind::Prompt { .. }) { 1 } else { 2 };
    if lines.len() > most {
        return Some(format!("{} lines, at most {most}", lines.len()));
    }
    let limit = match kind {
        Kind::Training { .. } => f32::from(frame.right - frame.left) - 2.0 * TRAINING_PAD[0],
        _ => f32::from(frame.right - frame.left),
    };
    let wide = lines.iter().map(|s| s[0]).fold(0.0, f32::max);
    (wide > limit).then(|| format!("{wide} pixels wide, over {limit}"))
}

/// Checks one HUD over its states in `modes`, keeping the pictures.
pub fn hud(src: &mut Source, d: &dyn Describe, modes: &[Mode]) -> Result<Report> {
    let hud = d.hud();
    let states = d.states();
    let mut r = Report::default();
    let name = hud.name;
    let finding = |check, state: Option<&str>, mode, elements: Vec<String>, text: String| Finding {
        check,
        hud: name,
        state: state.map(str::to_owned),
        mode,
        elements,
        text,
    };
    // One layout.
    for p in hud.problems() {
        r.push(finding(Check::OneLayout, None, None, Vec::new(), p));
    }
    for (s, v) in &states {
        for p in v.layout_problems(&hud) {
            r.push(finding(Check::OneLayout, Some(*s), None, Vec::new(), p));
        }
    }
    // World: the sample poses and the go green, before anything is
    // drawn.
    for (s, v) in &states {
        for (id, pose) in d.world(s) {
            let fault = match hud.element(id) {
                None => Some(format!("a pose for {id}, which is not an element")),
                Some(e) if e.kind.world().is_none() => {
                    Some(format!("a pose for {id}, which is a {}", e.kind.name()))
                }
                Some(_) => pose.fault().map(|why| format!("{id}'s pose has {why}")),
            };
            if let Some(text) = fault {
                r.push(finding(Check::World, Some(*s), None, vec![id.to_owned()], text));
            }
        }
        for e in hud.elements.iter().filter(|e| e.kind.world().is_none()) {
            if v.get(e.id).is_some_and(|x| x.shown && x.style == Style::Go) {
                r.push(finding(
                    Check::World,
                    Some(*s),
                    None,
                    vec![e.id.to_owned()],
                    format!("{} is a {} in the go green, a world mark's alone", e.id, e.kind.name()),
                ));
            }
        }
    }
    // Templates.
    let t = src.templates(&hud);
    for m in t.missing(&hud) {
        r.push(finding(Check::Templates, None, None, Vec::new(), m));
    }
    if r.of(Check::Templates).next().is_some() || !hud.problems().is_empty() {
        r.notes.push(format!(
            "{name}: not drawn; its templates or ids are missing"
        ));
        return Ok(r);
    }
    let compiled = compile::compile(&hud, &t)?;
    modes_static(&mut r, &hud, &compiled);
    // Glyphs.
    match src.fonts.as_ref() {
        None => r.skip(
            Check::Glyphs,
            "no fonts: the synthetic source's fixed-advance stand-in".into(),
        ),
        Some(fonts) => glyphs(&mut r, &hud, &states, &|f, u| fonts.has_glyph(f, u)),
    }
    if src.synthetic {
        r.notes
            .push("retail: against the synthetic map's flat boxes, not the disc's art".into());
    }
    r.notes.push(src.words_note.clone());
    let retail = hud_edit::model::read(&mut src.map, src.player)?;
    for &mode in modes {
        let covers = retail_covers(src, &retail, mode);
        let st = preview::retail_states(mode, src.words).remove(0);
        for (s, v) in &states {
            let shot = preview::shot(src, &hud, &compiled, s, v, &d.world(s), &st)?;
            per_shot(&mut r, src, &hud, v, &shot, &covers);
            r.shots.push(shot);
        }
    }
    Ok(r)
}

/// The modes rule on the compiled bytes: a text hidden in a mode has no
/// font there, and the widgets of the HUD agree on the modes they show in.
fn modes_static(r: &mut Report, hud: &Hud, c: &Compiled) {
    let back = c.as_hud_edit(preview::KIT_BASE);
    for m in [Mode::Half, Mode::Quarter] {
        let widgets: Vec<_> = hud
            .elements
            .iter()
            .filter(|e| !e.kind.is_channel())
            .collect();
        let (shown, hidden): (Vec<_>, Vec<_>) =
            widgets.iter().partition(|e| e.at.offset(m).is_some());
        if !shown.is_empty() && !hidden.is_empty() {
            let ids = |v: &[&&crate::model::Element]| {
                v.iter().map(|e| e.id.to_owned()).collect::<Vec<_>>()
            };
            r.push(Finding {
                check: Check::Modes,
                hud: hud.name,
                state: None,
                mode: Some(m),
                elements: ids(&hidden),
                text: format!(
                    "{} have no {} offset while {} do: the block would be drawn in part",
                    ids(&hidden).join(", "),
                    m.label(),
                    ids(&shown).join(", ")
                ),
            });
        }
        for w in back
            .widgets
            .iter()
            .filter(|w| w.key.kind == WidgetKind::Text)
        {
            let Some(e) = hud.element(&w.name) else {
                continue;
            };
            if e.at.offset(m).is_none() && hud_edit::layout::text_font(w.font(m)).is_some() {
                r.push(Finding {
                    check: Check::Modes,
                    hud: hud.name,
                    state: None,
                    mode: Some(m),
                    elements: vec![e.id.to_owned()],
                    text: format!(
                        "{} has no {} offset but a font there: the game would draw it",
                        e.id,
                        m.label()
                    ),
                });
            }
        }
    }
}

fn per_shot(
    r: &mut Report,
    src: &Source,
    hud: &Hud,
    values: &Values,
    s: &Shot,
    retail: &[(String, Cover)],
) {
    let state = Some(s.state.clone());
    let mode = Some(s.mode);
    let f = |check, elements: Vec<String>, text: String| Finding {
        check,
        hud: hud.name,
        state: state.clone(),
        mode,
        elements,
        text,
    };
    let screen = rects::screen();
    let frame = grown(s.frame.rect(), FRAME_SLACK);
    let fonts = src.fonts.as_ref();
    // The rectangle an element claims: its reserved area and where it
    // landed.
    let claims = |d: &preview::Drawn| d.reserved.into_iter().chain(d.rect).collect::<Vec<_>>();
    // `x` is drawn on `y` by design ([`crate::model::Element::over`]).
    let over = |x: &str, y: &str| hud.element(x).is_some_and(|e| e.over == Some(y));
    for d in &s.elements {
        let Some(e) = hud.element(d.id) else { continue };
        // Modes: hidden in a mode it has no offset for.
        if e.at.offset(s.mode).is_none() && d.drawn {
            r.push(f(
                Check::Modes,
                vec![d.id.into()],
                format!("{} is drawn with no {} offset", d.id, s.mode.label()),
            ));
        }
        // World: a mark shown here lies somewhere, draws lines and stays
        // in the frame.
        if let Some(w) = d.world.as_ref().filter(|_| d.shown && e.at.offset(s.mode).is_some()) {
            let fault = match (w.pose, w.bounds) {
                (None, _) => Some(format!("{} is shown with no sample pose", d.id)),
                (Some(_), _) if w.edges == 0 => {
                    Some(format!("{} is shown and its {} draws no lines", d.id, w.shape.name()))
                }
                (Some(_), Some(b)) if !frame.contains(&b) => {
                    Some(format!("{}'s sample leaves the HUD frame ({})", d.id, show(&b)))
                }
                _ => None,
            };
            if let Some(text) = fault {
                r.push(f(Check::World, vec![d.id.into()], text));
            }
        }
        // Frame.
        for c in claims(d) {
            if !screen.contains(&c) {
                r.push(f(
                    Check::Frame,
                    vec![d.id.into()],
                    format!("{} runs off the screen ({})", d.id, show(&c)),
                ));
            } else if !frame.contains(&c)
                && !grown(s.frame.rect(), f32::from(e.outside).max(FRAME_SLACK)).contains(&c)
            {
                let text = match e.outside {
                    0 => format!("{} leaves the HUD frame ({})", d.id, show(&c)),
                    by => format!(
                        "{} is more than its {by} outside the HUD frame ({})",
                        d.id,
                        show(&c)
                    ),
                };
                r.push(f(Check::Frame, vec![d.id.into()], text));
            }
        }
        // Fit: a text against its longest and its neighbours.
        if let Kind::Text { font, longest, .. } = &e.kind {
            let lw = render::text_size(fonts, font.engine(), longest)[0];
            if let Some(v) = values.get(e.id).filter(|v| v.shown) {
                let w = render::text_size(fonts, font.engine(), &v.text)[0];
                if w > lw + 0.5 {
                    r.push(f(
                        Check::Fit,
                        vec![d.id.into()],
                        format!(
                            "{}'s {:?} is {w} pixels, wider than its longest {:?} ({lw})",
                            d.id, v.text, longest
                        ),
                    ));
                }
            }
            if let Some(res) = d.reserved {
                if res.x0 < frame.x0 || res.x1 > frame.x1 {
                    r.push(f(
                        Check::Fit,
                        vec![d.id.into()],
                        format!(
                            "{}'s longest runs past the frame's edge ({})",
                            d.id,
                            show(&res)
                        ),
                    ));
                }
                for o in s
                    .elements
                    .iter()
                    .filter(|o| o.id != d.id && !over(d.id, o.id) && !over(o.id, d.id))
                {
                    if let Some(x) = claims(o).iter().find_map(|c| c.intersect(&res)) {
                        r.push(f(
                            Check::Fit,
                            vec![d.id.into(), o.id.into()],
                            format!("{}'s longest runs into {} ({})", d.id, o.id, show(&x)),
                        ));
                    }
                }
            }
        }
        // Drawn: a text shown here puts opaque pixels down.
        let is_text = matches!(e.kind, Kind::Text { .. }) || e.kind.is_channel();
        let text = d
            .text
            .as_deref()
            .filter(|t| t.chars().any(|c| !c.is_whitespace()));
        if let Some(t) = text
            && is_text
            && d.shown
            && e.at.offset(s.mode).is_some()
            && (!d.drawn || d.cover.as_ref().is_none_or(Cover::is_empty))
        {
            let why = if d.notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", d.notes.join("; "))
            };
            r.push(f(
                Check::Drawn,
                vec![d.id.into()],
                format!("{} shows {t:?} but draws no pixels{why}", d.id),
            ));
        }
        // Prompt: in the modes the channel shows in.
        if e.kind.is_lines() && e.at.offset(s.mode).is_some() {
            let font = d.font.unwrap_or(MESSAGE_FONT[0]);
            let mut texts = vec![(
                "its longest".to_owned(),
                e.kind.longest().unwrap_or("").to_owned(),
            )];
            if let Some(t) = &d.text {
                texts.push(("its text".to_owned(), t.clone()));
            }
            for (what, t) in texts {
                // A notification is one line the game copies 63 units of.
                if matches!(e.kind, Kind::Message { .. })
                    && let Some(why) = crate::model::message_fault(&t)
                {
                    r.push(f(
                        Check::Prompt,
                        vec![d.id.into()],
                        format!("{}: {what} is {why}", d.id),
                    ));
                }
                if let Some(why) =
                    prompt_fault(&e.kind, &s.frame, &preview::line_sizes(fonts, font, &t))
                {
                    r.push(f(
                        Check::Prompt,
                        vec![d.id.into()],
                        format!("{}: {what} is {why}", d.id),
                    ));
                }
            }
        }
        // Retail: widget elements only (a channel is the game's own text).
        if !e.kind.is_channel() {
            if let Some(c) = d.cover.as_ref().filter(|_| d.drawn) {
                for (name, rc) in retail {
                    let n = c.overlap(rc);
                    if n > 0 {
                        r.push(f(
                            Check::Retail,
                            vec![d.id.into(), name.clone()],
                            format!("{} meets the retail {name} ({n} opaque pixels)", d.id),
                        ));
                    }
                }
            }
            for c in claims(d) {
                for (what, rr) in RESERVED {
                    if let Some(x) = c.intersect(&rr) {
                        r.push(f(
                            Check::Retail,
                            vec![d.id.into()],
                            format!("{} covers {what} ({})", d.id, show(&x)),
                        ));
                    }
                }
            }
        }
    }
    // Own: no two elements overlap, by their claims and their pixels (two
    // channels are the game's, one at a time; an element drawn over
    // another by design, as a count on its pill, is not compared with it).
    for (i, a) in s.elements.iter().enumerate() {
        for b in &s.elements[i + 1..] {
            // A world mark lies in the world, under the HUD.
            if (a.channel && b.channel)
                || a.world.is_some()
                || b.world.is_some()
                || over(a.id, b.id)
                || over(b.id, a.id)
            {
                continue;
            }
            let ids = vec![a.id.to_owned(), b.id.to_owned()];
            if let (Some(x), Some(y)) = (&a.cover, &b.cover)
                && a.drawn
                && b.drawn
                && x.overlap(y) > 0
            {
                r.push(f(
                    Check::Own,
                    ids.clone(),
                    format!("{} and {} share {} pixels", a.id, b.id, x.overlap(y)),
                ));
            }
            let hit = claims(a)
                .iter()
                .find_map(|x| claims(b).iter().find_map(|y| x.intersect(y)));
            if let Some(x) = hit {
                r.push(f(
                    Check::Own,
                    ids,
                    format!("{} and {} overlap ({})", a.id, b.id, show(&x)),
                ));
            }
        }
    }
}

/// Checks every HUD of `ds`, and that no two of them overlap.
pub fn all(src: &mut Source, ds: &[&dyn Describe], modes: &[Mode]) -> Result<Report> {
    let mut out = Report::default();
    for d in ds {
        let r = hud(src, *d, modes)?;
        for f in r.findings {
            out.push(f);
        }
        for (c, why) in r.skipped {
            out.skip(c, why);
        }
        out.notes.extend(r.notes);
        out.shots.extend(r.shots);
    }
    // Across HUDs: the areas each claims in any state, per mode.
    let mut claimed: Vec<(&'static str, Mode, &'static str, bool, Rect)> = Vec::new();
    for s in &out.shots {
        for d in &s.elements {
            for c in d.reserved.into_iter().chain(d.rect) {
                claimed.push((s.hud, s.mode, d.id, d.channel, c));
            }
        }
    }
    let mut found = Vec::new();
    for (i, a) in claimed.iter().enumerate() {
        for b in &claimed[i + 1..] {
            if a.0 == b.0 || a.1 != b.1 || (a.3 && b.3) {
                continue;
            }
            if let Some(x) = a.4.intersect(&b.4) {
                found.push(Finding {
                    check: Check::Own,
                    hud: a.0,
                    state: None,
                    mode: Some(a.1),
                    elements: vec![format!("{}:{}", a.0, a.2), format!("{}:{}", b.0, b.2)],
                    text: format!(
                        "{} of {} and {} of {} overlap ({})",
                        a.2,
                        a.0,
                        b.2,
                        b.0,
                        show(&x)
                    ),
                });
            }
        }
    }
    for f in found {
        out.push(f);
    }
    Ok(out)
}
