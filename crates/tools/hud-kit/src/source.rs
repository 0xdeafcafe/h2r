//! The two sources the preview draws from (docs/game/hud-kit.md, "The
//! preview and the checks"): [`Source::disc`] reads a map, the fonts and
//! the XBE's colour constants from the user's disc; [`Source::synthetic`]
//! uses the invented map of [`crate::synthetic`], flat boxes and the
//! renderer's fixed-advance stand-in font.

use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use assets::ResourceFiles;
use hud_edit::render::{Art, Fonts, TEXT_COLOUR};
use map::{Map, TagId};
use scenario_edit::MapSource;
use scenario_edit::save::MapReader;

use crate::hudg::{self, Hudg};
use crate::model::{Hud, Pulse};
use crate::templates::{self, Templates};

/// The colours the preview draws texts in (RGBA), and where they came
/// from. No disc colour is written in this crate: the disc source reads
/// each from the map or the XBE; the synthetic source's are invented.
#[derive(Clone, Debug, PartialEq)]
pub struct Colours {
    /// A kit text widget's (the text widget colour is not decoded,
    /// decoding 4: the HUD editor's stand-in).
    pub widget: [u8; 4],
    /// The prompt line's (`hudg +0xa4`).
    pub prompt: [u8; 4],
    /// Notifications' (the user's HUD text colour, `0x0013927e`).
    pub message: [u8; 4],
    /// The training text's (`0x001e70c3`'s pair of floats, blue 1).
    pub training: [u8; 4],
    /// Each flash record's peak colour.
    pub flash: [[u8; 4]; 4],
    /// Where each came from, and what was assumed.
    pub notes: Vec<String>,
}

impl Colours {
    pub fn flash(&self, p: Pulse) -> [u8; 4] {
        self.flash[Pulse::ALL.iter().position(|x| *x == p).unwrap()]
    }

    /// The synthetic source's invented colours.
    pub fn synthetic() -> Self {
        Colours {
            widget: TEXT_COLOUR,
            prompt: [220, 230, 255, 255],
            message: [150, 200, 255, 255],
            training: [120, 170, 255, 255],
            flash: [
                [255, 255, 255, 255],
                [255, 230, 120, 255],
                [255, 170, 60, 255],
                [255, 80, 60, 255],
            ],
            notes: vec!["synthetic colours, invented for the preview".into()],
        }
    }
}

/// The HUD text colour triples, human and Elite (E `0x0013927e`; S: three
/// floats, red first).
pub const XBE_HUD_COLOUR: [u32; 2] = [0x0046_8c84, 0x0046_8c94];
/// The training text's two floats of the first set (E `0x001e70c3`; S:
/// red then green, blue 1).
pub const XBE_TRAINING_COLOUR: [u32; 2] = [0x0045_e518, 0x0045_e51c];

/// What the preview draws from.
pub struct Source {
    /// The map's name.
    pub name: String,
    pub map: Map<MapReader>,
    pub art: Art,
    pub fonts: Option<Fonts>,
    pub colours: Colours,
    pub hudg: Option<Hudg>,
    /// The player `nhdt` drawn and joined.
    pub player: TagId,
    pub synthetic: bool,
}

impl Source {
    /// The invented map (no disc).
    pub fn synthetic() -> Result<Self> {
        let bytes: Arc<[u8]> = crate::synthetic::build()?.into();
        let source = MapSource {
            bytes: Some(bytes.clone()),
            ..MapSource::file("synthetic.map")
        };
        let mut map = source.open().context("the synthetic map")?;
        let mut files = ResourceFiles::new();
        files.set_bytes(map::ResourceFile::Local, bytes);
        let hudg = hudg::read(&mut map).ok();
        let player = templates::player_huds(&mut map)?[0];
        Ok(Source {
            name: "synthetic".into(),
            map,
            art: Art::new(files),
            fonts: None,
            colours: Colours::synthetic(),
            hudg,
            player,
            synthetic: true,
        })
    }

    /// Map `name` of the disc image `iso`.
    pub fn disc(iso: &Path, name: &str) -> Result<Self> {
        let maps = MapSource::disc_maps(iso)
            .with_context(|| format!("listing the maps of {}", iso.display()))?;
        let (name, source) = maps
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .with_context(|| format!("no map {name} on {}", iso.display()))?;
        let mut map = source.open().with_context(|| format!("opening {name}"))?;
        let files = scenario_edit::scene::resource_files(&source);
        let fonts = Fonts::find(&source.path, true);
        let hudg = hudg::read(&mut map).ok();
        let player = templates::player_huds(&mut map)?[0];
        let colours = disc_colours(iso, hudg.as_ref());
        Ok(Source {
            name,
            map,
            art: Art::new(files),
            fonts,
            colours,
            hudg,
            player,
            synthetic: false,
        })
    }

    /// The templates `hud` needs, for the player `nhdt`.
    pub fn templates(&mut self, hud: &Hud) -> Templates {
        Templates::resolve(&mut self.map, self.player, self.hudg.as_ref(), hud)
    }
}

fn float_colour(v: [f32; 3]) -> [u8; 4] {
    let c = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    [c(v[0]), c(v[1]), c(v[2]), 255]
}

fn read_xbe(iso: &Path) -> Result<Vec<u8>> {
    let mut x = xiso::Xiso::open(BufReader::new(std::fs::File::open(iso)?))?;
    let e = x.find("default.xbe")?;
    Ok(x.read(&e)?)
}

/// The disc's colours: the prompt's and the flash peaks from the `hudg`,
/// the HUD text and training colours from the XBE. What cannot be read
/// keeps the synthetic colour and says so.
fn disc_colours(iso: &Path, hudg: Option<&Hudg>) -> Colours {
    let mut c = Colours::synthetic();
    c.notes.clear();
    c.widget = TEXT_COLOUR;
    c.notes
        .push("text widgets: the HUD editor's stand-in colour (decoding 4)".into());
    match hudg {
        Some(h) => {
            c.prompt = hudg::rgba(h.prompt_colour);
            for (i, p) in Pulse::ALL.iter().enumerate() {
                c.flash[i] = hudg::rgba(h.flash(*p).peak);
            }
            c.notes
                .push("prompt and flash colours: the map's hudg".into());
        }
        None => c
            .notes
            .push("no hudg: prompt and flash colours are stand-ins".into()),
    }
    let xbe = read_xbe(iso);
    let floats = |va: u32, n: usize| -> Result<Vec<f32>> {
        let data = xbe.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        let x = xbe::Xbe::parse(data)?;
        let b = x.bytes_at(va, 4 * n)?;
        Ok(b.chunks_exact(4)
            .map(|w| f32::from_le_bytes(w.try_into().unwrap()))
            .collect())
    };
    match floats(XBE_HUD_COLOUR[0], 3) {
        Ok(v) => {
            c.message = float_colour([v[0], v[1], v[2]]);
            c.notes.push(format!(
                "notifications: the XBE's {:#x} (S: red first)",
                XBE_HUD_COLOUR[0]
            ));
        }
        Err(e) => c.notes.push(format!("notifications: a stand-in ({e})")),
    }
    match (
        floats(XBE_TRAINING_COLOUR[0], 1),
        floats(XBE_TRAINING_COLOUR[1], 1),
    ) {
        (Ok(r), Ok(g)) => {
            c.training = float_colour([r[0], g[0], 1.0]);
            c.notes.push(format!(
                "training text: the XBE's {:#x}, {:#x} and blue 1 (S: the order)",
                XBE_TRAINING_COLOUR[0], XBE_TRAINING_COLOUR[1]
            ));
        }
        (Err(e), _) | (_, Err(e)) => c.notes.push(format!("training text: a stand-in ({e})")),
    }
    c
}
