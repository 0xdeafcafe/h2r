//! Elements to `nhdt` widget records (docs/game/hud-kit.md, "The model"):
//! a `0x64`-byte bitmap widget per meter, icon and pointer, a `0x54`-byte
//! text widget per text, built through the `nhdt` definition
//! (`tags::defs`) as `h1objects::h1hud` builds its widgets, with the
//! [`Templates`]' datums written in. The same bytes go to the game and to
//! the preview.
//!
//! What the bytes leave to the runtime's hooks: the four show words are
//! zero, so the retail gate (E `0x0013ac87`) never draws a kit widget by
//! itself; the inputs are [`KIT_INPUT`] (the hook answers `hud_input1`
//! with the fill); a text's string is the template's (the hook answers
//! the text). A text hidden in a screen mode has the font byte
//! [`NO_FONT`] there, so the game's own draw skips it (E `0x0022a85b`).

use anyhow::{Context as _, Result, bail};
use hud_edit::Mode;
use hud_edit::layout;
use hud_edit::model::{self as hm, WidgetKey, WidgetKind};
use map::build::{Block, FixupKind};
use tags::defs::{self, Defs, Record, Str, StructDef, Val};

use crate::model::{Element, Hud, Kind, Origin, Place};
use crate::templates::{BITM, Templates};

/// The input byte of every kit widget: input 0, "zero" (E `0x0013ad48`).
pub const KIT_INPUT: u8 = 0;
/// The font byte of a text in a mode it is hidden in: no font, no text.
pub const NO_FONT: i8 = -1;

/// One compiled widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: &'static str,
    pub kind: WidgetKind,
    pub bytes: Vec<u8>,
}

/// A HUD compiled: its widgets in the game's draw order (bitmaps, then
/// texts, each in element order: E `0x0013a532`), and its channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub hud: &'static str,
    pub widgets: Vec<Widget>,
    pub channels: Vec<&'static str>,
}

impl Compiled {
    pub fn widget(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }
    pub fn of(&self, kind: WidgetKind) -> impl Iterator<Item = &Widget> {
        self.widgets.iter().filter(move |w| w.kind == kind)
    }
    /// The widget at `key` (its kind and index in its block).
    pub fn at(&self, key: WidgetKey) -> Option<&Widget> {
        self.of(key.kind).nth(key.index)
    }

    /// The widgets read back as `hud_edit` reads a map's, at metadata
    /// address `base` (bitmaps, then texts), named by their element ids.
    pub fn as_hud_edit(&self, base: u32) -> hm::Hud {
        let mut widgets = Vec::new();
        let mut address = base;
        for kind in [WidgetKind::Bitmap, WidgetKind::Text] {
            for (index, w) in self.of(kind).enumerate() {
                widgets.push(hm::Widget {
                    key: WidgetKey { kind, index },
                    address,
                    bytes: w.bytes.clone(),
                    original: w.bytes.clone(),
                    name: w.id.to_owned(),
                });
                address += w.bytes.len() as u32;
            }
        }
        hm::Hud {
            id: map::TagId::NULL,
            name: self.hud.to_owned(),
            widgets,
        }
    }
}

fn nhdt() -> &'static StructDef {
    Defs::builtin()
        .group(hm::NHDT)
        .expect("the nhdt definition")
}

fn set(r: &mut Record, name: &str, v: Val) -> Result<()> {
    r.set(name, v).map_err(|e| anyhow::anyhow!("{name}: {e}"))
}

fn pair(x: i16, y: i16) -> Val {
    Val::List(vec![Val::Int(x.into()), Val::Int(y.into())])
}

const MODES: [(Mode, &str); 3] = [
    (Mode::Full, "full"),
    (Mode::Half, "half"),
    (Mode::Quarter, "quarter"),
];

/// The anchor and the offset per mode `at` gives on `t` (a hidden mode
/// keeps the fullscreen offset; the gate hides it). An offset from the
/// grenade row is added to the row's point, in the row's anchor.
pub fn placed(at: &Place, t: &Templates) -> Result<(u16, [(i16, i16); 3])> {
    let own = MODES.map(|(m, _)| at.offset(m).unwrap_or(at.full));
    if at.origin == Origin::Anchor {
        return Ok((at.anchor as u16, own));
    }
    let row = t
        .row
        .as_ref()
        .map_err(|why| anyhow::anyhow!("the grenade row: {why}"))?;
    let mut anchor = 0;
    let mut out = own;
    for (i, (m, _)) in MODES.iter().enumerate() {
        let (a, (x, y)) = row.at(at.origin, *m).expect("a row origin");
        anchor = a;
        out[i] = (x + own[i].0, y + own[i].1);
    }
    Ok((anchor, out))
}

/// The fields both widget kinds share: the inputs, the anchor and the
/// offsets ([`placed`]).
fn common(def: &StructDef, at: &Place, t: &Templates) -> Result<Record> {
    let mut r = defs::default_record(def);
    for i in 1..=4 {
        set(&mut r, &format!("input_{i}"), Val::Int(KIT_INPUT.into()))?;
    }
    let (anchor, offsets) = placed(at, t)?;
    set(&mut r, "anchor", Val::Enum(anchor.into()))?;
    for ((_, name), (x, y)) in MODES.iter().zip(offsets) {
        set(&mut r, &format!("offset_{name}"), pair(x, y))?;
    }
    Ok(r)
}

/// A built element's bytes with its tag references' datums written in
/// (the builder leaves them to a fixup).
fn bytes(def: &StructDef, r: &Record) -> Result<Vec<u8>> {
    let mut b: Block = defs::to_built(def, r, &mut |_| None).map_err(|e| anyhow::anyhow!("{e}"))?;
    for f in std::mem::take(&mut b.fixups) {
        let at = f.offset as usize;
        match f.kind {
            FixupKind::TagRef(id) => b.bytes[at + 4..at + 8].copy_from_slice(&id.0.to_le_bytes()),
            FixupKind::Block { count: 0, .. } => b.bytes[at..at + 8].fill(0),
            k => bail!("{}: a {k:?} at {at:#x} in a kit widget", def.name),
        }
    }
    if b.bytes.len() != def.size as usize {
        bail!(
            "{} is {:#x} bytes, not {:#x}",
            def.name,
            b.bytes.len(),
            def.size
        );
    }
    Ok(b.bytes)
}

fn bitmap(def: &StructDef, e: &Element, t: &Templates) -> Result<Vec<u8>> {
    let art = e.kind.art().expect("a bitmap element has art");
    let a = t
        .art(&art)
        .map_err(|why| anyhow::anyhow!("{}: {why}", e.id))?;
    let mut r = common(def, &e.at, t)?;
    set(&mut r, "bitmap", Val::tag(BITM, a.bitmap))?;
    set(&mut r, "shader", Val::tag(a.shader.0, a.shader.1))?;
    for (i, (_, name)) in MODES.iter().enumerate() {
        set(
            &mut r,
            &format!("sequence_{name}"),
            Val::Int(a.sequences[i].into()),
        )?;
        set(
            &mut r,
            &format!("registration_{name}"),
            Val::List(e.at.registration.map(Val::Float).into()),
        )?;
    }
    bytes(def, &r)
}

fn text(def: &StructDef, e: &Element, t: &Templates) -> Result<Vec<u8>> {
    let Kind::Text { font, justify, .. } = e.kind else {
        unreachable!("a text element")
    };
    let tt = t
        .text
        .as_ref()
        .map_err(|why| anyhow::anyhow!("{}: {why}", e.id))?;
    let mut r = common(def, &e.at, t)?;
    set(&mut r, "shader", Val::tag(tt.shader.0, tt.shader.1))?;
    set(&mut r, "text", Val::String(Str::Index(tt.string)))?;
    set(&mut r, "justification", Val::Enum(justify as u32))?;
    for (m, name) in MODES {
        let byte = if e.at.offset(m).is_some() {
            font.byte()
        } else {
            NO_FONT
        };
        set(&mut r, &format!("font_{name}"), Val::Int(byte.into()))?;
    }
    bytes(def, &r)
}

/// Compiles `hud` against `t`.
pub fn compile(hud: &Hud, t: &Templates) -> Result<Compiled> {
    if let Some(p) = hud.problems().first() {
        bail!("{}: {p}", hud.name);
    }
    let root = nhdt();
    let bdef = root
        .child("bitmap_widgets")
        .context("nhdt bitmap widgets")?;
    let tdef = root.child("text_widgets").context("nhdt text widgets")?;
    let mut bitmaps = Vec::new();
    let mut texts = Vec::new();
    let mut channels = Vec::new();
    for e in &hud.elements {
        match &e.kind {
            Kind::Meter { .. } | Kind::Icon { .. } | Kind::Pointer { .. } => bitmaps.push(Widget {
                id: e.id,
                kind: WidgetKind::Bitmap,
                bytes: bitmap(bdef, e, t)?,
            }),
            Kind::Text { .. } => texts.push(Widget {
                id: e.id,
                kind: WidgetKind::Text,
                bytes: text(tdef, e, t)?,
            }),
            _ => channels.push(e.id),
        }
    }
    debug_assert!(bitmaps.iter().all(|w| w.bytes.len() == layout::BITMAP_SIZE));
    debug_assert!(texts.iter().all(|w| w.bytes.len() == layout::TEXT_SIZE));
    bitmaps.extend(texts);
    Ok(Compiled {
        hud: hud.name,
        widgets: bitmaps,
        channels,
    })
}
