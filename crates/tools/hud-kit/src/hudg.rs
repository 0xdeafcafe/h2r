//! The HUD globals (`hudg`) the kit reads: the flash records, the message
//! fields, the training references and the waypoint arrow, each by its
//! field name in the `hudg` definition (`tags/src/defs/hudg.txt`), so no
//! offset is written twice. The engine reads a tag reference's datum, four
//! bytes in: the training shader the draw reads at `hudg +0x444` is the
//! reference `tutorial_material` at `+0x440` (docs/game/hud-kit.md,
//! "Training").

use std::io::{Read, Seek};

use anyhow::{Context as _, Result, bail};
use map::{FourCC, Map, TagId};
use tags::defs::{Defs, StructDef};

use crate::model::Pulse;

pub const HUDG: FourCC = FourCC(*b"hudg");

/// A flash record: a rest colour, a peak colour (packed ARGB), the period,
/// the delay and the count of flashes, flags, one flash's duration.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Flash {
    pub rest: u32,
    pub peak: u32,
    pub period: f32,
    pub delay: f32,
    pub count: i16,
    pub flags: u16,
    pub duration: f32,
}

/// `hudg` `arrow_assets[0]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrow {
    pub image: TagId,
    pub material: TagId,
    pub sequence: i16,
}

/// The training box's art: the shader and the four bitmaps (upper right,
/// upper centre, upper left, middle), for the human and the Elite HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrainingArt {
    pub shader: Option<TagId>,
    pub human: [Option<TagId>; 4],
    pub elite: [Option<TagId>; 4],
}

/// What the kit reads of the map's `hudg`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hudg {
    pub id: TagId,
    /// The prompt and help text colour, packed ARGB (`hudg +0xa4`, E
    /// `0x0024d0a6`).
    pub prompt_colour: u32,
    pub help: Flash,
    pub objective: Flash,
    pub time_warning: Flash,
    pub time_expired: Flash,
    /// Seconds a notification holds, then fades (E `0x0024d33a`..).
    pub message_hold: f32,
    pub message_fade: f32,
    /// The next notification starts the drawn height times this lower.
    pub message_spacing: f32,
    pub training: TrainingArt,
    pub arrow: Option<Arrow>,
}

/// Bit 0 of a flash record's flags: the flash runs from the peak colour
/// to the rest colour, not the other way (E `0x0024d1aa`, help's
/// `hudg +0xb6`; `h1_hud_text/flash.rs`).
pub const FLASH_REVERSED: u16 = 1;

impl Flash {
    /// The colour the text shows at the flash's start, strength 0: the
    /// rest colour, or the peak with [`FLASH_REVERSED`]. The engine's
    /// help line draws `+0xa4` or `+0xa8` this way (E `0x0024d194`), and
    /// its objective line only the rest colour `+0xcc` (E `0x0024d13e`).
    /// A record's peak may be unset (all zero, alpha 0), so the peak
    /// alone is no colour to draw a flashing text in.
    pub fn start(&self) -> u32 {
        if self.flags & FLASH_REVERSED != 0 {
            self.peak
        } else {
            self.rest
        }
    }
}

impl Hudg {
    pub fn flash(&self, p: Pulse) -> &Flash {
        match p {
            Pulse::Help => &self.help,
            Pulse::Objective => &self.objective,
            Pulse::TimeWarning => &self.time_warning,
            Pulse::TimeExpired => &self.time_expired,
        }
    }
}

/// The `hudg` definition.
pub fn def() -> &'static StructDef {
    Defs::builtin().group(HUDG).expect("the hudg definition")
}

struct Fields<'a> {
    def: &'a StructDef,
    bytes: &'a [u8],
}

impl Fields<'_> {
    fn at(&self, name: &str, len: usize) -> Result<&[u8]> {
        let f = self
            .def
            .field(name)
            .with_context(|| format!("{}: no field {name}", self.def.name))?;
        let at = f.offset as usize;
        self.bytes
            .get(at..at + len)
            .with_context(|| format!("{}: {name} is past the end", self.def.name))
    }
    fn u32(&self, name: &str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.at(name, 4)?.try_into().unwrap()))
    }
    fn f32(&self, name: &str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(name)?))
    }
    fn i16(&self, name: &str) -> Result<i16> {
        Ok(i16::from_le_bytes(self.at(name, 2)?.try_into().unwrap()))
    }
    fn u16(&self, name: &str) -> Result<u16> {
        Ok(self.i16(name)? as u16)
    }
    /// A tag reference's datum (`None` for a null one).
    fn reference(&self, name: &str) -> Result<Option<TagId>> {
        let b = self.at(name, 8)?;
        let v = u32::from_le_bytes(b[4..8].try_into().unwrap());
        Ok((v != u32::MAX).then_some(TagId(v)))
    }
    /// A block's count and address.
    fn block(&self, name: &str) -> Result<(u32, u32)> {
        let b = self.at(name, 8)?;
        Ok((
            u32::from_le_bytes(b[0..4].try_into().unwrap()),
            u32::from_le_bytes(b[4..8].try_into().unwrap()),
        ))
    }
    fn flash(&self, rest: &str, prefix: &str) -> Result<Flash> {
        Ok(Flash {
            rest: self.u32(rest)?,
            peak: self.u32(&format!("{prefix}_pulse_colour"))?,
            period: self.f32(&format!("{prefix}_pulse_period"))?,
            delay: self.f32(&format!("{prefix}_pulse_delay"))?,
            count: self.i16(&format!("{prefix}_pulse_count"))?,
            flags: self.u16(&format!("{prefix}_pulse_flags"))?,
            duration: self.f32(&format!("{prefix}_pulse_duration"))?,
        })
    }
}

/// The map's `hudg` tag (the first with data).
pub fn find<R: Read + Seek>(map: &Map<R>) -> Option<TagId> {
    map.tags()
        .iter()
        .find(|t| t.group == HUDG && t.location.is_some())
        .map(|t| t.id)
}

/// Reads the map's `hudg`.
pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Hudg> {
    let id = find(map).context("the map has no hudg")?;
    let bytes = map.read_tag_bytes(id)?;
    let def = def();
    if bytes.len() < def.size as usize {
        bail!(
            "the hudg is {:#x} bytes, under {:#x}",
            bytes.len(),
            def.size
        );
    }
    let f = Fields { def, bytes: &bytes };
    let arrow = match f.block("arrow_assets")? {
        (0, _) => None,
        (_, address) => {
            let child = def.child("arrow_assets").expect("hudg arrow_assets");
            let at = map.file_offset(address, child.size)?;
            let row = map.read_file_range(at, u64::from(child.size))?;
            let r = Fields {
                def: child,
                bytes: &row,
            };
            match (r.reference("arrow_image")?, r.reference("arrow_material")?) {
                (Some(image), Some(material)) => Some(Arrow {
                    image,
                    material,
                    sequence: r.i16("sprite_sequence")?,
                }),
                _ => None,
            }
        }
    };
    let refs = |names: [&str; 4]| -> Result<[Option<TagId>; 4]> {
        let mut out = [None; 4];
        for (o, n) in out.iter_mut().zip(names) {
            *o = f.reference(n)?;
        }
        Ok(out)
    };
    Ok(Hudg {
        id,
        prompt_colour: f.u32("word_a4")?,
        help: f.flash("word_a4", "help")?,
        objective: f.flash("objective_rest_colour", "objective")?,
        time_warning: f.flash("time_warning_rest_colour", "time_warning")?,
        time_expired: f.flash("time_expired_rest_colour", "time_expired")?,
        message_hold: f.f32("message_hold_time")?,
        message_fade: f.f32("message_fade_time")?,
        message_spacing: f.f32("message_spacing")?,
        training: TrainingArt {
            shader: f.reference("tutorial_material")?,
            human: refs([
                "human_prompt_upper_right",
                "human_prompt_upper_centre",
                "human_prompt_upper_left",
                "human_prompt_middle",
            ])?,
            elite: refs([
                "elite_prompt_upper_right",
                "elite_prompt_upper_centre",
                "elite_prompt_upper_left",
                "elite_prompt_middle",
            ])?,
        },
        arrow,
    })
}

/// Packed ARGB as RGBA bytes.
pub fn rgba(argb: u32) -> [u8; 4] {
    let [b, g, r, a] = argb.to_le_bytes();
    [r, g, b, a]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offsets the engine reads (docs/game/hud-kit.md) are where the
    /// definition puts these fields (a reference's datum four bytes in).
    #[test]
    fn the_definition_has_the_fields_where_the_engine_reads_them() {
        let d = def();
        let at = |n: &str| d.field(n).unwrap_or_else(|| panic!("{n}")).offset;
        assert_eq!(at("word_a4"), 0xa4);
        assert_eq!(at("help_pulse_colour"), 0xa8);
        assert_eq!(at("help_pulse_flags"), 0xb6);
        assert_eq!(at("objective_rest_colour"), 0xcc);
        assert_eq!(at("objective_fade_ticks"), 0xea);
        assert_eq!(at("time_warning_rest_colour"), 0x318);
        assert_eq!(at("time_expired_rest_colour"), 0x338);
        assert_eq!(at("message_hold_time"), 0x58);
        assert_eq!(at("message_fade_time"), 0x5c);
        assert_eq!(at("message_spacing"), 0x80);
        assert_eq!(at("hud_strings") + 4, 0x3fc);
        assert_eq!(at("arrow_assets"), 0x408);
        assert_eq!(at("training_prompts"), 0x420);
        assert_eq!(at("tutorial_material") + 4, 0x444);
        assert_eq!(at("human_prompt_upper_right") + 4, 0x44c);
        assert_eq!(at("human_prompt_middle") + 4, 0x464);
        assert_eq!(at("elite_prompt_upper_right") + 4, 0x46c);
        assert_eq!(at("elite_prompt_middle") + 4, 0x484);
        let arrow = d.child("arrow_assets").unwrap();
        assert_eq!(arrow.size, 0x24);
        assert_eq!(arrow.field("sprite_sequence").unwrap().offset, 0x10);
    }

    /// A flash starts at its rest colour, or its peak when reversed: an
    /// unset peak is not drawn.
    #[test]
    fn a_flash_starts_at_its_rest_colour() {
        let f = Flash {
            rest: 0xff20_40ff,
            peak: 0,
            ..Flash::default()
        };
        assert_eq!(f.start(), 0xff20_40ff);
        let r = Flash {
            flags: FLASH_REVERSED,
            peak: 0xffff_ffff,
            ..f
        };
        assert_eq!(r.start(), 0xffff_ffff);
    }

    #[test]
    fn argb_unpacks_to_rgba() {
        assert_eq!(rgba(0x80ff4020), [0xff, 0x40, 0x20, 0x80]);
    }
}
